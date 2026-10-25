use super::*;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Source<'tcx> {
    Argument(usize, Ty<'tcx>),
    Constant(Operand<'tcx>),
    Function { instance: Instance<'tcx>, ty: Ty<'tcx> },
    Aggregate { fields: Vec<(i128, Ty<'tcx>, Option<Source<'tcx>>)>, variant: Option<rustc_abi::VariantIdx>, ty: Ty<'tcx> },
    Discriminant { source: Arc<Source<'tcx>>, ty: Ty<'tcx> },
    Bytes { source: Arc<Source<'tcx>>, offset: i128, ty: Ty<'tcx> },
    Pointer { source: Arc<Source<'tcx>>, ty: Ty<'tcx> },
    Metadata { source: Arc<Source<'tcx>>, ty: Ty<'tcx> },
    LoadAt { pointer: Arc<Source<'tcx>>, offset: i128, ty: Ty<'tcx> },
    Borrow { source: Arc<Source<'tcx>>, ty: Ty<'tcx> },
    Field { argument: usize, argument_ty: Ty<'tcx>, offset: i128, ty: Ty<'tcx> },
    Load { argument: usize, pointer_ty: Ty<'tcx>, offset: i128, ty: Ty<'tcx> },
    Address { argument: usize, pointer_ty: Ty<'tcx>, offset: i128, ty: Ty<'tcx> },
    Binary { operation: mir::BinOp, left: Arc<Source<'tcx>>, right: Arc<Source<'tcx>>, ty: Ty<'tcx> },
    Branches { alternatives: Vec<(Vec<(Source<'tcx>, Box<[u128]>, bool)>, Source<'tcx>)>, ty: Ty<'tcx> },
}

impl<'tcx> Source<'tcx> {
    pub(super) fn ty(&self) -> Ty<'tcx> {
        match self {
            Self::Argument(_, ty) | Self::Load { ty, .. } | Self::Field { ty, .. }
                | Self::Address { ty, .. } | Self::Binary { ty, .. } | Self::Branches { ty, .. }
                | Self::Function { ty, .. } | Self::Aggregate { ty, .. } | Self::Discriminant { ty, .. } | Self::Bytes { ty, .. }
                | Self::Pointer { ty, .. } | Self::Metadata { ty, .. } | Self::LoadAt { ty, .. } | Self::Borrow { ty, .. } => *ty,
            Self::Constant(Operand::Constant(constant)) => constant.const_.ty(),
            Self::Constant(_) => unreachable!(),
        }
    }

    fn nodes(&self) -> usize {
        match self {
            Self::Binary { left, right, .. } => 1 + left.nodes() + right.nodes(),
            Self::Branches { alternatives, .. } => 1 + alternatives.iter().map(|(conditions, source)| source.nodes()
                + conditions.iter().map(|(source, values, _)| source.nodes() + values.len()).sum::<usize>()).sum::<usize>(),
            Self::Aggregate { fields, .. } => 1 + fields.iter().map(|(_, _, source)| 1 + source.as_ref().map_or(0, Source::nodes)).sum::<usize>(),
            Self::Discriminant { source, .. } | Self::Bytes { source, .. }
                | Self::Pointer { source, .. } | Self::Metadata { source, .. } | Self::Borrow { source, .. } => 1 + source.nodes(),
            Self::LoadAt { pointer, .. } => 1 + pointer.nodes(),
            _ => 1,
        }
    }

    fn substitute(&self, arguments: &[Option<Self>], can_load: bool) -> Option<Self> {
        match self {
            Self::LoadAt { pointer, offset, ty } => {
                let pointer = pointer.substitute(arguments, can_load)?;
                if let Self::Borrow { source, .. } = pointer {
                    return (source.nodes() < 32).then(|| Self::Bytes { source, offset: *offset, ty: *ty });
                }
                (can_load && pointer.nodes() < 32).then(|| Self::LoadAt { pointer: Arc::new(pointer), offset: *offset, ty: *ty })
            }
            Self::Borrow { source, ty } => {
                let source = source.substitute(arguments, can_load)?;
                (source.nodes() < 32).then(|| Self::Borrow { source: Arc::new(source), ty: *ty })
            }
            Self::Pointer { source, ty } | Self::Metadata { source, ty } => {
                let mut source = source.substitute(arguments, can_load)?;
                if matches!(self, Self::Pointer { .. }) {
                    while let Self::Pointer { source: original, .. } = source { source = (*original).clone(); }
                }
                if source.nodes() >= 32 { return None; }
                Some(if matches!(self, Self::Pointer { .. }) { Self::Pointer { source: Arc::new(source), ty: *ty } }
                    else { Self::Metadata { source: Arc::new(source), ty: *ty } })
            }
            Self::Bytes { source, offset, ty } => {
                let mut source = source.substitute(arguments, can_load)?;
                let mut offset = *offset;
                while let Self::Bytes { source: original, offset: base, .. } = source {
                    offset = offset.checked_add(base)?;
                    source = (*original).clone();
                }
                if offset == 0 && source.ty() == *ty { return Some(source); }
                (source.nodes() < 32).then_some(Self::Bytes { source: Arc::new(source), offset, ty: *ty })
            }
            Self::Discriminant { source, ty } => {
                let source = source.substitute(arguments, can_load)?;
                (source.nodes() < 32).then_some(Self::Discriminant { source: Arc::new(source), ty: *ty })
            }
            Self::Aggregate { fields, variant, ty } => {
                let fields = fields.iter().map(|(offset, ty, source)|
                    (*offset, *ty, source.as_ref().and_then(|source| source.substitute(arguments, can_load)))).collect();
                let result = Self::Aggregate { fields, variant: *variant, ty: *ty };
                (result.nodes() <= 32).then_some(result)
            }
            Self::Branches { alternatives, ty } => {
                let alternatives = alternatives.iter().map(|(conditions, source)| {
                    Some((conditions.iter().map(|(source, values, otherwise)|
                        Some((source.substitute(arguments, can_load)?, values.clone(), *otherwise))).collect::<Option<Vec<_>>>()?,
                        source.substitute(arguments, can_load)?))
                }).collect::<Option<Vec<_>>>()?;
                let result = Self::Branches { alternatives, ty: *ty };
                (result.nodes() <= 128).then_some(result)
            }
            Self::Binary { operation, left, right, ty } => {
                let left = left.substitute(arguments, can_load)?;
                let right = right.substitute(arguments, can_load)?;
                if left.nodes() + right.nodes() >= 32 { return None; }
                Some(Self::Binary { operation: *operation, left: Arc::new(left), right: Arc::new(right), ty: *ty })
            }
            Self::Constant(_) | Self::Function { .. } => Some(self.clone()),
            Self::Argument(argument, _) => arguments.get(*argument)?.clone(),
            Self::Field { argument, offset, ty, .. } => {
                if let Some(Self::LoadAt { pointer, offset: base, .. }) = arguments.get(*argument)?.as_ref() {
                    return Some(Self::LoadAt { pointer: pointer.clone(), offset: base.checked_add(*offset)?, ty: *ty });
                }
                if let Some(source @ Self::Aggregate { .. }) = arguments.get(*argument)?.as_ref() {
                    return (source.nodes() < 32).then(|| Self::Bytes { source: Arc::new(source.clone()), offset: *offset, ty: *ty });
                }
                if let Some(Self::Bytes { source, offset: base, .. }) = arguments.get(*argument)?.as_ref() {
                    return Some(Self::Bytes { source: source.clone(), offset: base.checked_add(*offset)?, ty: *ty });
                }
                if let Some(Self::Load { argument, pointer_ty, offset: base, .. }) = arguments.get(*argument)?.as_ref() {
                    return Some(Self::Load { argument: *argument, pointer_ty: *pointer_ty, offset: base.checked_add(*offset)?, ty: *ty });
                }
                let (argument, argument_ty, base) = match arguments.get(*argument)?.as_ref()? {
                    Self::Argument(argument, argument_ty) => (*argument, *argument_ty, 0i128),
                    Self::Field { argument, argument_ty, offset, .. } => (*argument, *argument_ty, *offset),
                    _ => return None,
                };
                Some(Self::Field { argument, argument_ty, offset: base.checked_add(*offset)?, ty: *ty })
            }
            Self::Load { argument, offset, ty, .. } | Self::Address { argument, offset, ty, .. } => {
                if matches!(self, Self::Load { .. }) && let Some(Self::Borrow { source, .. }) = arguments.get(*argument)?.as_ref() {
                    return (source.nodes() < 32).then(|| Self::Bytes { source: source.clone(), offset: *offset, ty: *ty });
                }
                if !can_load && matches!(self, Self::Load { .. }) { return None; }
                if matches!(self, Self::Load { .. }) && let Some(pointer) = arguments.get(*argument)?.as_ref()
                    && !matches!(pointer, Self::Argument(..) | Self::Address { .. }) {
                    return (pointer.nodes() < 32 && pointer.ty().is_any_ptr()).then(|| Self::LoadAt {
                        pointer: Arc::new(pointer.clone()), offset: *offset, ty: *ty });
                }
                let (argument, pointer_ty, base) = match arguments.get(*argument)?.as_ref()? {
                    Self::Argument(argument, pointer_ty) => (*argument, *pointer_ty, 0i128),
                    Self::Address { argument, pointer_ty, offset, .. } => (*argument, *pointer_ty, *offset),
                    _ => return None,
                };
                let offset = base.checked_add(*offset)?;
                Some(if matches!(self, Self::Load { .. }) { Self::Load { argument, pointer_ty, offset, ty: *ty } }
                    else { Self::Address { argument, pointer_ty, offset, ty: *ty } })
            }
        }
    }

    fn resolve(tcx: TyCtxt<'tcx>, instance: Instance<'tcx>, operand: &Operand<'tcx>,
        locals: &[Option<Self>], index: &BodyIndex<'tcx>, can_load: bool) -> Option<Self>
    {
        if let Operand::Copy(place) | Operand::Move(place) = operand
            && !place.projection.is_empty()
            && matches!(locals[place.local.as_usize()], Some(Self::Aggregate { .. })) {
            let mut source = locals[place.local.as_usize()].as_ref()?;
            for projection in place.projection {
                match (projection, source) {
                    (ProjectionElem::Field(field, _), Self::Aggregate { fields, .. }) => source = fields.get(field.as_usize())?.2.as_ref()?,
                    (ProjectionElem::Downcast(_, selected), Self::Aggregate { variant: Some(variant), .. }) if selected == *variant => {}
                    _ => return None,
                }
            }
            return Some(source.clone());
        }
        match operand {
            Operand::Copy(place) | Operand::Move(place) if place.projection.is_empty() => locals[place.local.as_usize()].clone(),
            Operand::Copy(place) | Operand::Move(place) if !place.projection.is_empty()
                && (can_load || place.projection.first() != Some(&ProjectionElem::Deref)) => {
                let dereference = place.projection.first() == Some(&ProjectionElem::Deref);
                let source = locals[place.local.as_usize()].as_ref()?;
                let mut offset = 0i128;
                let normalize = |ty| instance.instantiate_mir_and_normalize_erasing_regions(tcx,
                    TypingEnv::fully_monomorphized(), EarlyBinder::bind(tcx, ty));
                let mut variant = None;
                for position in usize::from(dereference)..place.projection.len() {
                    if let ProjectionElem::Downcast(_, selected) = place.projection[position] {
                        variant = Some(selected);
                        continue;
                    }
                    let ProjectionElem::Field(field, _) = place.projection[position] else { return None; };
                    let prefix = Place { local: place.local, projection: tcx.mk_place_elems(&place.projection[..position]) };
                    let ty = normalize(prefix.ty(index.body, tcx).ty);
                    let layout = tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(ty)).ok()?;
                    let layout = if matches!(ty.kind(), ty::Adt(adt, _) if adt.is_enum()) {
                        layout.for_variant(&ty::layout::LayoutCx::new(tcx, TypingEnv::fully_monomorphized()), variant?)
                    } else { layout };
                    variant = None;
                    offset = offset.checked_add(i128::from(layout.fields.offset(field.as_usize()).bytes()))?;
                }
                let ty = normalize(place.ty(index.body, tcx).ty);
                match source {
                    Self::Borrow { source, .. } if dereference && source.nodes() < 32 =>
                        Some(Self::Bytes { source: source.clone(), offset, ty }),
                    Self::Argument(argument, pointer_ty) => Some(if dereference {
                        Self::Load { argument: *argument, pointer_ty: *pointer_ty, offset, ty }
                    } else { Self::Field { argument: *argument, argument_ty: *pointer_ty, offset, ty } }),
                    Self::Address { argument, pointer_ty, offset: base, .. } if dereference =>
                        Some(Self::Load { argument: *argument, pointer_ty: *pointer_ty, offset: base.checked_add(offset)?, ty }),
                    _ if dereference && source.ty().is_any_ptr() && source.nodes() < 32 =>
                        Some(Self::LoadAt { pointer: Arc::new(source.clone()), offset, ty }),
                    Self::Load { argument, pointer_ty, offset: base, .. } if !dereference =>
                        Some(Self::Load { argument: *argument, pointer_ty: *pointer_ty, offset: base.checked_add(offset)?, ty }),
                    Self::LoadAt { pointer, offset: base, .. } if !dereference =>
                        Some(Self::LoadAt { pointer: pointer.clone(), offset: base.checked_add(offset)?, ty }),
                    _ if !dereference && source.nodes() < 32 =>
                        Some(Self::Bytes { source: Arc::new(source.clone()), offset, ty }),
                    _ => None,
                }
            }
            Operand::Constant(constant) => {
                let mut constant = (**constant).clone();
                constant.const_ = instance.instantiate_mir_and_normalize_erasing_regions(tcx,
                    TypingEnv::fully_monomorphized(), EarlyBinder::bind(tcx, constant.const_));
                Some(Self::Constant(Operand::Constant(Box::new(constant))))
            }
            _ => None,
        }
    }

    pub(super) fn operand<'s>(&'s self, solver: &Solver<'_, 'tcx>, point: Point,
        args: &'s [rustc_span::Spanned<Operand<'tcx>>], body: &mir::Body<'tcx>) -> Option<&'s Operand<'tcx>>
    {
        match self {
            Self::Argument(argument, expected) | Self::Load { argument, pointer_ty: expected, .. }
                | Self::Address { argument, pointer_ty: expected, .. }
                | Self::Field { argument, argument_ty: expected, .. } => args.get(*argument).map(|argument| &argument.node)
                .filter(|operand| solver.ty(point.frame, operand.ty(body, solver.tcx)) == *expected),
            Self::Constant(operand) => Some(operand),
            Self::Binary { .. } | Self::Branches { .. } | Self::Function { .. } | Self::Aggregate { .. }
                | Self::Discriminant { .. } | Self::Bytes { .. } | Self::Pointer { .. } | Self::Metadata { .. }
                | Self::LoadAt { .. } | Self::Borrow { .. } => None,
        }
    }

    pub(super) fn valid(&self, solver: &Solver<'_, 'tcx>, point: Point,
        args: &[rustc_span::Spanned<Operand<'tcx>>], body: &mir::Body<'tcx>) -> bool
    {
        match self {
            Self::Function { .. } => true,
            Self::LoadAt { pointer, .. } => pointer.valid(solver, point, args, body),
            Self::Discriminant { source, .. } | Self::Bytes { source, .. }
                | Self::Pointer { source, .. } | Self::Metadata { source, .. } | Self::Borrow { source, .. } => source.valid(solver, point, args, body),
            Self::Aggregate { fields, .. } => fields.iter().filter_map(|(_, _, source)| source.as_ref())
                .all(|source| source.valid(solver, point, args, body)),
            Self::Binary { left, right, .. } => left.valid(solver, point, args, body) && right.valid(solver, point, args, body),
            Self::Branches { alternatives, .. } => alternatives.iter().all(|(conditions, source)|
                source.valid(solver, point, args, body) && conditions.iter().all(|(source, _, _)| source.valid(solver, point, args, body))),
            _ => self.operand(solver, point, args, body).is_some(),
        }
    }

    fn normalize_branches(self, tcx: TyCtxt<'tcx>) -> Option<Self> {
        let Self::Branches { alternatives, ty } = self else { return Some(self); };
        let mut result = Vec::new();
        for (conditions, source) in alternatives {
            let mut paths = vec![Vec::new()];
            for (condition, accepted, otherwise) in conditions {
                let expanded = if let Self::Discriminant { source, ty } = &condition
                    && let Self::Branches { alternatives, ty: source_ty } = &**source
                    && let ty::Adt(adt, _) = source_ty.kind()
                    && adt.is_enum()
                    && let Ok(layout) = tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(*ty))
                    && (1..=128).contains(&layout.size.bits())
                    && alternatives.iter().all(|(_, source)| matches!(source, Self::Aggregate { variant: Some(_), ty, .. } if ty == source_ty))
                {
                    let mask = u128::MAX >> (128 - layout.size.bits());
                    Some(alternatives.iter().filter_map(|(conditions, source)| {
                        let Self::Aggregate { variant: Some(variant), .. } = source else { unreachable!() };
                        let bits = adt.discriminant_for_variant(tcx, *variant).val & mask;
                        (accepted.contains(&bits) != otherwise).then(|| conditions.clone())
                    }).collect::<Vec<_>>())
                } else { None };
                if let Some(expanded) = expanded {
                    let mut following = Vec::new();
                    for path in &paths {
                        for conditions in &expanded {
                            let mut path = path.clone();
                            for condition in conditions {
                                if !path.contains(condition) { path.push(condition.clone()); }
                            }
                            if !following.contains(&path) { following.push(path); }
                        }
                    }
                    if following.len() > 8 { return None; }
                    paths = following;
                } else {
                    for path in &mut paths {
                        let condition = (condition.clone(), accepted.clone(), otherwise);
                        if !path.contains(&condition) { path.push(condition); }
                    }
                }
            }
            for path in paths {
                let alternative = (path, source.clone());
                if !result.contains(&alternative) { result.push(alternative); }
            }
            if result.len() > 8 { return None; }
        }
        let result = Self::Branches { alternatives: result, ty };
        (result.nodes() <= 128).then_some(result)
    }

    fn reads_memory(&self, arguments: &[Option<Self>]) -> bool {
        match self {
            Self::Load { argument, .. } => !matches!(arguments.get(*argument), Some(Some(Self::Borrow { .. }))),
            Self::LoadAt { pointer, .. } => !matches!(pointer.substitute(arguments, true), Some(Self::Borrow { .. })),
            Self::Discriminant { source, .. } | Self::Bytes { source, .. }
                | Self::Pointer { source, .. } | Self::Metadata { source, .. } | Self::Borrow { source, .. } => source.reads_memory(arguments),
            Self::Aggregate { fields, .. } => fields.iter().filter_map(|(_, _, source)| source.as_ref()).any(|source| source.reads_memory(arguments)),
            Self::Binary { left, right, .. } => left.reads_memory(arguments) || right.reads_memory(arguments),
            Self::Branches { alternatives, .. } => alternatives.iter().any(|(conditions, source)|
                source.reads_memory(arguments) || conditions.iter().any(|(source, _, _)| source.reads_memory(arguments))),
            _ => false,
        }
    }

    pub(super) fn covers(&self, tcx: TyCtxt<'tcx>, offset: i128, requested: Ty<'tcx>, storage: bool) -> bool {
        let size = |ty| tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(ty)).ok().map(|layout| i128::from(layout.size.bytes()));
        let Some(bytes) = size(requested) else { return false; };
        if offset < 0 || !offset.checked_add(bytes).zip(size(self.ty())).is_some_and(|(end, extent)| end <= extent) { return false; }
        match self {
            Self::Borrow { .. } => false,
            Self::Aggregate { fields, variant, ty } => {
                if storage && offset == 0 && requested == *ty {
                    return fields.iter().all(|(_, ty, source)| source.as_ref().is_some_and(|source| source.covers(tcx, 0, *ty, true)));
                }
                if variant.is_some() && ((offset == 0 && requested == *ty) || fields.is_empty()) { return true; }
                fields.iter().find(|(field, ty, _)| *field <= offset && size(*ty)
                    .is_some_and(|extent| offset + bytes <= *field + extent))
                    .is_some_and(|(field, _, source)| source.as_ref().is_some_and(|source| source.covers(tcx, offset - *field, requested, storage)))
            }
            Self::Branches { alternatives, .. } => alternatives.iter().all(|(conditions, source)| source.covers(tcx, offset, requested, storage)
                && conditions.iter().all(|(source, _, _)| source.covers(tcx, 0, source.ty(), false))),
            Self::Bytes { source, offset: field, .. } => {
                let Some(offset) = offset.checked_add(*field) else { return false; };
                if matches!(&**source, Self::Binary { .. } | Self::Function { .. } | Self::Address { .. } | Self::Discriminant { .. }
                    | Self::Pointer { .. } | Self::Metadata { .. })
                    && offset == 0 && size(source.ty()) == Some(bytes) { source.covers(tcx, 0, source.ty(), false) }
                else { source.covers(tcx, offset, requested, true) }
            }
            Self::Binary { left, right, ty, .. } => offset == 0 && requested == *ty
                && left.covers(tcx, 0, left.ty(), false) && right.covers(tcx, 0, right.ty(), false),
            Self::Pointer { source, ty } => offset == 0
                && (requested == *ty || (requested.is_any_ptr() && ty.is_any_ptr()
                    && requested.builtin_deref(true) == ty.builtin_deref(true)))
                && source.covers(tcx, 0, source.ty(), false),
            Self::Discriminant { source, ty } | Self::Metadata { source, ty } =>
                offset == 0 && requested == *ty && source.covers(tcx, 0, source.ty(), false),
            Self::LoadAt { pointer, .. } => pointer.covers(tcx, 0, pointer.ty(), false),
            Self::Function { ty, .. } | Self::Address { ty, .. } => offset == 0 && requested == *ty,
            _ => true,
        }
    }

    pub(super) fn conditions(conditions: &[(Self, Box<[u128]>, bool)], solver: &mut Solver<'_, 'tcx>, point: Point,
        args: &[rustc_span::Spanned<Operand<'tcx>>], body: &mir::Body<'tcx>) -> Result<Vec<Alternative<'tcx>>, QueryStop>
    {
        let mut paths = vec![Alternative { value: Value::Bits(1), guards: Vec::new() }];
        for (condition, accepted, otherwise) in conditions {
            if paths.iter().all(|path| path.value == Value::Bits(0)) { break; }
            let alias_object = solver.alias_object.take();
            let options = condition.values(solver, point, args, body, 0, condition.ty());
            solver.alias_object = alias_object;
            let options = options?;
            let mut following = Vec::new();
            for path in paths {
                if path.value == Value::Bits(0) { following.push(path); continue; }
                for option in &options {
                    let Some(guards) = solver.merge(&path.guards, &option.guards)? else { continue; };
                    let value = match option.value {
                        Value::Bits(bits) if accepted.contains(&bits) == *otherwise => Value::Bits(0),
                        Value::Bits(_) => path.value,
                        _ => Value::Unknown("the summary selection condition is unresolved"),
                    };
                    following.push(Alternative { value, guards });
                }
            }
            paths = solver.bounded(following)?;
            if paths.is_empty() { break; }
        }
        for path in &mut paths {
            if matches!(path.value, Value::Unknown(_)) {
                let marker = (point.frame, Guard { required: true, proof: GuardProof::Widened,
                    iteration: None, branch: point.location, successor: mir::START_BLOCK,
                    index: None, input: false, phi: false, choice: None });
                if !path.guards.contains(&marker) { path.guards.push(marker); }
            }
        }
        Ok(paths)
    }

    pub(super) fn values(&self, solver: &mut Solver<'_, 'tcx>, point: Point,
        args: &[rustc_span::Spanned<Operand<'tcx>>], body: &mir::Body<'tcx>, offset: i128, requested: Ty<'tcx>)
        -> Result<Vec<Alternative<'tcx>>, QueryStop>
    {
        solver.budget.charge(0)?;
        if matches!(self, Self::Borrow { .. }) {
            return Ok(vec![Alternative { value: Value::Unknown("the summary local borrow escapes its captured value"), guards: Vec::new() }]);
        }
        if let Self::LoadAt { pointer, offset: field, .. } = self {
            solver.effects.loads += 1;
            let mut values = Vec::new();
            for pointer in pointer.values(solver, point, args, body, 0, pointer.ty())? {
                if let Value::Pointer(mut address, _) = pointer.value {
                    address.offset = address.offset.checked_add(*field).and_then(|base| base.checked_add(offset))
                        .ok_or(QueryStop::Unsupported)?;
                    for value in solver.summary_load(address, requested, point, &pointer.guards, false)? {
                        if let Some(guards) = solver.merge(&pointer.guards, &value.guards)? {
                            values.push(Alternative { value: value.value, guards });
                        }
                    }
                } else {
                    values.push(Alternative { value: Value::Unknown("a summary load has an unknown storage address"), guards: pointer.guards });
                }
            }
            return solver.bounded(values);
        }
        if let Self::Bytes { source, offset: field, .. } = self {
            let offset = offset.checked_add(*field).ok_or(QueryStop::Unsupported)?;
            if matches!(&**source, Self::Binary { .. } | Self::Function { .. } | Self::Address { .. } | Self::Discriminant { .. }
                | Self::Pointer { .. } | Self::Metadata { .. }) {
                let size = solver.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(requested))
                    .map_err(|_| QueryStop::Unsupported)?.size;
                let source_size = solver.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(source.ty()))
                    .map_err(|_| QueryStop::Unsupported)?.size;
                if offset == 0 && size == source_size { return source.values(solver, point, args, body, 0, source.ty()); }
            }
            return source.values(solver, point, args, body, offset, requested);
        }
        if let Self::Pointer { source, ty } | Self::Metadata { source, ty } = self {
            let compatible = requested == *ty || (matches!(self, Self::Pointer { .. })
                && requested.is_any_ptr() && ty.is_any_ptr()
                && requested.builtin_deref(true) == ty.builtin_deref(true));
            if offset != 0 || !compatible {
                return Ok(vec![Alternative { value: Value::Unknown("the summary pointer operation has a partial or incompatible read"), guards: Vec::new() }]);
            }
            let mut values = source.values(solver, point, args, body, 0, source.ty())?;
            let thin = ty.builtin_deref(true).is_some_and(|pointee| solver.tcx
                .layout_of(TypingEnv::fully_monomorphized().as_query_input(pointee)).is_ok_and(|layout| layout.is_sized()));
            for value in &mut values {
                value.value = match (self, value.value) {
                    (Self::Metadata { .. }, Value::Pointer(_, Metadata::Length(length))) => Value::Bits(length),
                    (Self::Metadata { .. }, Value::Pointer(_, metadata)) => Value::Metadata(metadata),
                    (Self::Pointer { .. }, Value::Pointer(address, metadata)) =>
                        Value::Pointer(address, if thin { Metadata::Thin } else { metadata }),
                    (Self::Pointer { .. }, Value::ObjectPointer(object)) => Value::ObjectPointer(object),
                    (Self::Pointer { .. }, Value::Function(instance)) if thin => Value::Function(instance),
                    _ => Value::Unknown("the summary pointer operation has no established input pointer"),
                };
            }
            return solver.bounded(values);
        }
        if let Self::Discriminant { source, ty } = self {
            if offset != 0 || requested != *ty {
                return Ok(vec![Alternative { value: Value::Unknown("the summary discriminant has a partial or incompatible read"), guards: Vec::new() }]);
            }
            let mut values = source.values(solver, point, args, body, 0, source.ty())?;
            let width = solver.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(*ty))
                .map_err(|_| QueryStop::Unsupported)?.size.bits();
            for value in &mut values {
                value.value = match (source.ty().kind(), value.value) {
                    (ty::Adt(adt, _), Value::Variant(variant)) if (1..=128).contains(&width) => Value::Bits(
                        adt.discriminant_for_variant(solver.tcx, rustc_abi::VariantIdx::from_u32(variant)).val & (u128::MAX >> (128 - width))),
                    _ => Value::Unknown("the summary enum variant is not established"),
                };
            }
            return solver.bounded(values);
        }
        if let Self::Aggregate { fields, variant, ty } = self {
            if offset == 0 && requested == *ty && let Some(variant) = variant {
                return Ok(vec![Alternative { value: Value::Variant(variant.as_u32()), guards: Vec::new() }]);
            }
            let size = solver.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(requested))
                .map_err(|_| QueryStop::Unsupported)?.size.bytes();
            for (field, ty, source) in fields {
                let extent = solver.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(*ty))
                    .map_err(|_| QueryStop::Unsupported)?.size.bytes();
                if *field <= offset && offset + i128::from(size) <= *field + i128::from(extent) {
                    if let Some(source) = source { return source.values(solver, point, args, body, offset - *field, requested); }
                    break;
                }
            }
            return Ok(vec![Alternative { value: Value::Unknown("the summary aggregate field is not established"), guards: Vec::new() }]);
        }
        if let Self::Function { instance, ty } = self {
            return Ok(vec![Alternative { value: if offset == 0 && requested == *ty { Value::Function(*instance) }
                else { Value::Unknown("the summary function pointer has a partial or incompatible read") }, guards: Vec::new() }]);
        }
        if let Self::Branches { alternatives, .. } = self {
            let mut values = Vec::new();
            for (conditions, source) in alternatives {
                let paths: Vec<_> = Self::conditions(conditions, solver, point, args, body)?.into_iter()
                    .filter(|path| path.value != Value::Bits(0)).collect();
                if paths.is_empty() { continue; }
                for value in source.values(solver, point, args, body, offset, requested)? {
                    for path in &paths {
                        if let Some(guards) = solver.merge(&value.guards, &path.guards)? {
                            values.push(Alternative { value: value.value, guards });
                        }
                    }
                }
            }
            return solver.bounded(values);
        }
        if let Self::Binary { operation, left, right, ty } = self {
            if offset != 0 || requested != *ty {
                return Ok(vec![Alternative { value: Value::Unknown("the summary expression has a partial or incompatible read"), guards: Vec::new() }]);
            }
            let left_ty = left.ty();
            let right_ty = right.ty();
            let left_values = left.values(solver, point, args, body, 0, left_ty)?;
            if *operation == mir::BinOp::Offset
                && solver.alias_object.is_some_and(|object| !left_values.is_empty() && left_values.iter().all(|value| match value.value {
                    Value::Pointer(address, _) => address.object != object,
                    Value::ObjectPointer(target) => target != object,
                    _ => false,
                })) {
                return solver.bounded(left_values.into_iter().map(|mut value| {
                    value.value = match value.value {
                        Value::Pointer(address, _) => Value::ObjectPointer(address.object),
                        Value::ObjectPointer(object) => Value::ObjectPointer(object),
                        _ => Value::Unknown("the summary offset has no established input object"),
                    };
                    value
                }).collect());
            }
            let right_values = right.values(solver, point, args, body, 0, right_ty)?;
            let width = solver.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(left_ty))
                .map_err(|_| QueryStop::Unsupported)?.size.bits();
            let mut values = Vec::new();
            for left in left_values {
                for right in &right_values {
                    let Some(guards) = solver.merge(&left.guards, &right.guards)? else { continue; };
                    let value = match (left.value, right.value) {
                        (Value::Symbolic(origin, bias, source_width), Value::Bits(bits)) if source_width == width && (1..=128).contains(&width) => {
                            let mask = u128::MAX >> (128 - width);
                            match operation {
                                mir::BinOp::Eq | mir::BinOp::Ne => Value::Predicate(origin, width, bits.wrapping_sub(bias) & mask, *operation == mir::BinOp::Eq),
                                mir::BinOp::Add | mir::BinOp::AddUnchecked => Value::Symbolic(origin, bias.wrapping_add(bits) & mask, width),
                                mir::BinOp::Sub | mir::BinOp::SubUnchecked => Value::Symbolic(origin, bias.wrapping_sub(bits) & mask, width),
                                _ => Value::Unknown("the summary symbolic arithmetic is not supported"),
                            }
                        }
                        (Value::Bits(bits), Value::Symbolic(origin, bias, source_width)) if source_width == width && (1..=128).contains(&width) => {
                            let mask = u128::MAX >> (128 - width);
                            match operation {
                                mir::BinOp::Eq | mir::BinOp::Ne => Value::Predicate(origin, width, bits.wrapping_sub(bias) & mask, *operation == mir::BinOp::Eq),
                                mir::BinOp::Add | mir::BinOp::AddUnchecked => Value::Symbolic(origin, bias.wrapping_add(bits) & mask, width),
                                _ => Value::Unknown("the summary symbolic arithmetic is not supported"),
                            }
                        }
                        (Value::Pointer(left, _), Value::Pointer(right, _)) if width == solver.tcx.data_layout.pointer_size().bits()
                            && left.object == right.object && solver.singleton_object(left.object)? => {
                            let mask = u128::MAX >> (128 - width);
                            let equal = left.offset as u128 & mask == right.offset as u128 & mask;
                            match operation {
                                mir::BinOp::Eq => Value::Bits(u128::from(equal)),
                                mir::BinOp::Ne => Value::Bits(u128::from(!equal)),
                                _ => Value::Unknown("the summary pointer comparison is not supported"),
                            }
                        }
                        (Value::Bits(left), Value::Bits(right)) if (1..=128).contains(&width)
                            && matches!(left_ty.kind(), ty::Uint(_) | ty::Int(_) | ty::Bool) => {
                            let mask = u128::MAX >> (128 - width);
                            let left = left & mask;
                            let right = right & mask;
                            let signed_left = ((left << (128 - width)) as i128) >> (128 - width);
                            let signed_right = ((right << (128 - width)) as i128) >> (128 - width);
                            let bits = match operation {
                                mir::BinOp::Add | mir::BinOp::AddUnchecked => Some(left.wrapping_add(right) & mask),
                                mir::BinOp::Sub | mir::BinOp::SubUnchecked => Some(left.wrapping_sub(right) & mask),
                                mir::BinOp::Mul | mir::BinOp::MulUnchecked => Some(left.wrapping_mul(right) & mask),
                                mir::BinOp::BitAnd => Some(left & right),
                                mir::BinOp::BitOr => Some(left | right),
                                mir::BinOp::BitXor => Some(left ^ right),
                                mir::BinOp::Eq => Some(u128::from(left == right)),
                                mir::BinOp::Ne => Some(u128::from(left != right)),
                                mir::BinOp::Lt => Some(u128::from(if left_ty.is_signed() { signed_left < signed_right } else { left < right })),
                                mir::BinOp::Le => Some(u128::from(if left_ty.is_signed() { signed_left <= signed_right } else { left <= right })),
                                mir::BinOp::Gt => Some(u128::from(if left_ty.is_signed() { signed_left > signed_right } else { left > right })),
                                mir::BinOp::Ge => Some(u128::from(if left_ty.is_signed() { signed_left >= signed_right } else { left >= right })),
                                _ => None,
                            };
                            bits.map_or(Value::Unknown("the summary arithmetic is not supported"), Value::Bits)
                        }
                        (Value::Pointer(mut address, metadata), Value::Bits(amount)) if *operation == mir::BinOp::Offset => {
                            let amount_width = solver.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(right_ty))
                                .map_err(|_| QueryStop::Unsupported)?.size.bits();
                            let stride = left_ty.builtin_deref(true).and_then(|pointee| solver.tcx
                                .layout_of(TypingEnv::fully_monomorphized().as_query_input(pointee)).ok()).map(|layout| layout.size.bytes());
                            let amount = if (1..=128).contains(&amount_width) {
                                if right_ty.is_signed() { Some(((amount << (128 - amount_width)) as i128) >> (128 - amount_width)) }
                                else { i128::try_from(amount).ok() }
                            } else { None };
                            if let Some(offset) = amount.zip(stride).and_then(|(amount, stride)| amount.checked_mul(i128::from(stride)))
                                .and_then(|delta| address.offset.checked_add(delta)) {
                                address.offset = offset;
                                Value::Pointer(address, metadata)
                            } else { Value::Unknown("the summary pointer offset cannot be represented") }
                        }
                        (Value::ObjectPointer(object), _) if *operation == mir::BinOp::Offset => Value::ObjectPointer(object),
                        (Value::Pointer(address, _), _) if *operation == mir::BinOp::Offset
                            && solver.alias_object.is_some_and(|object| object != address.object) => Value::ObjectPointer(address.object),
                        _ => Value::Unknown("the summary expression has an unknown input"),
                    };
                    if matches!(value, Value::Unknown(_)) && solver.effects.value_traces < 64
                        && std::env::var("IRQ_CHECK_TRACE_SUMMARY_VALUES")
                            .is_ok_and(|filter| solver.frames[point.frame].instance.to_string().contains(&filter)) {
                        solver.effects.value_traces += 1;
                        eprintln!("irq-check: summary unknown {} frame={} at={:?} operation={operation:?} alias={:?} left={:?} right={:?} guards={guards:?}",
                            solver.frames[point.frame].instance, point.frame, point.location, solver.alias_object, left, right);
                    }
                    values.push(Alternative { value, guards });
                }
            }
            return solver.bounded(values);
        }
        let operand = self.operand(solver, point, args, body).ok_or(QueryStop::Unsupported)?;
        if let Self::Field { offset: field, .. } = self {
            return solver.summary_operand(point, operand, body, offset.checked_add(*field).ok_or(QueryStop::Unsupported)?, requested);
        }
        if let Self::Address { pointer_ty, offset: field, ty, .. } = self {
            if offset != 0 || requested != *ty || !ty.builtin_deref(true).is_some_and(|pointee|
                solver.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(pointee)).is_ok_and(|layout| layout.is_sized())) {
                return Ok(vec![Alternative { value: Value::Unknown("the summary address has unsupported metadata or a partial read"), guards: Vec::new() }]);
            }
            let mut values = solver.summary_operand(point, operand, body, 0, *pointer_ty)?;
            for value in &mut values {
                value.value = match value.value {
                    Value::Pointer(mut address, _) => {
                        address.offset = address.offset.checked_add(*field).ok_or(QueryStop::Unsupported)?;
                        Value::Pointer(address, Metadata::Thin)
                    }
                    _ => Value::Unknown("the summary address has no established input pointer"),
                };
            }
            return Ok(values);
        }
        let Self::Load { offset: field, pointer_ty, .. } = self else {
            return solver.summary_operand(point, operand, body, offset, requested);
        };
        solver.effects.loads += 1;
        let mut values = Vec::new();
        for pointer in solver.summary_operand(point, operand, body, 0, *pointer_ty)? {
            if let Value::Pointer(mut address, _) = pointer.value {
                address.offset = address.offset.checked_add(*field).and_then(|offset_at| offset_at.checked_add(offset))
                    .ok_or(QueryStop::Unsupported)?;
                for value in solver.summary_load(address, requested, point, &pointer.guards, false)? {
                    if let Some(guards) = solver.merge(&pointer.guards, &value.guards)? {
                        values.push(Alternative { value: value.value, guards });
                    }
                }
            } else {
                values.push(Alternative { value: Value::Unknown("a summary load has an unknown storage address"), guards: pointer.guards });
            }
        }
        solver.bounded(values)
    }
}

#[derive(PartialEq)]
pub(super) struct Store<'tcx> {
    pub conditions: Vec<(Source<'tcx>, Box<[u128]>, bool)>,
    pub destination: Source<'tcx>,
    pub offset: i128,
    pub bytes: u64,
    pub source: Source<'tcx>,
}

#[derive(PartialEq)]
pub(super) struct Summary<'tcx> {
    pub stores: Vec<Store<'tcx>>,
    pub returned: Option<Source<'tcx>>,
}

impl<'tcx> Summary<'tcx> {
    fn push(&mut self, store: Store<'tcx>) {
        if store.conditions.is_empty() {
            self.stores.retain(|previous| previous.destination != store.destination
                || previous.offset != store.offset || previous.bytes != store.bytes);
        }
        self.stores.push(store);
    }
}

#[derive(Default)]
pub(crate) struct EffectCache<'tcx> {
    summaries: HashMap<(Instance<'tcx>, bool), Option<Arc<Summary<'tcx>>>>,
    active: HashSet<(Instance<'tcx>, bool)>,
    pending: HashMap<(Instance<'tcx>, bool), (Option<Arc<Summary<'tcx>>>, usize)>,
    evaluated: HashSet<(Instance<'tcx>, bool)>,
    pub(super) solving: bool,
    changed: bool,
    pending_bytes: usize,
    pub(super) normal: HashMap<(Instance<'tcx>, bool), bool>,
    pub(crate) bytes: usize,
    pub(crate) hits: usize,
    pub(crate) returned: usize,
    pub(crate) writes: usize,
    pub(crate) loads: usize,
    pub(crate) cycles: usize,
    pub(crate) iterations: usize,
    value_traces: usize,
}

impl<'a, 'tcx> Solver<'a, 'tcx> {
    fn summary_operand(&mut self, point: Point, operand: &Operand<'tcx>, body: &mir::Body<'tcx>, offset: i128, ty: Ty<'tcx>)
        -> Result<Vec<Alternative<'tcx>>, QueryStop>
    {
        let (Operand::Copy(place) | Operand::Move(place)) = operand else { return self.operand(point, operand, offset, ty); };
        if matches!(self.ty(point.frame, operand.ty(body, self.tcx)).kind(), ty::FnDef(..)) {
            return self.operand(point, operand, offset, ty);
        }
        let alias = self.alias_object.take();
        let addresses = self.address(point, *place, None, None);
        self.alias_object = alias;
        let mut result = Vec::new();
        for address in addresses? {
            let Value::Pointer(mut storage, _) = address.value else {
                result.push(Alternative { value: Value::Unknown("the summary argument storage is unknown"), guards: address.guards });
                continue;
            };
            storage.offset = storage.offset.checked_add(offset).ok_or(QueryStop::Unsupported)?;
            let capture = matches!(storage.object, Object::Local(owner, _) if owner == point.frame);
            for value in self.summary_load(storage, ty, point, &address.guards, capture)? {
                if let Some(guards) = self.merge(&address.guards, &value.guards)? {
                    result.push(Alternative { value: value.value, guards });
                }
            }
        }
        self.bounded(result)
    }

    fn summary_load(&mut self, address: Address, ty: Ty<'tcx>, point: Point, guards: &[(usize, Guard)], capture: bool)
        -> Result<Vec<Alternative<'tcx>>, QueryStop>
    {
        let values = self.read_guarded(address, ty, point, guards)?;
        if (!capture && self.flow.recursive.contains_key(&point.frame))
            || !self.singleton_object(address.object)? { return Ok(values); }
        let complete = self.flow.current.is_none() || self.flow.inputs_complete(&self.flow.current_inputs);
        if self.flow.singleton_read(&values, complete) { return Ok(values); }
        let source = self.choice_value(Value::Pointer(address, Metadata::Thin), ty)?;
        let mut result = Vec::new();
        for mut value in values {
            if matches!(value.value, Value::Pointer(..) | Value::Bits(_) | Value::Variant(_) | Value::Function(_)) {
                let target = self.choice_value(match value.value {
                    Value::Pointer(address, _) => Value::Pointer(address, Metadata::Thin),
                    value => value,
                }, ty)?;
                let guard = (point.frame, Guard { required: true, proof: GuardProof::Pending, iteration: None,
                    branch: point.location, successor: mir::START_BLOCK, index: None, input: false,
                    phi: false, choice: Some((source, target)) });
                let Some(guards) = self.merge(&value.guards, &[guard])? else { continue; };
                value.guards = guards;
            }
            result.push(value);
        }
        self.bounded(result)
    }

    pub(super) fn effect_summary(&mut self, instance: Instance<'tcx>, irq: bool)
        -> Result<Option<Arc<Summary<'tcx>>>, QueryStop>
    {
        let key = (instance, irq);
        if let Some(summary) = self.effects.summaries.get(&key) {
            self.effects.hits += 1;
            return Ok(summary.clone());
        }
        if self.effects.solving {
            if self.effects.active.contains(&key) || self.effects.evaluated.contains(&key) {
                if self.effects.active.contains(&key) { self.effects.cycles += 1; }
                return Ok(self.effects.pending.get(&key).map(|(summary, _)| summary.clone())
                    .unwrap_or_else(|| Some(Arc::new(Summary { stores: Vec::new(), returned: None }))));
            }
            let summary = self.effect_step(instance, irq)?;
            self.effects.evaluated.insert(key);
            if self.effects.pending.get(&key).map(|(summary, _)| summary) != Some(&summary) {
                let bytes = std::mem::size_of::<(Instance<'tcx>, bool)>() * 4
                    + summary.as_ref().map_or(0, |summary| std::mem::size_of::<Summary<'tcx>>() * 4
                        + summary.stores.iter().map(|store| (store.source.nodes() + store.destination.nodes()
                            + store.conditions.iter().map(|(source, values, _)| source.nodes() + values.len()).sum::<usize>())
                            * std::mem::size_of::<Source<'tcx>>() * 4).sum::<usize>()
                        + summary.returned.as_ref().map_or(0, |source| source.nodes() * std::mem::size_of::<Source<'tcx>>() * 4));
                let previous = self.effects.pending.get(&key).map_or(0, |(_, bytes)| *bytes);
                self.effects.pending_bytes = self.effects.pending_bytes.saturating_sub(previous).saturating_add(bytes);
                if self.effects.pending_bytes > self.cache_limit / 32 { return Err(QueryStop::RefinementLimit); }
                self.budget.reserve(bytes.saturating_sub(previous))?;
                self.effects.pending.insert(key, (summary.clone(), bytes));
                self.effects.changed = true;
            }
            return Ok(summary);
        }
        let scratch = self.budget.scratch;
        self.effects.solving = true;
        let answer = (|| {
            let limit = irq_check_rupta::limits::configuration().expect("validated analysis limits").alternatives.min(32);
            for _ in 0..limit {
                self.effects.iterations += 1;
                self.budget.scratch = scratch;
                self.budget.charge(self.effects.pending_bytes)?;
                self.effects.changed = false;
                self.effects.evaluated.clear();
                let result = self.effect_summary(instance, irq)?;
                if !self.effects.changed { return Ok((result, true)); }
            }
            Ok((None, false))
        })();
        self.effects.solving = false;
        self.effects.active.clear();
        let evaluated = std::mem::take(&mut self.effects.evaluated);
        let pending = std::mem::take(&mut self.effects.pending);
        let bytes = std::mem::take(&mut self.effects.pending_bytes);
        self.budget.scratch = scratch;
        let (result, complete) = answer?;
        if complete {
            if self.effects.bytes.saturating_add(bytes) > self.cache_limit / 32 {
                self.effects.summaries.clear();
                self.effects.normal.clear();
                self.effects.bytes = 0;
            }
            if bytes <= self.cache_limit / 32 {
                self.budget.reserve(bytes)?;
                for (key, (summary, _)) in pending {
                    if evaluated.contains(&key) { self.effects.summaries.insert(key, summary); }
                }
                self.effects.bytes += bytes;
            }
        }
        Ok(result)
    }

    fn effect_step(&mut self, instance: Instance<'tcx>, irq: bool)
        -> Result<Option<Arc<Summary<'tcx>>>, QueryStop>
    {
        let key = (instance, irq);
        if self.tcx.is_foreign_item(instance.def_id())
            || matches!(instance.def, InstanceKind::Intrinsic(_) | InstanceKind::LlvmIntrinsic(_) | InstanceKind::Virtual(..))
            || !irq_check_rupta::util::type_util::has_instance_mir(self.tcx, instance.def) { return Ok(None); }
        if self.tcx.crate_name(instance.def_id().krate).as_str() == "core"
            && self.tcx.def_path_str(instance.def_id()) == "core::sync::atomic::atomic_compare_exchange" {
            let body = self.tcx.instance_mir(instance.def);
            let normalize = |ty| instance.instantiate_mir_and_normalize_erasing_regions(self.tcx,
                TypingEnv::fully_monomorphized(), EarlyBinder::bind(self.tcx, ty));
            if body.arg_count == 5 {
                let pointer_ty = normalize(body.local_decls[mir::Local::from_usize(1)].ty);
                let value_ty = normalize(body.local_decls[mir::Local::from_usize(2)].ty);
                let failure_ty = normalize(body.local_decls[mir::Local::from_usize(5)].ty);
                let result_ty = normalize(body.local_decls[mir::RETURN_PLACE].ty);
                if pointer_ty.builtin_deref(true) == Some(value_ty)
                    && value_ty.is_integral()
                    && normalize(body.local_decls[mir::Local::from_usize(3)].ty) == value_ty
                    && let ty::Adt(ordering, _) = failure_ty.kind()
                    && let ty::Adt(result, _) = result_ty.kind()
                    && ordering.variants().len() == 5 && result.variants().len() == 2 {
                    let valid_orderings: Box<[_]> = ordering.variants().iter_enumerated()
                        .filter(|(_, variant)| matches!(variant.name.as_str(), "Relaxed" | "Acquire" | "SeqCst"))
                        .map(|(variant, _)| ordering.discriminant_for_variant(self.tcx, variant).val).collect();
                    let ok = result.variants().iter_enumerated().find(|(_, variant)| variant.name.as_str() == "Ok");
                    let error = result.variants().iter_enumerated().find(|(_, variant)| variant.name.as_str() == "Err");
                    if valid_orderings.len() == 3 && let (Some((ok, _)), Some((error, _))) = (ok, error) {
                        let value_layout = self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(value_ty))
                            .map_err(|_| QueryStop::Unsupported)?;
                        let result_layout = self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(result_ty))
                            .map_err(|_| QueryStop::Unsupported)?;
                        let previous = Source::Load { argument: 0, pointer_ty, offset: 0, ty: value_ty };
                        let equal = Source::Binary { operation: mir::BinOp::Eq, left: Arc::new(previous.clone()),
                            right: Arc::new(Source::Argument(1, value_ty)), ty: self.tcx.types.bool };
                        let valid = (Source::Discriminant { source: Arc::new(Source::Argument(4, failure_ty)),
                            ty: failure_ty.discriminant_ty(self.tcx) }, valid_orderings, false);
                        let success = vec![valid.clone(), (equal.clone(), Box::new([1]), false)];
                        let failure = vec![valid, (equal, Box::new([0]), false)];
                        let mut alternatives = Vec::new();
                        for (variant, conditions) in [(ok, success.clone()), (error, failure)] {
                            let layout = result_layout.for_variant(&ty::layout::LayoutCx::new(self.tcx,
                                TypingEnv::fully_monomorphized()), variant);
                            if layout.fields.count() != 1 { return Ok(None); }
                            alternatives.push((conditions, Source::Aggregate {
                                fields: vec![(i128::from(layout.fields.offset(0).bytes()), value_ty, Some(previous.clone()))],
                                variant: Some(variant), ty: result_ty,
                            }));
                        }
                        self.budget.reserve(std::mem::size_of::<Source<'tcx>>() * 64)?;
                        if std::env::var("IRQ_CHECK_TRACE_EFFECT").is_ok_and(|filter| instance.to_string().contains(&filter)) {
                            eprintln!("irq-check: atomic effect summary {instance} return_cases=2 stores=1");
                        }
                        return Ok(Some(Arc::new(Summary {
                            stores: vec![Store { conditions: success, destination: Source::Argument(0, pointer_ty),
                                offset: 0, bytes: value_layout.size.bytes(), source: Source::Argument(2, value_ty) }],
                            returned: Some(Source::Branches { alternatives, ty: result_ty }),
                        })));
                    }
                }
            }
        }
        let limits = irq_check_rupta::limits::configuration().expect("validated analysis limits");
        if self.effects.active.len() >= limits.call_depth { return Err(QueryStop::RefinementLimit); }
        if !self.effects.active.insert(key) { return Ok(None); }
        let scratch = self.budget.scratch;
        let retained = self.effects.pending_bytes;
        let answer = (|| {
            let index = if let Some(index) = self.indexes.get(&key) { index.clone() } else {
                let index = Arc::new(BodyIndex::build(self.tcx, self.trust, instance, irq)?);
                self.indexes.insert(key, index.clone());
                index
            };
            let body = index.body;
            let limits = irq_check_rupta::limits::configuration().expect("validated analysis limits");
            let mut bytes = body.local_decls.len().saturating_mul(std::mem::size_of::<Option<Source<'tcx>>>() * 4);
            if bytes > limits.function_scratch_mib * 1024 * 1024 {
                self.budget.record_limit(QueryStop::ScratchLimit, "function effect-summary bytes", bytes, limits.function_scratch_mib * 1024 * 1024);
                return Err(QueryStop::ScratchLimit);
            }
            self.budget.reserve(bytes)?;
            let tcx = self.tcx;
            let normalize = |ty| instance.instantiate_mir_and_normalize_erasing_regions(tcx,
                TypingEnv::fully_monomorphized(), EarlyBinder::bind(tcx, ty));
            let mut locals = vec![None; body.local_decls.len()];
            for argument in body.args_iter() {
                locals[argument.as_usize()] = Some(Source::Argument(argument.as_usize() - 1, normalize(body.local_decls[argument].ty)));
            }
            let mut summary = Summary { stores: Vec::new(), returned: None };
            let mut block = mir::START_BLOCK;
            let mut visited = HashSet::new();
            let mut pending = Vec::new();
            let mut conditions = Vec::new();
            let mut alternatives = Vec::new();
            let mut branched = false;
            let mut finished = 0usize;
            let mut returns_complete = true;
            loop {
                self.budget.charge(std::mem::size_of::<mir::BasicBlock>() * 4)?;
                if !visited.insert(block) || index.stops[block.as_usize()].is_some() { return Ok(None); }
                let data = &body.basic_blocks[block];
                for statement in &data.statements {
                    self.budget.charge(0)?;
                    match &statement.kind {
                        StatementKind::Assign(assignment) => {
                            let (place, value) = &**assignment;
                            if matches!(value, Rvalue::ThreadLocalRef(_)) { return Ok(None); }
                            let source = match value {
                                Rvalue::Use(operand, _) => Source::resolve(self.tcx, instance, operand, &locals, &index, summary.stores.is_empty()),
                                Rvalue::UnaryOp(mir::UnOp::PtrMetadata, operand) => {
                                    let source_ty = normalize(operand.ty(body, self.tcx));
                                    if source_ty.builtin_deref(true).is_some_and(|pointee| matches!(pointee.kind(), ty::Slice(_) | ty::Str)) {
                                        Source::resolve(self.tcx, instance, operand, &locals, &index, summary.stores.is_empty())
                                            .filter(|source| source.nodes() < 32).map(|source| Source::Metadata {
                                                source: Arc::new(source), ty: normalize(place.ty(body, self.tcx).ty) })
                                    } else { None }
                                }
                                Rvalue::Cast(mir::CastKind::PtrToPtr, operand, destination) => {
                                    let source_ty = normalize(operand.ty(body, self.tcx));
                                    let ty = normalize(*destination);
                                    let compatible = ty.builtin_deref(true).is_some_and(|pointee| self.tcx
                                        .layout_of(TypingEnv::fully_monomorphized().as_query_input(pointee)).is_ok_and(|layout| layout.is_sized()))
                                        || source_ty.builtin_deref(true) == ty.builtin_deref(true);
                                    if source_ty.is_any_ptr() && ty.is_any_ptr() && compatible {
                                        Source::resolve(self.tcx, instance, operand, &locals, &index, summary.stores.is_empty())
                                            .filter(|source| source.nodes() < 32).map(|source| Source::Pointer { source: Arc::new(source), ty })
                                    } else { None }
                                }
                                Rvalue::Cast(mir::CastKind::Transmute, operand, destination) => {
                                    let source_ty = normalize(operand.ty(body, self.tcx));
                                    let ty = normalize(*destination);
                                    let equal = self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(source_ty)).ok()
                                        .zip(self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(ty)).ok())
                                        .is_some_and(|(source, target)| source.size == target.size);
                                    if equal { Source::resolve(self.tcx, instance, operand, &locals, &index, summary.stores.is_empty())
                                        .filter(|source| source.nodes() < 32).map(|source| Source::Bytes { source: Arc::new(source), offset: 0, ty }) }
                                    else { None }
                                }
                                Rvalue::Aggregate(kind, operands) if operands.len() <= 16 => {
                                    let ty = normalize(place.ty(body, self.tcx).ty);
                                    let layout = self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(ty))
                                        .map_err(|_| QueryStop::Unsupported)?;
                                    let variant = match &**kind {
                                        mir::AggregateKind::Adt(def, variant, ..) if !self.tcx.adt_def(*def).is_union() => Some(*variant),
                                        mir::AggregateKind::Tuple => None,
                                        _ => return Ok(None),
                                    };
                                    let layout = variant.map_or(layout, |variant| layout.for_variant(
                                        &ty::layout::LayoutCx::new(self.tcx, TypingEnv::fully_monomorphized()), variant));
                                    let fields = operands.iter_enumerated().map(|(field, operand)| (
                                        i128::from(layout.fields.offset(field.as_usize()).bytes()), normalize(operand.ty(body, self.tcx)),
                                        Source::resolve(self.tcx, instance, operand, &locals, &index, summary.stores.is_empty()))).collect();
                                    let variant = variant.filter(|_| matches!(ty.kind(), ty::Adt(adt, _) if adt.is_enum()));
                                    let source = Source::Aggregate { fields, variant, ty };
                                    (source.nodes() <= 32).then_some(source)
                                }
                                Rvalue::Discriminant(source) => Source::resolve(self.tcx, instance, &Operand::Copy(*source),
                                    &locals, &index, summary.stores.is_empty()).filter(|source| source.nodes() < 32)
                                    .map(|source| Source::Discriminant { source: Arc::new(source), ty: normalize(place.ty(body, self.tcx).ty) }),
                                Rvalue::Cast(mir::CastKind::PointerCoercion(ty::adjustment::PointerCoercion::ReifyFnPointer(_), _), operand, destination) => {
                                    if let ty::FnDef(def, args) = *normalize(operand.ty(body, self.tcx)).kind() {
                                        Instance::resolve_for_fn_ptr(self.tcx, TypingEnv::fully_monomorphized(), def,
                                            self.tcx.instantiate_bound_regions_with_erased(args))
                                            .map(|instance| Source::Function { instance, ty: normalize(*destination) })
                                    } else { None }
                                }
                                Rvalue::BinaryOp(operation, operands) if matches!(operation,
                                    mir::BinOp::Add | mir::BinOp::AddUnchecked | mir::BinOp::Sub | mir::BinOp::SubUnchecked
                                    | mir::BinOp::Mul | mir::BinOp::MulUnchecked | mir::BinOp::BitAnd | mir::BinOp::BitOr
                                    | mir::BinOp::BitXor | mir::BinOp::Eq | mir::BinOp::Ne | mir::BinOp::Lt | mir::BinOp::Le
                                    | mir::BinOp::Gt | mir::BinOp::Ge | mir::BinOp::Offset) => {
                                    Source::resolve(self.tcx, instance, &operands.0, &locals, &index, summary.stores.is_empty())
                                        .zip(Source::resolve(self.tcx, instance, &operands.1, &locals, &index, summary.stores.is_empty()))
                                        .filter(|(left, right)| left.nodes() + right.nodes() < 32)
                                        .map(|(left, right)| Source::Binary { operation: *operation, left: Arc::new(left), right: Arc::new(right),
                                            ty: normalize(place.ty(body, self.tcx).ty) })
                                }
                                Rvalue::Ref(_, _, source) | Rvalue::RawPtr(_, source) => {
                                    if source.projection.is_empty() && matches!(value, Rvalue::Ref(_, mir::BorrowKind::Shared, _)) {
                                        locals[source.local.as_usize()].clone().filter(|source| source.nodes() < 32)
                                            .map(|source| Source::Borrow { source: Arc::new(source), ty: normalize(place.ty(body, self.tcx).ty) })
                                    } else if source.projection.len() == 1 && source.projection[0] == ProjectionElem::Deref {
                                        locals[source.local.as_usize()].clone().filter(|source| source.nodes() < 32)
                                            .map(|source| Source::Pointer { source: Arc::new(source), ty: normalize(place.ty(body, self.tcx).ty) })
                                    } else { match Source::resolve(self.tcx, instance, &Operand::Copy(*source), &locals, &index, true) {
                                        Some(Source::Load { argument, pointer_ty, offset, ty }) if self.tcx
                                            .layout_of(TypingEnv::fully_monomorphized().as_query_input(ty)).is_ok_and(|layout| layout.is_sized()) =>
                                            Some(Source::Address { argument, pointer_ty, offset, ty: normalize(place.ty(body, self.tcx).ty) }),
                                        _ => None,
                                    } }
                                }
                                _ => None,
                            };
                            if place.projection.is_empty() {
                                locals[place.local.as_usize()] = source;
                                continue;
                            }
                            if place.projection.first() != Some(&ProjectionElem::Deref)
                                || index.address_taken.contains(&place.local) { return Ok(None); }
                            let Some(mut destination) = locals[place.local.as_usize()].clone() else { return Ok(None); };
                            while let Source::Pointer { source, .. } = destination { destination = (*source).clone(); }
                            if matches!(destination, Source::Load { .. } | Source::Field { .. }
                                | Source::Binary { .. } | Source::Branches { .. } | Source::Function { .. }
                                | Source::Aggregate { .. } | Source::Discriminant { .. } | Source::Bytes { .. }
                                | Source::Pointer { .. } | Source::Metadata { .. } | Source::LoadAt { .. }
                                | Source::Borrow { .. }) { return Ok(None); }
                            let Some(source) = source else { return Ok(None); };
                            let (destination, mut offset) = match destination {
                                Source::Address { argument, pointer_ty, offset, .. } => (Source::Argument(argument, pointer_ty), offset),
                                source => (source, 0i128),
                            };
                            for position in 1..place.projection.len() {
                                let ProjectionElem::Field(field, _) = place.projection[position] else { return Ok(None); };
                                let prefix = Place { local: place.local,
                                    projection: self.tcx.mk_place_elems(&place.projection[..position]) };
                                let ty = normalize(prefix.ty(body, self.tcx).ty);
                                if matches!(ty.kind(), ty::Adt(adt, _) if adt.is_enum()) { return Ok(None); }
                                let layout = self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(ty))
                                    .map_err(|_| QueryStop::Unsupported)?;
                                offset = offset.checked_add(i128::from(layout.fields.offset(field.as_usize()).bytes()))
                                    .ok_or(QueryStop::Unsupported)?;
                            }
                            let ty = normalize(place.ty(body, self.tcx).ty);
                            let bytes = self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(ty))
                                .map_err(|_| QueryStop::Unsupported)?.size.bytes();
                            summary.push(Store { destination, offset, bytes, source, conditions: conditions.clone() });
                        }
                        StatementKind::StorageLive(local) | StatementKind::StorageDead(local) => locals[local.as_usize()] = None,
                        StatementKind::Nop | StatementKind::PlaceMention(..)
                            | StatementKind::AscribeUserType(..) | StatementKind::ConstEvalCounter | StatementKind::Coverage(..) => {}
                        _ => return Ok(None),
                    }
                }
                bytes = body.local_decls.len().saturating_mul(std::mem::size_of::<Option<Source<'tcx>>>() * 4)
                    .saturating_add(summary.stores.iter().map(|store| (store.source.nodes() + store.destination.nodes()
                        + store.conditions.iter().map(|(source, values, _)| source.nodes() + values.len()).sum::<usize>())
                        * std::mem::size_of::<Source<'tcx>>() * 4).sum::<usize>())
                    .saturating_add(locals.iter().flatten().map(|source| source.nodes() * std::mem::size_of::<Source<'tcx>>() * 4).sum::<usize>())
                    .saturating_add(pending.iter().map(|(_, state, previous, branch): &(_, Vec<Option<Source<'tcx>>>, HashSet<mir::BasicBlock>, Vec<(Source<'tcx>, Box<[u128]>, bool)>)|
                        state.len() * std::mem::size_of::<Option<Source<'tcx>>>() * 4
                            + state.iter().flatten().map(|source| source.nodes() * std::mem::size_of::<Source<'tcx>>() * 4).sum::<usize>()
                            + previous.len() * std::mem::size_of::<mir::BasicBlock>() * 4
                            + branch.iter().map(|(source, values, _)| (source.nodes() + values.len()) * std::mem::size_of::<Source<'tcx>>() * 4).sum::<usize>()).sum::<usize>())
                    .saturating_add(alternatives.iter().map(|(conditions, source): &(Vec<(Source<'tcx>, Box<[u128]>, bool)>, Source<'tcx>)|
                        (source.nodes() + conditions.iter().map(|(source, values, _)| source.nodes() + values.len()).sum::<usize>())
                            * std::mem::size_of::<Source<'tcx>>() * 4).sum::<usize>());
                if bytes > limits.function_scratch_mib * 1024 * 1024 {
                    self.budget.record_limit(QueryStop::ScratchLimit, "function effect-summary bytes", bytes, limits.function_scratch_mib * 1024 * 1024);
                    return Err(QueryStop::ScratchLimit);
                }
                self.budget.scratch = scratch.saturating_add(self.effects.pending_bytes.saturating_sub(retained));
                self.budget.reserve(bytes)?;
                match &data.terminator().kind {
                    TerminatorKind::Goto { target } => block = *target,
                    TerminatorKind::InlineAsm { options, operands, targets, .. }
                        if options.intersects(rustc_ast::InlineAsmOptions::NOMEM | rustc_ast::InlineAsmOptions::READONLY)
                            && targets.len() == 1 => {
                        for operand in operands {
                            let output = match operand {
                                mir::InlineAsmOperand::Out { place, .. } => *place,
                                mir::InlineAsmOperand::InOut { out_place, .. } => *out_place,
                                _ => None,
                            };
                            if let Some(output) = output {
                                if !output.projection.is_empty() || index.address_taken.contains(&output.local) { return Ok(None); }
                                locals[output.local.as_usize()] = None;
                            }
                        }
                        block = targets[0];
                    }
                    TerminatorKind::SwitchInt { discr, targets } if summary.stores.is_empty() => {
                        if let Operand::RuntimeChecks(check) = discr {
                            block = targets.target_for_value(check.value(self.tcx.sess) as u128);
                            continue;
                        }
                        let Some(condition) = Source::resolve(self.tcx, instance, discr, &locals, &index, true) else { return Ok(None); };
                        if let Source::Constant(Operand::Constant(constant)) = &condition
                            && let Ok(ConstValue::Scalar(Scalar::Int(bits))) = constant.const_.eval(self.tcx, TypingEnv::fully_monomorphized(), body.span) {
                            block = targets.target_for_value(bits.to_bits(bits.size()));
                            continue;
                        }
                        if !matches!(condition.ty().kind(), ty::Bool | ty::Int(_) | ty::Uint(_) | ty::Char) { return Ok(None); }
                        if condition.nodes() + conditions.iter().map(|(source, values, _): &(Source<'tcx>, Box<[u128]>, bool)|
                            source.nodes() + values.len()).sum::<usize>() + targets.all_targets().len() > 32 { return Ok(None); }
                        if pending.len() + finished + targets.all_targets().len() > limits.alternatives.min(8) { return Ok(None); }
                        let mut edges = Vec::new();
                        let excluded: Box<[_]> = targets.iter().map(|(value, _)| value).collect();
                        for (value, target) in targets.iter() { edges.push((target, vec![value].into_boxed_slice(), false)); }
                        edges.push((targets.otherwise(), excluded, true));
                        for (target, values, otherwise) in edges {
                            let mut branch = conditions.clone();
                            branch.push((condition.clone(), values, otherwise));
                            pending.push((target, locals.clone(), visited.clone(), branch));
                        }
                        branched = true;
                        let Some((next, state, previous, branch)) = pending.pop() else { return Ok(None); };
                        block = next;
                        locals = state;
                        visited = previous;
                        conditions = branch;
                    }
                    TerminatorKind::Call { func, args, destination, target: Some(successor), .. }
                        if destination.projection.is_empty() => {
                        let ty::FnDef(def, type_args) = *normalize(func.ty(body, self.tcx)).kind() else { return Ok(None); };
                        let Some(callee) = Instance::try_resolve(self.tcx, TypingEnv::fully_monomorphized(), def,
                            self.tcx.instantiate_bound_regions_with_erased(type_args)).ok().flatten() else { return Ok(None); };
                        let Some(callee) = self.linked_target(callee) else { return Ok(None); };
                        if matches!(callee.def, InstanceKind::Intrinsic(_)) && args.len() == 2
                            && matches!(self.tcx.item_name(callee.def_id()).as_str(), "offset" | "arith_offset") {
                            locals[destination.local.as_usize()] = Source::resolve(self.tcx, instance, &args[0].node, &locals, &index, summary.stores.is_empty())
                                .zip(Source::resolve(self.tcx, instance, &args[1].node, &locals, &index, summary.stores.is_empty()))
                                .filter(|(left, right)| left.nodes() + right.nodes() < 32)
                                .map(|(left, right)| Source::Binary { operation: mir::BinOp::Offset, left: Arc::new(left), right: Arc::new(right), ty: normalize(destination.ty(body, self.tcx).ty) });
                            block = *successor;
                            continue;
                        }
                        if matches!(callee.def, InstanceKind::Intrinsic(_)) && args.len() == 1
                            && self.tcx.item_name(callee.def_id()).as_str() == "transmute" {
                            let ty = normalize(destination.ty(body, self.tcx).ty);
                            let source_ty = normalize(args[0].node.ty(body, self.tcx));
                            let equal = self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(source_ty)).ok()
                                .zip(self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(ty)).ok())
                                .is_some_and(|(source, target)| source.size == target.size);
                            if !equal { return Ok(None); }
                            locals[destination.local.as_usize()] = Source::resolve(self.tcx, instance, &args[0].node, &locals, &index, summary.stores.is_empty())
                                .filter(|source| source.nodes() < 32).map(|source| Source::Bytes { source: Arc::new(source), offset: 0, ty });
                            block = *successor;
                            continue;
                        }
                        if matches!(callee.def, InstanceKind::Intrinsic(_)) && args.len() == 1
                            && matches!(self.tcx.item_name(callee.def_id()).as_str(), "black_box" | "likely" | "unlikely")
                            && normalize(args[0].node.ty(body, self.tcx)) == normalize(destination.ty(body, self.tcx).ty) {
                            locals[destination.local.as_usize()] = Source::resolve(self.tcx, instance, &args[0].node,
                                &locals, &index, summary.stores.is_empty());
                            block = *successor;
                            continue;
                        }
                        let Some(effects) = self.effect_summary(callee, irq)? else { return Ok(None); };
                        let callee_body = self.tcx.instance_mir(callee.def);
                        if args.len() != callee_body.arg_count { return Ok(None); }
                        let mut inputs = Vec::new();
                        for (position, argument) in args.iter().enumerate() {
                            let expected = callee.instantiate_mir_and_normalize_erasing_regions(self.tcx,
                                TypingEnv::fully_monomorphized(), EarlyBinder::bind(self.tcx,
                                    callee_body.local_decls[mir::Local::from_usize(position + 1)].ty));
                            if expected != normalize(argument.node.ty(body, self.tcx)) { return Ok(None); }
                            inputs.push(Source::resolve(self.tcx, instance, &argument.node, &locals, &index, summary.stores.is_empty()));
                        }
                        if !summary.stores.is_empty() && effects.stores.iter().any(|store| store.source.reads_memory(&inputs)
                            || store.conditions.iter().any(|(source, _, _)| source.reads_memory(&inputs))) { return Ok(None); }
                        let returned = effects.returned.as_ref().and_then(|source| source.substitute(&inputs, summary.stores.is_empty()))
                            .and_then(|source| source.normalize_branches(self.tcx));
                        let expected = callee.instantiate_mir_and_normalize_erasing_regions(self.tcx,
                            TypingEnv::fully_monomorphized(), EarlyBinder::bind(self.tcx, callee_body.local_decls[mir::RETURN_PLACE].ty));
                        if expected != normalize(destination.ty(body, self.tcx).ty) { return Ok(None); }
                        for store in &effects.stores {
                            let Some(mut destination) = store.destination.substitute(&inputs, true) else { return Ok(None); };
                            while let Source::Pointer { source, .. } = destination { destination = (*source).clone(); }
                            let Some(source) = store.source.substitute(&inputs, true) else { return Ok(None); };
                            let (destination, base) = match destination {
                                Source::Address { argument, pointer_ty, offset, .. } => (Source::Argument(argument, pointer_ty), offset),
                                source @ (Source::Argument(..) | Source::Constant(_)) => (source, 0),
                                _ => return Ok(None),
                            };
                            let offset = base.checked_add(store.offset).ok_or(QueryStop::Unsupported)?;
                            let mut required = conditions.clone();
                            for (condition, values, otherwise) in &store.conditions {
                                let Some(condition) = condition.substitute(&inputs, true) else { return Ok(None); };
                                required.push((condition, values.clone(), *otherwise));
                            }
                            if required.iter().map(|(source, values, _)| source.nodes() + values.len()).sum::<usize>() > 32 { return Ok(None); }
                            summary.push(Store { destination, source, offset, bytes: store.bytes, conditions: required });
                        }
                        locals[destination.local.as_usize()] = returned;
                        block = *successor;
                    }
                    TerminatorKind::Return | TerminatorKind::Unreachable => {
                        let returning = matches!(data.terminator().kind, TerminatorKind::Return);
                        if !returning && !summary.stores.is_empty() { return Ok(None); }
                        summary.returned = returning.then(|| Source::resolve(self.tcx, instance,
                            &Operand::Copy(Place::from(mir::RETURN_PLACE)), &locals, &index, summary.stores.is_empty())).flatten();
                        if branched {
                            finished += 1;
                            if let Some(source) = summary.returned.take() { alternatives.push((conditions, source)); }
                            else if returning { returns_complete = false; }
                            if let Some((next, state, previous, branch)) = pending.pop() {
                                block = next;
                                locals = state;
                                visited = previous;
                                conditions = branch;
                                continue;
                            }
                            let ty = normalize(body.local_decls[mir::RETURN_PLACE].ty);
                            let Some(source) = (Source::Branches { alternatives, ty }).normalize_branches(self.tcx) else { return Ok(None); };
                            summary.returned = (returning || finished > 1).then_some(source).filter(|_| returns_complete);
                        }
                        return Ok(Some(Arc::new(summary)));
                    }
                    _ => return Ok(None),
                }
            }
        })();
        self.effects.active.remove(&key);
        self.budget.scratch = scratch.saturating_add(self.effects.pending_bytes.saturating_sub(retained));
        let summary = answer?;
        if std::env::var("IRQ_CHECK_TRACE_EFFECT").is_ok_and(|filter| instance.to_string().contains(&filter)) {
            eprintln!("irq-check: effect summary {instance} returned={:?} stores={:?}",
                summary.as_ref().and_then(|summary| summary.returned.as_ref()), summary.as_ref().map(|summary| summary.stores.len()));
        }
        Ok(summary)
    }
}
