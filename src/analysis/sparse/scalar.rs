use super::*;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Fact {
    Bottom,
    Bits(u128),
    Range(u128, u128),
    Variant(u32),
    Argument(usize),
    Unknown,
}

impl Fact {
    fn join(self, other: Self) -> Self {
        if self == other || other == Self::Bottom { self }
        else if self == Self::Bottom { other }
        else { match (self, other) {
            (Self::Bits(left), Self::Bits(right)) => Self::Range(left.min(right), left.max(right)),
            (Self::Bits(bits), Self::Range(lower, upper)) if lower <= bits && bits <= upper => other,
            (Self::Range(lower, upper), Self::Bits(bits)) if lower <= bits && bits <= upper => self,
            (Self::Range(lower, upper), Self::Range(other_lower, other_upper))
                if lower <= other_lower && other_upper <= upper => self,
            _ => Self::Unknown,
        } }
    }

    fn bounds(self) -> Option<(u128, u128)> {
        match self {
            Self::Bits(bits) => Some((bits, bits)),
            Self::Range(lower, upper) => Some((lower, upper)),
            _ => None,
        }
    }

    fn operand<'tcx>(tcx: TyCtxt<'tcx>, instance: Instance<'tcx>, index: &BodyIndex<'tcx>,
        state: &[Self], operand: &Operand<'tcx>) -> Self
    {
        match operand {
            Operand::Copy(place) | Operand::Move(place) => {
                if place.projection.is_empty() && !index.address_taken.contains(&place.local) {
                    state[place.local.as_usize()]
                } else { Self::Unknown }
            }
            Operand::RuntimeChecks(check) => Self::Bits(check.value(tcx.sess) as u128),
            Operand::Constant(constant) => {
                let ty = instance.instantiate_mir_and_normalize_erasing_regions(tcx,
                    TypingEnv::fully_monomorphized(), EarlyBinder::bind(tcx, constant.const_.ty()));
                if !matches!(ty.kind(), ty::Bool | ty::Int(_) | ty::Uint(_) | ty::Char)
                    && !matches!(ty.kind(), ty::Adt(adt, _) if adt.is_enum()) { return Self::Unknown; }
                let constant = instance.instantiate_mir_and_normalize_erasing_regions(tcx,
                    TypingEnv::fully_monomorphized(), EarlyBinder::bind(tcx, constant.const_));
                match constant.eval(tcx, TypingEnv::fully_monomorphized(), index.body.span) {
                    Ok(value) if matches!(ty.kind(), ty::Adt(adt, _) if adt.is_enum()) => {
                        tcx.try_destructure_mir_constant_for_user_output(value, ty)
                            .and_then(|value| value.variant).map_or(Self::Unknown, |variant| Self::Variant(variant.as_u32()))
                    }
                    Ok(ConstValue::Scalar(Scalar::Int(bits))) => Self::Bits(bits.to_bits(bits.size())),
                    _ => Self::Unknown,
                }
            }
        }
    }
}

#[derive(Clone)]
struct Summary {
    returned: Fact,
    conditions: HashMap<Location, Fact>,
}

type SummaryKey<'tcx> = (Instance<'tcx>, bool, Option<usize>, Box<[Fact]>);

#[derive(Default)]
pub(crate) struct ScalarCache<'tcx> {
    summaries: HashMap<SummaryKey<'tcx>, Summary>,
    bytes: usize,
}

impl<'a, 'tcx> Solver<'a, 'tcx> {
    pub(super) fn scalar_condition(&mut self, point: Point) -> Result<Option<u128>, QueryStop> {
        let frame = self.frames[point.frame];
        let roots: Vec<_> = self.frame_contexts[point.frame].as_ref().filter(|contexts| !contexts.is_empty())
            .map_or_else(|| vec![(frame.instance, frame.irq, None, Box::<[Fact]>::default())],
                |contexts| contexts.iter().map(|&context| (frame.instance, frame.irq, Some(context), Box::<[Fact]>::default())).collect());
        if roots.iter().all(|root| self.scalars.summaries.contains_key(root)) {
            let value = roots.iter().fold(Fact::Bottom, |value, root| value.join(
                self.scalars.summaries[root].conditions.get(&point.location).copied().unwrap_or(Fact::Unknown)));
            return Ok(match value { Fact::Bits(bits) => Some(bits), _ => None });
        }
        let scratch = self.budget.scratch;
        let limits = irq_check_rupta::limits::configuration().expect("validated analysis limits");
        let mut summaries = HashMap::<SummaryKey<'tcx>, Summary>::new();
        let mut users = HashMap::<SummaryKey<'tcx>, HashSet<SummaryKey<'tcx>>>::new();
        let mut queue = VecDeque::new();
        let mut queued = HashSet::new();
        for root in &roots {
            if !self.scalars.summaries.contains_key(root) {
                summaries.insert(root.clone(), Summary { returned: Fact::Bottom, conditions: HashMap::new() });
                queue.push_back(root.clone());
                queued.insert(root.clone());
            }
        }
        let mut retained = 0usize;
        let before = self.budget.steps;
        let answer = (|| {
            while let Some(key) = queue.pop_front() {
                let (instance, irq, context, _) = key;
                queued.remove(&key);
                self.budget.scratch = scratch;
                self.budget.charge(retained)?;
                if self.tcx.is_foreign_item(instance.def_id())
                    || matches!(instance.def, InstanceKind::Intrinsic(_) | InstanceKind::LlvmIntrinsic(_) | InstanceKind::Virtual(..))
                    || !irq_check_rupta::util::type_util::has_instance_mir(self.tcx, instance.def)
                {
                    let summary = summaries.get_mut(&key).expect("queued scalar summary");
                    if summary.returned != Fact::Unknown {
                        summary.returned = Fact::Unknown;
                        for user in users.get(&key).into_iter().flatten() {
                            if queued.insert(user.clone()) { queue.push_back(user.clone()); }
                        }
                    }
                    continue;
                }
                let index_key = (instance, irq);
                let index = if let Some(index) = self.indexes.get(&index_key) { index.clone() } else {
                    let index = Arc::new(BodyIndex::build(self.tcx, self.trust, instance, irq)?);
                    self.indexes.insert(index_key, index.clone());
                    index
                };
                let body = index.body;
                let normalize = |ty| instance.instantiate_mir_and_normalize_erasing_regions(self.tcx,
                    TypingEnv::fully_monomorphized(), EarlyBinder::bind(self.tcx, ty));
                let mut needed = HashSet::new();
                let mut locals = VecDeque::from([mir::RETURN_PLACE]);
                for data in body.basic_blocks.iter() {
                    if let TerminatorKind::SwitchInt { discr: operand, .. } | TerminatorKind::Assert { cond: operand, .. } = &data.terminator().kind
                        && let Operand::Copy(place) | Operand::Move(place) = operand
                        && place.projection.is_empty() { locals.push_back(place.local); }
                }
                for operand in index.assumptions.values() {
                    if let Operand::Copy(place) | Operand::Move(place) = operand
                        && place.projection.is_empty() { locals.push_back(place.local); }
                }
                while let Some(local) = locals.pop_front() {
                    self.budget.charge(0)?;
                    if !needed.insert(local) { continue; }
                    for definition in index.local_definitions.get(&local).into_iter().flatten() {
                        let data = &body.basic_blocks[definition.location.block];
                        let mut operands = Vec::new();
                        if let Some(statement) = data.statements.get(definition.location.statement_index) {
                            if let StatementKind::Assign(assignment) = &statement.kind {
                                match &assignment.1 {
                                    Rvalue::Use(operand, _) | Rvalue::UnaryOp(_, operand) | Rvalue::Cast(_, operand, _) => operands.push(operand),
                                    Rvalue::BinaryOp(_, pair) => { operands.push(&pair.0); operands.push(&pair.1); }
                                    Rvalue::Discriminant(place) if place.projection.is_empty() => locals.push_back(place.local),
                                    _ => {}
                                }
                            }
                        } else if let TerminatorKind::Call { args, .. } | TerminatorKind::TailCall { args, .. } = &data.terminator().kind {
                            operands.extend(args.iter().map(|argument| &argument.node));
                        }
                        for operand in operands {
                            if let Operand::Copy(place) | Operand::Move(place) = operand
                                && place.projection.is_empty() { locals.push_back(place.local); }
                        }
                    }
                }
                let bytes = body.local_decls.len().saturating_mul(body.basic_blocks.len())
                    .saturating_mul(std::mem::size_of::<Fact>() * 2);
                if bytes > limits.function_scratch_mib * 1024 * 1024 { return Err(QueryStop::RefinementLimit); }
                let mut entries = vec![None::<Vec<Fact>>; body.basic_blocks.len()];
                let mut initial = vec![Fact::Unknown; body.local_decls.len()];
                for argument in 1..=body.arg_count {
                    initial[argument] = key.3.get(argument - 1).copied().filter(|fact| *fact != Fact::Unknown)
                        .unwrap_or(Fact::Argument(argument));
                }
                entries[mir::START_BLOCK.as_usize()] = Some(initial);
                let mut blocks = VecDeque::from([mir::START_BLOCK]);
                let mut block_queued = HashSet::from([mir::START_BLOCK]);
                let mut returned = Fact::Bottom;
                let mut conditions = HashMap::<Location, Fact>::new();
                while let Some(block) = blocks.pop_front() {
                    block_queued.remove(&block);
                    self.budget.scratch = scratch;
                    self.budget.charge(bytes.saturating_add(retained))?;
                    let mut state = entries[block.as_usize()].clone().expect("reachable scalar block");
                    let data = &body.basic_blocks[block];
                    for (statement_index, statement) in data.statements.iter().enumerate() {
                        if index.stops[block.as_usize()].is_some_and(|stop| statement_index >= stop) { break; }
                        self.budget.charge(0)?;
                        let at = Location { block, statement_index };
                        if let Some(condition) = index.assumptions.get(&at) {
                            let value = Fact::operand(self.tcx, instance, &index, &state, condition);
                            let previous = conditions.entry(at).or_insert(Fact::Bottom);
                            *previous = previous.join(value);
                        }
                        match &statement.kind {
                            StatementKind::Assign(assignment) => {
                                let (place, value) = &**assignment;
                                if place.is_indirect() || !needed.contains(&place.local) { continue; }
                                if !place.projection.is_empty() {
                                    state[place.local.as_usize()] = Fact::Unknown;
                                    continue;
                                }
                                let fact = match value {
                                    Rvalue::Use(operand, _) => Fact::operand(self.tcx, instance, &index, &state, operand),
                                    Rvalue::Discriminant(source) => {
                                        let value = Fact::operand(self.tcx, instance, &index, &state, &Operand::Copy(*source));
                                        match (normalize(source.ty(body, self.tcx).ty).kind(), value) {
                                            (ty::Adt(adt, _), Fact::Variant(variant)) => Fact::Bits(adt.discriminant_for_variant(
                                                self.tcx, rustc_abi::VariantIdx::from_u32(variant)).val),
                                            (_, Fact::Bottom) => Fact::Bottom,
                                            _ => Fact::Unknown,
                                        }
                                    }
                                    Rvalue::Aggregate(kind, _) => match &**kind {
                                        mir::AggregateKind::Adt(def, variant, ..) if self.tcx.adt_def(*def).is_enum() => Fact::Variant(variant.as_u32()),
                                        _ => Fact::Unknown,
                                    },
                                    Rvalue::UnaryOp(operation, operand) => {
                                        let value = Fact::operand(self.tcx, instance, &index, &state, operand);
                                        let ty = normalize(operand.ty(body, self.tcx));
                                        let width = self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(ty))
                                            .ok().map(|layout| if ty.is_bool() { 1 } else { layout.size.bits() });
                                        match (operation, value, width) {
                                            (_, Fact::Bottom, _) => Fact::Bottom,
                                            (mir::UnOp::Not, Fact::Bits(bits), Some(width @ 1..=128)) => Fact::Bits(!bits & (u128::MAX >> (128 - width))),
                                            (mir::UnOp::Neg, Fact::Bits(bits), Some(width @ 1..=128)) => Fact::Bits(bits.wrapping_neg() & (u128::MAX >> (128 - width))),
                                            _ => Fact::Unknown,
                                        }
                                    }
                                    Rvalue::Cast(mir::CastKind::IntToInt, operand, destination) => {
                                        let source = normalize(operand.ty(body, self.tcx));
                                        let destination = normalize(*destination);
                                        let source_width = self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(source)).ok().map(|layout| layout.size.bits());
                                        let destination_width = self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(destination)).ok().map(|layout| layout.size.bits());
                                        match (Fact::operand(self.tcx, instance, &index, &state, operand), source_width, destination_width) {
                                            (Fact::Bottom, _, _) => Fact::Bottom,
                                            (Fact::Bits(mut bits), Some(source_width @ 1..=128), Some(destination_width @ 1..=128)) => {
                                                if source.is_signed() && destination_width > source_width {
                                                    bits = (((bits << (128 - source_width)) as i128) >> (128 - source_width)) as u128;
                                                }
                                                Fact::Bits(bits & (u128::MAX >> (128 - destination_width)))
                                            }
                                            _ => Fact::Unknown,
                                        }
                                    }
                                    Rvalue::BinaryOp(operation, operands) => {
                                        let left = Fact::operand(self.tcx, instance, &index, &state, &operands.0);
                                        let right = Fact::operand(self.tcx, instance, &index, &state, &operands.1);
                                        let ty = normalize(operands.0.ty(body, self.tcx));
                                        let width = self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(ty)).ok().map(|layout| layout.size.bits());
                                        if left == Fact::Bottom || right == Fact::Bottom { Fact::Bottom }
                                        else if let (Fact::Bits(left), Fact::Bits(right), Some(width @ 1..=128)) = (left, right, width) {
                                            let mask = u128::MAX >> (128 - width);
                                            let signed_left = ((left << (128 - width)) as i128) >> (128 - width);
                                            let signed_right = ((right << (128 - width)) as i128) >> (128 - width);
                                            let value = match operation {
                                                mir::BinOp::Eq => Some(u128::from(left == right)),
                                                mir::BinOp::Ne => Some(u128::from(left != right)),
                                                mir::BinOp::Lt => Some(u128::from(if ty.is_signed() { signed_left < signed_right } else { left < right })),
                                                mir::BinOp::Le => Some(u128::from(if ty.is_signed() { signed_left <= signed_right } else { left <= right })),
                                                mir::BinOp::Gt => Some(u128::from(if ty.is_signed() { signed_left > signed_right } else { left > right })),
                                                mir::BinOp::Ge => Some(u128::from(if ty.is_signed() { signed_left >= signed_right } else { left >= right })),
                                                mir::BinOp::Add | mir::BinOp::AddUnchecked => Some(left.wrapping_add(right) & mask),
                                                mir::BinOp::Sub | mir::BinOp::SubUnchecked => Some(left.wrapping_sub(right) & mask),
                                                mir::BinOp::Mul | mir::BinOp::MulUnchecked => Some(left.wrapping_mul(right) & mask),
                                                mir::BinOp::BitAnd => Some(left & right),
                                                mir::BinOp::BitOr => Some(left | right),
                                                mir::BinOp::BitXor => Some(left ^ right),
                                                mir::BinOp::Shl | mir::BinOp::ShlUnchecked if right < width as u128 => Some((left << right) & mask),
                                                mir::BinOp::Shr | mir::BinOp::ShrUnchecked if right < width as u128 => Some(if ty.is_signed() { (signed_left >> right) as u128 & mask } else { left >> right }),
                                                mir::BinOp::Div if right != 0 && !ty.is_signed() => Some(left / right),
                                                mir::BinOp::Rem if right != 0 && !ty.is_signed() => Some(left % right),
                                                _ => None,
                                            };
                                            value.map_or(Fact::Unknown, Fact::Bits)
                                        } else if !ty.is_signed() && let (Some((left_lower, left_upper)), Some((right_lower, right_upper)), Some(width @ 1..=128))
                                            = (left.bounds(), right.bounds(), width) {
                                            let mask = u128::MAX >> (128 - width);
                                            let known = match operation {
                                                mir::BinOp::Eq if left_upper < right_lower || right_upper < left_lower => Some(false),
                                                mir::BinOp::Ne if left_upper < right_lower || right_upper < left_lower => Some(true),
                                                mir::BinOp::Lt if left_upper < right_lower => Some(true),
                                                mir::BinOp::Lt if left_lower >= right_upper => Some(false),
                                                mir::BinOp::Le if left_upper <= right_lower => Some(true),
                                                mir::BinOp::Le if left_lower > right_upper => Some(false),
                                                mir::BinOp::Gt if left_lower > right_upper => Some(true),
                                                mir::BinOp::Gt if left_upper <= right_lower => Some(false),
                                                mir::BinOp::Ge if left_lower >= right_upper => Some(true),
                                                mir::BinOp::Ge if left_upper < right_lower => Some(false),
                                                _ => None,
                                            };
                                            if let Some(value) = known { Fact::Bits(u128::from(value)) }
                                            else {
                                                let range = match operation {
                                                    mir::BinOp::Add | mir::BinOp::AddUnchecked => left_lower.checked_add(right_lower).zip(left_upper.checked_add(right_upper)),
                                                    mir::BinOp::Sub | mir::BinOp::SubUnchecked => left_lower.checked_sub(right_upper).zip(left_upper.checked_sub(right_lower)),
                                                    mir::BinOp::Mul | mir::BinOp::MulUnchecked => left_lower.checked_mul(right_lower).zip(left_upper.checked_mul(right_upper)),
                                                    _ => None,
                                                };
                                                match range {
                                                    Some((lower, upper)) if upper <= mask => {
                                                        if lower == upper { Fact::Bits(lower) } else { Fact::Range(lower, upper) }
                                                    }
                                                    _ => Fact::Unknown,
                                                }
                                            }
                                        } else { Fact::Unknown }
                                    }
                                    _ => Fact::Unknown,
                                };
                                state[place.local.as_usize()] = if let Fact::Bits(bits) = fact {
                                    let ty = normalize(place.ty(body, self.tcx).ty);
                                    if matches!(ty.kind(), ty::Bool | ty::Int(_) | ty::Uint(_) | ty::Char)
                                        && let Ok(layout) = self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(ty))
                                        && (1..=128).contains(&layout.size.bits()) {
                                        Fact::Bits(bits & (u128::MAX >> (128 - layout.size.bits())))
                                    } else { Fact::Unknown }
                                } else { fact };
                            }
                            StatementKind::SetDiscriminant { place, variant_index } if !place.is_indirect() => {
                                state[place.local.as_usize()] = if place.projection.is_empty() { Fact::Variant(variant_index.as_u32()) } else { Fact::Unknown };
                            }
                            StatementKind::StorageLive(local) | StatementKind::StorageDead(local) => state[local.as_usize()] = Fact::Unknown,
                            _ => {}
                        }
                    }
                    if index.stops[block.as_usize()].is_some() { continue; }
                    let at = Location { block, statement_index: data.statements.len() };
                    let mut successors: Vec<_> = data.terminator().successors().collect();
                    match &data.terminator().kind {
                        TerminatorKind::SwitchInt { discr, targets } => {
                            let value = Fact::operand(self.tcx, instance, &index, &state, discr);
                            let previous = conditions.entry(at).or_insert(Fact::Bottom);
                            *previous = previous.join(value);
                            if let Fact::Bits(bits) = value { successors = vec![targets.target_for_value(bits)]; }
                            else if value == Fact::Bottom { successors.clear(); }
                        }
                        TerminatorKind::Assert { cond, expected, target, unwind, .. } => {
                            let value = Fact::operand(self.tcx, instance, &index, &state, cond);
                            let previous = conditions.entry(at).or_insert(Fact::Bottom);
                            *previous = previous.join(value);
                            if let Fact::Bits(bits) = value {
                                successors = if (bits != 0) == *expected { vec![*target] }
                                    else if let mir::UnwindAction::Cleanup(block) = unwind { vec![*block] }
                                    else { Vec::new() };
                            } else if value == Fact::Bottom { successors.clear(); }
                        }
                        TerminatorKind::Return => {
                            returned = returned.join(Fact::operand(self.tcx, instance, &index, &state, &Operand::Copy(Place::from(mir::RETURN_PLACE))));
                        }
                        TerminatorKind::Call { func, args, .. }
                        | TerminatorKind::TailCall { func, args, .. } => {
                            let destination = match &data.terminator().kind {
                                TerminatorKind::Call { destination, .. } => *destination,
                                _ => Place::from(mir::RETURN_PLACE),
                            };
                            let result_ty = normalize(destination.ty(body, self.tcx).ty);
                            let mut result = Fact::Unknown;
                            if needed.contains(&destination.local) && destination.projection.is_empty() && (matches!(result_ty.kind(), ty::Bool | ty::Int(_) | ty::Uint(_))
                                || matches!(result_ty.kind(), ty::Adt(adt, _) if adt.is_enum())) {
                                let function_ty = normalize(func.ty(body, self.tcx));
                                let direct = if let ty::FnDef(def, args) = *function_ty.kind() {
                                    Instance::try_resolve(self.tcx, TypingEnv::fully_monomorphized(), def,
                                        self.tcx.instantiate_bound_regions_with_erased(args)).ok().flatten()
                                        .filter(|target| !matches!(target.def, InstanceKind::Virtual(..)))
                                } else { None };
                                let contextual = context.and_then(|context| self.facts.context_bindings.get(&(context, at)))
                                    .filter(|targets| !targets.is_empty());
                                let targets: Vec<_> = if let Some(targets) = contextual {
                                        targets.iter().map(|&(target, context)| (target, Some(context))).collect()
                                    } else if let Some(target) = direct { vec![(target, None)] }
                                    else if self.facts.covered_calls.contains(&(instance, at)) {
                                        self.facts.safe_candidates.get(&(instance, at)).into_iter().flatten().map(|&target| (target, None)).collect()
                                    } else { Vec::new() };
                                if !targets.is_empty() {
                                    result = Fact::Bottom;
                                    for (original, mut context) in targets {
                                        let Some(target) = self.linked_target(original) else { result = Fact::Unknown; break; };
                                        if target != original { context = None; }
                                        if matches!(target.def, InstanceKind::Intrinsic(_))
                                            && self.tcx.item_name(target.def_id()).as_str() == "black_box" && args.len() == 1 {
                                            result = result.join(Fact::operand(self.tcx, instance, &index, &state, &args[0].node));
                                            continue;
                                        }
                                        if !matches!(target.def, InstanceKind::Intrinsic(_) | InstanceKind::LlvmIntrinsic(_) | InstanceKind::Virtual(..))
                                            && !self.tcx.is_foreign_item(target.def_id())
                                            && irq_check_rupta::util::type_util::has_instance_mir(self.tcx, target.def) {
                                            let body = self.tcx.instance_mir(target.def);
                                            let returned_ty = target.instantiate_mir_and_normalize_erasing_regions(self.tcx,
                                                TypingEnv::fully_monomorphized(), EarlyBinder::bind(self.tcx, body.local_decls[mir::RETURN_PLACE].ty));
                                            if returned_ty != result_ty { result = Fact::Unknown; break; }
                                        }
                                        let callee_body = if !matches!(target.def, InstanceKind::Intrinsic(_) | InstanceKind::LlvmIntrinsic(_) | InstanceKind::Virtual(..))
                                            && !self.tcx.is_foreign_item(target.def_id())
                                            && irq_check_rupta::util::type_util::has_instance_mir(self.tcx, target.def) {
                                            Some(self.tcx.instance_mir(target.def))
                                        } else { None };
                                        let mut inputs: Vec<_> = args.iter().enumerate().map(|(position, argument)| {
                                            let compatible = callee_body.filter(|body| position < body.arg_count).is_some_and(|body| {
                                                let ty = target.instantiate_mir_and_normalize_erasing_regions(self.tcx,
                                                    TypingEnv::fully_monomorphized(), EarlyBinder::bind(self.tcx, body.local_decls[mir::Local::from_usize(position + 1)].ty));
                                                ty == normalize(argument.node.ty(index.body, self.tcx))
                                            });
                                            if !compatible { return Fact::Unknown; }
                                            match Fact::operand(self.tcx, instance, &index, &state, &argument.node) {
                                                fact @ (Fact::Bits(_) | Fact::Variant(_) | Fact::Bottom) => fact,
                                                _ => Fact::Unknown,
                                            }
                                        }).collect();
                                        if inputs.contains(&Fact::Bottom) { continue; }
                                        while inputs.last() == Some(&Fact::Unknown) { inputs.pop(); }
                                        let mut callee = (target, irq, context, inputs.into_boxed_slice());
                                        if !callee.3.is_empty() && !summaries.contains_key(&callee) && !self.scalars.summaries.contains_key(&callee)
                                            && summaries.keys().chain(self.scalars.summaries.keys())
                                                .filter(|other| other.0 == target && other.1 == irq && other.2 == context && !other.3.is_empty()).count() >= 8 {
                                            callee.3 = Box::default();
                                        }
                                        let fact = if let Some(summary) = self.scalars.summaries.get(&callee) { summary.returned }
                                            else {
                                                users.entry(callee.clone()).or_default().insert(key.clone());
                                                if !summaries.contains_key(&callee) {
                                                    summaries.insert(callee.clone(), Summary { returned: Fact::Bottom, conditions: HashMap::new() });
                                                    if queued.insert(callee.clone()) { queue.push_back(callee.clone()); }
                                                }
                                                summaries[&callee].returned
                                            };
                                        let fact = if let Fact::Argument(argument) = fact {
                                            args.get(argument.saturating_sub(1)).filter(|argument| normalize(argument.node.ty(body, self.tcx)) == result_ty)
                                                .map_or(Fact::Unknown, |argument| Fact::operand(self.tcx, instance, &index, &state, &argument.node))
                                        } else { fact };
                                        result = result.join(fact);
                                    }
                                }
                            }
                            if !destination.is_indirect() { state[destination.local.as_usize()] = result; }
                            if matches!(data.terminator().kind, TerminatorKind::TailCall { .. }) { returned = returned.join(result); }
                        }
                        TerminatorKind::InlineAsm { .. } => state.fill(Fact::Unknown),
                        TerminatorKind::Drop { place, .. } if !place.is_indirect() => state[place.local.as_usize()] = Fact::Unknown,
                        _ => {}
                    }
                    for successor in successors {
                        if !index.reachable.contains(&successor) { continue; }
                        let mut incoming = state.clone();
                        if let TerminatorKind::SwitchInt { discr: Operand::Copy(condition) | Operand::Move(condition), targets } = &data.terminator().kind
                            && condition.projection.is_empty() && normalize(condition.ty(body, self.tcx).ty).is_bool()
                            && let Some((position, statement)) = data.statements.iter().enumerate().rev().find(|(_, statement)|
                                matches!(&statement.kind, StatementKind::Assign(assignment) if assignment.0 == *condition))
                            && let StatementKind::Assign(assignment) = &statement.kind
                            && let Rvalue::BinaryOp(operation, operands) = &assignment.1
                            && matches!(operands.1, Operand::Constant(_) | Operand::RuntimeChecks(_))
                            && let Operand::Copy(source) | Operand::Move(source) = operands.0
                            && source.projection.is_empty() && !index.address_taken.contains(&source.local)
                            && data.statements[position + 1..].iter().all(|statement| match &statement.kind {
                                StatementKind::Assign(assignment) => assignment.0.local != source.local,
                                StatementKind::StorageLive(local) | StatementKind::StorageDead(local) => *local != source.local,
                                _ => true,
                            })
                            && let Fact::Bits(bound) = Fact::operand(self.tcx, instance, &index, &state, &operands.1)
                            && matches!(normalize(source.ty(body, self.tcx).ty).kind(), ty::Uint(_))
                            && targets.target_for_value(0) != targets.target_for_value(1)
                        {
                            let ty = normalize(source.ty(body, self.tcx).ty);
                            let width = self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(ty))
                                .map_err(|_| QueryStop::Unsupported)?.size.bits();
                            let mask = u128::MAX >> (128 - width);
                            let (mut lower, mut upper) = incoming[source.local.as_usize()].bounds().unwrap_or((0, mask));
                            let truth = successor == targets.target_for_value(1);
                            let range = match (operation, truth) {
                                (mir::BinOp::Lt, true) | (mir::BinOp::Ge, false) => bound.checked_sub(1).map(|end| (0, end)),
                                (mir::BinOp::Lt, false) | (mir::BinOp::Ge, true) => Some((bound, mask)),
                                (mir::BinOp::Le, true) | (mir::BinOp::Gt, false) => Some((0, bound)),
                                (mir::BinOp::Le, false) | (mir::BinOp::Gt, true) => bound.checked_add(1).filter(|start| *start <= mask).map(|start| (start, mask)),
                                (mir::BinOp::Eq, true) | (mir::BinOp::Ne, false) => Some((bound, bound)),
                                (mir::BinOp::Eq, false) | (mir::BinOp::Ne, true) if lower == bound => lower.checked_add(1).map(|start| (start, upper)),
                                (mir::BinOp::Eq, false) | (mir::BinOp::Ne, true) if upper == bound => upper.checked_sub(1).map(|end| (lower, end)),
                                _ => Some((lower, upper)),
                            };
                            let Some((start, end)) = range else { continue; };
                            lower = lower.max(start);
                            upper = upper.min(end);
                            if lower > upper { continue; }
                            incoming[source.local.as_usize()] = if lower == upper { Fact::Bits(lower) } else { Fact::Range(lower, upper) };
                        }
                        if let TerminatorKind::Call { target, destination, .. } = &data.terminator().kind
                            && *target != Some(successor) && !destination.is_indirect() {
                                incoming[destination.local.as_usize()] = Fact::Unknown;
                            }
                        let changed = if let Some(previous) = &mut entries[successor.as_usize()] {
                            let mut changed = false;
                            for (previous, incoming) in previous.iter_mut().zip(incoming) {
                                let joined = previous.join(incoming);
                                changed |= joined != *previous;
                                *previous = joined;
                            }
                            changed
                        } else { entries[successor.as_usize()] = Some(incoming); true };
                        if changed && block_queued.insert(successor) { blocks.push_back(successor); }
                    }
                }
                let summary = summaries.get_mut(&key).expect("evaluated scalar summary");
                let joined = summary.returned.join(returned);
                let changed = joined != summary.returned;
                summary.returned = joined;
                summary.conditions = conditions;
                if changed {
                    for user in users.get(&key).into_iter().flatten() {
                        if queued.insert(user.clone()) { queue.push_back(user.clone()); }
                    }
                }
                retained = summaries.values().map(|summary| std::mem::size_of::<Summary>() * 8
                    + summary.conditions.len() * std::mem::size_of::<(Location, Fact)>() * 4).sum::<usize>()
                    + summaries.keys().map(|key| std::mem::size_of::<SummaryKey<'tcx>>() * 4 + key.3.len() * std::mem::size_of::<Fact>()).sum::<usize>()
                    + users.values().flat_map(|users| users.iter()).map(|key| std::mem::size_of::<SummaryKey<'tcx>>() * 4 + key.3.len() * std::mem::size_of::<Fact>()).sum::<usize>();
                if retained > self.cache_limit / 16 { return Err(QueryStop::RefinementLimit); }
            }
            Ok(())
        })();
        self.budget.scratch = scratch;
        if std::env::var_os("IRQ_CHECK_TRACE_CONTROL").is_some() {
            eprintln!("irq-check: scalar summaries {} functions={} conditions={} constants={} steps={} bytes={retained} result={answer:?}",
                frame.instance, summaries.len(), summaries.values().map(|summary| summary.conditions.len()).sum::<usize>(),
                summaries.values().flat_map(|summary| summary.conditions.values()).filter(|fact| matches!(fact, Fact::Bits(_))).count(),
                self.budget.steps.saturating_sub(before));
        }
        if let Ok(filter) = std::env::var("IRQ_CHECK_TRACE_SCALAR") {
            for ((instance, _, context, inputs), summary) in &summaries {
                if instance.to_string().contains(&filter) {
                    eprintln!("irq-check: scalar result {instance} context={context:?} inputs={inputs:?} returned={:?} conditions={:?}", summary.returned, summary.conditions);
                }
            }
        }
        answer?;
        let value = roots.iter().fold(Fact::Bottom, |value, root| value.join(
            summaries.get(root).or_else(|| self.scalars.summaries.get(root))
                .and_then(|summary| summary.conditions.get(&point.location)).copied().unwrap_or(Fact::Unknown)));
        if self.scalars.bytes.saturating_add(retained) > self.cache_limit / 16 {
            self.scalars.summaries.clear();
            self.scalars.bytes = 0;
        }
        self.scalars.bytes += retained;
        self.scalars.summaries.extend(summaries);
        Ok(match value { Fact::Bits(bits) => Some(bits), _ => None })
    }
}
