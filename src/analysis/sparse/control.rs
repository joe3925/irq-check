use super::*;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ControlPath {
    guards: Vec<Guard>,
    returns: Vec<Location>,
    unresolved: bool,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ControlKey {
    point: Point,
    generation: usize,
    epoch: usize,
    choices: Vec<((Point, mir::Local), u128)>,
    inputs: Vec<(usize, usize)>,
    witnesses: Vec<(usize, u128)>,
    loop_entries: Box<[Point]>,
    cut_recursive_returns: bool,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct ConditionKey {
    pub(super) point: Point,
    pub(super) generation: usize,
    pub(super) required_binding: bool,
    pub(super) choices: Vec<((Point, mir::Local), u128)>,
    pub(super) inputs: Vec<(usize, usize)>,
    pub(super) witnesses: Vec<(usize, u128)>,
    pub(super) loop_entries: Box<[Point]>,
    pub(super) cut_recursive_returns: bool,
}

#[derive(Default)]
pub(crate) struct ControlCache<'tcx> {
    pub(super) stopped_conditions: HashMap<ConditionKey, (Point, QueryStop)>,
    pub(super) stopped_bytes: usize,
    pub(crate) condition_hits: usize,
    templates: HashMap<(Instance<'tcx>, bool, Box<[mir::BasicBlock]>, Box<[(Location, u128)]>, bool), Arc<[Vec<ControlPath>]>>,
    answers: HashMap<ControlKey, Arc<[Alternative<'tcx>]>>,
    active: HashSet<ControlKey>,
    incomplete: Vec<bool>,
    pending: HashMap<ControlKey, (Vec<Alternative<'tcx>>, usize)>,
    evaluated: HashSet<ControlKey>,
    pub(super) solving: bool,
    pub(super) proving_assertion: bool,
    changed: bool,
    pending_bytes: usize,
    witnesses: Vec<Alternative<'tcx>>,
    pub(crate) bytes: usize,
    pub(crate) evaluations: usize,
    pub(crate) hits: usize,
    pub(crate) widened: usize,
    pub(crate) refinement_stops: usize,
    pub(in crate::analysis) last_stop: Option<(Point, QueryStop)>,
    pub(in crate::analysis) dependency_stop: Option<(Point, QueryStop)>,
    pub(in crate::analysis) causes: HashMap<Point, (Point, QueryStop)>,
    pub(super) attempted_loops: HashSet<usize>,
    pub(super) loop_fallbacks: HashMap<(Instance<'tcx>, bool), (Point, QueryStop)>,
    pub(super) epoch: usize,
}

impl<'a, 'tcx> Solver<'a, 'tcx> {
    pub(super) fn returns_normally(&mut self, instance: Instance<'tcx>, irq: bool) -> Result<bool, QueryStop> {
        let root = (instance, irq);
        if let Some(&normal) = self.effects.normal.get(&root) { return Ok(normal); }
        let scratch = self.budget.scratch;
        let mut graph = HashMap::<_, (bool, HashSet<_>)>::new();
        let mut queue = VecDeque::from([root]);
        let limits = irq_check_rupta::limits::configuration().expect("validated analysis limits");
        let answer = (|| {
            while let Some(key @ (instance, irq)) = queue.pop_front() {
                self.budget.charge(0)?;
                if graph.contains_key(&key) || self.effects.normal.contains_key(&key) { continue; }
                if matches!(instance.def, InstanceKind::Intrinsic(_)) {
                    let normal = !matches!(self.tcx.item_name(instance.def_id()).as_str(), "abort" | "unreachable" | "breakpoint")
                        && super::super::intrinsic_effect(self.tcx, instance) != super::super::IntrinsicEffect::Unknown
                        && !self.tcx.fn_sig(instance.def_id()).instantiate(self.tcx, instance.args).skip_binder().output().is_never();
                    graph.insert(key, (normal, HashSet::new()));
                    continue;
                }
                if self.tcx.is_foreign_item(instance.def_id())
                    || matches!(instance.def, InstanceKind::LlvmIntrinsic(_) | InstanceKind::Virtual(..))
                    || !irq_check_rupta::util::type_util::has_instance_mir(self.tcx, instance.def) {
                    graph.insert(key, (false, HashSet::new()));
                    continue;
                }
                let index = if let Some(index) = self.indexes.get(&key) { index.clone() } else {
                    let index = Arc::new(BodyIndex::build(self.tcx, self.trust, instance, irq)?);
                    self.indexes.insert(key, index.clone());
                    index
                };
                let body = index.body;
                let bytes = body.basic_blocks.len().saturating_mul(128);
                if bytes > limits.function_scratch_mib * 1024 * 1024 { return Err(QueryStop::ScratchLimit); }
                self.budget.scratch = scratch;
                self.budget.reserve(bytes.saturating_add(graph.values().map(|(_, dependencies)| 256 + dependencies.len() * 128).sum::<usize>()))?;
                let tcx = self.tcx;
                let normalize = |ty| instance.instantiate_mir_and_normalize_erasing_regions(tcx,
                    TypingEnv::fully_monomorphized(), EarlyBinder::bind(tcx, ty));
                let mut successors = vec![Vec::<mir::BasicBlock>::new(); body.basic_blocks.len()];
                let mut returns = HashSet::new();
                let mut blocks = VecDeque::from([mir::START_BLOCK]);
                let mut visited = HashSet::new();
                let mut dependencies = HashSet::new();
                let mut supported = true;
                while let Some(block) = blocks.pop_front() {
                    self.budget.charge(0)?;
                    if !visited.insert(block) { continue; }
                    if index.stops[block.as_usize()].is_some()
                        || index.assumptions.keys().any(|location| location.block == block) {
                        supported = false;
                        break;
                    }
                    let data = &body.basic_blocks[block];
                    if data.statements.iter().any(|statement| matches!(&statement.kind,
                        StatementKind::Assign(assignment) if matches!(assignment.1, Rvalue::ThreadLocalRef(_)))) {
                        supported = false;
                        break;
                    }
                    let at = Location { block, statement_index: data.statements.len() };
                    match &data.terminator().kind {
                        TerminatorKind::Return => { returns.insert(block); }
                        TerminatorKind::Goto { target } => successors[block.as_usize()].push(*target),
                        TerminatorKind::SwitchInt { discr, targets } => {
                            let mut values = None;
                            if let Operand::Copy(place) | Operand::Move(place) = discr
                                && let Some(statement) = data.statements.iter().rev().find(|statement|
                                    matches!(&statement.kind, StatementKind::Assign(assignment) if assignment.0 == *place))
                                && let StatementKind::Assign(assignment) = &statement.kind
                                && let Rvalue::Discriminant(source) = &assignment.1
                                && let ty::Adt(adt, _) = normalize(source.ty(body, self.tcx).ty).kind()
                                && adt.is_enum()
                                && let Ok(layout) = self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(normalize(discr.ty(body, self.tcx))))
                                && (1..=128).contains(&layout.size.bits()) {
                                let mask = u128::MAX >> (128 - layout.size.bits());
                                values = Some(adt.discriminants(self.tcx).map(|(_, discriminant)| discriminant.val & mask).collect::<Vec<_>>());
                            } else if let Operand::RuntimeChecks(check) = discr {
                                values = Some(vec![check.value(self.tcx.sess) as u128]);
                            } else if let Operand::Constant(constant) = discr {
                                let constant = instance.instantiate_mir_and_normalize_erasing_regions(self.tcx,
                                    TypingEnv::fully_monomorphized(), EarlyBinder::bind(self.tcx, constant.const_));
                                if let Ok(ConstValue::Scalar(Scalar::Int(bits))) = constant.eval(self.tcx, TypingEnv::fully_monomorphized(), body.span) {
                                    values = Some(vec![bits.to_bits(bits.size())]);
                                }
                            }
                            if let Some(values) = values {
                                successors[block.as_usize()].extend(values.into_iter().map(|value| targets.target_for_value(value)));
                            } else { successors[block.as_usize()].extend(targets.all_targets().iter().copied()); }
                        }
                        TerminatorKind::Assert { cond: Operand::Constant(constant), expected, target, .. } => {
                            let constant = instance.instantiate_mir_and_normalize_erasing_regions(self.tcx,
                                TypingEnv::fully_monomorphized(), EarlyBinder::bind(self.tcx, constant.const_));
                            if constant.eval(self.tcx, TypingEnv::fully_monomorphized(), body.span).is_ok_and(|value|
                                matches!(value, ConstValue::Scalar(Scalar::Int(bits)) if (bits.to_bits(bits.size()) != 0) == *expected)) {
                                successors[block.as_usize()].push(*target);
                            } else { supported = false; break; }
                        }
                        TerminatorKind::Call { func, target: Some(_), .. } | TerminatorKind::TailCall { func, .. } => {
                            let function_ty = normalize(func.ty(body, self.tcx));
                            let direct = if let ty::FnDef(def, args) = *function_ty.kind() {
                                Instance::try_resolve(self.tcx, TypingEnv::fully_monomorphized(), def,
                                    self.tcx.instantiate_bound_regions_with_erased(args)).ok().flatten()
                                    .filter(|target| !matches!(target.def, InstanceKind::Virtual(..)))
                            } else { None };
                            let targets: Vec<_> = if let Some(target) = direct { vec![target] }
                                else if self.facts.covered_calls.contains(&(instance, at)) {
                                    self.facts.safe_candidates.get(&(instance, at)).into_iter().flatten().copied().collect()
                                } else { Vec::new() };
                            if targets.is_empty() { supported = false; break; }
                            for target in targets {
                                let Some(target) = self.linked_target(target) else { supported = false; break; };
                                dependencies.insert((target, irq));
                            }
                            if !supported { break; }
                            if let TerminatorKind::Call { target: Some(target), .. } = &data.terminator().kind {
                                successors[block.as_usize()].push(*target);
                            } else { returns.insert(block); }
                        }
                        TerminatorKind::Drop { place, target, .. } => {
                            dependencies.insert((Instance::resolve_drop_glue(self.tcx, normalize(place.ty(body, self.tcx).ty)), irq));
                            successors[block.as_usize()].push(*target);
                        }
                        _ => { supported = false; break; }
                    }
                    successors[block.as_usize()].sort_unstable();
                    successors[block.as_usize()].dedup();
                    blocks.extend(successors[block.as_usize()].iter().copied());
                }
                if supported {
                    let dominators = body.basic_blocks.dominators();
                    for (&(tail, header), loop_blocks) in &index.loops {
                        self.budget.charge(0)?;
                        if !visited.contains(&tail) || !successors[tail.as_usize()].contains(&header) { continue; }
                        let data = &body.basic_blocks[header];
                        let TerminatorKind::SwitchInt { discr: Operand::Copy(condition) | Operand::Move(condition), targets } = &data.terminator().kind else { continue; };
                        let Some(statement) = data.statements.iter().rev().find(|statement|
                            matches!(&statement.kind, StatementKind::Assign(assignment) if assignment.0 == *condition)) else { continue; };
                        let StatementKind::Assign(assignment) = &statement.kind else { continue; };
                        let Rvalue::BinaryOp(comparison, operands) = &assignment.1 else { continue; };
                        if !matches!(comparison, mir::BinOp::Lt | mir::BinOp::Gt) { continue; }
                        let (Operand::Copy(counter) | Operand::Move(counter)) = operands.0 else { continue; };
                        if !counter.projection.is_empty() || index.address_taken.contains(&counter.local)
                            || !matches!(normalize(counter.ty(body, self.tcx).ty).kind(), ty::Uint(_)) { continue; }
                        let bound = match &operands.1 {
                            Operand::Constant(_) => None,
                            Operand::Copy(place) | Operand::Move(place) if place.projection.is_empty()
                                && !index.address_taken.contains(&place.local) => Some(place.local),
                            _ => continue,
                        };
                        if bound.is_some_and(|local| index.local_definitions.get(&local).into_iter().flatten()
                            .any(|definition| loop_blocks.contains(&definition.location.block))) { continue; }
                        let updates: Vec<_> = index.local_definitions.get(&counter.local).into_iter().flatten()
                            .filter(|definition| loop_blocks.contains(&definition.location.block)).collect();
                        let [update] = updates.as_slice() else { continue; };
                        let location = update.location;
                        if location.block == header || !dominators.dominates(location.block, tail) { continue; }
                        let inside = targets.target_for_value(1);
                        let outside = targets.target_for_value(0);
                        if !loop_blocks.contains(&inside) || loop_blocks.contains(&outside)
                            || !dominators.dominates(inside, location.block) { continue; }
                        if index.loops.keys().any(|&(other_tail, other_header)|
                            (other_tail, other_header) != (tail, header)
                                && loop_blocks.contains(&other_tail) && loop_blocks.contains(&other_header)) { continue; }
                        if loop_blocks.iter().any(|block| body.basic_blocks[*block].statements.iter().any(|statement|
                            matches!(statement.kind, StatementKind::StorageLive(local) | StatementKind::StorageDead(local)
                                if local == counter.local || Some(local) == bound))) { continue; }
                        let Some(statement) = body.basic_blocks[location.block].statements.get(location.statement_index) else { continue; };
                        let StatementKind::Assign(assignment) = &statement.kind else { continue; };
                        if assignment.0 != counter { continue; }
                        let Rvalue::BinaryOp(operation, operands) = &assignment.1 else { continue; };
                        if !matches!((comparison, operation), (mir::BinOp::Lt, mir::BinOp::Add | mir::BinOp::AddUnchecked)
                            | (mir::BinOp::Gt, mir::BinOp::Sub | mir::BinOp::SubUnchecked)) { continue; }
                        if !matches!(operands.0, Operand::Copy(place) | Operand::Move(place) if place == counter) { continue; }
                        let Operand::Constant(constant) = &operands.1 else { continue; };
                        let constant = instance.instantiate_mir_and_normalize_erasing_regions(self.tcx,
                            TypingEnv::fully_monomorphized(), EarlyBinder::bind(self.tcx, constant.const_));
                        if !constant.eval(self.tcx, TypingEnv::fully_monomorphized(), body.span).is_ok_and(|value|
                            matches!(value, ConstValue::Scalar(Scalar::Int(bits)) if bits.to_bits(bits.size()) == 1)) { continue; }
                        for successor in &mut successors[tail.as_usize()] {
                            if *successor == header { *successor = outside; }
                        }
                        successors[tail.as_usize()].sort_unstable();
                        successors[tail.as_usize()].dedup();
                    }
                    let mut predecessors = vec![Vec::new(); body.basic_blocks.len()];
                    let mut remaining = vec![0usize; body.basic_blocks.len()];
                    for &block in &visited {
                        remaining[block.as_usize()] = successors[block.as_usize()].len();
                        for &successor in &successors[block.as_usize()] { predecessors[successor.as_usize()].push(block); }
                    }
                    let mut ready: VecDeque<_> = returns.iter().copied().collect();
                    while let Some(block) = ready.pop_front() {
                        self.budget.charge(0)?;
                        for &parent in &predecessors[block.as_usize()] {
                            remaining[parent.as_usize()] -= 1;
                            if remaining[parent.as_usize()] == 0 && returns.insert(parent) { ready.push_back(parent); }
                        }
                    }
                    supported = returns.contains(&mir::START_BLOCK);
                }
                if supported { queue.extend(dependencies.iter().copied()); }
                else { dependencies.clear(); }
                graph.insert(key, (supported, dependencies));
                let bytes = body.basic_blocks.len().saturating_mul(128);
                if bytes > limits.function_scratch_mib * 1024 * 1024 { return Err(QueryStop::ScratchLimit); }
                let retained = graph.values().map(|(_, dependencies)| 256 + dependencies.len() * 128).sum::<usize>();
                if retained > self.cache_limit / 32 { return Err(QueryStop::RefinementLimit); }
                self.budget.scratch = scratch;
                self.budget.reserve(bytes.saturating_add(retained))?;
            }
            let mut users = HashMap::<_, Vec<_>>::new();
            let mut remaining = HashMap::new();
            let mut ready = VecDeque::new();
            for (&key, (supported, dependencies)) in &graph {
                self.budget.charge(0)?;
                let mut count = 0usize;
                for &dependency in dependencies {
                    if self.effects.normal.get(&dependency) == Some(&true) { continue; }
                    count += 1;
                    users.entry(dependency).or_default().push(key);
                }
                remaining.insert(key, count);
                if *supported && count == 0 { ready.push_back(key); }
            }
            let mut proven = HashSet::new();
            while let Some(key) = ready.pop_front() {
                self.budget.charge(0)?;
                if !proven.insert(key) { continue; }
                for &user in users.get(&key).into_iter().flatten() {
                    let count = remaining.get_mut(&user).expect("return summary dependency");
                    *count -= 1;
                    if *count == 0 && graph[&user].0 { ready.push_back(user); }
                }
            }
            Ok(proven)
        })();
        self.budget.scratch = scratch;
        let proven = answer?;
        let result = proven.contains(&root);
        let bytes = graph.len().saturating_mul(128);
        if self.effects.bytes.saturating_add(bytes) <= self.cache_limit / 32 {
            self.budget.reserve(bytes)?;
            self.effects.bytes += bytes;
            for key in graph.into_keys() { self.effects.normal.insert(key, proven.contains(&key)); }
        }
        Ok(result)
    }

    pub(crate) fn control_summary(&mut self, point: Point) -> Result<Vec<Alternative<'tcx>>, QueryStop> {
        if !self.controls.solving {
            self.controls.last_stop = None;
            self.controls.dependency_stop = None;
            self.controls.causes.remove(&point);
            self.controls.attempted_loops.clear();
        }
        let scratch = self.budget.scratch;
        let stored = self.flow.bytes;
        let mut choices: Vec<_> = self.index_choices.iter().map(|(key, value)| (*key, *value)).collect();
        choices.sort_unstable_by_key(|((point, local), _)| (point.frame, point.location, *local));
        let mut inputs: Vec<_> = self.input_choices.iter().filter(|(frame, _)| !self.flow.single_input(**frame))
            .map(|(key, value)| (*key, *value)).collect();
        inputs.sort_unstable();
        let mut witnesses: Vec<_> = self.root_scalar_witness.iter().map(|(key, value)| (*key, *value)).collect();
        witnesses.sort_unstable();
        let key = ControlKey { point, generation: self.flow.generation, epoch: self.controls.epoch, choices, inputs, witnesses,
            loop_entries: self.loop_entries.clone(), cut_recursive_returns: self.cut_recursive_returns };
        if let Some(values) = self.controls.answers.get(&key) {
            self.controls.hits += 1;
            return Ok(values.to_vec());
        }
        let limits = irq_check_rupta::limits::configuration().expect("validated analysis limits");
        if self.controls.solving {
            if self.controls.active.contains(&key) || self.controls.evaluated.contains(&key) {
                if let Some(incomplete) = self.controls.incomplete.last_mut() { *incomplete = true; }
                return Ok(self.controls.pending.get(&key).map(|(values, _)| values.clone()).unwrap_or_default());
            }
            if self.controls.active.len() >= limits.call_depth { return Err(QueryStop::RefinementLimit); }
            self.controls.active.insert(key.clone());
            self.controls.incomplete.push(false);
            self.controls.evaluations += 1;
            let mut finished = Vec::new();
            let answer = self.control_step(point, true, false, &mut finished);
            let incomplete = self.controls.incomplete.pop().expect("active control request");
            self.controls.active.remove(&key);
            if key.generation != self.flow.generation && answer != Err(QueryStop::ProcessLimit) {
                return Err(QueryStop::ContextChanged);
            }
            if self.controls.active.is_empty() {
                finished.retain(Alternative::path_proven);
                self.controls.witnesses = finished;
            }
            if !incomplete {
                let values = answer?;
                if let Some((_, bytes)) = self.controls.pending.remove(&key) {
                    self.controls.pending_bytes = self.controls.pending_bytes.saturating_sub(bytes);
                }
                self.controls.evaluated.remove(&key);
                let bytes = std::mem::size_of::<ControlKey>() * 4
                    + (key.choices.len() + key.inputs.len() + key.witnesses.len() + key.loop_entries.len()) * 64
                    + values.len() * std::mem::size_of::<Alternative<'tcx>>() * 2
                    + values.iter().map(|value| value.guards.len() * std::mem::size_of::<(usize, Guard)>() * 2).sum::<usize>();
                if self.controls.bytes.saturating_add(bytes) <= self.cache_limit / 16 {
                    self.budget.reserve(bytes)?;
                    self.controls.answers.insert(key, Arc::from(values.clone()));
                    self.controls.bytes += bytes;
                }
                return Ok(values);
            }
            if let Some(incomplete) = self.controls.incomplete.last_mut() { *incomplete = true; }
            let mut values = self.controls.pending.get(&key).map(|(values, _)| values.clone()).unwrap_or_default();
            for value in answer? {
                if !values.contains(&value) { values.push(value); }
            }
            let values = self.bounded(values)?;
            if self.controls.pending.get(&key).is_none_or(|(previous, _)|
                previous.len() != values.len() || values.iter().any(|value| !previous.contains(value)))
            {
                let bytes = std::mem::size_of::<ControlKey>() * 4
                    + (key.choices.len() + key.inputs.len() + key.witnesses.len() + key.loop_entries.len()) * 64
                    + values.len() * std::mem::size_of::<Alternative<'tcx>>() * 2
                    + values.iter().map(|value| value.guards.len() * std::mem::size_of::<(usize, Guard)>() * 2).sum::<usize>();
                let previous_bytes = self.controls.pending.get(&key).map_or(0, |(_, bytes)| *bytes);
                self.controls.pending_bytes = self.controls.pending_bytes.saturating_sub(previous_bytes).saturating_add(bytes);
                if self.controls.pending_bytes > self.cache_limit / 16 { return Err(QueryStop::ScratchLimit); }
                self.budget.charge(bytes.saturating_sub(previous_bytes))?;
                self.controls.pending.insert(key.clone(), (values.clone(), bytes));
                self.controls.changed = true;
            }
            self.controls.evaluated.insert(key);
            return Ok(values);
        }
        let mut coarse = self.control_step(point, false, false, &mut Vec::new())?;
        if coarse.is_empty() { return Ok(coarse); }
        let before = self.budget.steps;
        let saved_refinement_limit = self.budget.refinement_limit;
        let allowance = if self.facts.exclusions.contains(&self.frames[point.frame].instance) {
            limits.scalar_steps
        } else { limits.scalar_steps.min(4096) };
        let allowance = allowance.min(self.budget.limit.saturating_sub(before) / 2);
        self.budget.refinement_limit = Some(saved_refinement_limit.unwrap_or(usize::MAX)
            .min(before.saturating_add(allowance)));
        self.controls.solving = true;
        let answer = loop {
            self.budget.scratch = scratch.saturating_add(self.flow.bytes.saturating_sub(stored))
                .saturating_add(self.controls.pending_bytes);
            self.controls.changed = false;
            self.controls.evaluated.clear();
            let answer = self.control_summary(point);
            if answer.is_err() || !self.controls.changed { break answer; }
        };
        self.budget.refinement_limit = saved_refinement_limit;
        self.controls.solving = false;
        self.controls.active.clear();
        self.controls.incomplete.clear();
        let mut pending = std::mem::take(&mut self.controls.pending);
        let bytes = std::mem::take(&mut self.controls.pending_bytes);
        let evaluated = std::mem::take(&mut self.controls.evaluated);
        let witnesses = std::mem::take(&mut self.controls.witnesses);
        self.budget.scratch = scratch.saturating_add(self.flow.bytes.saturating_sub(stored));
        match answer {
            Ok(values) => {
                if self.controls.bytes.saturating_add(bytes) <= self.cache_limit / 16 {
                    for key in evaluated {
                        if let Some((values, _)) = pending.remove(&key) {
                            self.controls.answers.insert(key, Arc::from(values));
                        }
                    }
                    self.controls.bytes += bytes;
                }
                Ok(values)
            }
            Err(reason @ (QueryStop::ProcessLimit | QueryStop::TransferLimit | QueryStop::ContextChanged | QueryStop::SplitIndex)) => Err(reason),
            Err(reason) => {
                self.controls.refinement_stops += 1;
                self.controls.last_stop = Some((point, reason));
                let cause = self.controls.dependency_stop.unwrap_or((point, reason));
                self.controls.causes.insert(point, cause);
                let mut changed = false;
                for frame in self.controls.attempted_loops.drain() {
                    if reason == QueryStop::ControlMemory { continue; }
                    let frame = self.frames[frame];
                    if self.controls.loop_fallbacks.insert((frame.instance, frame.irq), cause).is_none() {
                        changed = true;
                    }
                }
                if changed {
                    self.controls.epoch += 1;
                    self.retain_completed();
                    self.budget.scratch = scratch.saturating_add(self.flow.bytes.saturating_sub(stored));
                }
                for value in witnesses.into_iter().chain(pending.remove(&key).into_iter().flat_map(|(values, _)| values)) {
                    if coarse.len() >= limits.alternatives { break; }
                    if value.path_proven() && !coarse.contains(&value) { coarse.push(value); }
                }
                Ok(coarse)
            }
        }
    }

    pub(super) fn control_step(&mut self, point: Point, refine: bool, memory: bool, finished: &mut Vec<Alternative<'tcx>>)
        -> Result<Vec<Alternative<'tcx>>, QueryStop>
    {
        let scratch = self.budget.scratch;
        let stored = self.flow.bytes;
        let pending_stored = self.controls.pending_bytes;
        let index = self.index(point.frame)?;
        if !index.reachable.contains(&point.location.block)
            || index.stops[point.location.block.as_usize()]
                .is_some_and(|stop| point.location.statement_index >= stop)
        { return Ok(Vec::new()); }
        let marker = (point.frame, Guard { required: false, proof: GuardProof::Widened,
            iteration: None, branch: point.location, successor: mir::START_BLOCK,
            index: None, input: false, phi: false, choice: None });
        let limits = irq_check_rupta::limits::configuration().expect("validated analysis limits");
        let frame = self.frames[point.frame];
        let entries: Box<[_]> = self.loop_entries.iter().filter_map(|entry|
            (entry.frame == point.frame).then_some(entry.location.block)).collect();
        let mut constants = Vec::new();
        for block in index.body.basic_blocks.indices() {
            let data = &index.body.basic_blocks[block];
            if !matches!(data.terminator().kind, TerminatorKind::SwitchInt { .. } | TerminatorKind::Assert { .. }) { continue; }
            let at = Location { block, statement_index: data.statements.len() };
            match self.scalar_condition(Point { frame: point.frame, location: at }) {
                Ok(Some(bits)) => constants.push((at, bits)),
                Err(reason @ (QueryStop::ProcessLimit | QueryStop::TransferLimit | QueryStop::ContextChanged)) => return Err(reason),
                Err(_) => break,
                Ok(None) => {}
            }
        }
        let template_key = (frame.instance, frame.irq, entries.clone(), constants.clone().into_boxed_slice(), memory);
        let paths = if let Some(paths) = self.controls.templates.get(&template_key) {
            paths.clone()
        } else {
            let mut successors = vec![Vec::new(); index.body.basic_blocks.len()];
            for block in index.body.basic_blocks.indices() {
                for &(parent, guard) in &index.predecessors[block.as_usize()] {
                    if entries.contains(&block) && index.loops.contains_key(&(parent, block)) { continue; }
                    if let Some((_, bits)) = constants.iter().find(|(at, _)| at.block == parent) {
                        let possible = match &index.body.basic_blocks[parent].terminator().kind {
                            TerminatorKind::SwitchInt { targets, .. } => targets.target_for_value(*bits) == block,
                            TerminatorKind::Assert { expected, target, unwind, .. } => {
                                if (*bits != 0) == *expected { *target == block }
                                else { matches!(unwind, mir::UnwindAction::Cleanup(cleanup) if *cleanup == block) }
                            }
                            _ => true,
                        };
                        if !possible { continue; }
                    }
                    successors[parent.as_usize()].push((block, guard));
                }
            }
            let cap = limits.alternatives.min(8);
            let mut states = vec![Vec::<ControlPath>::new(); index.body.basic_blocks.len()];
            let mut merged = vec![false; states.len()];
            let mut initial = ControlPath::default();
            if !memory && frame.irq && index.loops.keys().any(|(_, header)| *header == mir::START_BLOCK) {
                initial.guards.push(Guard { required: true, proof: GuardProof::Established, iteration: Some(0),
                    branch: Location::START, successor: mir::START_BLOCK, index: None, input: false, phi: true, choice: None });
            }
            states[mir::START_BLOCK.as_usize()].push(initial);
            let mut queue = VecDeque::from([mir::START_BLOCK]);
            let mut queued = HashSet::from([mir::START_BLOCK]);
            while let Some(block) = queue.pop_front() {
                queued.remove(&block);
                self.budget.scratch = scratch.saturating_add(self.flow.bytes.saturating_sub(stored))
                    .saturating_add(self.controls.pending_bytes.saturating_sub(pending_stored));
                let bytes = states.iter().map(|paths| paths.capacity() * std::mem::size_of::<ControlPath>() * 2
                    + paths.iter().map(|path| path.guards.capacity() * std::mem::size_of::<Guard>() * 2
                        + path.returns.capacity() * std::mem::size_of::<Location>() * 2).sum::<usize>()).sum::<usize>()
                    + states.len() * std::mem::size_of::<Vec<ControlPath>>() * 4
                    + successors.iter().map(|edges| edges.len() * std::mem::size_of::<(mir::BasicBlock, Option<Guard>)>() * 4).sum::<usize>();
                self.budget.charge(bytes.saturating_add(self.controls.bytes))?;
                if bytes > limits.function_scratch_mib * 1024 * 1024 { return Err(QueryStop::ScratchLimit); }
                let data = &index.body.basic_blocks[block];
                let at = Location { block, statement_index: data.statements.len() };
                for &(successor, guard) in &successors[block.as_usize()] {
                    let mut incoming = states[block.as_usize()].clone();
                    for path in &mut incoming {
                        for &branch in index.assumptions.keys() {
                            if branch.block == block && branch.statement_index < at.statement_index {
                                let assumption = Guard { required: true, proof: GuardProof::Pending,
                                    iteration: None, branch, successor: block,
                                    index: None, input: false, phi: false, choice: None };
                                if !path.guards.contains(&assumption) { path.guards.push(assumption); }
                            }
                        }
                        let normal_call = match &data.terminator().kind {
                            TerminatorKind::Call { target, .. } => *target == Some(successor),
                            TerminatorKind::Drop { target, .. } => *target == successor,
                            _ => false,
                        };
                        if !memory && normal_call && self.facts.exclusions.contains(&frame.instance)
                            && !path.returns.contains(&at) { path.returns.push(at); }
                        if !memory && !normal_call && matches!(data.terminator().kind,
                            TerminatorKind::Call { .. } | TerminatorKind::Drop { .. }) { path.unresolved = true; }
                        if let Some(guard) = guard {
                            if path.guards.iter().any(|previous| previous.branch == guard.branch && previous.phi == guard.phi
                                && previous.successor != guard.successor) {
                                path.unresolved = true;
                                path.guards.retain(|previous| previous.branch != guard.branch || previous.phi != guard.phi);
                            } else if !path.guards.contains(&guard) { path.guards.push(guard); }
                        }
                        if let Some(blocks) = index.loops.get(&(block, successor)) {
                            path.guards.retain(|guard| !blocks.contains(&guard.branch.block));
                            path.returns.retain(|location| !blocks.contains(&location.block));
                            if memory {
                                path.guards.push(Guard { required: false, proof: GuardProof::Widened,
                                    iteration: None, branch: Location { block: successor, statement_index: 0 },
                                    successor: mir::START_BLOCK, index: None, input: false, phi: true, choice: None });
                            } else { path.unresolved = true; }
                        }
                        if !memory && index.loops.keys().any(|(_, header)| *header == successor) {
                            let header = Location { block: successor, statement_index: 0 };
                            if !index.loops.contains_key(&(block, successor)) {
                                let entry = Guard { required: true, proof: GuardProof::Established,
                                    iteration: Some(0), branch: header, successor: mir::START_BLOCK,
                                    index: None, input: false, phi: true, choice: None };
                                if !path.guards.contains(&entry) { path.guards.push(entry); }
                            }
                            let incoming = Guard { required: true, proof: GuardProof::Pending,
                                iteration: None, branch: header, successor: block,
                                index: None, input: false, phi: true, choice: None };
                            path.guards.retain(|guard| !(guard.phi && guard.iteration.is_none() && guard.branch == header));
                            path.guards.push(incoming);
                        }
                        path.guards.sort_unstable_by_key(|guard| (guard.branch, guard.successor));
                        path.returns.sort_unstable();
                    }
                    let previous = states[successor.as_usize()].clone();
                    let mut values = previous.clone();
                    for path in incoming {
                        if !values.contains(&path) { values.push(path); }
                    }
                    let mut forms = Vec::<(Vec<Location>, bool)>::new();
                    let mut alternatives = Vec::new();
                    for path in values {
                        let form = (path.returns, path.unresolved);
                        let identity = if let Some(identity) = forms.iter().position(|previous| *previous == form) {
                            identity
                        } else {
                            forms.push(form);
                            forms.len() - 1
                        };
                        alternatives.push(Alternative { value: Value::Bits(identity as u128),
                            guards: path.guards.into_iter().map(|guard| (point.frame, guard)).collect() });
                    }
                    let mut values: Vec<_> = self.bounded(alternatives)?.into_iter().map(|value| {
                        let Value::Bits(identity) = value.value else { unreachable!() };
                        let (returns, unresolved) = &forms[identity as usize];
                        ControlPath { guards: value.guards.into_iter().map(|(_, guard)| guard).collect(),
                            returns: returns.clone(), unresolved: *unresolved }
                    }).collect();
                    if values.len() > cap || merged[successor.as_usize()] {
                        let keep = cap.saturating_sub(1).min(values.len() - 1);
                        let mut common = values[keep].clone();
                        common.guards.retain(|guard| values[keep..].iter().all(|path| path.guards.contains(guard)));
                        common.returns.retain(|location| values[keep..].iter().all(|path| path.returns.contains(location)));
                        common.unresolved = true;
                        values.truncate(keep);
                        values.push(common);
                        if !merged[successor.as_usize()] { self.controls.widened += 1; }
                        merged[successor.as_usize()] = true;
                    }
                    if previous.len() != values.len() || values.iter().any(|value| !previous.contains(value)) {
                        states[successor.as_usize()] = values;
                        if queued.insert(successor) { queue.push_back(successor); }
                    }
                }
            }
            let bytes = states.iter().map(|paths| std::mem::size_of::<Vec<ControlPath>>() * 4
                + paths.len() * std::mem::size_of::<ControlPath>() * 2
                + paths.iter().map(|path| path.guards.len() * std::mem::size_of::<Guard>() * 2
                    + path.returns.len() * std::mem::size_of::<Location>() * 2).sum::<usize>()).sum::<usize>();
            let paths: Arc<[Vec<ControlPath>]> = Arc::from(states);
            if self.controls.bytes.saturating_add(bytes) > self.cache_limit / 16 {
                self.controls.templates.clear();
                self.controls.answers.clear();
                self.controls.bytes = 0;
            }
            if bytes <= self.cache_limit / 16 {
                self.controls.bytes += bytes;
                self.controls.templates.insert(template_key, paths.clone());
            }
            paths
        };
        self.budget.scratch = scratch.saturating_add(self.flow.bytes.saturating_sub(stored))
            .saturating_add(self.controls.pending_bytes.saturating_sub(pending_stored));
        let mut coarse = Vec::new();
        for path in &paths[point.location.block.as_usize()] {
            let mut guards: Vec<_> = path.guards.iter().map(|guard| (point.frame, *guard)).collect();
            for (&branch, _) in &index.assumptions {
                if branch.block == point.location.block && branch.statement_index < point.location.statement_index {
                    guards.push((point.frame, Guard { required: true, proof: GuardProof::Pending,
                        iteration: None, branch, successor: point.location.block,
                        index: None, input: false, phi: false, choice: None }));
                }
            }
            if path.unresolved || !path.returns.is_empty() { guards.push(marker); }
            coarse.push(Alternative { value: Value::Bits(1), guards });
        }
        if self.facts.safe.contains(&frame.instance)
            && let Some(&cause) = self.controls.loop_fallbacks.get(&(frame.instance, frame.irq))
        {
            self.controls.causes.insert(point, cause);
            for value in &mut coarse {
                for (_, guard) in &mut value.guards {
                    if guard.proof == GuardProof::Pending && !guard.input && !guard.phi
                        && guard.index.is_none() && guard.choice.is_none() { guard.proof = GuardProof::Unresolved; }
                }
                if !value.guards.contains(&marker) { value.guards.push(marker); }
            }
            return self.bounded(coarse);
        }
        if memory {
            return if refine { self.feasible(coarse) } else { self.bounded(coarse) };
        }
        if !refine {
            for value in &mut coarse {
                for (_, guard) in &mut value.guards {
                    if guard.proof == GuardProof::Pending && !guard.input && !guard.phi
                        && guard.index.is_none() && guard.choice.is_none() { guard.proof = GuardProof::Unresolved; }
                }
                if !value.guards.contains(&marker) { value.guards.push(marker); }
            }
            return Ok(coarse);
        }
        let answer = (|| {
            for (path, fallback) in paths[point.location.block.as_usize()].iter().zip(&coarse) {
                let mut base = fallback.clone();
                if !path.unresolved { base.guards.retain(|entry| *entry != marker); }
                let mut values = self.feasible(vec![base])?;
                for &location in &path.returns {
                    if values.is_empty() { break; }
                    let caller = Point { frame: point.frame, location };
                    let mut returns = Vec::new();
                    for target in self.targets(caller)? {
                        let Value::Function(instance) = target.value else {
                            let mut guards = target.guards;
                            guards.push(marker);
                            returns.push(Alternative { value: Value::Bits(1), guards });
                            continue;
                        };
                        if self.excludes_return(caller, instance) { continue; }
                        if !self.facts.exclusions.contains(&instance) || !self.trust.checks(instance.def_id())
                            || self.tcx.is_foreign_item(instance.def_id())
                            || matches!(instance.def, InstanceKind::Intrinsic(_) | InstanceKind::LlvmIntrinsic(_)
                                | InstanceKind::Virtual(..) | InstanceKind::Shim(ty::ShimKind::DropGlue(_, None)))
                            || !irq_check_rupta::util::type_util::has_instance_mir(self.tcx, instance.def)
                            || self.returns_normally(instance, frame.irq)?
                        {
                            returns.push(Alternative { value: Value::Bits(1), guards: target.guards });
                            continue;
                        }
                        let child = self.frame(Frame { instance, caller: Some(caller), irq: frame.irq, unknown_entry: false });
                        let returned = self.with_input(child, caller, |solver| {
                            let child_index = solver.index(child)?;
                            let mut returned = Vec::new();
                            for block in child_index.body.basic_blocks.indices() {
                                let data = &child_index.body.basic_blocks[block];
                                if !matches!(data.terminator().kind, TerminatorKind::Return | TerminatorKind::TailCall { .. }) { continue; }
                                let at = Point { frame: child, location: Location { block, statement_index: data.statements.len() } };
                                let paths = if solver.flow.current.is_some() { solver.control(at)? }
                                    else { solver.control_summary(at)? };
                                for path in paths {
                                    let unresolved = !path.path_proven();
                                    let mut exits = solver.exit_values(at, caller, vec![path], true)?;
                                    if unresolved {
                                        for value in &mut exits {
                                            if !value.guards.contains(&marker) { value.guards.push(marker); }
                                        }
                                    }
                                    returned.extend(exits);
                                }
                            }
                            Ok(returned)
                        })?;
                        for returned in returned {
                            if let Some(guards) = self.merge(&target.guards, &returned.guards)? {
                                returns.push(Alternative { value: Value::Bits(1), guards });
                            }
                        }
                    }
                    let mut joined = Vec::new();
                    for value in &values {
                        for returned in &returns {
                            if let Some(guards) = self.merge(&value.guards, &returned.guards)? {
                                let bytes = guards.capacity().saturating_mul(std::mem::size_of::<(usize, Guard)>() * 4);
                                if bytes > limits.function_scratch_mib * 1024 * 1024 {
                                    return Err(QueryStop::ScratchLimit);
                                }
                                joined.push(Alternative { value: Value::Bits(1), guards });
                                if joined.len() > limits.alternatives { joined = self.bounded(joined)?; }
                            }
                        }
                    }
                    values = self.feasible(joined)?;
                }
                finished.extend(values);
            }
            self.bounded(finished.clone())
        })();
        self.budget.scratch = scratch.saturating_add(self.flow.bytes.saturating_sub(stored))
            .saturating_add(self.controls.pending_bytes.saturating_sub(pending_stored));
        answer
    }
}
