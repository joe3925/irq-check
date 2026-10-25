use super::*;
use super::super::global::PointerObject;

#[derive(Clone)]
enum Version {
    Merge(Vec<(Location, Option<Guard>, bool)>),
    Definition { at: Location, previous: Location },
}

struct MemorySsa {
    versions: HashMap<Location, Version>,
    definitions: Vec<Vec<usize>>,
    aliases: HashMap<Location, (Location, Option<CallEdge>)>,
}

struct Demand<'tcx> {
    key: ReadKey<'tcx>,
    values: Vec<Alternative<'tcx>>,
    users: HashSet<usize>,
    inputs: HashSet<usize>,
    indexes: HashSet<(Point, mir::Local)>,
    state: DemandState,
    choice: bool,
    cardinality: bool,
    loop_updates: Option<usize>,
    loop_widened: bool,
    abstract_top: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum DemandState {
    Pending,
    Evaluated,
    Complete,
}

struct Request {
    root: usize,
    mode: RequestMode,
    state: RequestState,
    required: HashSet<usize>,
    pending: VecDeque<usize>,
    stopped_at: Option<usize>,
    steps: usize,
}

#[derive(Default)]
pub(crate) struct Flow<'tcx> {
    templates: HashMap<(Instance<'tcx>, bool, Option<mir::Local>, Option<PointerObject<'tcx>>, Option<(i128, u64)>, bool), Arc<MemorySsa>>,
    demands: Vec<Demand<'tcx>>,
    ids: HashMap<ReadKey<'tcx>, usize>,
    queued: HashSet<usize>,
    active: Option<Request>,
    pub(super) current: Option<usize>,
    pub(super) current_inputs: HashSet<usize>,
    pub(super) bytes: usize,
    function_bytes: HashMap<Instance<'tcx>, usize>,
    template_bytes: usize,
    ancestors: HashMap<usize, Arc<HashSet<usize>>>,
    ancestor_bytes: usize,
    pub(crate) scalar_steps: HashMap<Point, usize>,
    pub(crate) evaluations: usize,
    pub(crate) changes: usize,
    pub(crate) requirement_visits: usize,
    pub(crate) requirement_repairs: usize,
    pub(crate) requirement_repair_full: usize,
    pub(crate) requirement_repair_affected: usize,
    pub(crate) requirement_repair_visits: usize,
    pub(crate) generation: usize,
    diagnostic_root: Option<usize>,
    diagnostic_stop: Option<(Point, Address, QueryStop)>,
    loss_origins: HashMap<usize, (Point, &'static str, &'static str)>,
    pub(crate) inputs: HashMap<usize, Vec<Point>>,
    callees: HashMap<usize, Vec<(Location, usize)>>,
    pub(super) roots: HashSet<usize>,
    pub(crate) recursive: HashMap<usize, usize>,
}

impl Flow<'_> {
    pub(super) fn single_input(&self, frame: usize) -> bool {
        !self.roots.contains(&frame) && self.inputs.get(&frame).is_some_and(|inputs| inputs.len() == 1)
    }

    pub(super) fn singleton_read(&mut self, values: &[Alternative<'_>], complete: bool) -> bool {
        if !complete || !values.first().is_some_and(|first|
            matches!(first.value, Value::Pointer(..) | Value::Bits(_) | Value::Variant(_) | Value::Function(_))
                && values.iter().all(|value| value.value == first.value)) { return false; }
        if let Some(current) = self.current { self.demands[current].cardinality = true; }
        true
    }

    pub(super) fn inputs_complete(&self, inputs: &HashSet<usize>) -> bool {
        inputs.iter().all(|id| self.demands[*id].state == DemandState::Complete)
    }

    fn enqueue(&mut self, id: usize) {
        self.demands[id].state = DemandState::Pending;
        let mut pending: Vec<_> = self.demands[id].users.iter().copied().collect();
        while let Some(user) = pending.pop() {
            if self.demands[user].state == DemandState::Complete {
                self.demands[user].state = DemandState::Evaluated;
                pending.extend(self.demands[user].users.iter().copied());
            }
        }
        if self.queued.insert(id)
            && let Some(active) = &mut self.active
            && active.required.contains(&id)
        {
            active.pending.push_back(id);
        }
    }
}

impl<'a, 'tcx> Solver<'a, 'tcx> {
    fn require(&mut self, id: usize, schedule: bool) -> Result<(), QueryStop> {
        if self.flow.active.as_ref().expect("active value-flow request").required.contains(&id) {
            self.budget.charge(0)?;
            self.flow.requirement_visits += 1;
            return Ok(());
        }
        let mut pending = vec![id];
        while let Some(id) = pending.pop() {
            self.budget.charge(0)?;
            self.flow.requirement_visits += 1;
            let active = self.flow.active.as_mut().expect("active value-flow request");
            if !active.required.insert(id) { continue; }
            self.budget.reserve(std::mem::size_of::<usize>() * 8)?;
            if schedule && self.flow.queued.contains(&id) { active.pending.push_front(id); }
            let mut inputs: Vec<_> = self.flow.demands[id].inputs.iter().copied().collect();
            inputs.sort_unstable_by(|left, right| right.cmp(left));
            pending.extend(inputs);
        }
        Ok(())
    }

    pub(crate) fn request(
        &mut self,
        point: Point,
        mode: RequestMode,
        allowance: usize,
        mut operation: impl FnMut(&mut Self) -> Result<Vec<Alternative<'tcx>>, QueryStop>,
    ) -> ReadOutcome<Vec<Alternative<'tcx>>> {
        if mode == RequestMode::RequiredBinding || self.budget.refinement_limit.is_some() {
            let scratch = self.budget.scratch;
            let stored = self.flow.bytes;
            let mut staged = mode == RequestMode::RequiredBinding && self.budget.refinement_limit.is_none()
                && !self.controls.solving && self.flow.active.is_none();
            let stage_allowance = irq_check_rupta::limits::configuration().expect("validated analysis limits").scalar_steps;
            loop {
                let stopped_conditions = self.controls.stopped_conditions.len();
                let saved_refinement_limit = self.budget.refinement_limit;
                if staged {
                    self.budget.refinement_limit = Some(self.budget.steps.saturating_add(stage_allowance)
                        .min(self.budget.limit));
                }
                let answer = operation(self);
                self.budget.refinement_limit = saved_refinement_limit;
                let reason = match answer {
                    Ok(values) => return ReadOutcome::Complete(values),
                    Err(reason) => reason,
                };
                if mode == RequestMode::RequiredBinding && self.budget.refinement_limit.is_none()
                    && !self.controls.solving && self.flow.active.is_none()
                    && self.controls.stopped_conditions.len() > stopped_conditions
                    && matches!(reason, QueryStop::ControlMemory | QueryStop::RefinementLimit
                        | QueryStop::ScratchLimit | QueryStop::AlternativeLimit)
                {
                    self.controls.epoch += 1;
                    self.retain_completed();
                    self.budget.scratch = scratch.saturating_add(self.flow.bytes.saturating_sub(stored));
                    self.controls.dependency_stop = None;
                    continue;
                }
                if mode != RequestMode::RequiredBinding || self.budget.refinement_limit.is_some()
                    || self.controls.solving || self.flow.active.is_some()
                    || !matches!(reason, QueryStop::RefinementLimit | QueryStop::TransferLimit
                        | QueryStop::ScratchLimit | QueryStop::AlternativeLimit)
                { return ReadOutcome::Stopped(reason); }
                let cause = self.controls.dependency_stop.unwrap_or((point, reason));
                let mut changed = false;
                for frame in self.controls.attempted_loops.drain() {
                    let frame = self.frames[frame];
                    if let std::collections::hash_map::Entry::Vacant(entry) = self.controls.loop_fallbacks.entry((frame.instance, frame.irq)) {
                        entry.insert(cause);
                        changed = true;
                    }
                }
                if !changed {
                    if staged && reason == QueryStop::RefinementLimit && self.budget.steps < self.budget.limit {
                        staged = false;
                        continue;
                    }
                    return ReadOutcome::Stopped(reason);
                }
                self.controls.epoch += 1;
                self.retain_completed();
                self.budget.scratch = scratch.saturating_add(self.flow.bytes.saturating_sub(stored));
                if std::env::var_os("IRQ_CHECK_TRACE_FLOW_COST").is_some() {
                    eprintln!("irq-check: binding loop fallback {} at={:?} stop={reason:?} loops={} steps={} limit={}",
                        self.frames[point.frame].instance, point.location, self.controls.loop_fallbacks.len(),
                        self.budget.steps, self.budget.limit);
                }
                if self.budget.steps >= self.budget.limit { return ReadOutcome::Stopped(QueryStop::TransferLimit); }
                self.controls.dependency_stop = None;
            }
        }
        let scalar_allowance = irq_check_rupta::limits::configuration().expect("validated analysis limits").scalar_steps;
        let allowance = allowance.min(scalar_allowance);
        let spent = self.flow.scalar_steps.get(&point).copied().unwrap_or(0);
        if spent >= allowance { return ReadOutcome::Stopped(QueryStop::RefinementLimit); }
        let before = self.budget.steps;
        let saved_choices = self.index_choices.clone();
        let saved_split = self.split_index.take();
        let saved_scratch = self.budget.scratch;
        self.budget.refinement_limit = Some(before.saturating_add(allowance - spent));
        let mut pending = vec![saved_choices.clone()];
        let answer = (|| {
            let mut complete = Vec::new();
            while let Some(choices) = pending.pop() {
                if self.index_choices != choices {
                    self.index_choices = choices;
                    self.retain_completed();
                }
                match operation(self) {
                    Ok(values) => {
                        for value in values {
                            if !complete.contains(&value) { complete.push(value); }
                        }
                        complete = self.bounded(complete)?;
                    }
                    Err(QueryStop::SplitIndex) => {
                        let (point, local, length) = self.split_index.take().ok_or(QueryStop::Unsupported)?;
                        let length = usize::try_from(length).map_err(|_| QueryStop::ScratchLimit)?;
                        let bytes = length.checked_mul((self.index_choices.len() + 1)
                            * std::mem::size_of::<((Point, mir::Local), u128)>() * 4).ok_or(QueryStop::ScratchLimit)?;
                        self.budget.charge(bytes)?;
                        for index in (0..length).rev() {
                            let mut choices = self.index_choices.clone();
                            choices.insert((point, local), index as u128);
                            pending.push(choices);
                        }
                    }
                    Err(reason) => return Err(reason),
                }
            }
            Ok(complete)
        })();
        *self.flow.scalar_steps.entry(point).or_default() += self.budget.steps.saturating_sub(before);
        self.budget.refinement_limit = None;
        self.index_choices = saved_choices;
        self.split_index = saved_split;
        self.retain_completed();
        self.budget.scratch = saved_scratch;
        match answer {
            Ok(values) => ReadOutcome::Complete(values),
            Err(reason) => ReadOutcome::Stopped(reason),
        }
    }

    pub(crate) fn control(&mut self, point: Point) -> Result<Vec<Alternative<'tcx>>, QueryStop> {
        self.read_edge(Address { object: Object::Local(point.frame, mir::RETURN_PLACE), offset: 0 },
            self.tcx.types.unit, point, false, None, true)
    }

    pub(super) fn excludes_return(&self, point: Point, instance: Instance<'tcx>) -> bool {
        if !self.cut_recursive_returns { return false; }
        let mut ancestor = Some(point.frame);
        while let Some(parent) = ancestor {
            if self.frames[parent].instance == instance
                && self.frames[parent].caller.is_some_and(|caller|
                    self.frames[caller.frame].instance == self.frames[point.frame].instance
                        && caller.location == point.location)
            { return true; }
            ancestor = self.frames[parent].caller.map(|caller| caller.frame);
        }
        false
    }

    fn forget_loop_guards(&mut self, values: &mut [Alternative<'tcx>], point: Point,
        blocks: &HashSet<mir::BasicBlock>) -> Result<(), QueryStop>
    {
        let frame = point.frame;
        if let Some(current) = self.flow.current {
            self.flow.demands[current].cardinality = true;
        }
        let mut pending = Vec::new();
        for &(location, callee) in self.flow.callees.get(&frame).into_iter().flatten() {
            self.budget.charge(std::mem::size_of::<usize>() * 4)?;
            if blocks.contains(&location.block) {
                pending.push(callee);
            }
        }
        let mut varying_frames = HashSet::new();
        while let Some(callee) = pending.pop() {
            self.budget.charge(std::mem::size_of::<usize>() * 4)?;
            if !varying_frames.insert(callee) { continue; }
            pending.extend(self.flow.callees.get(&callee).into_iter().flatten().map(|(_, callee)| *callee));
        }
        for value in values {
            let mut lost = false;
            value.guards.retain(|(owner, guard)| {
                let keep = guard.proof == GuardProof::Widened || guard.input
                    || (*owner == frame && guard.iteration.is_some() && guard.branch == point.location)
                    || (*owner == frame && !blocks.contains(&guard.branch.block))
                    || (*owner != frame && !varying_frames.contains(owner));
                lost |= !keep && guard.proof != GuardProof::Established
                    && !guard.phi && guard.index.is_none() && guard.choice.is_none();
                keep
            });
            for (owner, guard) in &mut value.guards {
                if *owner == frame && guard.branch == point.location && let Some(iteration) = &mut guard.iteration {
                    *iteration = iteration.checked_add(1).ok_or(QueryStop::AlternativeLimit)?;
                }
            }
            if lost {
                let marker = (frame, Guard { required: false, iteration: None, proof: GuardProof::Widened,
                    branch: Location::START, successor: mir::START_BLOCK,
                    index: None, input: false, phi: false, choice: None });
                if !value.guards.contains(&marker) { value.guards.push(marker); }
            }
        }
        Ok(())
    }

    fn loop_values(&mut self, point: Point, parent: Option<mir::BasicBlock>, values: Vec<Alternative<'tcx>>)
        -> Result<Vec<Alternative<'tcx>>, QueryStop>
    {
        let index = self.index(point.frame)?;
        let header = Point { location: Location { block: point.location.block, statement_index: 0 }, ..point };
        let backedge = parent.and_then(|parent| index.loops.get(&(parent, header.location.block)));
        if !index.loops.keys().any(|(_, block)| *block == header.location.block) { return Ok(values); }
        let mut result = Vec::new();
        for mut value in values {
            if self.frames[point.frame].irq {
                if backedge.is_some() {
                    if !value.guards.iter().any(|(owner, guard)| *owner == point.frame
                        && guard.branch == header.location && guard.iteration.is_some()) { continue; }
                } else {
                    let iteration = (point.frame, Guard {
                        required: true, proof: GuardProof::Established, iteration: Some(0),
                        branch: header.location, successor: mir::START_BLOCK,
                        index: None, input: false, phi: true, choice: None,
                    });
                    let Some(guards) = self.merge(&value.guards, &[iteration])? else { continue; };
                    value.guards = guards;
                }
            }
            if let Some(parent) = parent {
                let phi = (point.frame, Guard {
                    required: true, proof: GuardProof::Pending, iteration: None,
                    branch: header.location, successor: parent,
                    index: None, input: false, phi: true, choice: None,
                });
                let Some(guards) = self.merge(&value.guards, &[phi])? else { continue; };
                value.guards = guards;
            }
            result.push(value);
        }
        Ok(result)
    }

    pub(super) fn singleton_object(&mut self, object: Object) -> Result<bool, QueryStop> {
        let Object::Heap(origin) = object else {
            return Ok(!matches!(object, Object::SummaryLocal(..)));
        };
        if let Some(current) = self.flow.current {
            self.flow.demands[current].cardinality = true;
        }
        let mut pending = vec![origin];
        let mut seen = HashSet::new();
        while let Some(point) = pending.pop() {
            self.budget.charge(0)?;
            if !seen.insert(point.frame) || self.flow.recursive.contains_key(&point.frame)
                || self.index(point.frame)?.loops.values().any(|blocks| blocks.contains(&point.location.block))
            {
                return Ok(false);
            }
            if let Some(inputs) = self.flow.inputs.get(&point.frame) {
                if inputs.len() > 1 || self.flow.roots.contains(&point.frame) {
                    return Ok(false);
                }
                pending.extend(inputs.iter().copied());
            }
        }
        Ok(true)
    }

    pub(crate) fn entry_guard(&self, frame: usize, caller: Point) -> (usize, Guard) {
        let input = self.flow.inputs[&frame].iter().position(|input| *input == caller)
            .expect("registered call input");
        (frame, Guard { required: true, iteration: None, proof: GuardProof::Pending, branch: Location::START, successor: mir::BasicBlock::from_usize(input), index: None, input: true, phi: false, choice: None })
    }

    pub(super) fn with_input<T>(&mut self, frame: usize, caller: Point,
        operation: impl FnOnce(&mut Self) -> Result<T, QueryStop>) -> Result<T, QueryStop>
    {
        let input = self.entry_guard(frame, caller).1.successor.as_usize();
        let previous = self.input_choices.insert(frame, input);
        let answer = operation(self);
        if let Some(previous) = previous { self.input_choices.insert(frame, previous); }
        else { self.input_choices.remove(&frame); }
        answer
    }

    pub(super) fn read_guarded(&mut self, address: Address, requested: Ty<'tcx>, point: Point,
        guards: &[(usize, Guard)]) -> Result<Vec<Alternative<'tcx>>, QueryStop>
    {
        if self.merge(guards, &[])?.is_none() { return Ok(Vec::new()); }
        for &(frame, guard) in guards {
            if let Some(local) = guard.index {
                let point = Point { frame, location: guard.branch };
                if self.index_choice((point, local))?.is_some_and(|choice| choice != guard.successor.as_usize() as u128) {
                    return Ok(Vec::new());
                }
            }
        }
        let saved_inputs = self.input_choices.clone();
        let saved_indexes = self.index_choices.clone();
        for &(frame, guard) in guards {
            if guard.input { self.input_choices.insert(frame, guard.successor.as_usize()); }
            if let Some(local) = guard.index {
                self.index_choices.insert((Point { frame, location: guard.branch }, local), guard.successor.as_usize() as u128);
            }
        }
        let answer = self.read(address, requested, point, false);
        self.input_choices = saved_inputs;
        self.index_choices = saved_indexes;
        answer
    }

    pub(super) fn index_choice(&mut self, key: (Point, mir::Local)) -> Result<Option<u128>, QueryStop> {
        if let Some(current) = self.flow.current
            && !self.flow.demands[current].indexes.contains(&key)
        {
            let bytes = std::mem::size_of::<(Point, mir::Local)>() * 4;
            let function = self.frames[self.flow.demands[current].key.point.frame].instance;
            if self.flow.function_bytes.get(&function).copied().unwrap_or(0).saturating_add(bytes)
                > irq_check_rupta::limits::configuration().expect("validated analysis limits").function_scratch_mib * 1024 * 1024
            {
                self.budget.record_limit(QueryStop::ScratchLimit, "function index bytes",
                    self.flow.function_bytes.get(&function).copied().unwrap_or(0).saturating_add(bytes),
                    irq_check_rupta::limits::configuration().expect("validated analysis limits").function_scratch_mib * 1024 * 1024);
                return Err(QueryStop::ScratchLimit);
            }
            self.budget.charge(bytes)?;
            self.flow.demands[current].indexes.insert(key);
            *self.flow.function_bytes.entry(function).or_default() += bytes;
            self.flow.bytes = self.flow.bytes.checked_add(bytes).ok_or(QueryStop::ScratchLimit)?;
        }
        Ok(self.index_choices.get(&key).copied())
    }

    pub(super) fn boundary_addresses(address: Address, frame: usize, entering: bool) -> Vec<Address> {
        let objects = match (address.object, entering) {
            (Object::Local(owner, local), true) if owner == frame => vec![Object::PriorLocal(owner, local)],
            (Object::PriorLocal(owner, local), true) if owner == frame => vec![Object::SummaryLocal(owner, local)],
            (Object::PriorLocal(owner, local), false) if owner == frame => vec![Object::Local(owner, local)],
            (Object::SummaryLocal(owner, local), false) if owner == frame =>
                vec![Object::PriorLocal(owner, local), Object::SummaryLocal(owner, local)],
            _ => vec![address.object],
        };
        objects.into_iter().map(|object| Address { object, ..address }).collect()
    }

    fn boundary_values(&mut self, frame: usize, values: Vec<Alternative<'tcx>>, entering: bool)
        -> Result<Vec<Alternative<'tcx>>, QueryStop>
    {
        let mut result = Vec::new();
        for value in values {
            let source = match value.value {
                Value::Pointer(address, _) | Value::Symbolic(address, ..) | Value::Predicate(address, ..) => Some(address),
                Value::ObjectPointer(object) => Some(Address { object, offset: 0 }),
                _ => None,
            };
            let Some(source) = source else { result.push(value); continue; };
            for address in Self::boundary_addresses(source, frame, entering) {
                let mapped = if !entering && matches!(source.object, Object::Local(owner, _) if owner == frame) {
                    Value::Unknown("the value refers to a completed stack activation")
                } else {
                    match value.value {
                        Value::Pointer(_, metadata) => Value::Pointer(address, metadata),
                        Value::ObjectPointer(_) => Value::ObjectPointer(address.object),
                        Value::Symbolic(_, bias, width) => Value::Symbolic(address, bias, width),
                        Value::Predicate(_, width, bits, equal) => Value::Predicate(address, width, bits, equal),
                        _ => value.value,
                    }
                };
                result.push(Alternative { value: mapped, guards: value.guards.clone() });
            }
        }
        self.bounded(result)
    }

    fn fixed_local(&mut self, point: Point, local: mir::Local) -> Result<bool, QueryStop> {
        self.budget.charge(0)?;
        let index = self.index(point.frame)?;
        Ok(!index.address_taken.contains(&local) && index.clobbers.is_empty()
            && self.ty(point.frame, index.body.local_decls[local].ty)
                .is_freeze(self.tcx, TypingEnv::fully_monomorphized())
            && index.local_definitions.get(&local).is_some_and(|definitions| {
                if definitions.len() != 1 { return false; }
                let source = definitions[0];
                matches!(source.kind, super::super::query::DefinitionKind::Assignment
                    | super::super::query::DefinitionKind::CallReturn)
                    && source.place.is_some_and(|place| place.projection.is_empty())
                    && !index.loops.values().any(|blocks| blocks.contains(&source.location.block))
                    && index.body.basic_blocks.dominators().dominates(source.location.block, point.location.block)
                    && (source.location.block != point.location.block
                        || source.location.statement_index < point.location.statement_index)
            }))
    }

    fn input_values(&mut self, frame: usize, caller: Point, values: Vec<Alternative<'tcx>>,
        refine: bool, storage: Option<Object>)
        -> Result<Vec<Alternative<'tcx>>, QueryStop>
    {
        if values.is_empty() { return Ok(values); }
        let refine = refine && match storage {
            Some(Object::Local(owner, local)) if owner == caller.frame => !self.fixed_local(caller, local)?,
            _ => true,
        };
        let targets = self.targets(caller)?;
        let paths = if refine { self.paths(caller, false)? }
            else { vec![Alternative { value: Value::Bits(1), guards: Vec::new() }] };
        let input = self.entry_guard(frame, caller);
        let mut result = Vec::new();
        let scoped_path = matches!(storage, Some(Object::Static(_)))
            || storage.is_none() && values.iter().all(|value|
            matches!(value.value, Value::Metadata(Metadata::Concrete(_))
                | Value::Pointer(_, Metadata::Concrete(_))));
        let binding_conditions: HashSet<_> = values.iter().chain(&targets).flat_map(|value|
            value.guards.iter().filter(|(_, guard)| guard.required)
                .map(|(frame, guard)| Point { frame: *frame, location: guard.branch })).collect();
        let mut path_conditions = HashSet::new();
        for value in values {
            for target in &targets {
                if matches!(target.value, Value::Function(instance) if instance != self.frames[frame].instance) { continue; }
                let Some(guards) = self.merge(&value.guards, &target.guards)? else { continue; };
                for path in &paths {
                    let mut path_guards = path.guards.clone();
                    for (owner, guard) in &mut path_guards {
                        guard.required = refine
                            && !value.guards.contains(&(*owner, Guard { required: false, ..*guard }));
                        let condition = Point { frame: *owner, location: guard.branch };
                        if scoped_path && guard.required && !binding_conditions.contains(&condition) {
                            path_conditions.insert(condition);
                        }
                    }
                    if let Some(guards) = self.merge(&guards, &path_guards)? {
                        result.push(Alternative { value: value.value, guards });
                    }
                }
            }
        }
        let values = self.feasible_with_paths(result, &path_conditions)?;
        let mut result = Vec::new();
        for value in values {
            let mut guards = value.guards;
            let component = self.flow.recursive.get(&frame).copied();
            guards.retain(|(owner, _)| *owner != frame
                && component.is_none_or(|component| self.flow.recursive.get(owner) != Some(&component)));
            if component.is_some_and(|component| self.flow.recursive.get(&caller.frame) == Some(&component)) {
                guards.clear();
            }
            if let Some(guards) = self.merge(&guards, &[input])? {
                result.push(Alternative { value: value.value, guards });
            }
        }
        self.boundary_values(frame, result, true)
    }

    pub(super) fn exit_values(&mut self, point: Point, caller: Point, values: Vec<Alternative<'tcx>>, control: bool)
        -> Result<Vec<Alternative<'tcx>>, QueryStop>
    {
        if values.is_empty() { return Ok(values); }
        let paths = if control { vec![Alternative { value: Value::Bits(1), guards: Vec::new() }] }
            else { self.paths(point, true)? };
        let mut result = Vec::new();
        let input = self.entry_guard(point.frame, caller);
        for value in values {
            let Some(guards) = self.merge(&value.guards, &[input])? else { continue; };
            for path in &paths {
                if let Some(mut guards) = self.merge(&guards, &path.guards)? {
                    let component = self.flow.recursive.get(&point.frame).copied();
                    guards.retain(|(owner, guard)| !(*owner == point.frame && guard.input)
                        && component.is_none_or(|component| self.flow.recursive.get(owner) != Some(&component)));
                    if component.is_some_and(|component| self.flow.recursive.get(&caller.frame) == Some(&component)) {
                        guards.clear();
                    }
                    result.push(Alternative { value: value.value, guards });
                }
            }
        }
        self.boundary_values(point.frame, result, false)
    }

    pub(crate) fn conservative_targets(
        &mut self,
        point: Point,
        allowance: usize,
    ) -> Result<Vec<Instance<'tcx>>, QueryStop> {
        let saved_limit = self.budget.limit;
        let saved_scratch = self.budget.scratch;
        self.budget.limit = self.budget.steps.saturating_add(allowance);
        let answer = (|| {
        let mut targets = Vec::new();
        let mut covered = false;
        if let Some(contexts) = &self.frame_contexts[point.frame] {
            covered = !contexts.is_empty();
            for &context in contexts {
                self.budget.charge(0)?;
                let Some(bindings) = self.facts.context_bindings.get(&(context, point.location))
                    .filter(|bindings| !bindings.is_empty()) else { covered = false; break; };
                for &(target, _) in bindings {
                    self.budget.charge(std::mem::size_of::<Instance<'tcx>>() * 4)?;
                    if !targets.contains(&target) { targets.push(target); }
                }
            }
        }
        if covered { return Ok(targets); }
        let key = (self.frames[point.frame].instance, point.location);
        if self.facts.covered_calls.contains(&key)
            && let Some(targets) = self.facts.safe_candidates.get(&key).filter(|targets| !targets.is_empty())
        {
            self.budget.charge(targets.len().saturating_mul(std::mem::size_of::<Instance<'tcx>>() * 4))?;
            return Ok(targets.iter().copied().collect());
        }
        Err(QueryStop::Unsupported)
        })();
        self.budget.limit = saved_limit;
        self.budget.scratch = saved_scratch;
        answer
    }

    pub(crate) fn begin_query(&mut self, limit: usize) {
        self.impacts.stopped.clear();
        self.controls.stopped_conditions.retain(|key, _| key.generation == self.flow.generation);
        self.controls.stopped_bytes = self.controls.stopped_conditions.keys().map(|key|
            std::mem::size_of::<(super::control::ConditionKey, (Point, QueryStop))>() * 4
                + (key.choices.len() + key.inputs.len() + key.witnesses.len() + key.loop_entries.len()) * 64).sum();
        self.controls.last_stop = None;
        self.controls.dependency_stop = None;
        self.controls.causes.clear();
        self.controls.attempted_loops.clear();
        self.index_choices.clear();
        self.loop_entries = Box::new([]);
        self.cut_recursive_returns = false;
        self.retain_completed();
        self.flow.scalar_steps.clear();
        self.input_choices.clear();
        self.condition_origin = None;
        self.budget = QueryBudget { steps: 0, limit, scratch: 0, refinement_limit: None, limit_usage: None };
    }

    pub(crate) fn retain_completed(&mut self) {
        assert!(self.flow.active.is_none());
        self.flow.diagnostic_root = None;
        self.flow.diagnostic_stop = None;
        let mut invalid: HashSet<_> = self.flow.demands.iter().enumerate().filter_map(|(id, demand)| {
            (demand.state != DemandState::Complete
                || demand.key.generation != self.flow.generation
                || demand.key.control_epoch != self.controls.epoch
                || demand.key.loop_entries != self.loop_entries
                || demand.key.cut_recursive_returns != self.cut_recursive_returns
                || demand.key.choices.iter().any(|(key, value)| self.index_choices.get(key) != Some(value))).then_some(id)
        }).collect();
        let mut pending: Vec<_> = invalid.iter().copied().collect();
        while let Some(id) = pending.pop() {
            for &user in &self.flow.demands[id].users {
                if invalid.insert(user) { pending.push(user); }
            }
        }
        if !invalid.is_empty() {
            let mut remap = HashMap::new();
            let mut retained = Vec::new();
            for (id, demand) in self.flow.demands.drain(..).enumerate() {
                if !invalid.contains(&id) {
                    remap.insert(id, retained.len());
                    retained.push(demand);
                }
            }
            self.flow.ids.clear();
            self.flow.bytes = 0;
            self.flow.function_bytes.clear();
            for (id, demand) in retained.iter_mut().enumerate() {
                demand.inputs = demand.inputs.iter().filter_map(|id| remap.get(id).copied()).collect();
                demand.users = demand.users.iter().filter_map(|id| remap.get(id).copied()).collect();
                let bytes = std::mem::size_of::<Demand<'tcx>>() * 2
                    + demand.key.choices.len() * std::mem::size_of::<((Point, mir::Local), u128)>() * 2
                    + demand.key.loop_entries.len() * std::mem::size_of::<Point>() * 2
                    + demand.key.inputs.len() * std::mem::size_of::<(usize, usize)>() * 2
                    + demand.key.condition_origin.as_ref().map_or(0, |key|
                        std::mem::size_of::<super::control::ConditionKey>() * 4
                            + (key.choices.len() + key.inputs.len() + key.witnesses.len() + key.loop_entries.len()) * 64)
                    + (demand.inputs.len() + demand.users.len()) * std::mem::size_of::<usize>() * 4
                    + demand.indexes.len() * std::mem::size_of::<(Point, mir::Local)>() * 4
                    + demand.values.len() * std::mem::size_of::<Alternative<'tcx>>() * 2
                    + demand.values.iter().map(|value| value.guards.len()
                        * std::mem::size_of::<(usize, Guard)>() * 2).sum::<usize>();
                self.flow.bytes += bytes;
                *self.flow.function_bytes.entry(self.frames[demand.key.point.frame].instance).or_default() += bytes;
                self.flow.ids.insert(demand.key.clone(), id);
            }
            self.flow.demands = retained;
            self.flow.loss_origins = self.flow.loss_origins.drain().filter_map(|(id, origin)|
                remap.get(&id).map(|&id| (id, origin))).collect();
            self.flow.queued.clear();
        }
    }

    pub(super) fn flow_input(&mut self, frame: usize, caller: Point) {
        let inputs = self.flow.inputs.entry(frame).or_default();
        if inputs.contains(&caller) { return; }
        let previous = inputs.len();
        inputs.push(caller);
        self.flow.callees.entry(caller.frame).or_default().push((caller.location, frame));
        self.flow.ancestors.clear();
        self.flow.ancestor_bytes = 0;
        if previous == 0 && !self.flow.roots.contains(&frame) { return; }
        let mut ancestors = HashSet::new();
        let mut successors = HashMap::<usize, Vec<usize>>::new();
        let same_component = self.flow.recursive.get(&frame)
            .is_some_and(|component| self.flow.recursive.get(&caller.frame) == Some(component));
        let mut pending = if same_component { Vec::new() } else { vec![caller.frame] };
        while let Some(parent) = pending.pop() {
            if !ancestors.insert(parent) { continue; }
            for input in self.flow.inputs.get(&parent).into_iter().flatten() {
                successors.entry(input.frame).or_default().push(parent);
                pending.push(input.frame);
            }
        }
        let mut changed = HashSet::new();
        if ancestors.contains(&frame) {
            pending.push(frame);
            let mut seen = HashSet::new();
            while let Some(member) = pending.pop() {
                if !seen.insert(member) { continue; }
                pending.extend(successors.get(&member).into_iter().flatten().copied());
            }
            let component = *seen.iter().min().expect("nonempty call cycle");
            for member in seen {
                if self.flow.recursive.insert(member, component) != Some(component) { changed.insert(member); }
            }
        }
        if previous == 0 && changed.is_empty() { return; }
        self.flow.generation += 1;
        let mut affected = Vec::new();
        for (id, demand) in self.flow.demands.iter().enumerate() {
            if demand.key.point.frame == frame && demand.key.point.location == Location::START
                || demand.cardinality
                || changed.contains(&demand.key.point.frame)
                || matches!(demand.key.address.object, Object::Local(owner, _) | Object::PriorLocal(owner, _) | Object::SummaryLocal(owner, _) if changed.contains(&owner))
                || matches!(demand.key.address.object, Object::Heap(origin) if changed.contains(&origin.frame))
            {
                affected.push(id);
            }
        }
        for id in affected { self.flow.enqueue(id); }
    }

    fn read_uncached(
        &mut self,
        address: Address,
        requested: Ty<'tcx>,
        point: Point,
        effects_only: bool,
        successor: Option<CallEdge>,
        control: bool,
    ) -> Result<Vec<Alternative<'tcx>>, QueryStop> {
        let index = self.index(point.frame)?;
        let frame = self.frames[point.frame];
        let body = index.body;
        let scalar_return = !control && !effects_only && address.object == Object::Local(point.frame, mir::RETURN_PLACE)
            && self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(requested))
                .is_ok_and(|layout| match layout.backend_repr {
                    rustc_abi::BackendRepr::Scalar(value) =>
                        !matches!(value.primitive(), rustc_abi::Primitive::Pointer(_)),
                    rustc_abi::BackendRepr::ScalarPair { a, b, .. } =>
                        !matches!(a.primitive(), rustc_abi::Primitive::Pointer(_))
                            && !matches!(b.primitive(), rustc_abi::Primitive::Pointer(_)),
                    _ => false,
                });
        if (!index.loops.is_empty() || scalar_return && self.facts.safe.contains(&frame.instance))
            && self.controls.attempted_loops.insert(point.frame)
        {
            self.budget.reserve(std::mem::size_of::<usize>() * 4)?;
        }
        if control {
            return self.control_step(point, true, false, &mut Vec::new());
        }
        if scalar_return {
            if let Some(&cause) = self.controls.loop_fallbacks.get(&(frame.instance, frame.irq)) {
                self.controls.causes.insert(point, cause);
                return Ok(vec![Alternative {
                    value: Value::Unknown("the bounded loop summary does not determine this return value"),
                    guards: vec![(point.frame, Guard { required: false, proof: GuardProof::Widened,
                        iteration: None, branch: point.location, successor: mir::START_BLOCK,
                        index: None, input: false, phi: true, choice: None })],
                }]);
            }
        }
        if let Object::Static(def) = address.object {
            if !self.tcx.is_foreign_item(def) && !self.tcx.is_mutable_static(def) {
                let ty = self.tcx.normalize_erasing_regions(
                    TypingEnv::fully_monomorphized(), self.tcx.type_of(def).instantiate_identity());
                if ty.is_freeze(self.tcx, TypingEnv::fully_monomorphized()) {
                    return if effects_only {
                        Ok(vec![Alternative { value: Value::Unchanged, guards: Vec::new() }])
                    } else {
                        match self.static_allocation(def)? {
                            Some(allocation) => self.allocation(allocation, address.offset, requested),
                            None => Ok(vec![Alternative {
                                value: Value::Unknown("the static initializer is not available"), guards: Vec::new(),
                            }]),
                        }
                    };
                }
            }
        }
        if let Object::Allocation(id) = address.object {
            if matches!(self.tcx.global_alloc(id), GlobalAlloc::Memory(memory)
                if memory.inner().mutability == rustc_hir::Mutability::Not)
            {
                return if effects_only {
                    Ok(vec![Alternative { value: Value::Unchanged, guards: Vec::new() }])
                } else { self.allocation(id, address.offset, requested) };
            }
        }
        let memory = self.memory_ssa(point.frame, address, requested)?;
        let version = memory.versions.get(&point.location).ok_or(QueryStop::Unsupported)?.clone();
        if let Version::Merge(inputs) = version {
            let choice = inputs.len() + usize::from(point.location.block == mir::START_BLOCK) > 1;
            let mut common = None;
            let mut result = if point.location.block == mir::START_BLOCK {
                self.flow_entry(address, requested, point, effects_only)?
            } else { Vec::new() };
            result = self.loop_values(point, None, result)?;
            for (location, guard, backedge) in inputs {
                if backedge && self.loop_entries.contains(&point) { continue; }
                let previous = Point { frame: point.frame, location };
                let values = self.read_edge(address, requested,
                    previous, effects_only, Some(CallEdge::Block(point.location.block)), false)?;
                if values.is_empty() { continue; }
                if common.is_none() {
                    let paths = if choice { self.paths(point, false)? } else { Vec::new() };
                    common = Some(paths.first().map_or_else(Vec::new, |path|
                        path.guards.iter().copied()
                            .filter(|guard| paths.iter().all(|path| path.guards.contains(guard))).collect::<Vec<_>>()));
                }
                let paths = if backedge && frame.irq { self.control(previous)? }
                    else if choice { self.paths(previous, false)? }
                    else { vec![Alternative { value: Value::Bits(1), guards: Vec::new() }] };
                let mut edge_values = Vec::new();
                for value in values {
                    for path in &paths {
                        let mut edge: Vec<_> = path.guards.iter().copied().filter(|guard| !common.as_ref().unwrap().contains(guard)).collect();
                        if choice && let Some(guard) = guard { edge.push((point.frame, guard)); }
                        if let Some(guards) = self.merge(&edge, &value.guards)? {
                            edge_values.push(Alternative { value: value.value, guards });
                        }
                    }
                }
                if backedge {
                    edge_values = self.feasible(edge_values)?;
                    self.forget_loop_guards(&mut edge_values, point,
                        &index.loops[&(location.block, point.location.block)])?;
                }
                result.extend(self.loop_values(point, Some(location.block), edge_values)?);
            }
            return self.feasible(result);
        }
        let Version::Definition { at: location, previous } = version else { unreachable!() };
        let at = Point { frame: point.frame, location };
        let ordinal = location.statement_index;
        let data = &body.basic_blocks[location.block];
        let bytes = self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(requested))
            .map_err(|_| QueryStop::Unsupported)?.size.bytes();
        let mut result = Vec::new();
        let mut bypasses = Vec::new();
        let mut killed = false;
        loop {
                let mut destination = None;
                let mut assignment = None;
                let mut discriminant = None;
                let mut call_result = false;
                if ordinal < data.statements.len() {
                    match &data.statements[ordinal].kind {
                        StatementKind::Assign(value) => {
                            destination = Some(value.0);
                            assignment = Some(&value.1);
                        }
                        StatementKind::SetDiscriminant {
                            place,
                            variant_index,
                        } => {
                            destination = Some(**place);
                            discriminant = Some(*variant_index);
                        }
                        StatementKind::StorageLive(local) | StatementKind::StorageDead(local)
                            if address.object == Object::Local(point.frame, *local) =>
                        {
                            if std::env::var_os("IRQ_CHECK_TRACE_UNINITIALIZED").is_some() {
                                eprintln!("irq-check: uninitialized {} frame={} at={:?} address={address:?} type={requested} statement={:?}",
                                    frame.instance, point.frame, at.location, data.statements[ordinal]);
                            }
                            result.push(Alternative {
                                value: Value::Unknown("the local storage is not initialized"),
                                guards: Vec::new(),
                            });
                            killed = true;
                            break;
                        }
                        StatementKind::Intrinsic(_) => {
                            let effects = self.statement_effect(at, address, requested)?;
                            for value in effects {
                                
                                let Some(merged) = self.merge(&[], &value.guards)? else {
                                    continue;
                                };
                                if value.value == Value::Unchanged {
                                    bypasses.push(merged);
                                } else {
                                    result.push(Alternative {
                                        value: value.value,
                                        guards: merged,
                                    });
                                }
                            }
                            killed = true;
                            break;
                        }
                        _ => {}
                    }
                } else {
                    match &data.terminator().kind {
                        TerminatorKind::Call {
                            destination: place,
                            target,
                            ..
                        } if target.map(CallEdge::Block) == successor && target.is_some() => {
                            destination = Some(*place);
                            call_result = true;
                        }
                        TerminatorKind::TailCall { .. }
                            if address.object == Object::Local(point.frame, mir::RETURN_PLACE) => {
                            destination = Some(Place::from(mir::RETURN_PLACE));
                            call_result = true;
                        }
                        TerminatorKind::InlineAsm { options, operands, .. } => {
                            let mut overlaps = !options.intersects(
                                rustc_ast::InlineAsmOptions::NOMEM | rustc_ast::InlineAsmOptions::READONLY);
                            for operand in operands {
                                let output = match operand {
                                    mir::InlineAsmOperand::Out { place, .. } => *place,
                                    mir::InlineAsmOperand::InOut { out_place, .. } => *out_place,
                                    _ => None,
                                };
                                if let Some(output) = output {
                                    let output_ty = self.ty(point.frame, output.ty(body, self.tcx).ty);
                                    let extent = self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(output_ty))
                                        .map_err(|_| QueryStop::Unsupported)?.size.bytes();
                                    for target in self.address(at, output, None, None)? {
                                        overlaps |= match target.value {
                                            Value::Pointer(target, _) => target.object == address.object
                                                && target.offset < address.offset + i128::from(bytes)
                                                && address.offset < target.offset + i128::from(extent),
                                            _ => true,
                                        };
                                    }
                                }
                            }
                            if !overlaps { break; }
                            self.record_value_loss(at, "inline assembly can change the storage binding",
                                "inline assembly can change the storage binding");
                            result.push(Alternative {
                                value: Value::Unknown(
                                    "inline assembly can change the storage binding",
                                ),
                                guards: Vec::new(),
                            });
                            killed = true;
                            break;
                        }
                        _ => {}
                    }
                }
                if let Some(destination) = destination {
                    if (destination.is_indirect()
                        || address.object == Object::Local(point.frame, destination.local))
                        && self.may_alias(at, destination, address.object)
                    {
                        let written_ty = self.ty(point.frame, destination.ty(body, self.tcx).ty);
                        let written_size = self
                            .tcx
                            .layout_of(TypingEnv::fully_monomorphized().as_query_input(written_ty))
                            .map_err(|_| QueryStop::Unsupported)?
                            .size
                            .bytes();
                        let destinations = self.address(at, destination, Some(address.object), Some((address, bytes)))?;
                        if destinations.is_empty() { killed = true; break; }
                        let mut any_write = false;
                        let mut bypassed = Vec::new();
                        for destination in destinations {
                            let Some(merged) = self.merge(&[], &destination.guards)? else {
                                continue;
                            };
                            match destination.value {
                                Value::Pointer(target, _)
                                    if target.object == address.object
                                        && address.offset
                                            < target.offset + i128::from(written_size)
                                        && target.offset < address.offset + i128::from(bytes) =>
                                {
                                    any_write = true;
                                    if !self.singleton_object(address.object)? {
                                        bypasses.push(merged.clone());
                                    }

                                    if address.offset >= target.offset
                                        && address.offset + i128::from(bytes)
                                            <= target.offset + i128::from(written_size)
                                    {
                                        let values = if let Some(value) = assignment {
                                            self.rvalue(
                                                at,
                                                value,
                                                written_ty,
                                                address.offset - target.offset,
                                                requested,
                                            )?
                                        } else if let Some(variant) = discriminant {
                                            vec![Alternative {
                                                value: if requested == written_ty
                                                    && address.offset == target.offset
                                                {
                                                    Value::Variant(variant.as_u32())
                                                } else {
                                                    Value::Unknown(
                                                        "the discriminant write can overlap the requested bytes",
                                                    )
                                                },
                                                guards: Vec::new(),
                                            }]
                                        } else if call_result {
                                            self.call_return(
                                                at,
                                                address.offset - target.offset,
                                                requested,
                                            )?
                                        } else {
                                            vec![Alternative {
                                                value: Value::Unknown(
                                                    "the storage write is not supported",
                                                ),
                                                guards: Vec::new(),
                                            }]
                                        };
                                        if std::env::var("IRQ_CHECK_TRACE_TRANSFER").is_ok_and(
                                            |filter| frame.instance.to_string().contains(&filter),
                                        ) && values
                                            .iter()
                                            .any(|value| matches!(value.value, Value::Unknown(_)))
                                        {
                                            eprintln!(
                                                "irq-check: transfer {} at {:?} assignment={assignment:?} offset={} type={requested} values={values:?}",
                                                frame.instance,
                                                at.location,
                                                address.offset - target.offset
                                            );
                                        }
                                        let paths = vec![Alternative { value: Value::Bits(1), guards: Vec::new() }];
                                        for value in values {
                                            if let Some(guards) =
                                                self.merge(&merged, &value.guards)?
                                            {
                                                for path in &paths {
                                                    if let Some(guards) =
                                                        self.merge(&guards, &path.guards)?
                                                    {
                                                        result.push(Alternative {
                                                            value: value.value,
                                                            guards,
                                                        });
                                                    }
                                                }
                                            }
                                        }
                                    } else {
                                        result.push(Alternative {
                                            value: Value::Unknown(
                                                "a partial write overlaps the pointer",
                                            ),
                                            guards: merged,
                                        });
                                    }
                                }
                                Value::ObjectPointer(object) if object != address.object => {
                                    bypassed.push(merged);
                                }
                                Value::Unknown(_) | Value::ObjectPointer(_) => {
                                    any_write = true;
                                    result.push(Alternative {
                                        value: Value::Unknown(
                                            "a write has an unknown storage alias",
                                        ),
                                        guards: merged,
                                    });
                                }
                                _ => {
                                    bypassed.push(merged);
                                }
                            }
                        }
                        if any_write {
                            bypasses.extend(bypassed);
                            killed = true;
                            break;
                        }
                    }
                }
                if ordinal == data.statements.len()
                    && matches!(
                        data.terminator().kind,
                        TerminatorKind::Call { .. }
                            | TerminatorKind::Drop { .. }
                            | TerminatorKind::TailCall { .. }
                            | TerminatorKind::Assert { .. }
                    )
                {
                    let local_unexposed = matches!(address.object, Object::Local(owner, local)
                        if owner == point.frame && !index.address_taken.contains(&local));
                    if !local_unexposed {
                        let unwind = successor == Some(CallEdge::Unwind) || match &data.terminator().kind {
                            TerminatorKind::Call { target, .. } => {
                                successor.is_some() && target.map(CallEdge::Block) != successor
                            }
                            TerminatorKind::Drop { target, .. } => {
                                successor.is_some_and(|successor| successor != CallEdge::Block(*target))
                            }
                            TerminatorKind::Assert { target, .. } => {
                                successor.is_some_and(|successor| successor != CallEdge::Block(*target))
                            }
                            _ => false,
                        };
                        for value in self.call_effect(at, address, requested, unwind)? {
                            
                            let Some(merged) = self.merge(&[], &value.guards)? else {
                                continue;
                            };
                            if value.value == Value::Unchanged {
                                bypasses.push(merged);
                            } else {
                                result.push(Alternative {
                                    value: value.value,
                                    guards: merged,
                                });
                            }
                        }
                        killed = true;
                        break;
                    }
                }
            break;
        }
        if !killed { bypasses.push(Vec::new()); }
        if !bypasses.is_empty() {
            let values = self.read_edge(address, requested,
                Point { frame: point.frame, location: previous }, effects_only, successor, false)?;
            for guards in bypasses {
                for value in &values {
                    if let Some(guards) = self.merge(&guards, &value.guards)? {
                        result.push(Alternative { value: value.value, guards });
                    }
                }
            }
        }
        self.bounded(result)
    }

    fn flow_entry(
        &mut self,
        address: Address,
        requested: Ty<'tcx>,
        point: Point,
        effects_only: bool,
    ) -> Result<Vec<Alternative<'tcx>>, QueryStop> {
        let saved_input = self.input_choices.remove(&point.frame);
        let selected = saved_input;
        let answer = (|| {
        if effects_only {
            return Ok(vec![Alternative { value: Value::Unchanged, guards: Vec::new() }]);
        }
        let frame = self.frames[point.frame];
        let index = self.index(point.frame)?;
        let body = index.body;
        let callers: Vec<_> = self.flow.inputs.get(&point.frame).into_iter().flatten().enumerate()
            .filter(|(input, _)| selected.is_none_or(|selected| selected == *input))
            .map(|(_, caller)| *caller).collect();
        let bytes = self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(requested))
            .map_err(|_| QueryStop::Unsupported)?.size.bytes();
        if let Object::Local(owner, local) = address.object {
            if owner == point.frame {
                if local.as_usize() == 0 || local.as_usize() > body.arg_count {
                    return Ok(vec![Alternative {
                        value: Value::Unknown("the local value has no definition"), guards: Vec::new(),
                    }]);
                }
                let mut result = Vec::new();
                if callers.is_empty() || self.flow.roots.contains(&point.frame) {
                    if frame.unknown_entry {
                        self.record_value_loss(point, "the caller inputs are not retained in this function summary",
                            "the shared function summary has no caller inputs");
                    }
                    result.extend(if frame.unknown_entry {
                        vec![Alternative {
                            value: Value::Unknown("the caller inputs are not retained in this function summary"),
                            guards: Vec::new(),
                        }]
                    } else if let Some(&value) = self.root_scalar_witness.get(&point.frame) && address.offset == 0
                        && matches!(requested.kind(), ty::Int(_) | ty::Uint(_) | ty::Bool)
                        && frame.instance.instantiate_mir_and_normalize_erasing_regions(
                            self.tcx, TypingEnv::fully_monomorphized(),
                            EarlyBinder::bind(self.tcx, body.local_decls[local].ty)) == requested
                    {
                        vec![Alternative { value: Value::Bits(value), guards: Vec::new() }]
                    } else if address.offset == 0 && matches!(requested.kind(), ty::Int(_) | ty::Uint(_))
                        && self.ty(point.frame, body.local_decls[local].ty) == requested
                        && bytes > 0 && bytes <= 16
                    {
                        vec![Alternative { value: Value::Symbolic(address, 0, bytes * 8), guards: Vec::new() }]
                    } else if requested.is_bool() {
                        vec![Alternative { value: Value::Bits(0), guards: Vec::new() },
                            Alternative { value: Value::Bits(1), guards: Vec::new() }]
                    } else {
                        vec![Alternative { value: Value::Unknown("the root argument has no binding"), guards: Vec::new() }]
                    });
                }
                for caller in callers {
                    let parent = self.index(caller.frame)?;
                    let mut fixed_input = false;
                    let values = match &parent.body.basic_blocks[caller.location.block]
                        .terminator()
                        .kind
                    {
                        TerminatorKind::Call { args, .. }
                        | TerminatorKind::TailCall { args, .. } => {
                            let tuple = args.last().and_then(|argument| {
                                let ty = self.ty(
                                    caller.frame,
                                    argument.node.ty(parent.body, self.tcx),
                                );
                                if let ty::Tuple(fields) = ty.kind() {
                                    (self.tcx.def_kind(frame.instance.def_id())
                                        == rustc_hir::def::DefKind::Closure
                                        && body.arg_count
                                            == args.len() - 1 + fields.len())
                                    .then_some((argument, ty, *fields))
                                } else {
                                    None
                                }
                            });
                            if let Some((argument, tuple_ty, fields)) =
                                tuple.filter(|_| local.as_usize() >= args.len())
                            {
                                let field = local.as_usize() - args.len();
                                let layout = self
                                    .tcx
                                    .layout_of(
                                        TypingEnv::fully_monomorphized()
                                            .as_query_input(tuple_ty),
                                    )
                                    .map_err(|_| QueryStop::Unsupported)?;
                                if fields.get(field).is_none() {
                                    return Err(QueryStop::MissingArgument);
                                }
                                self.operand(
                                    caller,
                                    &argument.node,
                                    address.offset
                                        + i128::from(
                                            layout.fields.offset(field).bytes(),
                                        ),
                                    requested,
                                )?
                            } else if let Some(argument) =
                                args.get(local.as_usize() - 1)
                            {
                                let mut source = argument.node.clone();
                                let mut at = caller.location;
                                let mut seen = HashSet::new();
                                loop {
                                    self.budget.charge(0)?;
                                    let place = match source {
                                        Operand::Constant(_) | Operand::RuntimeChecks(_) => { fixed_input = true; break; }
                                        Operand::Copy(place) | Operand::Move(place)
                                            if place.projection.iter().all(|element| matches!(element, ProjectionElem::Field(..))) => place,
                                        _ => break,
                                    };
                                    if !seen.insert(place.local) || parent.address_taken.contains(&place.local)
                                        || !parent.clobbers.is_empty() { break; }
                                    let Some(definitions) = parent.local_definitions.get(&place.local)
                                        .filter(|definitions| definitions.len() == 1) else { break; };
                                    let definition = definitions[0];
                                    if definition.kind != super::super::query::DefinitionKind::Assignment
                                        || !parent.body.basic_blocks.dominators().dominates(definition.location.block, at.block)
                                        || definition.location.block == at.block
                                            && definition.location.statement_index >= at.statement_index { break; }
                                    let StatementKind::Assign(assignment) = &parent.body.basic_blocks[definition.location.block]
                                        .statements[definition.location.statement_index].kind else { break; };
                                    if !assignment.0.projection.is_empty() { break; }
                                    source = match &assignment.1 {
                                        Rvalue::Use(operand, _) if place.projection.is_empty() => operand.clone(),
                                        Rvalue::Use(Operand::Copy(source) | Operand::Move(source), _) => {
                                            let projection: Vec<_> = source.projection.iter().chain(place.projection.iter()).collect();
                                            Operand::Copy(Place { local: source.local, projection: self.tcx.mk_place_elems(&projection) })
                                        }
                                        Rvalue::Cast(_, operand, _) if place.projection.is_empty() => operand.clone(),
                                        Rvalue::Aggregate(kind, operands)
                                            if matches!(&**kind, mir::AggregateKind::Tuple)
                                                || matches!(&**kind, mir::AggregateKind::Adt(def, _, _, _, None)
                                                    if self.tcx.adt_def(*def).is_struct()) => {
                                            let Some(ProjectionElem::Field(field, _)) = place.projection.first() else { break; };
                                            let Some(operand) = operands.get(*field) else { break; };
                                            if place.projection.len() == 1 { operand.clone() }
                                            else if let Operand::Copy(source) | Operand::Move(source) = operand {
                                                let projection: Vec<_> = source.projection.iter().chain(place.projection[1..].iter().copied()).collect();
                                                Operand::Copy(Place { local: source.local, projection: self.tcx.mk_place_elems(&projection) })
                                            } else { break; }
                                        }
                                        Rvalue::Ref(_, _, place) | Rvalue::RawPtr(_, place) | Rvalue::Reborrow(_, _, place)
                                            if place.projection.len() == 1 && matches!(place.projection[0], ProjectionElem::Deref)
                                            => Operand::Copy(Place::from(place.local)),
                                        Rvalue::Ref(_, mir::BorrowKind::Shared, referent)
                                            if referent.projection.is_empty()
                                                && self.fixed_local(Point { frame: caller.frame, location: definition.location }, referent.local)?
                                            => { fixed_input = true; break; }
                                        _ => break,
                                    };
                                    at = definition.location;
                                }
                                self.operand(
                                    caller,
                                    &argument.node,
                                    address.offset,
                                    requested,
                                )?
                            } else {
                                vec![Alternative {
                                    value: Value::Unknown(
                                        "the call argument is absent",
                                    ),
                                    guards: Vec::new(),
                                }]
                            }
                        }
                        TerminatorKind::Drop { place, .. }
                            if local.as_usize() == 1 && address.offset == 0 =>
                        {
                            self.address(caller, *place, None, None)?
                        }
                        _ => vec![Alternative {
                            value: Value::Unknown(
                                "the argument binding is not supported",
                            ),
                            guards: Vec::new(),
                        }],
                    };
                    result.extend(self.input_values(point.frame, caller, values, !fixed_input, None)?);
                }
                if self.flow.recursive.contains_key(&point.frame)
                    && matches!(requested.kind(), ty::Int(_) | ty::Uint(_)) && bytes > 0 && bytes <= 16
                {
                    for value in &mut result {
                        if matches!(value.value, Value::Symbolic(..)) {
                            value.value = Value::Symbolic(address, 0, bytes * 8);
                        }
                    }
                }
                return self.bounded(result);
            }
        }
        if (self.controls.solving || self.controls.proving_assertion || self.cut_recursive_returns)
            && matches!(address.object, Object::Static(_)) {
            for instance in self.discovery_roots.clone() {
                if self.preserves_storage(instance, address, requested)? || self.read_only(instance, false)? { continue; }
                self.controls.dependency_stop.get_or_insert((self.refinement_origin.unwrap_or(point), QueryStop::ControlMemory));
                return Err(QueryStop::ControlMemory);
            }
        }
        if !callers.is_empty() {
            let mut result = Vec::new();
            for caller in callers {
                if matches!(address.object, Object::PriorLocal(owner, _) | Object::SummaryLocal(owner, _) if owner == point.frame)
                    && !self.flow.recursive.get(&point.frame).is_some_and(|component|
                        self.flow.recursive.get(&caller.frame) == Some(component))
                {
                    continue;
                }
                for source in Self::boundary_addresses(address, point.frame, false) {
                    let values = self.read(source, requested, caller, false)?;
                    result.extend(self.input_values(point.frame, caller, values, true, Some(source.object))?);
                }
            }
            return self.bounded(result);
        }
        let mut values = match address.object {
            Object::Allocation(id) => self.allocation(id, address.offset, requested)?,
            Object::Static(def) if !self.tcx.is_foreign_item(def) => {
                match self.static_allocation(def)? {
                    Some(allocation) => self.allocation(allocation, address.offset, requested)?,
                    None => vec![Alternative {
                        value: Value::Unknown("the static initializer is not available"), guards: Vec::new(),
                    }],
                }
            }
            Object::Heap(origin) => self.call_effect(origin, address, requested, false)?,
            _ => vec![Alternative {
                value: Value::Unknown("the object has no established initialization"), guards: Vec::new(),
            }],
        };
        if matches!(address.object, Object::Static(_) | Object::Heap(_)) {
            for instance in self.discovery_roots.clone() {
                if self.preserves_storage(instance, address, requested)? || self.read_only(instance, false)? { continue; }
                if self.controls.solving || self.controls.proving_assertion || self.cut_recursive_returns {
                    self.controls.dependency_stop.get_or_insert((self.refinement_origin.unwrap_or(point), QueryStop::ControlMemory));
                    return Err(QueryStop::ControlMemory);
                }
                if !irq_check_rupta::util::type_util::has_instance_mir(self.tcx, instance.def) {
                    values.push(Alternative {
                        value: Value::Unknown("an initialization root has no checked memory effect"), guards: Vec::new(),
                    });
                    continue;
                }
                let source = self.frame(Frame { instance, caller: None, irq: false, unknown_entry: false });
                let source_index = self.index(source)?;
                for &block in &source_index.reachable {
                    let at = Point { frame: source, location: Location {
                        block, statement_index: source_index.body.basic_blocks[block].statements.len() + 1,
                    }};
                    values.extend(self.read(address, requested, at, true)?.into_iter()
                        .filter(|value| value.value != Value::Unchanged));
                }
            }
        }
        self.bounded(values)
        })();
        if let Some(selected) = saved_input { self.input_choices.insert(point.frame, selected); }
        answer
    }
    fn memory_ssa(&mut self, frame: usize, address: Address, requested: Ty<'tcx>) -> Result<Arc<MemorySsa>, QueryStop> {
        let local = match address.object {
            Object::Local(owner, local) if owner == frame => Some(local),
            _ => None,
        };
        let object = match address.object {
            Object::Local(owner, local) | Object::PriorLocal(owner, local) | Object::SummaryLocal(owner, local) =>
                Some(PointerObject::Local(self.frames[owner].instance, local)),
            Object::Static(def) => Some(PointerObject::Static(def)),
            _ => None,
        };
        let range = object.and_then(|_| self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(requested))
            .ok().filter(|layout| layout.is_sized()).map(|layout| (address.offset, layout.size.bytes())));
        let key = (self.frames[frame].instance, self.frames[frame].irq, local, object, range, self.cut_recursive_returns);
        if let Some(index) = self.flow.templates.get(&key) { return Ok(index.clone()); }
        let index = self.index(frame)?;
        let exposed = local.is_none_or(|local| index.address_taken.contains(&local));
        let mut definitions = vec![Vec::new(); index.body.basic_blocks.len()];
        let mut versions = HashMap::new();
        let mut excluded = 0usize;
        for &block in &index.reachable {
            self.budget.charge(0)?;
            let entries = &mut definitions[block.as_usize()];
            if let Some(local) = local {
                for definition in index.definitions.get(&(local, block)).into_iter().flatten() {
                    let mut disjoint = false;
                    if let Some((offset, size)) = range
                        && let Some(place) = definition.place
                        && place.projection.iter().all(|projection| matches!(projection, ProjectionElem::Field(..)))
                        && let Ok(layout) = self.tcx.layout_of(TypingEnv::fully_monomorphized()
                            .as_query_input(self.ty(frame, place.ty(index.body, self.tcx).ty)))
                        && layout.is_sized()
                    {
                        let targets = self.address(Point { frame, location: definition.location }, place, None, None)?;
                        disjoint = !targets.is_empty() && targets.iter().all(|value| {
                            let Value::Pointer(target, _) = value.value else { return false; };
                            target.object == address.object && offset.checked_add(i128::from(size))
                                .zip(target.offset.checked_add(i128::from(layout.size.bytes())))
                                .is_some_and(|(read_end, write_end)| read_end <= target.offset || write_end <= offset)
                        });
                    }
                    if disjoint { excluded += 1; }
                    else { entries.push(definition.location.statement_index); }
                }
            }
            if exposed {
                for definition in index.memory.get(&block).into_iter().flatten() {
                    let site = (self.frames[frame].instance, definition.location);
                    let mut preserved = false;
                    if range.is_some() && !self.cut_recursive_returns && definition.place.is_none()
                        && definition.location.statement_index == index.body.basic_blocks[block].statements.len()
                        && self.facts.covered_calls.contains(&site)
                        && let Some(targets) = self.facts.safe_candidates.get(&site).cloned().filter(|targets| !targets.is_empty())
                    {
                        preserved = true;
                        for target in targets {
                            if !self.preserves_storage(target, address, requested)? { preserved = false; break; }
                        }
                    }
                    preserved |= definition.place.is_none()
                        && definition.location.statement_index == index.body.basic_blocks[block].statements.len()
                        && matches!(index.body.basic_blocks[block].terminator().kind,
                            TerminatorKind::Assert { unwind: mir::UnwindAction::Unreachable, .. });
                    preserved |= object.is_some_and(|object| definition.place.is_some_and(|place|
                        matches!(place.projection.first(), Some(ProjectionElem::Deref))
                            && !place.projection[1..].iter().any(|projection| matches!(projection, ProjectionElem::Deref))
                            && self.facts.pointees.get(&(self.frames[frame].instance, place.local))
                                .is_some_and(|targets| !targets.contains(&object))));
                    if !preserved && range.is_some() && !self.effects.solving
                        && definition.place.is_none()
                        && definition.location.statement_index == index.body.basic_blocks[block].statements.len()
                        && let TerminatorKind::Call { func, args, unwind: mir::UnwindAction::Unreachable, .. } =
                            &index.body.basic_blocks[block].terminator().kind
                        && let ty::FnDef(def, substitutions) = *self.ty(frame, func.ty(index.body, self.tcx)).kind()
                        && let Ok(Some(callee)) = Instance::try_resolve(self.tcx, TypingEnv::fully_monomorphized(), def,
                            self.tcx.instantiate_bound_regions_with_erased(substitutions))
                        && let Some(summary) = self.effect_summary(callee, self.frames[frame].irq)?
                        && (!self.cut_recursive_returns || self.returns_normally(callee, self.frames[frame].irq)?)
                    {
                        let at = Point { frame, location: definition.location };
                        preserved = true;
                        for store in &summary.stores {
                            self.budget.charge(0)?;
                            let mut destination = &store.destination;
                            while let effects::Source::Pointer { source, .. } = destination { destination = source; }
                            let offset = match destination {
                                effects::Source::Argument(..) => Some(store.offset),
                                effects::Source::Address { offset, .. } => offset.checked_add(store.offset),
                                _ => None,
                            };
                            let Some(offset) = offset else { preserved = false; break; };
                            let Some(operand) = destination.operand(self, at, args, index.body)
                            else { preserved = false; break; };
                            let mut operand = operand.clone();
                            let mut used_at = at.location;
                            let mut seen = HashSet::new();
                            let targets = loop {
                                self.budget.charge(0)?;
                                let (Operand::Copy(pointer) | Operand::Move(pointer)) = operand
                                else { break Vec::new(); };
                                if !pointer.projection.is_empty() || index.address_taken.contains(&pointer.local)
                                    || !index.clobbers.is_empty() || !seen.insert(pointer.local)
                                { break Vec::new(); }
                                let Some(definitions) = index.local_definitions.get(&pointer.local)
                                    .filter(|definitions| definitions.len() == 1)
                                else { break Vec::new(); };
                                let origin = definitions[0];
                                if !index.body.basic_blocks.dominators().dominates(origin.location.block, used_at.block)
                                    || origin.location.block == used_at.block
                                        && origin.location.statement_index >= used_at.statement_index
                                { break Vec::new(); }
                                let Some(statement) = index.body.basic_blocks[origin.location.block].statements
                                    .get(origin.location.statement_index)
                                else { break Vec::new(); };
                                let StatementKind::Assign(assignment) = &statement.kind
                                else { break Vec::new(); };
                                let (assigned, source) = &**assignment;
                                if !assigned.projection.is_empty() { break Vec::new(); }
                                used_at = origin.location;
                                let place = match source {
                                    Rvalue::Use(source, _) if self.ty(frame, source.ty(index.body, self.tcx)).is_any_ptr() => {
                                        operand = source.clone();
                                        continue;
                                    }
                                    Rvalue::Ref(_, _, place) | Rvalue::RawPtr(_, place) => place,
                                    _ => break Vec::new(),
                                };
                                if place.projection.len() == 1 && place.is_indirect() {
                                    operand = Operand::Copy(Place::from(place.local));
                                    continue;
                                }
                                if !place.projection.iter().all(|projection| matches!(projection, ProjectionElem::Field(..)))
                                { break Vec::new(); }
                                break self.address(Point { frame, location: origin.location }, *place, None, None)?;
                            };
                            let (read_start, read_bytes) = range.unwrap();
                            if targets.is_empty() || !targets.iter().all(|value| {
                                let Value::Pointer(target, _) = value.value else { return false; };
                                if target.object != address.object { return true; }
                                target.offset.checked_add(offset).and_then(|start|
                                    start.checked_add(i128::from(store.bytes)).zip(read_start.checked_add(i128::from(read_bytes)))
                                        .map(|(end, read_end)| end <= read_start || read_end <= start)).unwrap_or(false)
                            }) { preserved = false; break; }
                        }
                    }
                    if preserved { excluded += 1; }
                    else { entries.push(definition.location.statement_index); }
                }
            }
            entries.extend(index.clobbers.get(&block).into_iter().flatten()
                .map(|definition| definition.location.statement_index));
            entries.sort_unstable();
            entries.dedup();
            let mut previous = Location { block, statement_index: 0 };
            for &statement_index in entries.iter() {
                let at = Location { block, statement_index };
                let after = Location { block, statement_index: statement_index + 1 };
                versions.insert(after, Version::Definition { at, previous });
                previous = after;
            }
        }
        if excluded != 0 && std::env::var_os("IRQ_CHECK_TRACE").is_some() {
            eprintln!("irq-check: field memory SSA {} local={local:?} range={range:?} excluded={excluded}", self.frames[frame].instance);
        }
        for &block in &index.reachable {
            self.budget.charge(0)?;
            let inputs = index.predecessors[block.as_usize()].iter().map(|&(parent, guard)| {
                let statement_index = definitions[parent.as_usize()].last().map_or(0, |index| index + 1);
                (Location { block: parent, statement_index }, guard, index.loops.contains_key(&(parent, block)))
            }).collect();
            versions.insert(Location { block, statement_index: 0 }, Version::Merge(inputs));
        }
        let mut aliases = HashMap::new();
        loop {
            let mut changed = false;
            for (&location, version) in &versions {
                if location.block == mir::START_BLOCK || aliases.contains_key(&location) { continue; }
                let Version::Merge(inputs) = version else { continue; };
                self.budget.charge(0)?;
                let mut source = None;
                let mut different = false;
                for &(mut input, _, _) in inputs {
                    let mut successor = match versions.get(&input) {
                        Some(Version::Definition { at, .. })
                            if at.statement_index == index.body.basic_blocks[at.block].statements.len() => Some(CallEdge::Block(location.block)),
                        _ => None,
                    };
                    while let Some(&(next, edge)) = aliases.get(&input) {
                        self.budget.charge(0)?;
                        input = next;
                        successor = edge;
                    }
                    if input == location { continue; }
                    if source.is_some_and(|source| source != (input, successor)) {
                        different = true;
                        break;
                    }
                    source = Some((input, successor));
                }
                if !different && let Some(source) = source {
                    aliases.insert(location, source);
                    changed = true;
                }
            }
            if !changed { break; }
        }
        for location in aliases.keys().copied().collect::<Vec<_>>() {
            let mut source = aliases[&location];
            while let Some(&next) = aliases.get(&source.0) {
                self.budget.charge(0)?;
                source = next;
            }
            aliases.insert(location, source);
        }
        let bytes = versions.capacity() * (std::mem::size_of::<Location>() + std::mem::size_of::<Version>()) * 2
            + aliases.capacity() * (std::mem::size_of::<Location>() * 4 + std::mem::size_of::<mir::BasicBlock>() * 2)
            + definitions.iter().map(|items| items.capacity() * std::mem::size_of::<usize>()).sum::<usize>()
            + versions.values().map(|version| match version {
                Version::Merge(inputs) => inputs.capacity() * std::mem::size_of::<(Location, Option<Guard>, bool)>(),
                _ => 0,
            }).sum::<usize>();
        if self.flow.template_bytes.saturating_add(bytes) > self.cache_limit / 4 {
            self.flow.templates.clear();
            self.flow.template_bytes = 0;
        }
        self.budget.charge(bytes)?;
        let result = Arc::new(MemorySsa { versions, definitions, aliases });
        if bytes <= self.cache_limit / 4 {
            self.flow.template_bytes += bytes;
            self.flow.templates.insert(key, result.clone());
        }
        Ok(result)
    }

    pub(super) fn choice_value(&mut self, value: Value<'tcx>, ty: Ty<'tcx>) -> Result<usize, QueryStop> {
        let key = (value, ty);
        if let Some(&id) = self.choice_values.get(&key) { return Ok(id); }
        let bytes = (std::mem::size_of_val(&key) + std::mem::size_of::<usize>()) * 4;
        let id = self.choice_values.len();
        if id.saturating_add(1).saturating_mul(bytes) > self.cache_limit / 16 {
            self.budget.record_limit(QueryStop::ScratchLimit, "choice cache bytes",
                id.saturating_add(1).saturating_mul(bytes), self.cache_limit / 16);
            if std::env::var_os("IRQ_CHECK_TRACE").is_some() {
                eprintln!("irq-check: choice storage limit entries={id} bytes={} limit={}", id * bytes, self.cache_limit / 16);
            }
            return Err(QueryStop::ScratchLimit);
        }
        self.budget.charge(bytes)?;
        self.choice_values.insert(key, id);
        if id < 128 && std::env::var_os("IRQ_CHECK_TRACE_SUMMARY_CHOICES").is_some() {
            eprintln!("irq-check: summary choice id={id} value={value:?} type={ty}");
        }
        Ok(id)
    }

    fn flow_values(&mut self, id: usize) -> Result<Vec<Alternative<'tcx>>, QueryStop> {
        let demand = &self.flow.demands[id];
        if self.flow.current.is_none() && demand.values.iter().any(|value| matches!(value.value, Value::Unknown(_))) {
            self.flow.diagnostic_root = Some(id);
        }
        let point = demand.key.point;
        let source = demand.key.address;
        let requested = demand.key.ty;
        let recursive = self.flow.recursive.contains_key(&point.frame);
        let scalar_choice = demand.key.refinement_origin.is_some()
            && matches!(requested.kind(), ty::Int(_) | ty::Uint(_) | ty::Bool);
        let choice = (demand.choice || point.location == Location::START)
            && (scalar_choice || !recursive)
            && !matches!(source.object, Object::SummaryLocal(..) | Object::Heap(_));
        let complete = demand.state == DemandState::Complete;
        let values = demand.values.clone();
        let choice = choice && !self.flow.singleton_read(&values, complete);
        let mut result = Vec::new();
        for mut value in values {
            let target = match value.value {
                Value::Pointer(target, _) if !recursive => Some(Value::Pointer(target, Metadata::Thin)),
                Value::Bits(_) | Value::Symbolic(..) if scalar_choice => Some(value.value),
                _ => None,
            };
            if choice && let Some(target) = target {
                let source = self.choice_value(Value::Pointer(source, Metadata::Thin), requested)?;
                let target = self.choice_value(target, requested)?;
                let guard = (point.frame, Guard {
                    required: true, iteration: None, proof: GuardProof::Pending,
                    phi: false,
                    branch: point.location, successor: mir::START_BLOCK,
                    index: None, input: false, choice: Some((source, target)),
                });
                let Some(guards) = self.merge(&value.guards, &[guard])? else { continue; };
                value.guards = guards;
            }
            result.push(value);
        }
        self.bounded(result)
    }

    pub(super) fn read_edge(
        &mut self,
        address: Address,
        requested: Ty<'tcx>,
        mut point: Point,
        effects_only: bool,
        successor: Option<CallEdge>,
        control: bool,
    ) -> Result<Vec<Alternative<'tcx>>, QueryStop> {
        if !control && let Object::Local(owner, local) = address.object
            && owner != point.frame && !self.index(owner)?.address_taken.contains(&local)
        {
            if effects_only {
                return Ok(vec![Alternative { value: Value::Unchanged, guards: Vec::new() }]);
            }
            point.location = Location::START;
        }
        let memory = if control { None } else { Some(self.memory_ssa(point.frame, address, requested)?) };
        let successor = if let Some(memory) = &memory {
            let definitions = &memory.definitions[point.location.block.as_usize()];
            let end = definitions.partition_point(|ordinal| *ordinal < point.location.statement_index);
            point.location.statement_index = end.checked_sub(1).map_or(0, |ordinal| definitions[ordinal] + 1);
            let successor = if let Some(&(source, edge)) = memory.aliases.get(&point.location) {
                point.location = source;
                edge
            } else { successor };
            match memory.versions.get(&point.location) {
                Some(Version::Definition { at, .. })
                    if at.statement_index == self.index(point.frame)?.body.basic_blocks[at.block].statements.len() => successor,
                _ => None,
            }
        } else { None };
        let scalar = matches!(requested.kind(), ty::Bool | ty::Char | ty::Int(_) | ty::Uint(_))
            || self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(requested))
                .is_ok_and(|layout| match layout.backend_repr {
                    rustc_abi::BackendRepr::Scalar(value) =>
                        !matches!(value.primitive(), rustc_abi::Primitive::Pointer(_)),
                    rustc_abi::BackendRepr::ScalarPair { a, b, .. } =>
                        !matches!(a.primitive(), rustc_abi::Primitive::Pointer(_))
                            && !matches!(b.primitive(), rustc_abi::Primitive::Pointer(_)),
                    _ => false,
                });
        let origin = (scalar && !control).then(|| {
            if self.budget.refinement_limit.is_some() { self.refinement_origin.unwrap_or(point) }
            else { point }
        });
        let ancestors = if let Some(ancestors) = self.flow.ancestors.get(&point.frame) {
            ancestors.clone()
        } else {
            let mut ancestors = HashSet::new();
            let mut pending = vec![point.frame];
            while let Some(frame) = pending.pop() {
                self.budget.charge(0)?;
                if !ancestors.insert(frame) { continue; }
                pending.extend(self.flow.inputs.get(&frame).into_iter().flatten().map(|caller| caller.frame));
            }
            let bytes = ancestors.capacity().saturating_mul(std::mem::size_of::<usize>() * 4)
                .saturating_add(std::mem::size_of::<HashSet<usize>>() * 4);
            self.budget.charge(bytes)?;
            let ancestors = Arc::new(ancestors);
            if self.flow.ancestor_bytes.saturating_add(bytes) > self.cache_limit / 16 {
                self.flow.ancestors.clear();
                self.flow.ancestor_bytes = 0;
            }
            if bytes <= self.cache_limit / 16 {
                self.flow.ancestor_bytes += bytes;
                self.flow.ancestors.insert(point.frame, ancestors.clone());
            }
            ancestors
        };
        let mut choices: Vec<_> = self.index_choices.iter().map(|(key, value)| (*key, *value)).collect();
        choices.sort_unstable_by_key(|((point, local), _)|
            (point.frame, point.location.block, point.location.statement_index, *local));
        let mut inputs: Vec<_> = self.input_choices.iter()
            .filter(|(frame, _)| ancestors.contains(frame) && !self.flow.single_input(**frame))
            .map(|(frame, input)| (*frame, *input)).collect();
        inputs.sort_unstable();
        let key = ReadKey {
            generation: self.flow.generation,
            control_epoch: self.controls.epoch,
            address, ty: requested, point, effects_only, successor,
            choices: choices.into_boxed_slice(),
            loop_entries: self.loop_entries.clone(),
            cut_recursive_returns: self.cut_recursive_returns,
            inputs: inputs.into_boxed_slice(),
            alias_object: if control || scalar { None } else { self.alias_object },
            refinement_origin: origin, condition_origin: self.condition_origin.clone(), control,
        };
        if self.flow.current.is_none() && self.flow.queued.is_empty()
            && self.flow.bytes > self.cache_limit / 2
        {
            self.flow.diagnostic_root = None;
            self.flow.diagnostic_stop = None;
            self.flow.loss_origins.clear();
            self.flow.demands.clear();
            self.flow.ids.clear();
            self.flow.queued.clear();
            self.flow.bytes = 0;
            self.flow.function_bytes.clear();
        }
        let mut existing = self.flow.ids.get(&key).copied();
        if existing.is_none() && !key.choices.is_empty() {
            let mut general = key.clone();
            general.choices = Box::new([]);
            if let Some(&candidate) = self.flow.ids.get(&general)
                && self.flow.demands[candidate].state == DemandState::Complete
            {
                let scratch = self.budget.scratch;
                let compatible = (|| {
                    let mut pending = vec![candidate];
                    let mut seen = HashSet::new();
                    while let Some(id) = pending.pop() {
                        if !seen.insert(id) { continue; }
                        self.budget.charge(std::mem::size_of::<usize>() * 8)?;
                        let demand = &self.flow.demands[id];
                        if demand.state != DemandState::Complete
                            || key.choices.iter().any(|(index, _)| demand.indexes.contains(index))
                        { return Ok(false); }
                        pending.extend(demand.inputs.iter().copied());
                    }
                    Ok::<_, QueryStop>(true)
                })();
                self.budget.scratch = scratch;
                if compatible? { existing = Some(candidate); }
            }
        }
        let id = if let Some(id) = existing { id } else {
            let id = self.flow.demands.len();
            let bytes = std::mem::size_of::<Demand<'tcx>>() * 2
                + key.choices.len() * std::mem::size_of::<((Point, mir::Local), u128)>() * 2
                + key.loop_entries.len() * std::mem::size_of::<Point>() * 2
                + key.inputs.len() * std::mem::size_of::<(usize, usize)>() * 2;
            let bytes = bytes + key.condition_origin.as_ref().map_or(0, |key|
                std::mem::size_of::<super::control::ConditionKey>() * 4
                    + (key.choices.len() + key.inputs.len() + key.witnesses.len() + key.loop_entries.len()) * 64);
            let function = self.frames[key.point.frame].instance;
            if self.flow.function_bytes.get(&function).copied().unwrap_or(0).saturating_add(bytes) > irq_check_rupta::limits::configuration().expect("validated analysis limits").function_scratch_mib * 1024 * 1024 {
                self.budget.record_limit(QueryStop::ScratchLimit, "function demand bytes",
                    self.flow.function_bytes.get(&function).copied().unwrap_or(0).saturating_add(bytes),
                    irq_check_rupta::limits::configuration().expect("validated analysis limits").function_scratch_mib * 1024 * 1024);
                if std::env::var_os("IRQ_CHECK_TRACE").is_some() {
                    eprintln!("irq-check: value-flow demand storage limit function={function} bytes={} total={} demands={}",
                        self.flow.function_bytes[&function], self.flow.bytes, self.flow.demands.len());
                    let mut states = HashMap::<_, usize>::new();
                    let mut input_sets = HashSet::new();
                    let mut index_sets = HashSet::new();
                    for demand in &self.flow.demands {
                        if self.frames[demand.key.point.frame].instance != function { continue; }
                        let mut state = demand.key.clone();
                        input_sets.insert(state.inputs.clone());
                        index_sets.insert(state.choices.clone());
                        state.inputs = Box::new([]);
                        state.choices = Box::new([]);
                        *states.entry(state).or_default() += 1;
                    }
                    eprintln!("irq-check: value-flow key dimensions states={} input-environments={} index-environments={} largest-environment-count={}",
                        states.len(), input_sets.len(), index_sets.len(), states.values().copied().max().unwrap_or(0));
                }
                return Err(QueryStop::ScratchLimit);
            }
            self.budget.charge(bytes)?;
            self.flow.bytes = self.flow.bytes.checked_add(bytes).ok_or(QueryStop::ScratchLimit)?;
            *self.flow.function_bytes.entry(function).or_default() += bytes;
            self.flow.ids.insert(key.clone(), id);
            let choice = memory.as_ref().is_some_and(|memory|
                matches!(memory.versions.get(&point.location), Some(Version::Merge(inputs)) if inputs.len() > 1));
            let loop_updates = memory.as_ref().is_some_and(|memory|
                matches!(memory.versions.get(&point.location), Some(Version::Merge(inputs))
                    if inputs.iter().any(|(_, _, backedge)| *backedge))).then_some(0);
            self.flow.demands.push(Demand {
                key,
                values: Vec::new(), users: HashSet::new(), inputs: HashSet::new(), indexes: HashSet::new(), state: DemandState::Pending,
                choice, cardinality: false, loop_updates, loop_widened: false, abstract_top: false,
            });
            self.flow.enqueue(id);
            id
        };
        if let Some(user) = self.flow.current {
            self.flow.current_inputs.insert(id);
            if !self.flow.demands[id].users.contains(&user) {
                let dependency = self.frames[self.flow.demands[id].key.point.frame].instance;
                let consumer = self.frames[self.flow.demands[user].key.point.frame].instance;
                for function in [dependency, consumer] {
                    let bytes = std::mem::size_of::<usize>() * if dependency == consumer { 8 } else { 4 };
                    if self.flow.function_bytes.get(&function).copied().unwrap_or(0).saturating_add(bytes) > irq_check_rupta::limits::configuration().expect("validated analysis limits").function_scratch_mib * 1024 * 1024 {
                        self.budget.record_limit(QueryStop::ScratchLimit, "function dependency bytes",
                            self.flow.function_bytes.get(&function).copied().unwrap_or(0).saturating_add(bytes),
                            irq_check_rupta::limits::configuration().expect("validated analysis limits").function_scratch_mib * 1024 * 1024);
                        if std::env::var_os("IRQ_CHECK_TRACE").is_some() {
                            eprintln!("irq-check: value-flow dependency storage limit function={function} bytes={} total={} demands={}",
                                self.flow.function_bytes[&function], self.flow.bytes, self.flow.demands.len());
                        }
                        return Err(QueryStop::ScratchLimit);
                    }
                }
                self.flow.demands[id].users.insert(user);
                self.flow.demands[user].inputs.insert(id);
                for function in [dependency, consumer] {
                    *self.flow.function_bytes.entry(function).or_default() += std::mem::size_of::<usize>() * 4;
                }
                self.flow.bytes = self.flow.bytes.checked_add(std::mem::size_of::<usize>() * 8)
                    .ok_or(QueryStop::ScratchLimit)?;
                self.budget.charge(std::mem::size_of::<usize>() * 8)?;
            }
            self.require(id, true)?;
            return self.flow_values(id);
        }
        let saved_choices = self.index_choices.clone();
        let saved_inputs = self.input_choices.clone();
        let saved_alias = self.alias_object;
        let saved_origin = self.refinement_origin;
        let saved_condition = self.condition_origin.clone();
        let scratch = self.budget.scratch;
        let initial_bytes = self.flow.bytes;
        let before_request = self.budget.steps;
        self.flow.active = Some(Request {
            root: id,
            mode: if self.budget.refinement_limit.is_some() { RequestMode::OptionalRefinement } else { RequestMode::RequiredBinding },
            state: RequestState::Pending,
            required: HashSet::new(), pending: VecDeque::new(), stopped_at: None, steps: 0,
        });
        let result = (|| {
            self.require(id, true)?;
            while let Some(current) = self.flow.active.as_mut().unwrap().pending.pop_front() {
                self.flow.active.as_mut().unwrap().stopped_at = Some(current);
                self.budget.charge(0)?;
                self.flow.queued.remove(&current);
                let key = self.flow.demands[current].key.clone();
                let scalar = matches!(key.ty.kind(), ty::Bool | ty::Char | ty::Int(_) | ty::Uint(_))
                    || self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(key.ty))
                        .is_ok_and(|layout| match layout.backend_repr {
                            rustc_abi::BackendRepr::Scalar(value) =>
                                !matches!(value.primitive(), rustc_abi::Primitive::Pointer(_)),
                            rustc_abi::BackendRepr::ScalarPair { a, b, .. } =>
                                !matches!(a.primitive(), rustc_abi::Primitive::Pointer(_))
                                    && !matches!(b.primitive(), rustc_abi::Primitive::Pointer(_)),
                            _ => false,
                        });
                self.index_choices = key.choices.iter().copied().collect();
                self.input_choices = key.inputs.iter().copied().collect();
                self.alias_object = key.alias_object;
                let origin = key.refinement_origin;
                self.refinement_origin = origin;
                self.condition_origin = key.condition_origin.clone();
                self.flow.current = Some(current);
                self.flow.current_inputs.clear();
                self.flow.evaluations += 1;
                let before = self.budget.steps;
                let generation = self.flow.generation;
                let mut widen = self.flow.demands[current].abstract_top || self.flow.demands[current].loop_updates.is_some_and(|updates|
                    updates >= irq_check_rupta::limits::configuration().expect("validated analysis limits").alternatives.max(1));
                let step = if self.flow.demands[current].abstract_top {
                    Ok(self.flow.demands[current].values.clone())
                } else if widen {
                    if !self.flow.demands[current].loop_widened && std::env::var_os("IRQ_CHECK_TRACE").is_some() {
                        eprintln!("irq-check: loop state widened {} at={:?} address={:?} type={} updates={:?}",
                            self.frames[key.point.frame].instance, key.point.location, key.address, key.ty,
                            self.flow.demands[current].loop_updates);
                    }
                    self.flow.demands[current].loop_widened = true;
                    Ok(vec![Alternative { value: Value::Unknown("the loop changes the requested value beyond the bounded summary"), guards: Vec::new() }])
                } else {
                    self.read_uncached(key.address, key.ty, key.point, key.effects_only, key.successor, key.control)
                };
                let step = if generation != self.flow.generation
                    && step != Err(QueryStop::ProcessLimit)
                { Err(QueryStop::ContextChanged) } else { step };
                if std::env::var_os("IRQ_CHECK_TRACE_FLOW_COST").is_some()
                    && (self.budget.steps.saturating_sub(before) >= 4096 || step.is_err())
                {
                    eprintln!("irq-check: flow cost demand={current} steps={} stop={:?} {} frame={} at={:?} address={:?} type={} control={} inputs={} values={} pending={} guards={} max-guards={}",
                        self.budget.steps.saturating_sub(before), step.as_ref().err(),
                        self.frames[key.point.frame].instance, key.point.frame, key.point.location,
                        key.address, key.ty, key.control, self.flow.demands[current].inputs.len(),
                        self.flow.demands[current].values.len(), self.flow.queued.len(),
                        self.flow.demands[current].values.iter().map(|value| value.guards.len()).sum::<usize>(),
                        self.flow.demands[current].values.iter().map(|value| value.guards.len()).max().unwrap_or(0));
                }
                self.flow.current = None;
                if step.is_err() && std::env::var_os("IRQ_CHECK_TRACE_FLOW_VALUES").is_some() {
                    let values: HashSet<_> = self.flow.demands[current].values.iter().map(|value| value.value).collect();
                    eprintln!("irq-check: stopped demand={current} key={key:?} distinct-values={values:?}");
                    if step == Err(QueryStop::AlternativeLimit) {
                        let mut choices: Vec<_> = self.choice_values.iter().collect();
                        choices.sort_unstable_by_key(|(_, id)| **id);
                        for ((value, ty), id) in choices {
                            eprintln!("irq-check: choice={id} value={value:?} type={ty}");
                        }
                    }
                }
                let mut values = match step {
                    Ok(values) => values,
                    Err(QueryStop::Unsupported | QueryStop::MissingArgument) => {
                        vec![Alternative { value: Value::Unknown("the value-flow operation is not supported"), guards: Vec::new() }]
                    }
                    Err(reason) => {
                        self.flow.enqueue(current);
                        return Err(reason);
                    }
                };
                let summarize = !widen && self.flow.demands[current].loop_updates.is_some()
                    && (self.flow.demands[current].loop_widened || self.flow.demands[current].values.iter().chain(values.iter()
                        .filter(|value| !self.flow.demands[current].values.contains(value)))
                        .map(|value| value.guards.len()).sum::<usize>()
                        > irq_check_rupta::limits::configuration().expect("validated analysis limits").alternatives);
                if summarize {
                    if self.controls.solving || self.controls.proving_assertion || self.cut_recursive_returns {
                        self.budget.record_limit(QueryStop::AlternativeLimit, "loop guard entries",
                            self.flow.demands[current].values.iter().chain(values.iter()
                                .filter(|value| !self.flow.demands[current].values.contains(value)))
                                .map(|value| value.guards.len()).sum(),
                            irq_check_rupta::limits::configuration().expect("validated analysis limits").alternatives);
                        self.controls.dependency_stop.get_or_insert((key.point, QueryStop::AlternativeLimit));
                        self.flow.enqueue(current);
                        return Err(QueryStop::AlternativeLimit);
                    }
                    let mut joined = self.flow.demands[current].values.clone();
                    for value in values {
                        if !joined.contains(&value) { joined.push(value); }
                    }
                    let mut grouped = Vec::<Alternative<'tcx>>::new();
                    let marker = (key.point.frame, Guard { required: false, proof: GuardProof::Widened,
                        iteration: None, branch: key.point.location, successor: mir::START_BLOCK,
                        index: None, input: false, phi: true, choice: None });
                    for value in &joined {
                        if grouped.iter().any(|previous| previous.value == value.value) { continue; }
                        let mut guards = value.guards.clone();
                        guards.retain(|guard| joined.iter().filter(|other| other.value == value.value)
                            .all(|other| other.guards.contains(guard)));
                        let lost = joined.iter().filter(|other| other.value == value.value)
                            .any(|other| other.guards != guards);
                        if lost && !guards.contains(&marker) { guards.push(marker); }
                        grouped.push(Alternative { value: value.value, guards });
                    }
                    for value in joined {
                        if matches!(value.value, Value::Function(_)) && value.path_proven()
                            && !grouped.contains(&value) { grouped.push(value); }
                    }
                    if !self.flow.demands[current].loop_widened && std::env::var_os("IRQ_CHECK_TRACE").is_some() {
                        eprintln!("irq-check: loop relation summarized demand={current} {} key={key:?} values={:?}",
                            self.frames[key.point.frame].instance, grouped.iter().map(|value| value.value).collect::<Vec<_>>());
                    }
                    self.flow.demands[current].loop_widened = true;
                    values = grouped;
                }
                if !widen && self.flow.inputs_complete(&self.flow.current_inputs)
                    && !values.iter().chain(&self.flow.demands[current].values)
                        .any(|value| matches!(value.value, Value::Function(_)) && value.path_proven())
                    && let Some(top) = values.iter().find(|value| matches!(value.value, Value::Unknown(_))
                        && value.guards.iter().all(|(_, guard)| guard.proof == GuardProof::Widened)).cloned()
                {
                    widen = true;
                    values = vec![top];
                }
                if !self.flow.loss_origins.contains_key(&current) && self.flow.loss_origins.len() < 128 {
                    let origin = self.flow.current_inputs.iter().take(128).filter_map(|input| {
                        let origin = *self.flow.loss_origins.get(input)?;
                        (values.iter().any(|value| value.value == Value::Unknown(origin.1))
                            && self.flow.demands[*input].values.iter().any(|value| value.value == Value::Unknown(origin.1)))
                            .then_some((*input, origin))
                    }).min_by_key(|(input, _)| *input).map(|(_, origin)| origin);
                    if let Some(origin) = origin { self.flow.loss_origins.insert(current, origin); }
                }
                if widen {
                    for value in &self.flow.demands[current].values {
                        if matches!(value.value, Value::Function(_)) && value.path_proven()
                            && !values.contains(value) { values.push(value.clone()); }
                    }
                    self.flow.demands[current].abstract_top = true;
                    self.flow.current_inputs.clear();
                }
                if std::env::var_os("IRQ_CHECK_TRACE_UNKNOWN").is_some() {
                    for value in &values {
                        if let Value::Unknown(reason) = value.value
                            && !self.flow.demands[current].values.iter().any(|previous| previous.value == value.value)
                        {
                            eprintln!("irq-check: unknown value demand={current} reason={reason} {} key={key:?} dependencies={:?} guards={:?}",
                                self.frames[key.point.frame].instance, self.flow.current_inputs, value.guards);
                        }
                    }
                }
                if self.flow.demands[current].values.is_empty() || widen {
                    let removed: Vec<_> = self.flow.demands[current].inputs.difference(&self.flow.current_inputs).copied().collect();
                    if !removed.is_empty() {
                        self.budget.charge(removed.len() * std::mem::size_of::<usize>() * 4)?;
                        let consumer = self.frames[key.point.frame].instance;
                        for &input in &removed {
                            self.flow.demands[current].inputs.remove(&input);
                            self.flow.demands[input].users.remove(&current);
                            self.flow.demands[input].users.shrink_to_fit();
                            let dependency = self.frames[self.flow.demands[input].key.point.frame].instance;
                            for function in [consumer, dependency] {
                                let bytes = self.flow.function_bytes.get_mut(&function).unwrap();
                                *bytes = bytes.saturating_sub(std::mem::size_of::<usize>() * 4);
                            }
                            self.flow.bytes = self.flow.bytes.saturating_sub(std::mem::size_of::<usize>() * 8);
                        }
                        self.flow.demands[current].inputs.shrink_to_fit();
                        self.flow.requirement_repairs += 1;
                        self.flow.requirement_repair_full += self.flow.active.as_ref().unwrap().required.len();
                        let repair_scratch = self.budget.scratch;
                        let mut affected = HashSet::new();
                        let mut pending = removed;
                        self.budget.reserve(pending.capacity() * std::mem::size_of::<usize>())?;
                        while let Some(input) = pending.pop() {
                            self.budget.charge(0)?;
                            self.flow.requirement_repair_visits += 1;
                            if input == id || input == current
                                || !self.flow.active.as_ref().unwrap().required.contains(&input)
                                || self.flow.demands[input].users.contains(&id)
                                || self.flow.demands[input].users.contains(&current)
                                || !affected.insert(input)
                            { continue; }
                            self.budget.reserve(std::mem::size_of::<usize>() * 8)?;
                            let capacity = pending.capacity();
                            pending.extend(self.flow.demands[input].inputs.iter().copied());
                            self.budget.reserve((pending.capacity() - capacity) * std::mem::size_of::<usize>())?;
                        }
                        self.flow.requirement_repair_affected += affected.len();
                        if !affected.is_empty() {
                            self.budget.reserve(affected.len() * std::mem::size_of::<usize>())?;
                            let mut anchors = Vec::with_capacity(affected.len());
                            for &input in &affected {
                                for &user in &self.flow.demands[input].users {
                                    self.budget.charge(0)?;
                                    self.flow.requirement_repair_visits += 1;
                                    if self.flow.active.as_ref().unwrap().required.contains(&user)
                                        && !affected.contains(&user)
                                    {
                                        anchors.push(input);
                                        break;
                                    }
                                }
                            }
                            anchors.sort_unstable();
                            let active = self.flow.active.as_mut().unwrap();
                            let required = active.required.len();
                            for input in &affected { active.required.remove(input); }
                            for input in anchors { self.require(input, false)?; }
                            let active = self.flow.active.as_mut().unwrap();
                            if active.required.len() != required {
                                active.pending.retain(|id| active.required.contains(id) && self.flow.queued.contains(id));
                            }
                        }
                        self.budget.scratch = repair_scratch;
                    }
                }
                let mut joined = if widen || summarize { Vec::new() } else { self.flow.demands[current].values.clone() };
                for value in values {
                    if !joined.contains(&value) { joined.push(value); }
                }
                if scalar && joined.iter().any(|value| matches!(value.value, Value::Unknown(_)) && value.guards.is_empty()) {
                    if let Some((_, reason, _)) = self.flow.loss_origins.get_mut(&current) {
                        *reason = "the scalar value is not proved";
                    }
                    joined = vec![Alternative {
                        value: Value::Unknown("the scalar value is not proved"), guards: Vec::new(),
                    }];
                }
                let joined_count = joined.len();
                let joined = match self.bounded(joined) {
                    Ok(values) => values,
                    Err(reason) => {
                        if std::env::var_os("IRQ_CHECK_TRACE").is_some() {
                            eprintln!("irq-check: value-flow stop={reason:?} alternatives={joined_count} {} {:?} address={:?} type={}",
                                self.frames[key.point.frame].instance, key.point.location, key.address, key.ty);
                            let mut values: HashMap<_, usize> = HashMap::new();
                            for value in &self.flow.demands[current].values {
                                *values.entry(value.value).or_default() += 1;
                            }
                            eprintln!("irq-check: value-flow existing-values={values:?}");
                            if std::env::var_os("IRQ_CHECK_TRACE_FLOW_VALUES").is_some() {
                                eprintln!("irq-check: limited alternatives key={key:?} values={:?}", self.flow.demands[current].values);
                            }
                        }
                        self.flow.enqueue(current);
                        return Err(reason);
                    }
                };
                if joined.len() != self.flow.demands[current].values.len()
                    || joined.iter().any(|value| !self.flow.demands[current].values.contains(value)) {
                    let old = &self.flow.demands[current].values;
                    let old_bytes = old.len() * std::mem::size_of::<Alternative<'tcx>>() * 2
                        + old.iter().map(|value| value.guards.len() * std::mem::size_of::<(usize, Guard)>() * 2).sum::<usize>();
                    let new_bytes = joined.len() * std::mem::size_of::<Alternative<'tcx>>() * 2
                        + joined.iter().map(|value| value.guards.len() * std::mem::size_of::<(usize, Guard)>() * 2).sum::<usize>();
                    let function = self.frames[key.point.frame].instance;
                    let function_bytes = self.flow.function_bytes.get(&function).copied().unwrap_or(0)
                        .saturating_sub(old_bytes).saturating_add(new_bytes);
                    if function_bytes > irq_check_rupta::limits::configuration().expect("validated analysis limits").function_scratch_mib * 1024 * 1024 {
                        self.budget.record_limit(QueryStop::ScratchLimit, "function alternative bytes", function_bytes,
                            irq_check_rupta::limits::configuration().expect("validated analysis limits").function_scratch_mib * 1024 * 1024);
                        if std::env::var_os("IRQ_CHECK_TRACE").is_some() {
                            eprintln!("irq-check: value-flow alternative storage limit function={function} bytes={function_bytes} total={} demands={}",
                                self.flow.bytes, self.flow.demands.len());
                            let mut retained: Vec<_> = self.flow.demands.iter().enumerate()
                                .filter(|(_, demand)| self.frames[demand.key.point.frame].instance == function)
                                .map(|(id, demand)| (demand.values.iter().map(|value| value.guards.len()).sum::<usize>(), id))
                                .collect();
                            retained.sort_unstable_by(|left, right| right.cmp(left));
                            for (guards, id) in retained.into_iter().take(6) {
                                let demand = &self.flow.demands[id];
                                let proof = demand.values.iter().flat_map(|value| &value.guards)
                                    .filter(|(_, guard)| guard.proof == GuardProof::Established).count();
                                let choices = demand.values.iter().flat_map(|value| &value.guards)
                                    .filter(|(_, guard)| guard.choice.is_some()).count();
                                eprintln!("irq-check: retained demand={id} alternatives={} guards={guards} established={proof} choices={choices} key={:?}",
                                    demand.values.len(), demand.key);
                            }
                        }
                        self.flow.enqueue(current);
                        return Err(QueryStop::ScratchLimit);
                    }
                    self.flow.function_bytes.insert(function, function_bytes);
                    self.flow.bytes = self.flow.bytes.saturating_sub(old_bytes).checked_add(new_bytes)
                        .ok_or(QueryStop::ScratchLimit)?;
                    let changed_value = joined.iter().any(|value| !old.iter().any(|previous| previous.value == value.value));
                    self.flow.demands[current].values = joined;
                    if changed_value && let Some(updates) = &mut self.flow.demands[current].loop_updates {
                        *updates = updates.saturating_add(1);
                    }
                    self.flow.changes += 1;
                    let mut users: Vec<_> = self.flow.demands[current].users.iter().copied().collect();
                    users.sort_unstable();
                    for user in users { self.flow.enqueue(user); }
                }
                if !self.flow.queued.contains(&current) {
                    let complete = self.flow.inputs_complete(&self.flow.demands[current].inputs);
                    self.flow.demands[current].state = if complete { DemandState::Complete } else { DemandState::Evaluated };
                    if complete {
                        let mut users: Vec<_> = self.flow.demands[current].users.iter().copied().collect();
                        users.sort_unstable();
                        for user in users { self.flow.enqueue(user); }
                    }
                }
                self.budget.scratch = scratch.saturating_add(self.flow.bytes.saturating_sub(initial_bytes));
                if self.budget.scratch > irq_check_rupta::limits::configuration().expect("validated analysis limits").query_scratch_mib * 1024 * 1024 {
                    self.budget.record_limit(QueryStop::ScratchLimit, "query worklist bytes", self.budget.scratch,
                        irq_check_rupta::limits::configuration().expect("validated analysis limits").query_scratch_mib * 1024 * 1024);
                    if std::env::var_os("IRQ_CHECK_TRACE").is_some() {
                        eprintln!("irq-check: worklist scratch limit bytes={} initial={initial_bytes} stored={}",
                            self.budget.scratch, self.flow.bytes);
                    }
                    return Err(QueryStop::ScratchLimit);
                }
            }
            let active = self.flow.active.as_mut().unwrap();
            if active.required.iter().any(|id| self.flow.queued.contains(id)
                || self.flow.demands[*id].state == DemandState::Pending)
            {
                return Err(QueryStop::Unsupported);
            }
            for &id in &active.required { self.flow.demands[id].state = DemandState::Complete; }
            active.stopped_at = None;
            Ok(())
        })();
        self.flow.current = None;
        self.flow.current_inputs.clear();
        self.index_choices = saved_choices;
        self.input_choices = saved_inputs;
        self.alias_object = saved_alias;
        self.refinement_origin = saved_origin;
        self.condition_origin = saved_condition;
        let result = result.and_then(|()| self.flow_values(id));
        if result.is_err() && let Some(current) = self.flow.active.as_ref().unwrap().stopped_at {
            self.flow.enqueue(current);
        }
        let mut active = self.flow.active.take().unwrap();
        if let Err(reason) = &result {
            let demand = &self.flow.demands[active.stopped_at.unwrap_or(active.root)];
            self.flow.diagnostic_stop = Some((demand.key.point, demand.key.address, *reason));
        }
        if let Err(reason @ (QueryStop::ControlMemory | QueryStop::RefinementLimit
            | QueryStop::ScratchLimit | QueryStop::AlternativeLimit)) = &result
            && let Some(current) = active.stopped_at
            && let Some(key) = self.flow.demands[current].key.condition_origin.clone()
            && key.generation == self.flow.generation
        {
            let bytes = std::mem::size_of::<(super::control::ConditionKey, (Point, QueryStop))>() * 4
                + (key.choices.len() + key.inputs.len() + key.witnesses.len() + key.loop_entries.len()) * 64;
            if self.controls.stopped_bytes.saturating_add(bytes) <= self.cache_limit / 16
                && !self.controls.stopped_conditions.contains_key(key.as_ref())
            {
                self.controls.stopped_bytes += bytes;
                self.controls.stopped_conditions.insert((*key).clone(),
                    self.controls.dependency_stop.unwrap_or((self.flow.demands[current].key.point, *reason)));
            }
        }
        if let Err(reason) = &result
            && self.controls.dependency_stop.is_none()
        {
            let stopped = active.stopped_at.unwrap_or(active.root);
            self.controls.dependency_stop = Some((self.flow.demands[stopped].key.point, *reason));
        }
        active.steps = self.budget.steps.saturating_sub(before_request);
        active.state = match &result { Ok(_) => RequestState::Complete, Err(reason) => RequestState::Stopped(*reason) };
        if std::env::var_os("IRQ_CHECK_TRACE_FLOW_COST").is_some() && result.is_err() {
            eprintln!("irq-check: request root={} mode={:?} state={:?} steps={} required={} pending={} stopped-at={:?}",
                active.root, active.mode, active.state, active.steps, active.required.len(), self.flow.queued.len(), active.stopped_at);
            let mut states = HashSet::new();
            let mut origins = HashSet::new();
            for &id in &active.required {
                let mut key = self.flow.demands[id].key.clone();
                origins.extend(key.refinement_origin.take());
                states.insert(key);
            }
            eprintln!("irq-check: request key dimensions without-owner={} owners={}", states.len(), origins.len());
            let mut pending = VecDeque::from([active.root]);
            let mut parents = HashMap::from([(active.root, active.root)]);
            while let Some(current) = pending.pop_front() {
                if Some(current) == active.stopped_at { break; }
                let mut inputs: Vec<_> = self.flow.demands[current].inputs.iter().copied().collect();
                inputs.sort_unstable();
                for input in inputs {
                    if let std::collections::hash_map::Entry::Vacant(entry) = parents.entry(input) {
                        entry.insert(current);
                        pending.push_back(input);
                    }
                }
            }
            let mut chain = Vec::new();
            if let Some(mut current) = active.stopped_at.filter(|id| parents.contains_key(id)) {
                loop {
                    chain.push(current);
                    if current == active.root { break; }
                    current = parents[&current];
                }
            }
            let length = chain.len();
            eprintln!("irq-check: request dependency chain length={length}");
            for (ordinal, id) in chain.into_iter().rev().enumerate() {
                if ordinal >= 6 && ordinal + 40 < length { continue; }
                let demand = &self.flow.demands[id];
                eprintln!("irq-check: request dependency demand={id} {} key={:?} values={}",
                    self.frames[demand.key.point.frame].instance, demand.key, demand.values.len());
            }
        }
        self.budget.scratch = scratch;
        result
    }

    pub(super) fn record_value_loss(&mut self, point: Point, result: &'static str, cause: &'static str) {
        if let Some(id) = self.flow.current
            && (self.flow.loss_origins.len() < 128 || self.flow.loss_origins.contains_key(&id))
        {
            self.flow.loss_origins.entry(id).or_insert((point, result, cause));
        }
    }

    pub(crate) fn explain_read(&self, stop: Option<QueryStop>) -> Vec<(rustc_span::Span, String)> {
        let mut notes = Vec::new();
        if let Some(reason) = stop {
            let usage = match reason {
                QueryStop::TransferLimit => Some(("query transfer steps", self.budget.steps, self.budget.limit)),
                _ => self.budget.limit_usage.filter(|(recorded, _, _, _, steps)| *recorded == reason && *steps == self.budget.steps)
                    .map(|(_, resource, used, limit, _)| (resource, used, limit)),
            };
            if let Some((resource, used, limit)) = usage {
                notes.push((rustc_span::DUMMY_SP, format!("analysis limit reached: {resource}: {used} used, {limit} allowed")));
            } else {
                notes.push((rustc_span::DUMMY_SP, format!("analysis stopped with {reason:?}; exact resource usage was not retained")));
            }
            let point = self.flow.diagnostic_stop.filter(|(_, _, recorded)| *recorded == reason).map(|(point, _, _)| point)
                .or_else(|| self.controls.dependency_stop.filter(|(_, recorded)| *recorded == reason).map(|(point, _)| point));
            if let Some(point) = point {
                let frame = self.frames[point.frame];
                if let Some(index) = self.indexes.get(&(frame.instance, frame.irq)) {
                    let block = &index.body.basic_blocks[point.location.block];
                    let span = block.statements.get(point.location.statement_index)
                        .map_or(block.terminator().source_info.span, |statement| statement.source_info.span);
                    notes.push((span.source_callsite(), format!("analysis stopped while resolving a dependency in {}", frame.instance)));
                }
            }
            notes.push((rustc_span::DUMMY_SP, "the stop location does not identify the source of excessive analysis work".into()));
            return notes;
        }
        let Some(root) = self.flow.diagnostic_root.filter(|id| *id < self.flow.demands.len()) else { return notes; };
        let mut pending = VecDeque::from([root]);
        let mut seen = HashSet::from([root]);
        let mut truncated = false;
        while let Some(id) = pending.pop_front() {
            let demand = &self.flow.demands[id];
            let Some(reason) = demand.values.iter().find_map(|value| match value.value {
                Value::Unknown(reason) => Some(reason), _ => None,
            }) else { continue; };
            if let Some(&(point, result, cause)) = self.flow.loss_origins.get(&id)
                && demand.values.iter().any(|value| value.value == Value::Unknown(result))
            {
                let frame = self.frames[point.frame];
                if let Some(index) = self.indexes.get(&(frame.instance, frame.irq)) {
                    let block = &index.body.basic_blocks[point.location.block];
                    let span = block.statements.get(point.location.statement_index)
                        .map_or(block.terminator().source_info.span, |statement| statement.source_info.span).source_callsite();
                    let note = (span, format!("information was lost at this operation: {cause}"));
                    if !notes.contains(&note) { notes.push(note); }
                    if notes.len() == 3 { truncated = true; break; }
                }
            }
            truncated |= demand.inputs.len() > 128;
            let mut inputs: Vec<_> = demand.inputs.iter().take(128).copied().filter(|input|
                self.flow.demands[*input].values.iter().any(|value| matches!(value.value, Value::Unknown(_)))).collect();
            inputs.sort_unstable();
            if inputs.is_empty() || id == root {
                let point = demand.key.point;
                let frame = self.frames[point.frame];
                if let Some(index) = self.indexes.get(&(frame.instance, frame.irq)) {
                    let block = &index.body.basic_blocks[point.location.block];
                    let span = block.statements.get(point.location.statement_index)
                        .map_or(block.terminator().source_info.span, |statement| statement.source_info.span).source_callsite();
                    let object = match demand.key.address.object {
                        Object::Static(def) => format!("static {}", self.tcx.def_path_str(def)),
                        Object::Local(owner, local) | Object::PriorLocal(owner, local) | Object::SummaryLocal(owner, local) =>
                            format!("local {local:?} in {}", self.frames[owner].instance),
                        Object::Heap(at) => format!("allocation in {}", self.frames[at.frame].instance),
                        Object::Allocation(_) => "constant allocation".into(),
                    };
                    let note = (span, format!("unresolved storage read recorded during this query: {reason}; {object}, byte offset {}", demand.key.address.offset)
                        .chars().take(512).collect());
                    if !notes.contains(&note) { notes.push(note); }
                    if notes.len() == 3 { truncated |= !pending.is_empty() || !inputs.is_empty(); break; }
                }
            }
            for input in inputs {
                if seen.len() >= 128 { truncated = true; break; }
                if seen.insert(input) { pending.push_back(input); }
            }
        }
        notes.push((rustc_span::DUMMY_SP, if truncated {
            "the dependency explanation is bounded; it does not identify every loss of information"
        } else {
            "these are recorded unresolved reads; the first loss of information is not established"
        }.into()));
        notes
    }
}
