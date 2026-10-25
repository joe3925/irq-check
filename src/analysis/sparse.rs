use super::query::{BodyIndex, Guard, GuardProof, QueryBudget, QueryStop, ReadOutcome, RequestMode, RequestState};
use crate::trust::Trust;
use rustc_abi::Size;
use rustc_hir::def_id::DefId;
use rustc_middle::mir::interpret::{AllocId, AllocRange, GlobalAlloc, Scalar};
use rustc_middle::mir::{
    self, ConstValue, Location, Operand, Place, ProjectionElem, Rvalue, StatementKind,
    TerminatorKind,
};
use rustc_middle::ty::{self, EarlyBinder, Instance, InstanceKind, Ty, TyCtxt, TypingEnv};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

mod flow;
mod control;
mod scalar;
mod effects;
mod impact;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct Point {
    pub frame: usize,
    pub location: Location,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum Object {
    Local(usize, mir::Local),
    PriorLocal(usize, mir::Local),
    SummaryLocal(usize, mir::Local),
    Heap(Point),
    Allocation(AllocId),
    Static(DefId),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct Address {
    pub object: Object,
    pub offset: i128,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum Metadata<'tcx> {
    Thin,
    Length(u128),
    Concrete(Ty<'tcx>),
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum Value<'tcx> {
    Bits(u128),
    Symbolic(Address, u128, u64),
    Predicate(Address, u64, u128, bool),
    Function(Instance<'tcx>),
    Pointer(Address, Metadata<'tcx>),
    ObjectPointer(Object),
    Metadata(Metadata<'tcx>),
    Variant(u32),
    Unknown(&'static str),
    Unchanged,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct Alternative<'tcx> {
    pub value: Value<'tcx>,
    pub guards: Vec<(usize, Guard)>,
}

impl Alternative<'_> {
    pub(super) fn path_proven(&self) -> bool {
        self.guards.iter().all(|(_, guard)| guard.proof != GuardProof::Widened
            && (matches!(guard.proof, GuardProof::Established | GuardProof::Predicate)
                || guard.input || guard.phi || guard.index.is_some() || guard.choice.is_some()))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct Frame<'tcx> {
    pub instance: Instance<'tcx>,
    pub caller: Option<Point>,
    pub irq: bool,
    pub unknown_entry: bool,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum FrameContext {
    Backend(Vec<usize>, Option<Point>),
    Callsite(Point),
    Root,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum CallEdge {
    Block(mir::BasicBlock),
    Unwind,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ReadKey<'tcx> {
    generation: usize,
    control_epoch: usize,
    address: Address,
    ty: Ty<'tcx>,
    point: Point,
    effects_only: bool,
    successor: Option<CallEdge>,
    choices: Box<[((Point, mir::Local), u128)]>,
    loop_entries: Box<[Point]>,
    cut_recursive_returns: bool,
    inputs: Box<[(usize, usize)]>,
    alias_object: Option<Object>,
    refinement_origin: Option<Point>,
    condition_origin: Option<Arc<control::ConditionKey>>,
    control: bool,
}

pub(super) struct Solver<'a, 'tcx> {
    pub(super) flow: flow::Flow<'tcx>,
    pub(super) controls: control::ControlCache<'tcx>,
    scalars: scalar::ScalarCache<'tcx>,
    pub(super) effects: effects::EffectCache<'tcx>,
    pub(super) impacts: impact::ImpactCache<'tcx>,
    pub tcx: TyCtxt<'tcx>,
    pub trust: &'a Trust,
    facts: &'a super::global::PointerFacts<'tcx>,
    pub frames: Vec<Frame<'tcx>>,
    frame_contexts: Vec<Option<Vec<usize>>>,
    frame_depth: Vec<usize>,
    frame_ids: HashMap<Frame<'tcx>, usize>,
    context_frames: HashMap<(Instance<'tcx>, bool, bool, FrameContext), usize>,
    pub indexes: HashMap<(Instance<'tcx>, bool), Arc<BodyIndex<'tcx>>>,
    alias_object: Option<Object>,
    refinement_origin: Option<Point>,
    condition_origin: Option<Arc<control::ConditionKey>>,
    pub cache_limit: usize,
    pub budget: QueryBudget,
    pub exported: &'a HashMap<String, Instance<'tcx>>,
    pub discovery_roots: Vec<Instance<'tcx>>,
    readonly: HashMap<(Instance<'tcx>, bool), bool>,
    pub(super) caller_locations: HashMap<rustc_span::Span, Value<'tcx>>,
    pub(super) static_allocations: HashMap<DefId, Option<AllocId>>,
    pub index_choices: HashMap<(Point, mir::Local), u128>,
    pub loop_entries: Box<[Point]>,
    pub cut_recursive_returns: bool,
    pub root_scalar_witness: HashMap<usize, u128>,
    pub input_choices: HashMap<usize, usize>,
    choice_values: HashMap<(Value<'tcx>, Ty<'tcx>), usize>,
    pub split_index: Option<(Point, mir::Local, u128)>,
}

impl<'a, 'tcx> Solver<'a, 'tcx> {
    pub fn new(
        tcx: TyCtxt<'tcx>,
        trust: &'a Trust,
        exported: &'a HashMap<String, Instance<'tcx>>,
        facts: &'a super::global::PointerFacts<'tcx>,
    ) -> Self {
        Self {
            flow: flow::Flow::default(), controls: control::ControlCache::default(), scalars: scalar::ScalarCache::default(), effects: effects::EffectCache::default(), impacts: impact::ImpactCache::default(), tcx, trust, exported, facts,
            frames: Vec::new(), frame_contexts: Vec::new(), frame_ids: HashMap::new(), context_frames: HashMap::new(),
            indexes: HashMap::new(), frame_depth: Vec::new(),
            cache_limit: 1024 * 1024 * 1024,
            alias_object: None, refinement_origin: None, condition_origin: None,
            budget: QueryBudget { steps: 0, limit: 0, scratch: 0, refinement_limit: None, limit_usage: None },
            discovery_roots: Vec::new(), readonly: HashMap::new(),
            caller_locations: HashMap::new(), static_allocations: HashMap::new(),
            index_choices: HashMap::new(), loop_entries: Box::new([]), cut_recursive_returns: false, input_choices: HashMap::new(), choice_values: HashMap::new(), split_index: None,
            root_scalar_witness: HashMap::new(),
        }
    }

    pub fn frame(&mut self, frame: Frame<'tcx>) -> usize {
        let key = frame;
        if let Some(&id) = self.frame_ids.get(&key) {
            if frame.caller.is_none() { self.flow.roots.insert(id); }
            return id;
        }
        let contexts = if let Some(caller) = frame.caller {
            self.frame_contexts[caller.frame].as_ref().and_then(|contexts| {
                let mut result = Vec::new();
                for &context in contexts {
                    let targets = self.facts.context_bindings.get(&(context, caller.location))?;
                    result.extend(targets.iter().filter(|(target, _)| *target == frame.instance).map(|(_, context)| *context));
                }
                result.sort_unstable();
                result.dedup();
                (!result.is_empty()).then_some(result)
            })
        } else {
            self.facts.instance_contexts.get(&frame.instance).cloned()
        };
        let mut ancestor = frame.caller.map(|caller| caller.frame);
        while let Some(id) = ancestor {
            let previous = self.frames[id];
            if previous.instance == frame.instance && previous.irq == frame.irq
                && previous.unknown_entry == frame.unknown_entry && self.frame_contexts[id] == contexts
                && previous.caller.map(|caller| (self.frames[caller.frame].instance, caller.location))
                    == frame.caller.map(|caller| (self.frames[caller.frame].instance, caller.location))
            {
                if let Some(caller) = frame.caller { self.flow_input(id, caller); }
                self.frame_ids.insert(key, id);
                return id;
            }
            ancestor = previous.caller.map(|caller| caller.frame);
        }
        let context = contexts.as_ref().map_or_else(|| frame.caller.map_or(FrameContext::Root, FrameContext::Callsite),
            |contexts| FrameContext::Backend(contexts.clone(), frame.caller));
        let context_key = (frame.instance, frame.irq, frame.unknown_entry, context);
        if let Some(&id) = self.context_frames.get(&context_key) {
            if let Some(caller) = frame.caller {
                self.flow_input(id, caller);
            } else { self.flow.roots.insert(id); }
            self.frame_ids.insert(frame, id);
            return id;
        }
        let id = self.frames.len();
        self.frame_contexts.push(contexts);
        self.frame_depth.push(frame.caller.map_or(1, |caller| self.frame_depth[caller.frame] + 1));
        self.frames.push(frame);
        self.frame_ids.insert(key, id);
        self.context_frames.insert(context_key, id);
        if let Some(caller) = frame.caller { self.flow_input(id, caller); }
        else { self.flow.roots.insert(id); }
        id
    }

    pub fn context_call_safe(&self, frame: usize, location: Location) -> bool {
        self.frame_contexts[frame].as_ref().is_some_and(|contexts|
            !contexts.is_empty() && contexts.iter().all(|context|
                self.facts.context_bindings.get(&(*context, location)).is_some_and(|targets|
                    !targets.is_empty() && targets.iter().all(|(_, target)| self.facts.safe_contexts.contains(target)))))
    }

    pub fn index(&mut self, frame: usize) -> Result<Arc<BodyIndex<'tcx>>, QueryStop> {
        let frame = self.frames[frame];
        if let Some(index) = self.indexes.get(&(frame.instance, frame.irq)) {
            return Ok(index.clone());
        }
        let index = Arc::new(BodyIndex::build(
            self.tcx,
            self.trust,
            frame.instance,
            frame.irq,
        )?);
        self.indexes
            .insert((frame.instance, frame.irq), index.clone());
        Ok(index)
    }

    fn object_alignment(&mut self, object: Object) -> Result<Option<u64>, QueryStop> {
        self.budget.charge(0)?;
        let ty = match object {
            Object::Local(owner, local) | Object::PriorLocal(owner, local) | Object::SummaryLocal(owner, local) => {
                let ty = self.index(owner)?.body.local_decls[local].ty;
                self.ty(owner, ty)
            }
            Object::Static(def) => self.tcx.normalize_erasing_regions(
                TypingEnv::fully_monomorphized(),
                self.tcx.type_of(def).instantiate_identity(),
            ),
            Object::Allocation(id) => return Ok(match self.tcx.global_alloc(id) {
                GlobalAlloc::Memory(memory) => Some(memory.inner().align.bytes()),
                _ => None,
            }),
            Object::Heap(origin) => {
                let index = self.index(origin.frame)?;
                let TerminatorKind::Call { destination, .. } =
                    &index.body.basic_blocks[origin.location.block].terminator().kind
                else { return Ok(None); };
                let ty = self.ty(origin.frame, destination.ty(index.body, self.tcx).ty);
                let ty::Adt(def, args) = ty.kind() else { return Ok(None); };
                if !def.is_box() { return Ok(None); }
                args.type_at(0)
            }
        };
        Ok(self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(ty))
            .ok().map(|layout| layout.align.abi.bytes()))
    }

    fn pointer_objects(&self, object: Object) -> Vec<super::global::PointerObject<'tcx>> {
        use super::global::PointerObject;
        match object {
            Object::Local(owner, local) | Object::PriorLocal(owner, local) | Object::SummaryLocal(owner, local) => {
                vec![PointerObject::Local(self.frames[owner].instance, local)]
            }
            Object::Static(def) => vec![PointerObject::Static(def)],
            Object::Heap(origin) => {
                let frame = self.frames[origin.frame];
                let body = self.tcx.instance_mir(frame.instance.def);
                if let TerminatorKind::Call { destination, .. } =
                    &body.basic_blocks[origin.location.block].terminator().kind
                    && destination.projection.is_empty()
                    && let Some(objects) = self
                        .facts
                        .pointees
                        .get(&(frame.instance, destination.local))
                {
                    objects.iter().copied().collect()
                } else {
                    Vec::new()
                }
            }
            Object::Allocation(_) => Vec::new(),
        }
    }

    fn may_alias(&self, point: Point, place: Place<'tcx>, object: Object) -> bool {
        if !matches!(place.projection.first(), Some(ProjectionElem::Deref))
            || place.projection[1..]
                .iter()
                .any(|projection| matches!(projection, ProjectionElem::Deref))
        {
            return true;
        }
        let objects = self.pointer_objects(object);
        if objects.is_empty() {
            return true;
        }
        if let Some(contexts) = &self.frame_contexts[point.frame] {
            let mut complete = !contexts.is_empty();
            for context in contexts {
                let Some(nodes) = self.facts.local_nodes.get(&(*context, place.local)) else {
                    complete = false;
                    break;
                };
                let mut covered = false;
                for &node in nodes {
                    let source = &self.facts.nodes[node];
                    if !source.projection.is_empty() || source.offset != 0
                        || !source.ty.is_some_and(|ty| ty.is_any_ptr()) { continue; }
                    covered = true;
                    if !source.complete { complete = false; break; }
                    for &target in &source.targets {
                        let target = &self.facts.nodes[target];
                        let Some(pointee) = target.object else {
                            if target.function.is_none() { return true; }
                            continue;
                        };
                        if !objects.contains(&pointee) { continue; }
                        let matching_context = match object {
                            Object::Local(owner, _) | Object::PriorLocal(owner, _) | Object::SummaryLocal(owner, _) => self.frame_contexts[owner].as_ref()
                                .is_none_or(|contexts| target.context.is_none_or(|context| contexts.contains(&context))),
                            _ => true,
                        };
                        if matching_context { return true; }
                    }
                }
                if !covered { complete = false; }
                if !complete { break; }
            }
            if complete { return false; }
        }
        self.facts
            .pointees
            .get(&(self.frames[point.frame].instance, place.local))
            .is_none_or(|points| objects.iter().any(|object| points.contains(object)))
    }

    pub fn ty(&self, frame: usize, ty: Ty<'tcx>) -> Ty<'tcx> {
        let mut value = self.frames[frame]
            .instance
            .instantiate_mir_and_normalize_erasing_regions(
                self.tcx,
                TypingEnv::fully_monomorphized(),
                EarlyBinder::bind(self.tcx, ty),
            );
        while let ty::Pat(base, _) = *value.kind() {
            value = base;
        }
        value
    }

    pub fn merge(
        &mut self,
        left: &[(usize, Guard)],
        right: &[(usize, Guard)],
    ) -> Result<Option<Vec<(usize, Guard)>>, QueryStop> {
        self.budget
            .charge((left.len() + right.len()) * std::mem::size_of::<(usize, Guard)>())?;
        let scratch = self.budget.scratch;
        let answer = (|| {
        self.budget.reserve((left.len() + right.len())
            * std::mem::size_of::<((usize, Location, Option<mir::Local>, bool, bool, bool, Option<usize>), usize)>() * 4)?;
        let mut positions: HashMap<_, usize> = HashMap::new();
        let mut result: Vec<(usize, Guard)> = Vec::new();
        for &(frame, guard) in left.iter().chain(right) {
            if guard.proof == GuardProof::Widened {
                if !result.contains(&(frame, guard)) { result.push((frame, guard)); }
                continue;
            }
            if guard.input && self.flow.single_input(frame) {
                if guard.successor.as_usize() != 0 { return Ok(None); }
                continue;
            }
            if guard.input && let Some(&input) = self.input_choices.get(&frame) {
                if guard.successor.as_usize() != input { return Ok(None); }
                continue;
            }
            let key = (frame, guard.branch, guard.index, guard.input, guard.phi,
                guard.iteration.is_some(), guard.choice.map(|choice| choice.0));
            if let Some(&ordinal) = positions.get(&key) {
                let previous = &mut result[ordinal].1;
                if previous.successor != guard.successor || previous.choice != guard.choice || previous.iteration != guard.iteration {
                    return Ok(None);
                }
                previous.required |= guard.required;
                if guard.proof == GuardProof::Established
                    || guard.proof == GuardProof::Predicate && matches!(previous.proof, GuardProof::Pending | GuardProof::Unresolved)
                    || guard.proof == GuardProof::Unresolved && previous.proof == GuardProof::Pending
                { previous.proof = guard.proof; }
            } else {
                positions.insert(key, result.len());
                result.push((frame, guard));
            }
        }
        let iterations: HashSet<_> = result.iter().filter_map(|(frame, guard)|
            guard.iteration.map(|_| (*frame, guard.branch))).collect();
        result.retain(|(frame, guard)| !(guard.proof == GuardProof::Widened && guard.phi
            && iterations.contains(&(*frame, guard.branch))));
        result.sort_unstable_by_key(|(frame, guard)| {
            (
                *frame,
                guard.branch.block,
                guard.branch.statement_index,
                guard.index,
                guard.input,
                guard.phi,
                guard.iteration,
                guard.choice,
            )
        });
        Ok(Some(result))
        })();
        self.budget.scratch = scratch;
        answer
    }

    fn bounded(
        &mut self,
        values: Vec<Alternative<'tcx>>,
    ) -> Result<Vec<Alternative<'tcx>>, QueryStop> {
        let scratch = self.budget.scratch;
        let stored = self.flow.bytes;
        let answer = (|| {
        let limit = irq_check_rupta::limits::configuration().expect("validated analysis limits").alternatives;
        self.budget
            .charge(values.capacity() * std::mem::size_of::<Alternative<'tcx>>())?;
        let mut seen = HashMap::new();
        let mut result: Vec<Alternative<'tcx>> = Vec::new();
        for mut value in values {
            if value.guards.iter().any(|(frame, guard)| guard.input
                && (self.input_choices.contains_key(frame) || self.flow.recursive.contains_key(frame))) {
                let Some(guards) = self.merge(&value.guards, &[])? else { continue; };
                value.guards = guards;
            }
            let mut identity = value.clone();
            for (_, guard) in &mut identity.guards {
                if guard.proof != GuardProof::Widened { guard.proof = GuardProof::Pending; }
            }
            if let Some(&ordinal) = seen.get(&identity) {
                let previous: &mut Alternative<'tcx> = &mut result[ordinal];
                for ((_, previous), (_, next)) in previous.guards.iter_mut().zip(&value.guards) {
                    if next.proof == GuardProof::Established
                        || next.proof == GuardProof::Predicate && matches!(previous.proof, GuardProof::Pending | GuardProof::Unresolved)
                        || next.proof == GuardProof::Unresolved && previous.proof == GuardProof::Pending
                    { previous.proof = next.proof; }
                }
            } else {
                seen.insert(identity, result.len());
                result.push(value);
            }
        }
        drop(seen);
        loop {
            self.budget.scratch = scratch.saturating_add(self.flow.bytes.saturating_sub(stored));
            self.budget.reserve(result.capacity() * std::mem::size_of::<Alternative<'tcx>>() * 4
                + result.iter().map(|value| value.guards.capacity() * std::mem::size_of::<(usize, Guard)>() * 4).sum::<usize>())?;
            let distinct: HashSet<_> = result.iter().map(|value| value.value).collect();
            if distinct.len() == result.len() && !result.iter().any(|value| matches!(value.value, Value::Unknown(_))) {
                if result.len() > limit {
                    self.budget.record_limit(QueryStop::AlternativeLimit, "value alternatives", result.len(), limit);
                    if std::env::var_os("IRQ_CHECK_TRACE_FLOW_VALUES").is_some() {
                        eprintln!("irq-check: alternative limit values={result:?}");
                    }
                    return Err(QueryStop::AlternativeLimit);
                }
                break;
            }
            let mut remove = HashSet::new();
            let guard_key = |&(frame, guard): &(usize, Guard)| (
                frame, guard.branch.block, guard.branch.statement_index, guard.successor,
                guard.index, guard.input, guard.phi, guard.iteration, guard.choice,
            );
            self.budget.reserve(result.len() * (std::mem::size_of::<Option<Vec<(usize, Guard)>>>() + 1) * 2
                + result.iter().filter(|value| value.guards.len() >= 16)
                    .map(|value| value.guards.len() * std::mem::size_of::<(usize, Guard)>() * 2).sum::<usize>())?;
            let indexed_guards: Vec<_> = result.iter().map(|value| {
                if value.guards.len() < 16 { return None; }
                let mut guards = value.guards.clone();
                guards.sort_unstable_by_key(|entry| (guard_key(entry), entry.1.proof as u8, entry.1.required));
                Some(guards)
            }).collect();
            let proven: Vec<_> = result.iter().map(Alternative::path_proven).collect();
            let contains_guard = |ordinal: usize, needle: (usize, Guard), relaxed: bool| {
                if let Some(guards) = &indexed_guards[ordinal] {
                    if relaxed {
                        guards.binary_search_by_key(&guard_key(&needle), guard_key).is_ok()
                    } else {
                        guards.binary_search_by_key(&(guard_key(&needle), needle.1.proof as u8, needle.1.required),
                            |entry| (guard_key(entry), entry.1.proof as u8, entry.1.required)).is_ok()
                    }
                } else if relaxed {
                    result[ordinal].guards.iter().any(|&(owner, other)| owner == needle.0
                        && Guard { proof: needle.1.proof, required: needle.1.required, ..other } == needle.1)
                } else {
                    result[ordinal].guards.contains(&needle)
                }
            };
            for (index, value) in result.iter().enumerate() {
                self.budget.charge(0)?;
                if result.iter().enumerate().any(|(other, cover)| {
                    other != index
                        && !remove.contains(&other)
                        && (cover.value == value.value || matches!(cover.value, Value::Unknown(_)))
                        && !(proven[index] && !proven[other])
                        && !(matches!(value.value, Value::Function(_)) && proven[index]
                            && (matches!(cover.value, Value::Unknown(_)) || !proven[other]))
                        && (other < index || !((cover.value == value.value
                            || matches!((cover.value, value.value), (Value::Unknown(_), Value::Unknown(_))))
                            && value.guards.iter().all(|&(frame, guard)| guard.proof == GuardProof::Widened
                                || contains_guard(other, (frame, guard), matches!(value.value, Value::Unknown(_))))))
                        && cover
                            .guards
                            .iter()
                            .all(|&(frame, guard)| guard.proof == GuardProof::Widened
                                || contains_guard(index, (frame, guard), matches!(cover.value, Value::Unknown(_))))
                }) {
                    remove.insert(index);
                }
            }
            if !remove.is_empty() {
                result = result
                    .into_iter()
                    .enumerate()
                    .filter_map(|(index, value)| (!remove.contains(&index)).then_some(value))
                    .collect();
                continue;
            }
            if distinct.len() == result.len() {
                if result.len() > limit {
                    self.budget.record_limit(QueryStop::AlternativeLimit, "value alternatives", result.len(), limit);
                    return Err(QueryStop::AlternativeLimit);
                }
                break;
            }
            let branches: HashSet<_> = result
                .iter()
                .flat_map(|value| value.guards.iter())
                .filter(|(_, guard)| guard.proof != GuardProof::Widened && guard.index.is_none() && !guard.input && !guard.phi && guard.choice.is_none())
                .map(|(frame, guard)| (*frame, guard.branch))
                .collect();
            let mut reduced = false;
            let grouping_scratch = self.budget.scratch;
            for (frame, branch) in branches {
                self.budget.scratch = grouping_scratch.saturating_add(self.flow.bytes.saturating_sub(stored));
                let index = self.index(frame)?;
                if branch.statement_index != index.body.basic_blocks[branch.block].statements.len() { continue; }
                let TerminatorKind::SwitchInt { targets, .. } =
                    &index.body.basic_blocks[branch.block].terminator().kind
                else {
                    continue;
                };
                let expected: HashSet<_> = targets.all_targets().iter().copied().collect();
                let mut groups: HashMap<_, (HashSet<mir::BasicBlock>, Vec<usize>)> = HashMap::new();
                for (ordinal, value) in result.iter().enumerate() {
                    self.budget
                        .charge(value.guards.len() * std::mem::size_of::<(usize, Guard)>())?;
                    if let Some((_, guard)) = value.guards.iter().find(|(owner, guard)| {
                        guard.proof != GuardProof::Widened && *owner == frame && guard.branch == branch && guard.index.is_none() && !guard.input && !guard.phi && guard.choice.is_none()
                    }) {
                        let guards: Vec<_> = value
                            .guards
                            .iter()
                            .filter(|(owner, other)| {
                                !(other.proof != GuardProof::Widened && *owner == frame
                                    && other.branch == branch
                                    && other.index.is_none()
                                    && !other.input
                                    && !other.phi
                                    && other.choice.is_none())
                            })
                            .copied()
                            .collect();
                        let group = groups.entry((value.value, guards)).or_default();
                        group.0.insert(guard.successor);
                        group.1.push(ordinal);
                    }
                }
                if let Some(((value, guards), (_, ordinals))) = groups
                    .into_iter()
                    .find(|(_, (actual, _))| expected.is_subset(actual))
                {
                    let removed: HashSet<_> = ordinals.into_iter().collect();
                    result = result
                        .into_iter()
                        .enumerate()
                        .filter_map(|(index, value)| (!removed.contains(&index)).then_some(value))
                        .collect();
                    result.push(Alternative { value, guards });
                    reduced = true;
                    break;
                }
            }
            if !reduced {
                if result.len() > limit {
                    self.budget.record_limit(QueryStop::AlternativeLimit, "value and guard alternatives", result.len(), limit);
                    if std::env::var_os("IRQ_CHECK_TRACE_FLOW_VALUES").is_some() {
                        eprintln!("irq-check: alternative limit values={result:?}");
                    }
                    return Err(QueryStop::AlternativeLimit);
                }
                break;
            }
        }
        Ok(result)
        })();
        self.budget.scratch = scratch.saturating_add(self.flow.bytes.saturating_sub(stored));
        if let Ok(values) = &answer {
            self.budget.reserve(values.capacity() * std::mem::size_of::<Alternative<'tcx>>() * 4
                + values.iter().map(|value| value.guards.capacity() * std::mem::size_of::<(usize, Guard)>() * 4).sum::<usize>())?;
        }
        answer
    }

    pub fn operand(
        &mut self,
        point: Point,
        operand: &Operand<'tcx>,
        offset: i128,
        requested: Ty<'tcx>,
    ) -> Result<Vec<Alternative<'tcx>>, QueryStop> {
        self.budget.charge(0)?;
        let frame = self.frames[point.frame];
        let body = self.index(point.frame)?.body;
        let operand_ty = self.ty(point.frame, operand.ty(body, self.tcx));
        if let ty::FnDef(def, args) = *operand_ty.kind() {
            let args = self.tcx.instantiate_bound_regions_with_erased(args);
            let target =
                Instance::try_resolve(self.tcx, TypingEnv::fully_monomorphized(), def, args)
                    .ok()
                    .flatten();
            return Ok(vec![Alternative {
                value: target.map_or(
                    Value::Unknown("the function instance is not available"),
                    Value::Function,
                ),
                guards: Vec::new(),
            }]);
        }
        match operand {
            Operand::RuntimeChecks(check) if offset == 0 => Ok(vec![Alternative {
                value: Value::Bits(check.value(self.tcx.sess) as u128),
                guards: Vec::new(),
            }]),
            Operand::Copy(place) | Operand::Move(place) => {
                let alias_object = self.alias_object;
                self.alias_object = None;
                let addresses = self.address(point, *place, None, None);
                self.alias_object = alias_object;
                let addresses = addresses?;
                let mut values = Vec::new();
                for address in addresses {
                    let Value::Pointer(mut address_value, _) = address.value else {
                        self.record_value_loss(point, "the storage address is unknown", match address.value {
                            Value::Unknown(reason) => reason,
                            _ => "the address calculation did not produce an established pointer",
                        });
                        if std::env::var_os("IRQ_CHECK_TRACE_UNINITIALIZED").is_some() {
                            eprintln!("irq-check: unknown-address {} frame={} at={:?} place={place:?} type={requested} value={:?}",
                                frame.instance, point.frame, point.location, address.value);
                        }
                        values.push(Alternative {
                            value: Value::Unknown("the storage address is unknown"),
                            guards: address.guards,
                        });
                        continue;
                    };
                    let Some(next) = address_value.offset.checked_add(offset) else {
                        return Err(QueryStop::Unsupported);
                    };
                    address_value.offset = next;
                    for value in self.read_guarded(address_value, requested, point, &address.guards)? {
                        if let Some(guards) = self.merge(&address.guards, &value.guards)? {
                            values.push(Alternative {
                                value: value.value,
                                guards,
                            });
                        }
                    }
                }
                self.bounded(values)
            }
            Operand::Constant(constant) => {
                let constant = frame
                    .instance
                    .instantiate_mir_and_normalize_erasing_regions(
                        self.tcx,
                        TypingEnv::fully_monomorphized(),
                        EarlyBinder::bind(self.tcx, constant.const_),
                    );
                let value =
                    match constant.eval(self.tcx, TypingEnv::fully_monomorphized(), body.span) {
                        Ok(ConstValue::Scalar(Scalar::Int(bits))) if offset == 0 => {
                            Value::Bits(bits.to_bits(bits.size()))
                        }
                        Ok(ConstValue::Scalar(Scalar::Ptr(pointer, _))) if offset == 0 => self
                            .constant_pointer(
                                pointer.provenance.alloc_id(),
                                i128::from(pointer.prov_and_relative_offset().1.bytes()),
                            ),
                        Ok(ConstValue::Indirect {
                            alloc_id,
                            offset: base,
                        }) => {
                            return self.allocation(
                                alloc_id,
                                i128::from(base.bytes()) + offset,
                                requested,
                            );
                        }
                        Ok(ConstValue::Slice { alloc_id, meta }) if offset == 0 => Value::Pointer(
                            Address {
                                object: Object::Allocation(alloc_id),
                                offset: 0,
                            },
                            Metadata::Length(meta as u128),
                        ),
                        Ok(ConstValue::Slice { meta, .. })
                            if offset
                                == i128::from(self.tcx.data_layout.pointer_size().bytes()) =>
                        {
                            Value::Bits(meta as u128)
                        }
                        _ => Value::Unknown("the constant value is not available"),
                    };
                Ok(vec![Alternative {
                    value,
                    guards: Vec::new(),
                }])
            }
            _ => Ok(vec![Alternative {
                value: Value::Unknown("the operand is not supported"),
                guards: Vec::new(),
            }]),
        }
    }

    fn constant_pointer(&self, allocation: AllocId, offset: i128) -> Value<'tcx> {
        match self.tcx.global_alloc(allocation) {
            GlobalAlloc::Function { instance } if offset == 0 => Value::Function(instance),
            GlobalAlloc::VTable(concrete, _) if offset == 0 => {
                Value::Metadata(Metadata::Concrete(concrete))
            }
            GlobalAlloc::Static(def) => Value::Pointer(
                Address {
                    object: Object::Static(def),
                    offset,
                },
                Metadata::Thin,
            ),
            GlobalAlloc::Memory(_) => Value::Pointer(
                Address {
                    object: Object::Allocation(allocation),
                    offset,
                },
                Metadata::Thin,
            ),
            _ => Value::Unknown("the constant pointer has an unsupported allocation"),
        }
    }

    fn static_allocation(&mut self, def: DefId) -> Result<Option<AllocId>, QueryStop> {
        self.budget.charge(0)?;
        if let Some(&allocation) = self.static_allocations.get(&def) { return Ok(allocation); }
        let allocation = self.tcx.eval_static_initializer(def).ok()
            .map(|memory| self.tcx.reserve_and_set_memory_alloc(memory));
        self.static_allocations.insert(def, allocation);
        Ok(allocation)
    }

    pub fn address(
        &mut self,
        point: Point,
        place: Place<'tcx>,
        filter: Option<Object>,
        range: Option<(Address, u64)>,
    ) -> Result<Vec<Alternative<'tcx>>, QueryStop> {
        let alias_object = self.alias_object;
        self.alias_object = filter.or(alias_object);
        let answer = (|| {
            let body = self.index(point.frame)?.body;
            let mut values = vec![Alternative {
                value: Value::Pointer(
                    Address {
                        object: Object::Local(point.frame, place.local),
                        offset: 0,
                    },
                    Metadata::Thin,
                ),
                guards: Vec::new(),
            }];
            let mut variant = None;
            for (position, projection) in place.projection.iter().enumerate() {
                self.budget.charge(0)?;
                if let Some(object) = filter {
                    if !place.projection[position..].iter().any(|projection| matches!(projection, ProjectionElem::Deref))
                    && place.projection[position..].iter().any(|projection| matches!(projection, ProjectionElem::Index(_)))
                    && values.iter().all(|value| matches!(value.value, Value::Pointer(address, _) if address.object != object)
                        || matches!(value.value, Value::ObjectPointer(origin) if origin != object)) {
                    return Ok(values.into_iter().map(|mut value| {
                        if let Value::Pointer(address, _) = value.value { value.value = Value::ObjectPointer(address.object); }
                        value
                    }).collect());
                }
                }
                let prefix = Place {
                    local: place.local,
                    projection: self.tcx.mk_place_elems(&place.projection[..position]),
                };
                let source_ty = self.ty(point.frame, prefix.ty(body, self.tcx).ty);
                let mut next = Vec::new();
                match projection {
                    ProjectionElem::Deref => {
                        for value in values {
                            let Value::Pointer(address, _) = value.value else {
                                next.push(Alternative {
                                    value: Value::Unknown("the dereference offset is not known"),
                                    guards: value.guards,
                                });
                                continue;
                            };
                            let read_ty = if filter.is_some()
                                && source_ty.builtin_deref(true).is_some_and(|pointee| {
                                    matches!(pointee.kind(), ty::Dynamic(..))
                                }) {
                                Ty::new_imm_ptr(self.tcx, self.tcx.types.unit)
                            } else {
                                source_ty
                            };
                            for pointer in self.read_guarded(address, read_ty, point, &value.guards)? {
                                if let Some(guards) = self.merge(&value.guards, &pointer.guards)? {
                                    let value = match pointer.value {
                                        Value::Pointer(..)
                                        | Value::ObjectPointer(_)
                                        | Value::Unknown(_) => pointer.value,
                                        _ => Value::Unknown("a dereference has no storage origin"),
                                    };
                                    next.push(Alternative { value, guards });
                                }
                            }
                        }
                        variant = None;
                    }
                    ProjectionElem::Field(field, _) => {
                        let layout = self
                            .tcx
                            .layout_of(TypingEnv::fully_monomorphized().as_query_input(source_ty))
                            .map_err(|_| QueryStop::Unsupported)?;
                        let layout = variant.map_or(layout, |variant| {
                            layout.for_variant(
                                &ty::layout::LayoutCx::new(
                                    self.tcx,
                                    TypingEnv::fully_monomorphized(),
                                ),
                                variant,
                            )
                        });
                        let offset = i128::from(layout.fields.offset(field.as_usize()).bytes());
                        for mut value in values {
                            if let Value::Pointer(ref mut address, ref mut metadata) = value.value {
                                address.offset = address
                                    .offset
                                    .checked_add(offset)
                                    .ok_or(QueryStop::Unsupported)?;
                                *metadata = Metadata::Thin;
                            }
                            next.push(value);
                        }
                        variant = None;
                    }
                    ProjectionElem::Downcast(_, selected) => {
                        variant = Some(selected);
                        next = values;
                    }
                    ProjectionElem::Index(local) => {
                        if let Some((requested, bytes)) = range
                            && matches!(source_ty.kind(), ty::Array(..))
                            && !place.projection[position + 1..].iter().any(|element| matches!(element, ProjectionElem::Deref))
                            && let Ok(layout) = self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(source_ty))
                            && !values.is_empty() && values.iter().all(|value| match value.value {
                                Value::Pointer(target, _) if target.object != requested.object => true,
                                Value::Pointer(target, _) => target.offset.checked_add(i128::from(layout.size.bytes()))
                                    .is_some_and(|end| end <= requested.offset)
                                    || requested.offset.checked_add(i128::from(bytes)).is_some_and(|end| end <= target.offset),
                                Value::ObjectPointer(object) => object != requested.object,
                                _ => false,
                            })
                        {
                            return Ok(values.into_iter().map(|value| Alternative {
                                value: Value::Unchanged, guards: value.guards,
                            }).collect());
                        }
                        let index_ty = self.ty(point.frame, body.local_decls[local].ty);
                        let indices =
                            self.operand(point, &Operand::Copy(Place::from(local)), 0, index_ty)?;
                        let element = match source_ty.kind() {
                            ty::Array(element, _) | ty::Slice(element) => *element,
                            _ => return Err(QueryStop::Unsupported),
                        };
                        let size = self
                            .tcx
                            .layout_of(TypingEnv::fully_monomorphized().as_query_input(element))
                            .map_err(|_| QueryStop::Unsupported)?
                            .size
                            .bytes();
                        let combinations = values.len().saturating_mul(indices.len());
                        for value in values {
                            for index in &indices {
                                let Some(guards) = self.merge(&value.guards, &index.guards)? else {
                                    continue;
                                };
                                let mut choices = vec![index.value];
                                if matches!(
                                    index.value,
                                    Value::Unknown(_) | Value::Symbolic(..) | Value::Predicate(..)
                                ) {
                                    let length = match source_ty.kind() {
                                        ty::Array(_, length) => {
                                            length.try_to_target_usize(self.tcx).map(u128::from)
                                        }
                                        _ => match value.value {
                                            Value::Pointer(_, Metadata::Length(length)) => {
                                                Some(length)
                                            }
                                            _ => None,
                                        },
                                    };
                                    if let Some(length) = length.filter(|length| *length > 0) {
                                        if let Some(choice) =
                                            self.index_choice((point, local))?
                                        {
                                            choices = vec![Value::Bits(choice)];
                                        } else if length == 1 {
                                            choices = vec![Value::Bits(0)];
                                        } else if self.flow.current.is_some() && length <= 32
                                            && combinations.saturating_mul(length as usize) <= 32 {
                                            choices = (0..length).map(Value::Bits).collect();
                                        } else {
                                            if (self.controls.solving || self.controls.proving_assertion || self.cut_recursive_returns)
                                                && length > irq_check_rupta::limits::configuration().expect("validated analysis limits").alternatives as u128
                                            {
                                                self.controls.dependency_stop.get_or_insert((point, QueryStop::AlternativeLimit));
                                                return Err(QueryStop::AlternativeLimit);
                                            }
                                            self.split_index = Some((point, local, length));
                                            return Err(QueryStop::SplitIndex);
                                        }
                                    }
                                }
                                for choice in choices {
                                    let guards = if let Value::Bits(index) = choice {
                                        let Ok(index) = usize::try_from(index) else {
                                            return Err(QueryStop::Unsupported);
                                        };
                                        if index >= u32::MAX as usize {
                                            return Err(QueryStop::Unsupported);
                                        }
                                        let Some(guards) = self.merge(
                                            &guards,
                                            &[(
                                                point.frame,
                                                Guard {
                                                    required: true, iteration: None, proof: GuardProof::Pending,
                                                    phi: false,
                                                    branch: point.location,
                                                    successor: mir::BasicBlock::from_usize(index),
                                                    index: Some(local),
                                                    input: false,
                                                    choice: None,
                                                },
                                            )],
                                        )?
                                        else {
                                            continue;
                                        };
                                        guards
                                    } else {
                                        guards.clone()
                                    };
                                    let result = match (value.value, choice) {
                                        (Value::Pointer(mut address, _), Value::Bits(index)) => {
                                            let offset =
                                                i128::try_from(index).ok().and_then(|index| {
                                                    index.checked_mul(i128::from(size))
                                                });
                                            if let Some(offset) = offset.and_then(|offset| {
                                                address.offset.checked_add(offset)
                                            }) {
                                                address.offset = offset;
                                                Value::Pointer(address, Metadata::Thin)
                                            } else {
                                                Value::Unknown(
                                                    "the array index is outside the pointer range",
                                                )
                                            }
                                        }
                                        _ => Value::Unknown("the array index is not known"),
                                    };
                                    next.push(Alternative {
                                        value: result,
                                        guards,
                                    });
                                }
                            }
                        }
                    }
                    ProjectionElem::ConstantIndex {
                        offset, from_end, ..
                    } => {
                        let element = match source_ty.kind() {
                            ty::Array(element, _) | ty::Slice(element) => *element,
                            _ => return Err(QueryStop::Unsupported),
                        };
                        let size = self
                            .tcx
                            .layout_of(TypingEnv::fully_monomorphized().as_query_input(element))
                            .map_err(|_| QueryStop::Unsupported)?
                            .size
                            .bytes();
                        for mut value in values {
                            if let Value::Pointer(mut address, metadata) = value.value {
                                let length = match source_ty.kind() {
                                    ty::Array(_, length) => {
                                        length.try_to_target_usize(self.tcx).map(u128::from)
                                    }
                                    _ => match metadata {
                                        Metadata::Length(length) => Some(length),
                                        _ => None,
                                    },
                                };
                                let index = if from_end {
                                    length.and_then(|length| length.checked_sub(u128::from(offset)))
                                } else {
                                    Some(u128::from(offset))
                                };
                                value.value = match index
                                    .and_then(|index| i128::try_from(index).ok())
                                    .and_then(|index| index.checked_mul(i128::from(size)))
                                    .and_then(|offset| address.offset.checked_add(offset))
                                {
                                    Some(offset) => {
                                        address.offset = offset;
                                        Value::Pointer(address, Metadata::Thin)
                                    }
                                    None => Value::Unknown("the array length is not known"),
                                };
                            }
                            next.push(value);
                        }
                    }
                    ProjectionElem::Subslice { from, to, from_end } => {
                        let element = match source_ty.kind() {
                            ty::Array(element, _) | ty::Slice(element) => *element,
                            _ => return Err(QueryStop::Unsupported),
                        };
                        let size = self
                            .tcx
                            .layout_of(TypingEnv::fully_monomorphized().as_query_input(element))
                            .map_err(|_| QueryStop::Unsupported)?
                            .size
                            .bytes();
                        for mut value in values {
                            if let Value::Pointer(mut address, metadata) = value.value {
                                let length = match source_ty.kind() {
                                    ty::Array(_, length) => {
                                        length.try_to_target_usize(self.tcx).map(u128::from)
                                    }
                                    _ => match metadata {
                                        Metadata::Length(length) => Some(length),
                                        _ => None,
                                    },
                                };
                                let end = if from_end {
                                    length.and_then(|length| length.checked_sub(u128::from(to)))
                                } else {
                                    Some(u128::from(to))
                                };
                                let length = end.and_then(|end| end.checked_sub(u128::from(from)));
                                address.offset = address
                                    .offset
                                    .checked_add(i128::from(from) * i128::from(size))
                                    .ok_or(QueryStop::Unsupported)?;
                                value.value = Value::Pointer(
                                    address,
                                    length.map_or(Metadata::Unknown, Metadata::Length),
                                );
                            }
                            next.push(value);
                        }
                    }
                    ProjectionElem::OpaqueCast(_) | ProjectionElem::UnwrapUnsafeBinder(_) => {
                        next = values
                    }
                    ProjectionElem::PhantomDeref => return Err(QueryStop::Unsupported),
                }
                values = self.bounded(next)?;
            }
            Ok(values)
        })();
        self.alias_object = alias_object;
        answer
    }

    fn read(
        &mut self,
        address: Address,
        requested: Ty<'tcx>,
        point: Point,
        effects_only: bool,
    ) -> Result<Vec<Alternative<'tcx>>, QueryStop> {
        self.read_edge(address, requested, point, effects_only, None, false)
    }


    pub fn feasible(
        &mut self,
        values: Vec<Alternative<'tcx>>,
    ) -> Result<Vec<Alternative<'tcx>>, QueryStop> {
        self.feasible_with_paths(values, &HashSet::new())
    }

    fn feasible_with_paths(
        &mut self,
        values: Vec<Alternative<'tcx>>,
        path_conditions: &HashSet<Point>,
    ) -> Result<Vec<Alternative<'tcx>>, QueryStop> {
        let scratch = self.budget.scratch;
        let stored = self.flow.bytes;
        let input_bytes = values.capacity() * std::mem::size_of::<Alternative<'tcx>>() * 4
            + values.iter().map(|value| value.guards.capacity() * std::mem::size_of::<(usize, Guard)>() * 4).sum::<usize>();
        let answer = (|| {
        let mut result = Vec::new();
        let mut conditions = HashMap::<(usize, Location), (Vec<Alternative<'tcx>>, bool)>::new();
        let mut generation = self.flow.generation;
        let mut condition_bytes = 0usize;
        for value in values {
            let mut pending = VecDeque::from([(
                value,
                HashSet::new(),
                Vec::<(Address, u64, u128, bool)>::new(),
            )]);
            while let Some((value, mut checked, relations)) = pending.pop_front() {
                self.budget.scratch = scratch.saturating_add(self.flow.bytes.saturating_sub(stored));
                let live_bytes = input_bytes
                    + condition_bytes
                    + result.capacity() * std::mem::size_of::<Alternative<'tcx>>() * 4
                    + result.iter().map(|value: &Alternative<'tcx>| value.guards.capacity() * std::mem::size_of::<(usize, Guard)>() * 4).sum::<usize>()
                    + pending.capacity() * std::mem::size_of::<(Alternative<'tcx>, HashSet<(usize, Guard)>, Vec<(Address, u64, u128, bool)>)>() * 4
                    + pending.iter().map(|(value, checked, relations)|
                        (value.guards.capacity() + checked.capacity()) * std::mem::size_of::<(usize, Guard)>() * 4
                        + relations.capacity() * std::mem::size_of::<(Address, u64, u128, bool)>() * 4).sum::<usize>()
                    + (value.guards.capacity() + checked.capacity()) * std::mem::size_of::<(usize, Guard)>() * 4
                    + relations.capacity() * std::mem::size_of::<(Address, u64, u128, bool)>() * 4;
                self.budget.charge(live_bytes)?;
                let next = value
                    .guards
                    .iter()
                    .find(|guard| {
                        let condition = &guard.1;
                        condition.required
                            && !matches!(condition.proof, GuardProof::Widened | GuardProof::Established | GuardProof::Unresolved)
                            && condition.index.is_none() && !condition.input && !condition.phi
                            && condition.choice.is_none() && !checked.contains(*guard)
                    })
                    .copied();
                let Some((frame, guard)) = next else {
                    let mut value = value;
                    if let Value::Symbolic(origin, bias, width) = value.value {
                        if let Some((_, _, bits, _)) =
                            relations.iter().find(|&&(other, other_width, _, equal)| {
                                equal && other == origin && other_width == width
                            })
                        {
                            value.value =
                                Value::Bits(bits.wrapping_add(bias) & (u128::MAX >> (128 - width)));
                        }
                    }
                    result.push(value);
                    continue;
                };
                checked.insert((frame, guard));
                let index = self.index(frame)?;
                let (discr, targets, expected) = if let Some(condition) = index.assumptions.get(&guard.branch) {
                    (condition, None, true)
                } else {
                    match &index.body.basic_blocks[guard.branch.block].terminator().kind {
                        TerminatorKind::SwitchInt { discr, targets } => (discr, Some(targets), true),
                        TerminatorKind::Assert { cond, expected, target, .. } =>
                            (cond, None, *expected == (*target == guard.successor)),
                        _ => return Err(QueryStop::Unsupported),
                    }
                };
                let accepts = |bits| targets.map_or(bits == u128::from(expected),
                    |targets| targets.target_for_value(bits) == guard.successor);
                let ty = self.ty(frame, discr.ty(index.body, self.tcx));
                let condition_key = (self.controls.solving || self.controls.proving_assertion
                    || self.cut_recursive_returns || matches!(value.value, Value::Function(_))
                    || path_conditions.contains(&Point { frame, location: guard.branch })).then(|| {
                    let mut choices: Vec<_> = self.index_choices.iter().map(|(key, value)| (*key, *value)).collect();
                    choices.sort_unstable_by_key(|((point, local), _)| (point.frame, point.location, *local));
                    let mut inputs: Vec<_> = self.input_choices.iter().filter(|(frame, _)| !self.flow.single_input(**frame))
                        .map(|(key, value)| (*key, *value)).collect();
                    inputs.sort_unstable();
                    let mut witnesses: Vec<_> = self.root_scalar_witness.iter().map(|(key, value)| (*key, *value)).collect();
                    witnesses.sort_unstable();
                    control::ConditionKey {
                        point: Point { frame, location: guard.branch }, generation: self.flow.generation,
                        required_binding: !self.controls.solving && !self.controls.proving_assertion && !self.cut_recursive_returns,
                        choices, inputs, witnesses, loop_entries: self.loop_entries.clone(),
                        cut_recursive_returns: self.cut_recursive_returns,
                    }
                });
                if let Some(key) = &condition_key
                    && let Some(&cause) = self.controls.stopped_conditions.get(key)
                {
                    self.controls.condition_hits += 1;
                    self.controls.causes.insert(key.point, cause);
                    let mut value = value;
                    for entry in &mut value.guards {
                        if *entry == (frame, guard) { entry.1.proof = GuardProof::Unresolved; }
                    }
                    pending.push_back((value, checked, relations));
                    continue;
                }
                if generation != self.flow.generation {
                    conditions.clear();
                    condition_bytes = 0;
                    generation = self.flow.generation;
                }
                let (answer, complete) = if let Some((values, complete)) = conditions.get(&(frame, guard.branch)) {
                    (Ok(values.clone()), *complete)
                } else {
                    let refinement_origin = self.refinement_origin;
                    self.refinement_origin.get_or_insert(Point { frame, location: guard.branch });
                    let inputs = std::mem::take(&mut self.flow.current_inputs);
                    let point = Point { frame, location: guard.branch };
                    let saved_condition = self.condition_origin.clone();
                    if self.condition_origin.is_none() {
                        self.condition_origin = condition_key.clone().map(Arc::new);
                    }
                    let scalar = self.scalar_condition(point);
                    let answer = match scalar {
                        Ok(Some(bits)) => Ok(vec![Alternative { value: Value::Bits(bits), guards: Vec::new() }]),
                        Ok(None) => self.operand(point, discr, 0, ty),
                        Err(reason) => Err(reason),
                    };
                    self.condition_origin = saved_condition;
                    let complete = self.flow.current.is_none() || self.flow.inputs_complete(&self.flow.current_inputs);
                    self.flow.current_inputs.extend(inputs);
                    self.refinement_origin = refinement_origin;
                    if self.flow.current.is_some() && generation == self.flow.generation
                        && let Ok(values) = &answer
                    {
                        let bytes = std::mem::size_of::<((usize, Location), (Vec<Alternative<'tcx>>, bool))>() * 4
                            + values.len() * std::mem::size_of::<Alternative<'tcx>>() * 4
                            + values.iter().map(|value| value.guards.len() * std::mem::size_of::<(usize, Guard)>() * 4).sum::<usize>();
                        self.budget.reserve(bytes)?;
                        condition_bytes = condition_bytes.checked_add(bytes).ok_or(QueryStop::ScratchLimit)?;
                        conditions.insert((frame, guard.branch), (values.clone(), complete));
                    }
                    (answer, complete)
                };
                let alternatives = match answer {
                    Ok(values) => values,
                    Err(
                        QueryStop::Unsupported
                        | QueryStop::MissingArgument,
                    ) => vec![Alternative {
                        value: Value::Unknown("the branch condition is not proved"),
                        guards: Vec::new(),
                    }],
                    Err(reason) => {
                        if matches!(reason, QueryStop::ControlMemory | QueryStop::RefinementLimit | QueryStop::TransferLimit
                            | QueryStop::ScratchLimit | QueryStop::AlternativeLimit)
                            && let Some(key) = condition_key
                        {
                            let bytes = std::mem::size_of::<(control::ConditionKey, (Point, QueryStop))>() * 4
                                + (key.choices.len() + key.inputs.len() + key.witnesses.len() + key.loop_entries.len()) * 64;
                            if self.controls.stopped_bytes.saturating_add(bytes) <= self.cache_limit / 16
                                && !self.controls.stopped_conditions.contains_key(&key)
                            {
                                self.controls.stopped_bytes += bytes;
                                self.controls.stopped_conditions.insert(key.clone(),
                                    self.controls.dependency_stop.unwrap_or((key.point, reason)));
                            }
                        }
                        return Err(reason);
                    }
                };
                let implied = complete && !alternatives.is_empty() && alternatives.iter().all(|alternative|
                    matches!(alternative.value, Value::Bits(bits) if accepts(bits)));
                let constraints: Vec<_> = value
                    .guards
                    .iter()
                    .copied()
                    .filter(|entry| !implied || *entry != (frame, guard))
                    .collect();
                for alternative in alternatives {
                    let mut relations = relations.clone();
                    let mut constraints = constraints.clone();
                    let proof = match alternative.value {
                        Value::Bits(_) => GuardProof::Established,
                        Value::Predicate(..) => GuardProof::Predicate,
                        Value::Unknown(_) if complete => GuardProof::Unresolved,
                        _ => GuardProof::Pending,
                    };
                    for (owner, constraint) in &mut constraints {
                        if *owner == frame && *constraint == guard { constraint.proof = proof; }
                    }
                    let mut checked = checked.clone();
                    checked.insert((frame, Guard { proof, ..guard }));
                    if let Value::Bits(bits) = alternative.value {
                        if !accepts(bits) {
                            continue;
                        }
                    }
                    if let Value::Predicate(origin, width, bits, equal) = alternative.value {
                        let takes_false = accepts(0);
                        let takes_true = accepts(1);
                        if !takes_false && !takes_true {
                            continue;
                        }
                        if takes_false != takes_true {
                            let equal = equal == takes_true;
                            let relation = (origin, width, bits, equal);
                            if relations
                                .iter()
                                .any(|&(other, other_width, value, is_equal)| {
                                    other == origin
                                        && other_width == width
                                        && (equal && is_equal && value != bits
                                            || equal != is_equal && value == bits)
                                })
                            {
                                continue;
                            }
                            if !relations.contains(&relation) {
                                self.budget.charge(std::mem::size_of_val(&relation) * 4)?;
                                relations.push(relation);
                            }
                        }
                    }
                    if let Some(guards) = self.merge(&constraints, &alternative.guards)? {
                        pending.push_back((
                            Alternative {
                                value: value.value,
                                guards,
                            },
                            checked,
                            relations,
                        ));
                    }
                }
            }
        }
        self.bounded(result)
        })();
        self.budget.scratch = scratch.saturating_add(self.flow.bytes.saturating_sub(stored));
        if let Ok(values) = &answer {
            self.budget.reserve(values.capacity() * std::mem::size_of::<Alternative<'tcx>>() * 4
                + values.iter().map(|value| value.guards.capacity() * std::mem::size_of::<(usize, Guard)>() * 4).sum::<usize>())?;
        }
        answer
    }

    pub fn paths(&mut self, point: Point, refine: bool) -> Result<Vec<Alternative<'tcx>>, QueryStop> {
        self.control_step(point, refine, true, &mut Vec::new())
    }

    fn allocation(
        &mut self,
        id: AllocId,
        offset: i128,
        requested: Ty<'tcx>,
    ) -> Result<Vec<Alternative<'tcx>>, QueryStop> {
        self.budget.charge(0)?;
        let mut value = Value::Unknown("the constant allocation cannot be read at this offset");
        if let (GlobalAlloc::Memory(memory), Ok(layout), Ok(offset)) = (
            self.tcx.global_alloc(id),
            self.tcx
                .layout_of(TypingEnv::fully_monomorphized().as_query_input(requested)),
            u64::try_from(offset),
        ) {
            let memory = memory.inner();
            let pointer = matches!(
                requested.kind(),
                ty::FnPtr(..) | ty::Ref(..) | ty::RawPtr(..)
            );
            let size = if pointer {
                self.tcx.data_layout.pointer_size()
            } else {
                layout.size
            };
            let range = AllocRange {
                start: Size::from_bytes(offset),
                size,
            };
            if size.bytes() > 0 && size.bytes() <= 16 && range.end().bytes() <= memory.len() as u64
            {
                value = match memory.read_scalar(&self.tcx, range, pointer) {
                    Ok(Scalar::Int(bits)) => Value::Bits(bits.to_bits(bits.size())),
                    Ok(Scalar::Ptr(pointer, _)) => self.constant_pointer(
                        pointer.provenance.alloc_id(),
                        i128::from(pointer.prov_and_relative_offset().1.bytes()),
                    ),
                    Err(_) => Value::Unknown("the constant bytes are not initialized"),
                };
                if pointer && layout.size > size {
                    let range = AllocRange {
                        start: range.end(),
                        size,
                    };
                    let metadata = if range.end().bytes() <= memory.len() as u64 {
                        match memory.read_scalar(&self.tcx, range, true) {
                            Ok(Scalar::Ptr(pointer, _)) => match self.constant_pointer(
                                pointer.provenance.alloc_id(),
                                i128::from(pointer.prov_and_relative_offset().1.bytes()),
                            ) {
                                Value::Metadata(metadata) => metadata,
                                _ => Metadata::Unknown,
                            },
                            Ok(Scalar::Int(bits)) => Metadata::Length(bits.to_bits(bits.size())),
                            _ => Metadata::Unknown,
                        }
                    } else {
                        Metadata::Unknown
                    };
                    if let Value::Pointer(address, _) = value {
                        value = Value::Pointer(address, metadata);
                    }
                }
            }
        }
        Ok(vec![Alternative {
            value,
            guards: Vec::new(),
        }])
    }

    fn rvalue(
        &mut self,
        point: Point,
        value: &Rvalue<'tcx>,
        destination: Ty<'tcx>,
        offset: i128,
        requested: Ty<'tcx>,
    ) -> Result<Vec<Alternative<'tcx>>, QueryStop> {
        self.budget.charge(0)?;
        let body = self.index(point.frame)?.body;
        let unknown = Value::Unknown("the value transfer has no supported pointer binding");
        match value {
            Rvalue::Use(operand, _) | Rvalue::WrapUnsafeBinder(operand, _) => {
                self.operand(point, operand, offset, requested)
            }
            Rvalue::CopyForDeref(place) | Rvalue::Reborrow(_, _, place) => {
                self.operand(point, &Operand::Copy(*place), offset, requested)
            }
            Rvalue::Ref(_, _, place) | Rvalue::RawPtr(_, place) if offset == 0 => {
                if matches!(place.projection.last(), Some(ProjectionElem::Deref))
                    && self
                        .tcx
                        .layout_of(TypingEnv::fully_monomorphized().as_query_input(requested))
                        .is_ok_and(|layout| layout.size == self.tcx.data_layout.pointer_size())
                {
                    let prefix = Place {
                        local: place.local,
                        projection: self
                            .tcx
                            .mk_place_elems(&place.projection[..place.projection.len() - 1]),
                    };
                    return self.operand(point, &Operand::Copy(prefix), 0, requested);
                }
                self.address(point, *place, self.alias_object, None)
            }
            Rvalue::Ref(_, _, place) | Rvalue::RawPtr(_, place)
                if offset == i128::from(self.tcx.data_layout.pointer_size().bytes())
                    && self
                        .tcx
                        .layout_of(TypingEnv::fully_monomorphized().as_query_input(destination))
                        .is_ok_and(|layout| {
                            layout.size.bytes() == self.tcx.data_layout.pointer_size().bytes() * 2
                        }) =>
            {
                if matches!(place.projection.last(), Some(ProjectionElem::Deref)) {
                    let prefix = Place {
                        local: place.local,
                        projection: self
                            .tcx
                            .mk_place_elems(&place.projection[..place.projection.len() - 1]),
                    };
                    return self.operand(point, &Operand::Copy(prefix), offset, requested);
                }
                let mut values = self.address(point, *place, None, None)?;
                for value in &mut values {
                    value.value = match value.value {
                        Value::Pointer(_, Metadata::Length(length)) => Value::Bits(length),
                        Value::Pointer(_, metadata) => Value::Metadata(metadata),
                        Value::Unknown(reason) => Value::Unknown(reason),
                        _ => unknown,
                    };
                }
                self.bounded(values)
            }
            Rvalue::ThreadLocalRef(def) if offset == 0 => Ok(vec![Alternative {
                value: Value::Pointer(
                    Address {
                        object: Object::Static(*def),
                        offset: 0,
                    },
                    Metadata::Thin,
                ),
                guards: Vec::new(),
            }]),
            Rvalue::Cast(kind, operand, _) => {
                let source = self.ty(point.frame, operand.ty(body, self.tcx));
                if matches!(
                    kind,
                    mir::CastKind::PointerCoercion(
                        ty::adjustment::PointerCoercion::ClosureFnPointer(_),
                        _
                    )
                ) {
                    if let ty::Closure(def, args) = *source.kind() {
                        return Ok(vec![Alternative {
                            value: Value::Function(Instance::resolve_closure(
                                self.tcx,
                                def,
                                args,
                                ty::ClosureKind::FnOnce,
                            )),
                            guards: Vec::new(),
                        }]);
                    }
                }
                if matches!(
                    kind,
                    mir::CastKind::PointerCoercion(
                        ty::adjustment::PointerCoercion::ReifyFnPointer(_),
                        _
                    )
                ) {
                    if let ty::FnDef(def, args) = *source.kind() {
                        let args = self.tcx.instantiate_bound_regions_with_erased(args);
                        let instance = Instance::resolve_for_fn_ptr(
                            self.tcx,
                            TypingEnv::fully_monomorphized(),
                            def,
                            args,
                        )
                        .map(Value::Function)
                        .unwrap_or(unknown);
                        return Ok(vec![Alternative {
                            value: instance,
                            guards: Vec::new(),
                        }]);
                    }
                }
                if matches!(
                    kind,
                    mir::CastKind::PointerCoercion(ty::adjustment::PointerCoercion::Unsize, _)
                ) {
                    let width = i128::from(self.tcx.data_layout.pointer_size().bytes());
                    let metadata_only = offset >= width
                        && self
                            .pointer_projection(source, destination, offset - width)
                            .is_some()
                        && self
                            .tcx
                            .layout_of(TypingEnv::fully_monomorphized().as_query_input(requested))
                            .is_ok_and(|layout| i128::from(layout.size.bytes()) == width);
                    let Some((source_offset, source_pointer, target_pointer)) = self
                        .pointer_projection(
                            source,
                            destination,
                            if metadata_only {
                                offset - width
                            } else {
                                offset
                            },
                        )
                    else {
                        return Ok(vec![Alternative {
                            value: unknown,
                            guards: Vec::new(),
                        }]);
                    };
                    let concrete = source_pointer
                        .builtin_deref(true)
                        .ok_or(QueryStop::Unsupported)?;
                    let target = target_pointer
                        .builtin_deref(true)
                        .ok_or(QueryStop::Unsupported)?;
                    if metadata_only {
                        let metadata = if matches!(target.kind(), ty::Dynamic(..))
                            && !matches!(concrete.kind(), ty::Dynamic(..))
                        {
                            Some(Value::Metadata(Metadata::Concrete(concrete)))
                        } else if let ty::Array(_, count) = concrete.kind() {
                            if matches!(target.kind(), ty::Slice(_)) {
                                count
                                    .try_to_target_usize(self.tcx)
                                    .map(|length| Value::Bits(length.into()))
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        if let Some(value) = metadata {
                            return Ok(vec![Alternative {
                                value,
                                guards: Vec::new(),
                            }]);
                        }
                        if concrete == target {
                            return self.operand(point, operand, source_offset + width, requested);
                        }
                        return Ok(vec![Alternative {
                            value: unknown,
                            guards: Vec::new(),
                        }]);
                    }
                    let mut values = self.operand(point, operand, source_offset, source_pointer)?;
                    for value in &mut values {
                        value.value = match value.value {
                            Value::Unknown(reason) => Value::Unknown(reason),
                            Value::ObjectPointer(object) => Value::ObjectPointer(object),
                            Value::Pointer(address, metadata) => {
                                let metadata = if matches!(target.kind(), ty::Dynamic(..))
                                    && !matches!(concrete.kind(), ty::Dynamic(..))
                                {
                                    Metadata::Concrete(concrete)
                                } else if let ty::Array(_, length) = concrete.kind() {
                                    if matches!(target.kind(), ty::Slice(_)) {
                                        length
                                            .try_to_target_usize(self.tcx)
                                            .map(|length| Metadata::Length(length.into()))
                                            .unwrap_or(Metadata::Unknown)
                                    } else {
                                        Metadata::Unknown
                                    }
                                } else if concrete == target {
                                    metadata
                                } else {
                                    Metadata::Unknown
                                };
                                Value::Pointer(address, metadata)
                            }
                            _ => unknown,
                        };
                    }
                    return self.bounded(values);
                }
                let partial_pointer = source.builtin_deref(true).is_some()
                    && self
                        .tcx
                        .layout_of(TypingEnv::fully_monomorphized().as_query_input(source))
                        .ok()
                        .zip(
                            self.tcx
                                .layout_of(
                                    TypingEnv::fully_monomorphized().as_query_input(requested),
                                )
                                .ok(),
                        )
                        .is_some_and(|(source, requested)| requested.size < source.size);
                let mut values = self.operand(
                    point,
                    operand,
                    offset,
                    if offset == 0
                        && !partial_pointer
                        && (source.builtin_deref(true).is_some()
                            || source.is_integral()
                            || matches!(source.kind(), ty::Bool | ty::Char)
                            || source.is_fn_ptr())
                    {
                        source
                    } else {
                        requested
                    },
                )?;
                let source_size = self
                    .tcx
                    .layout_of(TypingEnv::fully_monomorphized().as_query_input(source))
                    .ok()
                    .map(|layout| layout.size.bits());
                let destination_size = self
                    .tcx
                    .layout_of(TypingEnv::fully_monomorphized().as_query_input(destination))
                    .ok()
                    .map(|layout| layout.size.bits());
                for value in &mut values {
                    match kind {
                        mir::CastKind::PointerCoercion(
                            ty::adjustment::PointerCoercion::Unsize,
                            _,
                        ) => {
                            if let Value::Pointer(address, metadata) = value.value {
                                let metadata = match (
                                    source.builtin_deref(true),
                                    destination.builtin_deref(true),
                                ) {
                                    (Some(concrete), Some(target))
                                        if matches!(target.kind(), ty::Dynamic(..))
                                            && !matches!(concrete.kind(), ty::Dynamic(..)) =>
                                    {
                                        Metadata::Concrete(concrete)
                                    }
                                    (Some(array), Some(target))
                                        if matches!(target.kind(), ty::Slice(_)) =>
                                    {
                                        match array.kind() {
                                            ty::Array(_, count) => count
                                                .try_to_target_usize(self.tcx)
                                                .map_or(Metadata::Unknown, |count| {
                                                    Metadata::Length(u128::from(count))
                                                }),
                                            _ => metadata,
                                        }
                                    }
                                    _ => metadata,
                                };
                                value.value = Value::Pointer(address, metadata);
                            } else {
                                value.value = unknown;
                            }
                        }
                        mir::CastKind::IntToInt => {
                            value.value = match (value.value, source_size, destination_size) {
                                (Value::Bits(bits), Some(source_width), Some(width))
                                    if source_width > 0
                                        && source_width <= 128
                                        && width > 0
                                        && width <= 128 =>
                                {
                                    let bits = if matches!(source.kind(), ty::Int(_))
                                        && width > source_width
                                    {
                                        (((bits << (128 - source_width)) as i128)
                                            >> (128 - source_width))
                                            as u128
                                    } else {
                                        bits
                                    };
                                    Value::Bits(bits & (u128::MAX >> (128 - width)))
                                }
                                (
                                    value @ (Value::Pointer(..)
                                    | Value::Function(_)
                                    | Value::Symbolic(..)),
                                    Some(source_width),
                                    Some(width),
                                ) if source_width == width => value,
                                _ => unknown,
                            };
                        }
                        mir::CastKind::PtrToPtr
                        | mir::CastKind::FnPtrToPtr
                        | mir::CastKind::PointerExposeProvenance
                        | mir::CastKind::PointerWithExposedProvenance
                        | mir::CastKind::Transmute
                        | mir::CastKind::BoxDerefTransmute
                        | mir::CastKind::Subtype => {
                            if source_size != destination_size
                                && !matches!(kind, mir::CastKind::PtrToPtr)
                            {
                                value.value = unknown;
                            }
                        }
                        mir::CastKind::PointerCoercion(
                            ty::adjustment::PointerCoercion::UnsafeFnPointer,
                            _,
                        ) => {}
                        _ => value.value = unknown,
                    }
                }
                self.bounded(values)
            }
            Rvalue::Aggregate(kind, operands) => {
                if matches!(&**kind, mir::AggregateKind::RawPtr(..))
                    && offset == i128::from(self.tcx.data_layout.pointer_size().bytes())
                {
                    let metadata = operands.raw.get(1).ok_or(QueryStop::MissingArgument)?;
                    return self.operand(point, metadata, 0, requested);
                }
                if let mir::AggregateKind::Adt(def, variant, _, _, _) = &**kind {
                    if self.tcx.adt_def(*def).is_enum() && requested == destination && offset == 0 {
                        return Ok(vec![Alternative {
                            value: Value::Variant(variant.as_u32()),
                            guards: Vec::new(),
                        }]);
                    }
                }
                if matches!(&**kind, mir::AggregateKind::RawPtr(..)) && offset == 0 {
                    let Some(data) = operands.raw.first() else {
                        return Err(QueryStop::Unsupported);
                    };
                    let Some(meta) = operands.raw.get(1) else {
                        return Err(QueryStop::Unsupported);
                    };
                    let data_ty = self.ty(point.frame, data.ty(body, self.tcx));
                    let meta_ty = self.ty(point.frame, meta.ty(body, self.tcx));
                    let data_values = self.operand(point, data, 0, data_ty)?;
                    let metadata_values = self.operand(point, meta, 0, meta_ty)?;
                    let mut values = Vec::new();
                    for data in data_values {
                        for metadata in &metadata_values {
                            let Some(guards) = self.merge(&data.guards, &metadata.guards)? else {
                                continue;
                            };
                            let metadata = match metadata.value {
                                Value::Bits(length) => Metadata::Length(length),
                                Value::Metadata(metadata) => metadata,
                                _ => Metadata::Unknown,
                            };
                            let value = match data.value {
                                Value::Pointer(address, _) => Value::Pointer(address, metadata),
                                Value::ObjectPointer(object) => Value::ObjectPointer(object),
                                _ => unknown,
                            };
                            values.push(Alternative { value, guards });
                        }
                    }
                    return self.bounded(values);
                }
                let layout = self
                    .tcx
                    .layout_of(TypingEnv::fully_monomorphized().as_query_input(destination))
                    .map_err(|_| QueryStop::Unsupported)?;
                let layout = match &**kind {
                    mir::AggregateKind::Adt(_, variant, _, _, _) => layout.for_variant(
                        &ty::layout::LayoutCx::new(self.tcx, TypingEnv::fully_monomorphized()),
                        *variant,
                    ),
                    _ => layout,
                };
                let requested_size = self
                    .tcx
                    .layout_of(TypingEnv::fully_monomorphized().as_query_input(requested))
                    .map_err(|_| QueryStop::Unsupported)?
                    .size
                    .bytes();
                for (field, operand) in operands.iter_enumerated() {
                    let field_number = match &**kind {
                        mir::AggregateKind::Adt(_, _, _, _, Some(active)) => active.as_usize(),
                        _ => field.as_usize(),
                    };
                    let field_offset = i128::from(layout.fields.offset(field_number).bytes());
                    let field_ty = self.ty(point.frame, operand.ty(body, self.tcx));
                    let size = self
                        .tcx
                        .layout_of(TypingEnv::fully_monomorphized().as_query_input(field_ty))
                        .map_err(|_| QueryStop::Unsupported)?
                        .size
                        .bytes();
                    if offset >= field_offset
                        && offset + i128::from(requested_size) <= field_offset + i128::from(size)
                    {
                        return self.operand(point, operand, offset - field_offset, requested);
                    }
                }
                Ok(vec![Alternative {
                    value: unknown,
                    guards: Vec::new(),
                }])
            }
            Rvalue::Repeat(operand, _) => {
                let element = self.ty(point.frame, operand.ty(body, self.tcx));
                let size = self
                    .tcx
                    .layout_of(TypingEnv::fully_monomorphized().as_query_input(element))
                    .map_err(|_| QueryStop::Unsupported)?
                    .size
                    .bytes();
                if size == 0 {
                    return Ok(vec![Alternative {
                        value: unknown,
                        guards: Vec::new(),
                    }]);
                }
                self.operand(
                    point,
                    operand,
                    offset.rem_euclid(i128::from(size)),
                    requested,
                )
            }
            Rvalue::UnaryOp(operation, operand) => {
                let source = self.ty(point.frame, operand.ty(body, self.tcx));
                let width = self
                    .tcx
                    .layout_of(TypingEnv::fully_monomorphized().as_query_input(source))
                    .map_err(|_| QueryStop::Unsupported)?
                    .size
                    .bits();
                if *operation == mir::UnOp::PtrMetadata
                    && source.is_any_ptr()
                    && source
                        .builtin_deref(true)
                        .is_some_and(|pointee| matches!(pointee.kind(), ty::Dynamic(..)))
                    && width == self.tcx.data_layout.pointer_size().bits() * 2
                    && offset == 0
                    && self
                        .tcx
                        .layout_of(TypingEnv::fully_monomorphized().as_query_input(requested))
                        .is_ok_and(|layout| layout.size == self.tcx.data_layout.pointer_size())
                {
                    return self.operand(
                        point,
                        operand,
                        i128::from(self.tcx.data_layout.pointer_size().bytes()),
                        Ty::new_imm_ptr(self.tcx, self.tcx.types.unit),
                    );
                }
                let mut values = self.operand(point, operand, 0, source)?;
                for value in &mut values {
                    value.value = match (operation, value.value) {
                        (mir::UnOp::PtrMetadata, Value::Pointer(_, Metadata::Length(length))) => {
                            Value::Bits(length)
                        }
                        (mir::UnOp::PtrMetadata, Value::Pointer(_, metadata)) => {
                            Value::Metadata(metadata)
                        }
                        (mir::UnOp::Not, Value::Bits(bits)) if source.is_bool() => {
                            Value::Bits(u128::from(bits == 0))
                        }
                        (mir::UnOp::Not, Value::Predicate(origin, width, bits, equal))
                            if source.is_bool() =>
                        {
                            Value::Predicate(origin, width, bits, !equal)
                        }
                        (mir::UnOp::Not, Value::Bits(bits)) if width > 0 && width <= 128 => {
                            Value::Bits(!bits & (u128::MAX >> (128 - width)))
                        }
                        (mir::UnOp::Neg, Value::Bits(bits)) if width > 0 && width <= 128 => {
                            Value::Bits(bits.wrapping_neg() & (u128::MAX >> (128 - width)))
                        }
                        _ => unknown,
                    };
                }
                self.bounded(values)
            }
            Rvalue::BinaryOp(operation, operands) => {
                let left_ty = self.ty(point.frame, operands.0.ty(body, self.tcx));
                let right_ty = self.ty(point.frame, operands.1.ty(body, self.tcx));
                let left = self.operand(point, &operands.0, 0, left_ty)?;
                if *operation == mir::BinOp::Offset
                    && let Some(read_object) = self.alias_object
                    && !left.is_empty()
                    && left.iter().all(|value| match value.value {
                        Value::Pointer(address, _) => address.object != read_object,
                        Value::ObjectPointer(object) => object != read_object,
                        _ => false,
                    })
                {
                    return self.bounded(
                        left.into_iter()
                            .map(|mut value| {
                                if let Value::Pointer(address, _) = value.value {
                                    value.value = Value::ObjectPointer(address.object);
                                }
                                value
                            })
                            .collect(),
                    );
                }
                let right = self.operand(point, &operands.1, 0, right_ty)?;
                if std::env::var("IRQ_CHECK_TRACE_TRANSFER").is_ok_and(|filter| {
                        self.frames[point.frame]
                            .instance
                            .to_string()
                            .contains(&filter)
                    })
                {
                    eprintln!(
                        "irq-check: binary {operation:?} {} frame={} at {:?} left={left:?} right={right:?}",
                        self.frames[point.frame].instance, point.frame, point.location
                    );
                }
                let width = self
                    .tcx
                    .layout_of(TypingEnv::fully_monomorphized().as_query_input(left_ty))
                    .map_err(|_| QueryStop::Unsupported)?
                    .size
                    .bits();
                let signed = matches!(left_ty.kind(), ty::Int(_));
                let mut values = Vec::new();
                let combinations = left.len().saturating_mul(right.len());
                for left in left {
                    for right in &right {
                        let Some(guards) = self.merge(&left.guards, &right.guards)? else {
                            continue;
                        };
                        let mut result = unknown;
                        if let (Value::Bits(a), Value::Bits(b)) = (left.value, right.value) {
                            if width > 0 && width <= 128 {
                                let mask = u128::MAX >> (128 - width);
                                let (a, b) = (a & mask, b & mask);
                                let sa = ((a << (128 - width)) as i128) >> (128 - width);
                                let sb = ((b << (128 - width)) as i128) >> (128 - width);
                                let mut overflow = false;
                                let bits = match operation {
                                    mir::BinOp::Eq => Some(u128::from(a == b)),
                                    mir::BinOp::Ne => Some(u128::from(a != b)),
                                    mir::BinOp::Lt => {
                                        Some(u128::from(if signed { sa < sb } else { a < b }))
                                    }
                                    mir::BinOp::Le => {
                                        Some(u128::from(if signed { sa <= sb } else { a <= b }))
                                    }
                                    mir::BinOp::Gt => {
                                        Some(u128::from(if signed { sa > sb } else { a > b }))
                                    }
                                    mir::BinOp::Ge => {
                                        Some(u128::from(if signed { sa >= sb } else { a >= b }))
                                    }
                                    mir::BinOp::BitAnd => Some(a & b),
                                    mir::BinOp::BitOr => Some(a | b),
                                    mir::BinOp::BitXor => Some(a ^ b),
                                    mir::BinOp::Shl | mir::BinOp::ShlUnchecked
                                        if b < u128::from(width) =>
                                    {
                                        Some((a << b) & mask)
                                    }
                                    mir::BinOp::Shr | mir::BinOp::ShrUnchecked
                                        if b < u128::from(width) =>
                                    {
                                        Some(if signed {
                                            (sa >> b) as u128 & mask
                                        } else {
                                            a >> b
                                        })
                                    }
                                    mir::BinOp::Div if b != 0 => {
                                        if signed {
                                            sa.checked_div(sb).map(|value| value as u128 & mask)
                                        } else {
                                            Some(a / b)
                                        }
                                    }
                                    mir::BinOp::Rem if b != 0 => {
                                        if signed {
                                            sa.checked_rem(sb).map(|value| value as u128 & mask)
                                        } else {
                                            Some(a % b)
                                        }
                                    }
                                    mir::BinOp::Add
                                    | mir::BinOp::AddUnchecked
                                    | mir::BinOp::AddWithOverflow
                                    | mir::BinOp::Sub
                                    | mir::BinOp::SubUnchecked
                                    | mir::BinOp::SubWithOverflow
                                    | mir::BinOp::Mul
                                    | mir::BinOp::MulUnchecked
                                    | mir::BinOp::MulWithOverflow => {
                                        let addition = matches!(
                                            operation,
                                            mir::BinOp::Add
                                                | mir::BinOp::AddUnchecked
                                                | mir::BinOp::AddWithOverflow
                                        );
                                        let subtraction = matches!(
                                            operation,
                                            mir::BinOp::Sub
                                                | mir::BinOp::SubUnchecked
                                                | mir::BinOp::SubWithOverflow
                                        );
                                        let (bits, wide) = if addition {
                                            a.overflowing_add(b)
                                        } else if subtraction {
                                            a.overflowing_sub(b)
                                        } else {
                                            a.overflowing_mul(b)
                                        };
                                        overflow = if signed {
                                            let result = if addition {
                                                sa.checked_add(sb)
                                            } else if subtraction {
                                                sa.checked_sub(sb)
                                            } else {
                                                sa.checked_mul(sb)
                                            };
                                            result.is_none_or(|value| {
                                                width < 128
                                                    && (value < -(1i128 << (width - 1))
                                                        || value >= 1i128 << (width - 1))
                                            })
                                        } else {
                                            wide || bits > mask
                                        };
                                        Some(bits & mask)
                                    }
                                    _ => None,
                                };
                                result = bits.map_or(unknown, |bits| {
                                    Value::Bits(
                                        if offset != 0
                                            && matches!(
                                                operation,
                                                mir::BinOp::AddWithOverflow
                                                    | mir::BinOp::SubWithOverflow
                                                    | mir::BinOp::MulWithOverflow
                                            )
                                        {
                                            u128::from(overflow)
                                        } else {
                                            bits
                                        },
                                    )
                                });
                            }
                        } else if let (Value::Pointer(a, _), Value::Pointer(b, _)) =
                            (left.value, right.value)
                        {
                            if width == self.tcx.data_layout.pointer_size().bits()
                                && a.object == b.object
                                && self.singleton_object(a.object)?
                            {
                                let mask = u128::MAX >> (128 - width);
                                let equal = a.offset as u128 & mask == b.offset as u128 & mask;
                                result = match operation {
                                    mir::BinOp::Eq => Value::Bits(u128::from(equal)),
                                    mir::BinOp::Ne => Value::Bits(u128::from(!equal)),
                                    _ => unknown,
                                };
                            }
                        } else if let (
                            Value::Symbolic(origin, bias, source_width),
                            Value::Bits(bits),
                        ) = (left.value, right.value)
                        {
                            if source_width == width && width > 0 && width <= 128 {
                                let mask = u128::MAX >> (128 - width);
                                result = match operation {
                                    mir::BinOp::Eq | mir::BinOp::Ne => Value::Predicate(
                                        origin,
                                        width,
                                        bits.wrapping_sub(bias) & mask,
                                        *operation == mir::BinOp::Eq,
                                    ),
                                    mir::BinOp::Add
                                    | mir::BinOp::AddUnchecked
                                    | mir::BinOp::AddWithOverflow
                                        if offset == 0 =>
                                    {
                                        Value::Symbolic(
                                            origin,
                                            bias.wrapping_add(bits) & mask,
                                            width,
                                        )
                                    }
                                    mir::BinOp::Sub
                                    | mir::BinOp::SubUnchecked
                                    | mir::BinOp::SubWithOverflow
                                        if offset == 0 =>
                                    {
                                        Value::Symbolic(
                                            origin,
                                            bias.wrapping_sub(bits) & mask,
                                            width,
                                        )
                                    }
                                    _ => unknown,
                                };
                            }
                        } else if let (
                            Value::Bits(bits),
                            Value::Symbolic(origin, bias, source_width),
                        ) = (left.value, right.value)
                        {
                            if source_width == width && width > 0 && width <= 128 {
                                let mask = u128::MAX >> (128 - width);
                                result = match operation {
                                    mir::BinOp::Eq | mir::BinOp::Ne => Value::Predicate(
                                        origin,
                                        width,
                                        bits.wrapping_sub(bias) & mask,
                                        *operation == mir::BinOp::Eq,
                                    ),
                                    mir::BinOp::Add
                                    | mir::BinOp::AddUnchecked
                                    | mir::BinOp::AddWithOverflow
                                        if offset == 0 =>
                                    {
                                        Value::Symbolic(
                                            origin,
                                            bias.wrapping_add(bits) & mask,
                                            width,
                                        )
                                    }
                                    _ => unknown,
                                };
                            }
                        } else if let (Value::Pointer(mut address, metadata), Value::Bits(amount)) =
                            (left.value, right.value)
                        {
                            if matches!(
                                operation,
                                mir::BinOp::Offset | mir::BinOp::Add | mir::BinOp::Sub
                            ) {
                                let right_width = self
                                    .tcx
                                    .layout_of(
                                        TypingEnv::fully_monomorphized().as_query_input(right_ty),
                                    )
                                    .map_err(|_| QueryStop::Unsupported)?
                                    .size
                                    .bits();
                                let amount = if matches!(right_ty.kind(), ty::Int(_))
                                    && right_width > 0
                                    && right_width <= 128
                                {
                                    ((amount << (128 - right_width)) as i128) >> (128 - right_width)
                                } else {
                                    i128::try_from(amount).map_err(|_| QueryStop::Unsupported)?
                                };
                                let stride = if *operation == mir::BinOp::Offset {
                                    let pointee = left_ty
                                        .builtin_deref(true)
                                        .ok_or(QueryStop::Unsupported)?;
                                    i128::from(
                                        self.tcx
                                            .layout_of(
                                                TypingEnv::fully_monomorphized()
                                                    .as_query_input(pointee),
                                            )
                                            .map_err(|_| QueryStop::Unsupported)?
                                            .size
                                            .bytes(),
                                    )
                                } else {
                                    1
                                };
                                let delta = amount.checked_mul(stride).and_then(|amount| {
                                    if *operation == mir::BinOp::Sub {
                                        amount.checked_neg()
                                    } else {
                                        Some(amount)
                                    }
                                });
                                if let Some(offset) =
                                    delta.and_then(|delta| address.offset.checked_add(delta))
                                {
                                    address.offset = offset;
                                    result = Value::Pointer(address, metadata);
                                }
                            } else if amount == 0
                                && matches!(
                                    address.object,
                                    Object::Local(..) | Object::PriorLocal(..) | Object::SummaryLocal(..) | Object::Static(_) | Object::Allocation(_)
                                )
                            {
                                result = match operation {
                                    mir::BinOp::Eq => Value::Bits(0),
                                    mir::BinOp::Ne => Value::Bits(1),
                                    _ => unknown,
                                };
                            }
                        }
                        if matches!(result, Value::Unknown(_)) && width > 0 && width <= 128 {
                            let mask = u128::MAX >> (128 - width);
                            if *operation == mir::BinOp::BitAnd {
                                let address_mask = match (left.value, right.value) {
                                    (Value::Pointer(address, _), Value::Bits(bits))
                                    | (Value::Bits(bits), Value::Pointer(address, _)) => Some((address, bits & mask)),
                                    _ => None,
                                };
                                if let Some((address, bits)) = address_mask
                                    && let Some(alignment) = self.object_alignment(address.object)?
                                    && bits & !u128::from(alignment - 1) == 0
                                {
                                    result = Value::Bits(address.offset as u128 & bits);
                                }
                            }
                            let known = match (left.value, right.value) {
                                (Value::Bits(bits), _) | (_, Value::Bits(bits)) => Some(bits & mask),
                                _ => None,
                            };
                            if *operation == mir::BinOp::BitAnd && known == Some(0) {
                                result = Value::Bits(0);
                            } else if *operation == mir::BinOp::BitOr && known == Some(mask) {
                                result = Value::Bits(mask);
                            }
                        }
                        let mut guards = guards;
                        if *operation == mir::BinOp::Offset {
                            if let Value::ObjectPointer(object) = left.value {
                                values.push(Alternative {
                                    value: Value::ObjectPointer(object),
                                    guards,
                                });
                                continue;
                            }
                            if let (
                                Value::Pointer(mut address, metadata),
                                Value::Unknown(_) | Value::Symbolic(..) | Value::Predicate(..),
                            ) = (left.value, right.value)
                            {
                                if self
                                    .alias_object
                                    .is_some_and(|object| object != address.object)
                                {
                                    values.push(Alternative {
                                        value: Value::ObjectPointer(address.object),
                                        guards,
                                    });
                                    continue;
                                }
                                let pointee =
                                    left_ty.builtin_deref(true).ok_or(QueryStop::Unsupported)?;
                                let stride = self
                                    .tcx
                                    .layout_of(
                                        TypingEnv::fully_monomorphized().as_query_input(pointee),
                                    )
                                    .map_err(|_| QueryStop::Unsupported)?
                                    .size
                                    .bytes();
                                let storage_ty = match address.object {
                                    Object::Local(owner, local) | Object::PriorLocal(owner, local) | Object::SummaryLocal(owner, local) => {
                                        let ty = self.index(owner)?.body.local_decls[local].ty;
                                        Some(self.ty(owner, ty))
                                    }
                                    Object::Static(def) => {
                                        Some(self.tcx.normalize_erasing_regions(
                                            TypingEnv::fully_monomorphized(),
                                            self.tcx.type_of(def).instantiate_identity(),
                                        ))
                                    }
                                    _ => None,
                                };
                                let extent = storage_ty
                                    .and_then(|ty| {
                                        self.tcx
                                            .layout_of(
                                                TypingEnv::fully_monomorphized().as_query_input(ty),
                                            )
                                            .ok()
                                            .map(|layout| layout.size.bytes())
                                    })
                                    .or_else(|| match address.object {
                                        Object::Allocation(id) => match self.tcx.global_alloc(id) {
                                            GlobalAlloc::Memory(memory) => {
                                                Some(memory.inner().len() as u64)
                                            }
                                            _ => None,
                                        },
                                        _ => None,
                                    });
                                if stride == 0 {
                                    result = Value::Pointer(address, metadata);
                                } else if let Some(extent) = extent {
                                    let start = if matches!(right_ty.kind(), ty::Int(_)) {
                                        address.offset.rem_euclid(i128::from(stride))
                                    } else {
                                        address.offset
                                    };
                                    if start >= 0 && start < i128::from(extent) {
                                        let count =
                                            (i128::from(extent) - start) / i128::from(stride);
                                        if count > 0 {
                                            let local = match &operands.1 {
                                                Operand::Copy(place) | Operand::Move(place) => {
                                                    place.local
                                                }
                                                _ => return Err(QueryStop::Unsupported),
                                            };
                                            if self.flow.current.is_some()
                                                && count <= 32
                                                && combinations.saturating_mul(count as usize) <= 32
                                                && !self.index_choices.contains_key(&(point, local))
                                            {
                                                for choice in 0..count {
                                                    let Some(guards) = self.merge(
                                                        &guards,
                                                        &[(
                                                            point.frame,
                                                            Guard {
                                                                required: true, iteration: None, proof: GuardProof::Pending,
                                                                phi: false,
                                                                branch: point.location,
                                                                successor:
                                                                    mir::BasicBlock::from_usize(
                                                                        choice as usize,
                                                                    ),
                                                                index: Some(local),
                                                                input: false,
                                                                choice: None,
                                                            },
                                                        )],
                                                    )?
                                                    else {
                                                        continue;
                                                    };
                                                    let address = Address {
                                                        object: address.object,
                                                        offset: start + choice * i128::from(stride),
                                                    };
                                                    values.push(Alternative {
                                                        value: Value::Pointer(address, metadata),
                                                        guards,
                                                    });
                                                }
                                                continue;
                                            }
                                            let choice = if let Some(choice) =
                                                self.index_choice((point, local))?
                                            {
                                                choice
                                            } else if count == 1 {
                                                0
                                            } else {
                                                self.split_index =
                                                    Some((point, local, count as u128));
                                                return Err(QueryStop::SplitIndex);
                                            };
                                            if choice >= count as u128
                                                || choice >= u128::from(u32::MAX)
                                            {
                                                return Err(QueryStop::Unsupported);
                                            }
                                            let guard = Guard {
                                                required: true, iteration: None, proof: GuardProof::Pending,
                                                phi: false,
                                                branch: point.location,
                                                successor: mir::BasicBlock::from_usize(
                                                    choice as usize,
                                                ),
                                                index: Some(local),
                                                input: false,
                                                choice: None,
                                            };
                                            let Some(merged) =
                                                self.merge(&guards, &[(point.frame, guard)])?
                                            else {
                                                continue;
                                            };
                                            guards = merged;
                                            address.offset =
                                                start + choice as i128 * i128::from(stride);
                                            result = Value::Pointer(address, metadata);
                                        }
                                    }
                                }
                            }
                        }
                        values.push(Alternative {
                            value: result,
                            guards,
                        });
                    }
                }
                self.bounded(values)
            }
            Rvalue::Discriminant(place) => {
                let source = self.ty(point.frame, place.ty(body, self.tcx).ty);
                let mut values = self.operand(point, &Operand::Copy(*place), 0, source)?;
                for value in &mut values {
                    value.value = match (source.kind(), value.value) {
                        (ty::Adt(adt, _), Value::Variant(variant)) => Value::Bits(
                            adt.discriminant_for_variant(
                                self.tcx,
                                rustc_abi::VariantIdx::from_u32(variant),
                            )
                            .val,
                        ),
                        _ => unknown,
                    };
                }
                self.bounded(values)
            }
            _ => Ok(vec![Alternative {
                value: unknown,
                guards: Vec::new(),
            }]),
        }
    }

    pub fn linked_target(&self, instance: Instance<'tcx>) -> Option<Instance<'tcx>> {
        if self.tcx.is_foreign_item(instance.def_id())
            && self.trust.checks(instance.def_id())
            && crate::forbidden(self.tcx, instance).is_none()
            && let Some(&target) = self.exported.get(self.tcx.symbol_name(instance).name)
        {
            let declaration = self.tcx.normalize_erasing_regions(TypingEnv::fully_monomorphized(),
                self.tcx.fn_sig(instance.def_id()).instantiate(self.tcx, instance.args));
            return irq_check_rupta::util::type_util::call_abi_compatible(
                self.tcx, declaration, target,
            ).then_some(target);
        }
        Some(instance)
    }

    pub fn bound_target(&self, point: Point) -> Option<Instance<'tcx>> {
        let frame = self.frames[point.frame];
        let index = self.indexes.get(&(frame.instance, frame.irq))?;
        if !matches!(index.body.basic_blocks[point.location.block].terminator().kind,
            TerminatorKind::Call { .. } | TerminatorKind::TailCall { .. } | TerminatorKind::Drop { .. })
        {
            return None;
        }
        let contextual = (|| {
            let contexts = self.frame_contexts[point.frame].as_ref().filter(|contexts| !contexts.is_empty())?;
            let mut singleton = None;
            for &context in contexts {
                let targets = self.facts.context_bindings.get(&(context, point.location)).filter(|targets| !targets.is_empty())?;
                for &(target, _) in targets {
                    if singleton.is_some_and(|previous| previous != target) { return None; }
                    singleton = Some(target);
                }
            }
            singleton
        })();
        let site = (frame.instance, point.location);
        let target = contextual.or_else(|| self.facts.pointer_calls.contains(&site)
            .then(|| self.facts.safe_candidates.get(&site)).flatten()
            .filter(|targets| targets.len() == 1)
            .and_then(|targets| targets.iter().next().copied()))?;
        self.linked_target(target)
    }

    pub fn targets(&mut self, point: Point) -> Result<Vec<Alternative<'tcx>>, QueryStop> {
        let alias_object = self.alias_object.take();
        let answer = (|| {
            let index = self.index(point.frame)?;
            if let Some(target) = self.bound_target(point) {
                return Ok(vec![Alternative {
                    value: Value::Function(target),
                    guards: Vec::new(),
                }]);
            }
            let (func, args) = match &index.body.basic_blocks[point.location.block]
                .terminator()
                .kind
            {
                TerminatorKind::Call { func, args, .. }
                | TerminatorKind::TailCall { func, args, .. } => (func, args),
                TerminatorKind::InlineAsm { .. } => {
                    return Ok(vec![Alternative {
                        value: Value::Unknown("assembly has no checked Rust call path"),
                        guards: Vec::new(),
                    }]);
                }
                TerminatorKind::Drop { place, .. } => {
                    let ty = self.ty(point.frame, place.ty(index.body, self.tcx).ty);
                    if !matches!(ty.kind(), ty::Dynamic(..)) {
                        return Ok(vec![Alternative {
                            value: Value::Function(Instance::resolve_drop_glue(self.tcx, ty)),
                            guards: Vec::new(),
                        }]);
                    }
                    let mut targets = Vec::new();
                    for pointer in self.address(point, *place, None, None)? {
                        let value = match pointer.value {
                            Value::Pointer(_, Metadata::Concrete(concrete)) => {
                                Value::Function(Instance::resolve_drop_glue(self.tcx, concrete))
                            }
                            _ => Value::Unknown(
                                "the drop receiver has no established vtable binding",
                            ),
                        };
                        targets.push(Alternative {
                            value,
                            guards: pointer.guards,
                        });
                    }
                    return self.bounded(targets);
                }
                TerminatorKind::Assert {
                    cond,
                    expected,
                    msg,
                    ..
                } => {
                    use rustc_hir::attrs::lang_items::LangItem;
                    let condition_ty = self.ty(point.frame, cond.ty(index.body, self.tcx));
                    let item = match &**msg {
                        mir::AssertKind::BoundsCheck { .. } => LangItem::PanicBoundsCheck,
                        mir::AssertKind::MisalignedPointerDereference { .. } => {
                            LangItem::PanicMisalignedPointerDereference
                        }
                        mir::AssertKind::NullPointerDereference => {
                            LangItem::PanicNullPointerDereference
                        }
                        mir::AssertKind::NullReferenceConstructed => {
                            LangItem::PanicNullReferenceConstructed
                        }
                        mir::AssertKind::InvalidEnumConstruction(_) => {
                            LangItem::PanicInvalidEnumConstruction
                        }
                        other => other.panic_function(),
                    };
                    let target = self
                        .tcx
                        .lang_items()
                        .get(item)
                        .map(|def| Instance::mono(self.tcx, def));
                    let mut targets = Vec::new();
                    let optional = self.flow.current.is_none() && !self.controls.solving;
                    let refinement_limit = self.budget.refinement_limit;
                    let scratch = self.budget.scratch;
                    let proving_assertion = self.controls.proving_assertion;
                    if optional {
                        self.controls.proving_assertion = true;
                        let allowance = irq_check_rupta::limits::configuration().expect("validated analysis limits").scalar_steps;
                        let allowance = if self.facts.exclusions.contains(&self.frames[point.frame].instance) {
                            allowance
                        } else { allowance.min(4096) };
                        let allowance = allowance.min(self.budget.limit.saturating_sub(self.budget.steps) / 2);
                        self.budget.refinement_limit = Some(refinement_limit.unwrap_or(usize::MAX)
                            .min(self.budget.steps.saturating_add(allowance)));
                    }
                    let answer = {
                        let refinement_origin = self.refinement_origin;
                        self.refinement_origin.get_or_insert(point);
                        let scalar = if optional { self.scalar_condition(point) } else { Ok(None) };
                        let answer = match scalar {
                            Ok(Some(bits)) => Ok(vec![Alternative { value: Value::Bits(bits), guards: Vec::new() }]),
                            Ok(None) => self.operand(point, cond, 0, condition_ty),
                            Err(reason) => Err(reason),
                        };
                        self.refinement_origin = refinement_origin;
                        answer
                    };
                    self.budget.refinement_limit = refinement_limit;
                    self.budget.scratch = scratch;
                    self.controls.proving_assertion = proving_assertion;
                    let conditions = match answer {
                        Ok(conditions) => conditions,
                        Err(QueryStop::Unsupported | QueryStop::MissingArgument) => vec![Alternative {
                            value: Value::Unknown("the assertion condition is not proved"),
                            guards: Vec::new(),
                        }],
                        Err(reason @ (QueryStop::RefinementLimit | QueryStop::ScratchLimit | QueryStop::AlternativeLimit | QueryStop::ControlMemory))
                            if optional =>
                        {
                            self.controls.causes.insert(point, self.controls.dependency_stop.unwrap_or((point, reason)));
                            self.retain_completed();
                            vec![Alternative {
                                value: Value::Unknown("the assertion condition is not proved"), guards: Vec::new(),
                            }]
                        }
                        Err(reason) => return Err(reason),
                    };
                    for condition in conditions {
                        if matches!(condition.value, Value::Bits(bits) if bits == u128::from(*expected))
                        {
                            continue;
                        }
                        let proof = if matches!(condition.value, Value::Bits(_)) { GuardProof::Established }
                            else { GuardProof::Unresolved };
                        let guard = (point.frame, Guard {
                            required: true, proof, iteration: None, branch: point.location,
                            successor: mir::BasicBlock::from_usize(index.body.basic_blocks.len()),
                            index: None, input: false, phi: false, choice: None,
                        });
                        let Some(guards) = self.merge(&condition.guards, &[guard])? else { continue; };
                        targets.push(Alternative {
                            value: target.map_or(
                                Value::Unknown("the compiler panic entry is absent"),
                                Value::Function,
                            ),
                            guards,
                        });
                    }
                    return self.bounded(targets);
                }
                _ => return Err(QueryStop::Unsupported),
            };
            let function_ty = self.ty(point.frame, func.ty(index.body, self.tcx));
            let mut targets = self.operand(point, func, 0, function_ty)?;
            if let ty::FnDef(def, generic_args) = *function_ty.kind() {
                let generic_args = self.tcx.instantiate_bound_regions_with_erased(generic_args);
                if targets.iter().any(|target| {
                    matches!(
                        target.value,
                        Value::Function(Instance {
                            def: InstanceKind::Virtual(..),
                            ..
                        })
                    )
                }) {
                    targets.clear();
                    let receiver = args.first().ok_or(QueryStop::MissingArgument)?;
                    let receiver_ty = self.ty(point.frame, receiver.node.ty(index.body, self.tcx));
                    let (offset, pointer_ty, _) = self
                        .pointer_projection(receiver_ty, receiver_ty, 0)
                        .ok_or(QueryStop::Unsupported)?;
                    let metadata_ty = Ty::new_imm_ptr(self.tcx, self.tcx.types.unit);
                    let metadata_offset =
                        offset + i128::from(self.tcx.data_layout.pointer_size().bytes());
                    let receivers = if self
                        .tcx
                        .layout_of(TypingEnv::fully_monomorphized().as_query_input(pointer_ty))
                        .is_ok_and(|layout| {
                            layout.size.bytes() == self.tcx.data_layout.pointer_size().bytes() * 2
                        }) {
                        self.operand(point, &receiver.node, metadata_offset, metadata_ty)?
                    } else {
                        self.operand(point, &receiver.node, offset, pointer_ty)?
                    };
                    for receiver in receivers {
                        let target = match receiver.value {
                            Value::Pointer(_, Metadata::Concrete(concrete))
                            | Value::Metadata(Metadata::Concrete(concrete)) => {
                                let args = self.tcx.mk_args_from_iter(
                                    generic_args.iter().enumerate().map(|(index, arg)| {
                                        if index == 0 { concrete.into() } else { arg }
                                    }),
                                );
                                Instance::try_resolve(
                                    self.tcx,
                                    TypingEnv::fully_monomorphized(),
                                    def,
                                    args,
                                )
                                .ok()
                                .flatten()
                                .filter(|target| !matches!(target.def, InstanceKind::Virtual(..)))
                                .map(Value::Function)
                            }
                            _ => None,
                        };
                        targets.push(Alternative {
                            value: target.unwrap_or_else(|| match receiver.value {
                                Value::Unknown(reason) => Value::Unknown(reason),
                                _ => {
                                    Value::Unknown("the receiver has no established vtable binding")
                                }
                            }),
                            guards: receiver.guards,
                        });
                    }
                }
            }
            for target in &mut targets {
                if let Value::Function(instance) = target.value {
                    let Some(callable) = self.linked_target(instance) else {
                        target.value = Value::Unknown("the linked definition does not have a compatible call ABI");
                        continue;
                    };
                    target.value = if function_ty.is_fn_ptr()
                        && crate::forbidden(self.tcx, callable).is_none()
                        && !irq_check_rupta::util::type_util::call_abi_compatible(
                            self.tcx,
                            function_ty.fn_sig(self.tcx),
                            callable,
                        ) {
                        Value::Unknown("the original callable does not have a compatible call ABI")
                    } else {
                        Value::Function(callable)
                    };
                } else if !matches!(target.value, Value::Unknown(_)) {
                    target.value =
                        Value::Unknown("the callable pointer has no established function binding");
                }
            }
            self.bounded(targets)
        })();
        self.alias_object = alias_object;
        answer
    }

    fn loop_call_summary(
        &mut self,
        point: Point,
        requested: Ty<'tcx>,
        effect: Option<Address>,
    ) -> Result<Option<Vec<Alternative<'tcx>>>, QueryStop> {
        if self.controls.loop_fallbacks.is_empty()
            || self.frame_depth[point.frame] >= irq_check_rupta::limits::configuration().expect("validated analysis limits").call_depth
        { return Ok(None); }
        if effect.is_none() && !self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(requested))
            .is_ok_and(|layout| match layout.backend_repr {
                rustc_abi::BackendRepr::Scalar(value) =>
                    !matches!(value.primitive(), rustc_abi::Primitive::Pointer(_)),
                rustc_abi::BackendRepr::ScalarPair { a, b, .. } =>
                    !matches!(a.primitive(), rustc_abi::Primitive::Pointer(_))
                        && !matches!(b.primitive(), rustc_abi::Primitive::Pointer(_)),
                _ => false,
            })
        { return Ok(None); }
        let index = self.index(point.frame)?;
        let instance = self.bound_target(point).or_else(|| {
            let (TerminatorKind::Call { func, .. } | TerminatorKind::TailCall { func, .. }) =
                &index.body.basic_blocks[point.location.block].terminator().kind
            else { return None; };
            let ty::FnDef(def, args) = *self.ty(point.frame, func.ty(index.body, self.tcx)).kind()
                else { return None; };
            Instance::try_resolve(self.tcx, TypingEnv::fully_monomorphized(), def,
                self.tcx.instantiate_bound_regions_with_erased(args)).ok().flatten()
        });
        let (at, cause) = if let Some(instance) = instance {
            if self.excludes_return(point, instance) { return Ok(None); }
            let Some(&cause) = self.controls.loop_fallbacks.get(&(instance, self.frames[point.frame].irq))
                else { return Ok(None); };
            if let Some(address) = effect
                && address.object != Object::Heap(point) && self.preserves_storage(instance, address, requested)?
            { return Ok(Some(vec![Alternative { value: Value::Unchanged, guards: Vec::new() }])); }
            (point, cause)
        } else {
            let site = (self.frames[point.frame].instance, point.location);
            let Some(targets) = self.facts.safe_candidates.get(&site)
                .filter(|targets| !targets.is_empty() && self.facts.covered_calls.contains(&site))
            else { return Ok(None); };
            let Some((_, &cause)) = self.controls.loop_fallbacks.iter()
                .filter(|((instance, irq), _)| *irq == self.frames[point.frame].irq && targets.contains(instance))
                .min_by_key(|(_, (point, _))| (point.frame, point.location.block.as_usize(), point.location.statement_index))
            else { return Ok(None); };
            (point, cause)
        };
        self.controls.causes.insert(at, cause);
        if effect.is_some() {
            self.record_value_loss(cause.0, "the bounded loop summary cannot exclude a change to this storage",
                "the loop summary cannot exclude a write to the requested storage");
        }
        Ok(Some(vec![Alternative {
            value: Value::Unknown(if effect.is_some() {
                "the bounded loop summary cannot exclude a change to this storage"
            } else { "the bounded loop summary does not determine this return value" }),
            guards: vec![(at.frame, Guard { required: false, proof: GuardProof::Widened,
                iteration: None, branch: at.location, successor: mir::START_BLOCK,
                index: None, input: false, phi: true, choice: None })],
        }]))
    }

    fn call_return(
        &mut self,
        point: Point,
        offset: i128,
        requested: Ty<'tcx>,
    ) -> Result<Vec<Alternative<'tcx>>, QueryStop> {
        let scratch = self.budget.scratch;
        let stored = self.flow.bytes;
        let answer = (|| {
        self.budget.charge(0)?;
        if let Some(values) = self.loop_call_summary(point, requested, None)? { return Ok(values); }
        if self.flow.current.is_some() && self.paths(point, true)?.is_empty() {
            return Ok(Vec::new());
        }
        if self.frame_depth[point.frame] >= irq_check_rupta::limits::configuration().expect("validated analysis limits").call_depth {
            return Ok(vec![Alternative {
                value: Value::Unknown("the return binding exceeds the call-depth limit"),
                guards: Vec::new(),
            }]);
        }
        let index = self.index(point.frame)?;
        let (args, destination) = match &index.body.basic_blocks[point.location.block]
            .terminator()
            .kind
        {
            TerminatorKind::Call {
                args, destination, ..
            } => (args, *destination),
            TerminatorKind::TailCall { args, .. } => (args, Place::from(mir::RETURN_PLACE)),
            _ => return Err(QueryStop::Unsupported),
        };
        let result_ty = self.ty(point.frame, destination.ty(index.body, self.tcx).ty);
        let mut result = Vec::new();
        for target in self.targets(point)? {
            let Value::Function(instance) = target.value else {
                result.push(target);
                continue;
            };
            if self.excludes_return(point, instance) { continue; }
            let name = self
                .tcx
                .opt_item_name(instance.def_id())
                .map(|name| name.as_str().to_owned())
                .unwrap_or_default();
            let crate_name = self.tcx.crate_name(instance.def_id().krate);
            let intrinsic = matches!(instance.def, InstanceKind::Intrinsic(_));
            let allocation = self
                .tcx
                .codegen_fn_attrs(instance.def_id())
                .flags
                .intersects(
                rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags::ALLOCATOR
                    | rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags::ALLOCATOR_ZEROED
                    | rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags::REALLOCATOR,
            );
            let box_new = crate_name.as_str() == "alloc"
                && result_ty.is_box()
                && matches!(name.as_str(), "new" | "box_new" | "new_uninit");
            let values = if allocation || box_new {
                vec![Alternative {
                    value: Value::Pointer(
                        Address {
                            object: Object::Heap(point),
                            offset: 0,
                        },
                        Metadata::Thin,
                    ),
                    guards: Vec::new(),
                }]
            } else if intrinsic && name == "is_val_statically_known" && offset == 0 && requested.is_bool() {
                vec![Alternative { value: Value::Bits(0), guards: Vec::new() },
                    Alternative { value: Value::Bits(1), guards: Vec::new() }]
            } else if intrinsic && name == "caller_location" && offset == 0 {
                let mut pending = vec![point];
                let mut visited = HashSet::new();
                let mut values = Vec::new();
                while let Some(call) = pending.pop() {
                    self.budget.charge(0)?;
                    if !visited.insert(call) { continue; }
                    let caller = self.frames[call.frame];
                    let caller_index = self.index(call.frame)?;
                    let source_info = caller_index.body.basic_blocks[call.location.block]
                        .terminator().source_info;
                    let inherited = caller.instance.def.requires_caller_location(self.tcx).then_some(None);
                    if let Some(span) = caller_index.body.caller_location_span(
                        source_info, inherited, self.tcx, Some)
                    {
                        let value = if let Some(&value) = self.caller_locations.get(&span) { value } else {
                            let value = match self.tcx.span_as_caller_location(span) {
                                ConstValue::Scalar(Scalar::Ptr(pointer, _)) => self.constant_pointer(
                                    pointer.provenance.alloc_id(),
                                    i128::from(pointer.prov_and_relative_offset().1.bytes())),
                                _ => Value::Unknown("the tracked caller location has no entry binding"),
                            };
                            self.caller_locations.insert(span, value);
                            value
                        };
                        values.push(Alternative { value, guards: Vec::new() });
                        continue;
                    }
                    let parents: Vec<_> = self.flow.inputs.get(&call.frame).into_iter().flatten().enumerate()
                        .filter(|(input, _)| self.input_choices.get(&call.frame).is_none_or(|selected| *selected == *input))
                        .map(|(_, parent)| *parent).collect();
                    if parents.is_empty() || self.flow.roots.contains(&call.frame) {
                        values.push(Alternative {
                            value: Value::Unknown("the tracked caller location has no entry binding"),
                            guards: Vec::new(),
                        });
                    }
                    pending.extend(parents);
                }
                self.bounded(values)?
            } else if intrinsic && matches!(name.as_str(), "black_box" | "likely" | "unlikely") {
                let argument = args.first().ok_or(QueryStop::MissingArgument)?;
                self.operand(point, &argument.node, offset, requested)?
            } else if intrinsic && name == "select_unpredictable" && args.len() == 3 {
                let mut values = Vec::new();
                let condition_ty = self.ty(point.frame, args[0].node.ty(index.body, self.tcx));
                for condition in self.operand(point, &args[0].node, 0, condition_ty)? {
                    for (choice, argument) in [(1, &args[1]), (0, &args[2])] {
                        if matches!(condition.value, Value::Bits(value) if value != choice) {
                            continue;
                        }
                        for value in self.operand(point, &argument.node, offset, requested)? {
                            if let Some(guards) = self.merge(&condition.guards, &value.guards)? {
                                values.push(Alternative {
                                    value: value.value,
                                    guards,
                                });
                            }
                        }
                    }
                }
                values
            } else if intrinsic && offset == 0
                && matches!(
                    name.as_str(),
                    "ctpop" | "ctlz" | "ctlz_nonzero" | "cttz" | "cttz_nonzero"
                        | "bswap" | "bitreverse"
                )
            {
                let argument = args.first().ok_or(QueryStop::MissingArgument)?;
                let argument_ty = self.ty(point.frame, argument.node.ty(index.body, self.tcx));
                let width = self
                    .tcx
                    .layout_of(TypingEnv::fully_monomorphized().as_query_input(argument_ty))
                    .map_err(|_| QueryStop::Unsupported)?
                    .size
                    .bits();
                let mut values = self.operand(point, &argument.node, 0, argument_ty)?;
                for value in &mut values {
                    value.value = match value.value {
                        Value::Bits(bits) if width > 0 && width <= 128 => {
                            let bits = bits & (u128::MAX >> (128 - width));
                            let bits = match name.as_str() {
                                "ctpop" => Some(u128::from(bits.count_ones())),
                                "ctlz" | "ctlz_nonzero"
                                    if bits != 0 || name == "ctlz" =>
                                {
                                    Some(u128::from(bits.leading_zeros()) - (128 - u128::from(width)))
                                }
                                "cttz" | "cttz_nonzero"
                                    if bits != 0 || name == "cttz" =>
                                {
                                    Some(u128::from(bits.trailing_zeros()).min(u128::from(width)))
                                }
                                "bswap" if width % 8 == 0 => Some(bits.swap_bytes() >> (128 - width)),
                                "bitreverse" => Some(bits.reverse_bits() >> (128 - width)),
                                _ => None,
                            };
                            bits.map_or(Value::Unknown("the integer intrinsic input is invalid"), Value::Bits)
                        }
                        _ => Value::Unknown("the integer intrinsic input is unknown"),
                    };
                }
                values
            } else if intrinsic
                && matches!(name.as_str(), "size_of" | "min_align_of" | "pref_align_of")
            {
                let ty = instance.args.types().next().ok_or(QueryStop::Unsupported)?;
                let layout = self
                    .tcx
                    .layout_of(TypingEnv::fully_monomorphized().as_query_input(ty))
                    .map_err(|_| QueryStop::Unsupported)?;
                vec![Alternative {
                    value: Value::Bits(u128::from(if name == "size_of" {
                        layout.size.bytes()
                    } else {
                        layout.align.abi.bytes()
                    })),
                    guards: Vec::new(),
                }]
            } else if intrinsic
                && matches!(
                    name.as_str(),
                    "read_via_copy"
                        | "volatile_load"
                        | "unaligned_volatile_load"
                        | "atomic_load"
                        | "atomic_xchg"
                        | "atomic_cxchg"
                        | "atomic_cxchgweak"
                        | "atomic_xadd"
                        | "atomic_xsub"
                        | "atomic_and"
                        | "atomic_nand"
                        | "atomic_or"
                        | "atomic_xor"
                        | "atomic_max"
                        | "atomic_min"
                        | "atomic_umax"
                        | "atomic_umin"
                )
            {
                let argument = args.first().ok_or(QueryStop::MissingArgument)?;
                let argument_ty = self.ty(point.frame, argument.node.ty(index.body, self.tcx));
                let mut values = Vec::new();
                let compare = matches!(name.as_str(), "atomic_cxchg" | "atomic_cxchgweak");
                let result_layout = self
                    .tcx
                    .layout_of(TypingEnv::fully_monomorphized().as_query_input(result_ty))
                    .map_err(|_| QueryStop::Unsupported)?;
                if compare
                    && offset == i128::from(result_layout.fields.offset(1).bytes())
                    && requested.is_bool()
                {
                    values.extend([
                        Alternative {
                            value: Value::Bits(0),
                            guards: Vec::new(),
                        },
                        Alternative {
                            value: Value::Bits(1),
                            guards: Vec::new(),
                        },
                    ]);
                } else {
                    for pointer in self.operand(point, &argument.node, 0, argument_ty)? {
                        if let Value::Pointer(mut address, _) = pointer.value {
                            address.offset = address
                                .offset
                                .checked_add(offset)
                                .ok_or(QueryStop::Unsupported)?;
                            for value in self.read_guarded(address, requested, point, &pointer.guards)? {
                                if let Some(guards) = self.merge(&pointer.guards, &value.guards)? {
                                    values.push(Alternative {
                                        value: value.value,
                                        guards,
                                    });
                                }
                            }
                        } else {
                            values.push(Alternative {
                                value: Value::Unknown("the load address is unknown"),
                                guards: pointer.guards,
                            });
                        }
                    }
                }
                values
            } else if intrinsic
                && matches!(name.as_str(), "arith_offset" | "offset")
                && args.len() == 2
            {
                self.rvalue(
                    point,
                    &Rvalue::BinaryOp(
                        mir::BinOp::Offset,
                        Box::new((args[0].node.clone(), args[1].node.clone())),
                    ),
                    result_ty,
                    offset,
                    requested,
                )?
            } else if intrinsic && name == "transmute" && args.len() == 1 {
                self.operand(point, &args[0].node, offset, requested)?
            } else if self.tcx.is_foreign_item(instance.def_id())
                || matches!(
                    instance.def,
                    InstanceKind::Intrinsic(_)
                        | InstanceKind::LlvmIntrinsic(_)
                        | InstanceKind::Virtual(..)
                )
                || !irq_check_rupta::util::type_util::has_instance_mir(self.tcx, instance.def)
            {
                vec![Alternative {
                    value: Value::Unknown("the return value crosses an unsupported call boundary"),
                    guards: Vec::new(),
                }]
            } else if let Some(summary) = self.effect_summary(instance, self.frames[point.frame].irq)?
                && let Some(source) = &summary.returned
                && source.valid(self, point, args, index.body)
                && source.ty() == result_ty
                && source.covers(self.tcx, offset, requested, false)
            {
                self.effects.returned += 1;
                source.values(self, point, args, index.body, offset, requested)?
            } else {
                let child = self.frame(Frame {
                    instance,
                    caller: Some(point),
                    irq: self.frames[point.frame].irq,
                    unknown_entry: false,
                });
                self.with_input(child, point, |solver| {
                let child_index = solver.index(child)?;
                if let Some(&cause) = solver.controls.loop_fallbacks.get(&(instance, solver.frames[child].irq))
                    && solver.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(requested))
                        .is_ok_and(|layout| match layout.backend_repr {
                            rustc_abi::BackendRepr::Scalar(value) =>
                                !matches!(value.primitive(), rustc_abi::Primitive::Pointer(_)),
                            rustc_abi::BackendRepr::ScalarPair { a, b, .. } =>
                                !matches!(a.primitive(), rustc_abi::Primitive::Pointer(_))
                                    && !matches!(b.primitive(), rustc_abi::Primitive::Pointer(_)),
                            _ => false,
                        })
                {
                    let at = Point { frame: child, location: Location { block: mir::START_BLOCK, statement_index: 0 } };
                    solver.controls.causes.insert(at, cause);
                    return Ok(vec![Alternative {
                        value: Value::Unknown("the bounded loop summary does not determine this return value"),
                        guards: vec![(child, Guard { required: false, proof: GuardProof::Widened,
                            iteration: None, branch: at.location, successor: mir::START_BLOCK,
                            index: None, input: false, phi: true, choice: None })],
                    }]);
                }
                let mut values = Vec::new();
                for block in child_index
                    .body
                    .basic_blocks
                    .indices()
                    .filter(|block| child_index.reachable.contains(block))
                {
                    let data = &child_index.body.basic_blocks[block];
                    if child_index.stops[block.as_usize()].is_none()
                        && matches!(data.terminator().kind, TerminatorKind::Return)
                    {
                        let at = Point {
                            frame: child,
                            location: Location {
                                block,
                                statement_index: data.statements.len(),
                            },
                        };
                        let returned = solver.read(
                            Address {
                                object: Object::Local(child, mir::RETURN_PLACE),
                                offset,
                            },
                            requested,
                            at,
                            false,
                        )?;
                        values.extend(solver.exit_values(at, point, returned, false)?);
                    } else if child_index.stops[block.as_usize()].is_none()
                        && matches!(data.terminator().kind, TerminatorKind::TailCall { .. })
                    {
                        let at = Point {
                            frame: child,
                            location: Location {
                                block,
                                statement_index: data.statements.len(),
                            },
                        };
                        let returned = solver.read(Address {
                            object: Object::Local(child, mir::RETURN_PLACE), offset,
                        }, requested, Point { location: Location {
                            statement_index: at.location.statement_index + 1, ..at.location
                        }, ..at }, false)?;
                        values.extend(solver.exit_values(at, point, returned, false)?);
                    }
                }
                Ok(values)
                })?
            };
            for value in values {
                if let Some(guards) = self.merge(&target.guards, &value.guards)? {
                    result.push(Alternative {
                        value: value.value,
                        guards,
                    });
                }
            }
        }
        self.bounded(result)
        })();
        let retained = answer.as_ref().map_or(0, |values|
            values.capacity() * std::mem::size_of::<Alternative<'tcx>>() * 4
                + values.iter().map(|value| value.guards.capacity() * std::mem::size_of::<(usize, Guard)>() * 4).sum::<usize>());
        let live = scratch.saturating_add(self.flow.bytes.saturating_sub(stored));
        if self.budget.scratch.saturating_sub(live.saturating_add(retained)) >= 1024 * 1024
            && std::env::var_os("IRQ_CHECK_TRACE_FLOW_COST").is_some()
        {
            eprintln!("irq-check: return scratch release {} at={:?} charged={} outer={scratch} flow-growth={} result={retained} stop={:?}",
                self.frames[point.frame].instance, point.location, self.budget.scratch,
                self.flow.bytes.saturating_sub(stored), answer.as_ref().err());
        }
        self.budget.scratch = live;
        if answer.is_ok() { self.budget.reserve(retained)?; }
        answer
    }

    fn copy_effect(
        &mut self,
        point: Point,
        source: &Operand<'tcx>,
        destination: &Operand<'tcx>,
        count: Option<&Operand<'tcx>>,
        address: Address,
        requested: Ty<'tcx>,
    ) -> Result<Vec<Alternative<'tcx>>, QueryStop> {
        let index = self.index(point.frame)?;
        let source_ty = self.ty(point.frame, source.ty(index.body, self.tcx));
        let destination_ty = self.ty(point.frame, destination.ty(index.body, self.tcx));
        let element = source_ty
            .builtin_deref(true)
            .ok_or(QueryStop::Unsupported)?;
        let stride = self
            .tcx
            .layout_of(TypingEnv::fully_monomorphized().as_query_input(element))
            .map_err(|_| QueryStop::Unsupported)?
            .size
            .bytes();
        let size = self
            .tcx
            .layout_of(TypingEnv::fully_monomorphized().as_query_input(requested))
            .map_err(|_| QueryStop::Unsupported)?
            .size
            .bytes();
        let destinations = self.aliases(point, destination, destination_ty, address.object)?;
        if destinations.iter().all(|value| {
            matches!(value.value, Value::Pointer(target, _) if target.object != address.object)
                || matches!(value.value, Value::ObjectPointer(object) if object != address.object)
        }) {
            return Ok(destinations
                .into_iter()
                .map(|value| Alternative {
                    value: Value::Unchanged,
                    guards: value.guards,
                })
                .collect());
        }
        let sources = self.operand(point, source, 0, source_ty)?;
        let counts = if let Some(count) = count {
            let count_ty = self.ty(point.frame, count.ty(index.body, self.tcx));
            self.operand(point, count, 0, count_ty)?
        } else {
            vec![Alternative { value: Value::Bits(1), guards: Vec::new() }]
        };
        let mut result = Vec::new();
        for destination in destinations {
            for count in &counts {
                let Some(guards) = self.merge(&destination.guards, &count.guards)? else {
                    continue;
                };
                match (destination.value, count.value) {
                    (_, Value::Bits(0)) => result.push(Alternative {
                        value: Value::Unchanged,
                        guards,
                    }),
                    (Value::Pointer(target, _), _) if target.object != address.object => result
                        .push(Alternative {
                            value: Value::Unchanged,
                            guards,
                        }),
                    (Value::ObjectPointer(object), _) if object != address.object => {
                        result.push(Alternative {
                            value: Value::Unchanged,
                            guards,
                        })
                    }
                    (Value::Pointer(target, _), Value::Bits(count)) => {
                        let end = i128::try_from(count)
                            .ok()
                            .and_then(|count| count.checked_mul(i128::from(stride)))
                            .and_then(|bytes| target.offset.checked_add(bytes))
                            .ok_or(QueryStop::Unsupported)?;
                        if address.offset >= end
                            || address.offset + i128::from(size) <= target.offset
                        {
                            result.push(Alternative {
                                value: Value::Unchanged,
                                guards,
                            });
                            continue;
                        }
                        if address.offset < target.offset || address.offset + i128::from(size) > end
                        {
                            result.push(Alternative {
                                value: Value::Unknown(
                                    "a memory copy covers only part of the pointer",
                                ),
                                guards,
                            });
                            continue;
                        }
                        for source in &sources {
                            let Some(guards) = self.merge(&guards, &source.guards)? else {
                                continue;
                            };
                            if let Value::Pointer(mut origin, _) = source.value {
                                origin.offset = origin
                                    .offset
                                    .checked_add(address.offset - target.offset)
                                    .ok_or(QueryStop::Unsupported)?;
                                for value in self.read_guarded(origin, requested, point, &guards)? {
                                    if let Some(guards) = self.merge(&guards, &value.guards)? {
                                        result.push(Alternative {
                                            value: value.value,
                                            guards,
                                        });
                                    }
                                }
                            } else {
                                result.push(Alternative {
                                    value: Value::Unknown("the copy source address is unknown"),
                                    guards,
                                });
                            }
                        }
                    }
                    _ => result.push(Alternative {
                        value: Value::Unknown("the copy range is unknown"),
                        guards,
                    }),
                }
            }
        }
        self.bounded(result)
    }

    fn statement_effect(
        &mut self,
        point: Point,
        address: Address,
        requested: Ty<'tcx>,
    ) -> Result<Vec<Alternative<'tcx>>, QueryStop> {
        let index = self.index(point.frame)?;
        let statement = &index.body.basic_blocks[point.location.block].statements
            [point.location.statement_index];
        if let StatementKind::Intrinsic(intrinsic) = &statement.kind {
            if let mir::NonDivergingIntrinsic::CopyNonOverlapping(copy) = &**intrinsic {
                return self.copy_effect(
                    point,
                    &copy.src,
                    &copy.dst,
                    Some(&copy.count),
                    address,
                    requested,
                );
            }
        }
        Ok(vec![Alternative {
            value: Value::Unchanged,
            guards: Vec::new(),
        }])
    }


    fn preserves_storage(&mut self, instance: Instance<'tcx>, address: Address, requested: Ty<'tcx>) -> Result<bool, QueryStop> {
        let objects = self.pointer_objects(address.object);
        let read_range = self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(requested))
            .ok().filter(|layout| layout.is_sized()).and_then(|layout| {
                let start = u64::try_from(address.offset).ok()?;
                Some((start, start.checked_add(layout.size.bytes())?))
            });
        if objects.is_empty() { return Ok(false); }
        for object in objects {
            match self.write_impact(instance, object, read_range) {
                Ok(impact::WriteImpact::Preserved) => {}
                Ok(_) | Err(QueryStop::RefinementLimit) => return Ok(false),
                Err(stop) => return Err(stop),
            }
        }
        Ok(true)
    }

    fn call_effect(
        &mut self,
        point: Point,
        address: Address,
        requested: Ty<'tcx>,
        unwind: bool,
    ) -> Result<Vec<Alternative<'tcx>>, QueryStop> {
        self.budget.charge(0)?;
        if self.frame_depth[point.frame] >= irq_check_rupta::limits::configuration().expect("validated analysis limits").call_depth {
            return Ok(vec![Alternative {
                value: Value::Unknown("the storage effect exceeds the call-depth limit"),
                guards: Vec::new(),
            }]);
        }
        let index = self.index(point.frame)?;
        if !unwind && matches!(
            index.body.basic_blocks[point.location.block]
                .terminator()
                .kind,
            TerminatorKind::Assert { .. }
        ) {
            return Ok(vec![Alternative {
                value: Value::Unchanged,
                guards: Vec::new(),
            }]);
        }
        let args = match &index.body.basic_blocks[point.location.block]
            .terminator()
            .kind
        {
            TerminatorKind::Call { args, .. } | TerminatorKind::TailCall { args, .. } => {
                args.as_ref()
            }
            TerminatorKind::Drop { .. } | TerminatorKind::Assert { .. } => &[],
            _ => return Err(QueryStop::Unsupported),
        };
        let mut result = Vec::new();
        let site = (self.frames[point.frame].instance, point.location);
        if !self.cut_recursive_returns && address.object != Object::Heap(point) && self.facts.covered_calls.contains(&site)
            && let Some(targets) = self.facts.safe_candidates.get(&site).cloned().filter(|targets| !targets.is_empty())
        {
            let mut preserved = true;
            for target in targets {
                if !self.preserves_storage(target, address, requested)? { preserved = false; break; }
            }
            if preserved { return Ok(vec![Alternative { value: Value::Unchanged, guards: Vec::new() }]); }
        }
        if !self.cut_recursive_returns && address.object != Object::Heap(point)
            && let TerminatorKind::Call { func, .. } | TerminatorKind::TailCall { func, .. } =
                &index.body.basic_blocks[point.location.block].terminator().kind
            && let ty::FnDef(def, substitutions) = *self.ty(point.frame, func.ty(index.body, self.tcx)).kind()
            && let Ok(Some(instance)) = Instance::try_resolve(self.tcx, TypingEnv::fully_monomorphized(), def,
                self.tcx.instantiate_bound_regions_with_erased(substitutions))
            && let Some(instance) = self.linked_target(instance)
            && (unwind || !self.excludes_return(point, instance))
            && self.read_only(instance, !unwind)?
        {
            return Ok(vec![Alternative { value: Value::Unchanged, guards: Vec::new() }]);
        }
        if !unwind && let Some(values) = self.loop_call_summary(point, requested, Some(address))? {
            return Ok(values);
        }
        if self.flow.current.is_some() && self.paths(point, true)?.is_empty() {
            return Ok(Vec::new());
        }
        let targets = self.targets(point)?;
        for target in targets {
            let Value::Function(instance) = target.value else {
                result.push(Alternative {
                    value: match target.value {
                        Value::Unknown(reason) => Value::Unknown(reason),
                        _ => Value::Unknown("an unknown call can change this storage"),
                    },
                    guards: target.guards,
                });
                continue;
            };
            if !unwind && self.excludes_return(point, instance) { continue; }
            if address.object != Object::Heap(point) && self.preserves_storage(instance, address, requested)? {
                result.push(Alternative { value: Value::Unchanged, guards: target.guards });
                continue;
            }
            let name = self
                .tcx
                .opt_item_name(instance.def_id())
                .map(|name| name.as_str().to_owned())
                .unwrap_or_default();
            let intrinsic = matches!(instance.def, InstanceKind::Intrinsic(_));
            let flags = self.tcx.codegen_fn_attrs(instance.def_id()).flags;
            let allocation = flags.intersects(
                rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags::ALLOCATOR
                    | rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags::ALLOCATOR_ZEROED
                    | rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags::REALLOCATOR,
            );
            let box_new = self.tcx.crate_name(instance.def_id().krate).as_str() == "alloc"
                && matches!(name.as_str(), "new" | "box_new" | "new_uninit")
                && self
                    .tcx
                    .fn_sig(instance.def_id())
                    .instantiate(self.tcx, instance.args)
                    .skip_binder()
                    .output()
                    .is_box();
            let values = if address.object == Object::Heap(point) && (allocation || box_new) {
                if box_new && !args.is_empty() {
                    self.operand(point, &args[0].node, address.offset, requested)?
                } else if flags.contains(
                    rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags::REALLOCATOR,
                ) && args.len() == 4
                {
                    let pointer_ty = self.ty(point.frame, args[0].node.ty(index.body, self.tcx));
                    let old_size_ty = self.ty(point.frame, args[1].node.ty(index.body, self.tcx));
                    let new_size_ty = self.ty(point.frame, args[3].node.ty(index.body, self.tcx));
                    let old_sizes = self.operand(point, &args[1].node, 0, old_size_ty)?;
                    let new_sizes = self.operand(point, &args[3].node, 0, new_size_ty)?;
                    let size = self
                        .tcx
                        .layout_of(TypingEnv::fully_monomorphized().as_query_input(requested))
                        .map_err(|_| QueryStop::Unsupported)?
                        .size
                        .bytes();
                    let mut values = Vec::new();
                    for pointer in self.operand(point, &args[0].node, 0, pointer_ty)? {
                        for old_size in &old_sizes {
                            let Some(guards) = self.merge(&pointer.guards, &old_size.guards)?
                            else {
                                continue;
                            };
                            for new_size in &new_sizes {
                                let Some(guards) = self.merge(&guards, &new_size.guards)? else {
                                    continue;
                                };
                                if let (
                                    Value::Pointer(mut source, _),
                                    Value::Bits(old),
                                    Value::Bits(new),
                                ) = (pointer.value, old_size.value, new_size.value)
                                {
                                    if address.offset >= 0
                                        && (address.offset as u128)
                                            .checked_add(u128::from(size))
                                            .is_some_and(|end| end <= old && end <= new)
                                    {
                                        source.offset = source
                                            .offset
                                            .checked_add(address.offset)
                                            .ok_or(QueryStop::Unsupported)?;
                                        for value in self.read_guarded(source, requested, point, &guards)? {
                                            if let Some(guards) =
                                                self.merge(&guards, &value.guards)?
                                            {
                                                values.push(Alternative {
                                                    value: value.value,
                                                    guards,
                                                });
                                            }
                                        }
                                        continue;
                                    }
                                }
                                values.push(Alternative {
                                    value: Value::Unknown(
                                        "the reallocation does not establish these bytes",
                                    ),
                                    guards,
                                });
                            }
                        }
                    }
                    values
                } else if flags.contains(
                    rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags::ALLOCATOR_ZEROED,
                ) {
                    vec![Alternative {
                        value: Value::Bits(0),
                        guards: Vec::new(),
                    }]
                } else {
                    vec![Alternative {
                        value: Value::Unknown("the allocation bytes are not initialized"),
                        guards: Vec::new(),
                    }]
                }
            } else if crate::known_intrinsic(self.tcx, instance)
                || self.read_only(instance, !unwind)?
            {
                vec![Alternative {
                    value: Value::Unchanged,
                    guards: Vec::new(),
                }]
            } else if intrinsic && name == "typed_swap_nonoverlapping" && args.len() == 2 {
                let left = self.copy_effect(point, &args[1].node, &args[0].node, None, address, requested)?;
                let right = self.copy_effect(point, &args[0].node, &args[1].node, None, address, requested)?;
                let mut values = Vec::new();
                for left in left {
                    for right in &right {
                        let Some(guards) = self.merge(&left.guards, &right.guards)? else { continue; };
                        let value = match (left.value, right.value) {
                            (Value::Unchanged, value) | (value, Value::Unchanged) => value,
                            (left, right) if left == right => left,
                            _ => Value::Unknown("the swap does not establish a complete storage value"),
                        };
                        values.push(Alternative { value, guards });
                    }
                }
                self.bounded(values)?
            } else if intrinsic
                && matches!(name.as_str(), "copy" | "copy_nonoverlapping")
                && args.len() == 3
            {
                self.copy_effect(
                    point,
                    &args[0].node,
                    &args[1].node,
                    Some(&args[2].node),
                    address,
                    requested,
                )?
            } else if intrinsic
                && matches!(
                    name.as_str(),
                    "volatile_copy_memory" | "volatile_copy_nonoverlapping_memory"
                )
                && args.len() == 3
            {
                self.copy_effect(
                    point,
                    &args[1].node,
                    &args[0].node,
                    Some(&args[2].node),
                    address,
                    requested,
                )?
            } else if intrinsic
                && matches!(name.as_str(), "write_bytes" | "volatile_set_memory")
                && args.len() == 3
            {
                let pointer_ty = self.ty(point.frame, args[0].node.ty(index.body, self.tcx));
                let byte_ty = self.ty(point.frame, args[1].node.ty(index.body, self.tcx));
                let count_ty = self.ty(point.frame, args[2].node.ty(index.body, self.tcx));
                let element = pointer_ty
                    .builtin_deref(true)
                    .ok_or(QueryStop::Unsupported)?;
                let stride = self
                    .tcx
                    .layout_of(TypingEnv::fully_monomorphized().as_query_input(element))
                    .map_err(|_| QueryStop::Unsupported)?
                    .size
                    .bytes();
                let size = self
                    .tcx
                    .layout_of(TypingEnv::fully_monomorphized().as_query_input(requested))
                    .map_err(|_| QueryStop::Unsupported)?
                    .size
                    .bytes();
                let counts = self.operand(point, &args[2].node, 0, count_ty)?;
                let bytes = self.operand(point, &args[1].node, 0, byte_ty)?;
                let mut values = Vec::new();
                for pointer in self.aliases(point, &args[0].node, pointer_ty, address.object)? {
                    for count in &counts {
                        let Some(guards) = self.merge(&pointer.guards, &count.guards)? else {
                            continue;
                        };
                        let length = match count.value {
                            Value::Bits(bits) => bits
                                .checked_mul(stride.into())
                                .and_then(|bytes| i128::try_from(bytes).ok()),
                            _ => None,
                        };
                        let value = match (pointer.value, length) {
                            (_, Some(0)) => Some(Value::Unchanged),
                            (Value::Pointer(destination, _), _)
                                if destination.object != address.object =>
                            {
                                Some(Value::Unchanged)
                            }
                            (Value::ObjectPointer(object), _) if object != address.object => {
                                Some(Value::Unchanged)
                            }
                            (Value::Pointer(destination, _), Some(length)) => {
                                let end = destination
                                    .offset
                                    .checked_add(length)
                                    .ok_or(QueryStop::Unsupported)?;
                                if address.offset >= end
                                    || address.offset + i128::from(size) <= destination.offset
                                {
                                    Some(Value::Unchanged)
                                } else if address.offset >= destination.offset
                                    && address.offset + i128::from(size) <= end
                                    && size <= 16
                                {
                                    None
                                } else {
                                    Some(Value::Unknown(
                                        "the byte write overlaps part of the requested value",
                                    ))
                                }
                            }
                            _ => Some(Value::Unknown(
                                "the byte write has an unknown alias or range",
                            )),
                        };
                        if let Some(value) = value {
                            values.push(Alternative { value, guards });
                        } else {
                            for byte in &bytes {
                                if let Some(guards) = self.merge(&guards, &byte.guards)? {
                                    let value = match byte.value {
                                        Value::Bits(byte) if byte <= 255 => Value::Bits(
                                            (0..size)
                                                .fold(0, |bits, shift| bits | byte << (shift * 8)),
                                        ),
                                        _ => Value::Unknown("the repeated byte is unknown"),
                                    };
                                    values.push(Alternative { value, guards });
                                }
                            }
                        }
                    }
                }
                values
            } else if intrinsic
                && matches!(
                    name.as_str(),
                    "write_via_move"
                        | "volatile_store"
                        | "unaligned_volatile_store"
                        | "atomic_store"
                        | "atomic_xchg"
                        | "atomic_cxchg"
                        | "atomic_cxchgweak"
                        | "atomic_xadd"
                        | "atomic_xsub"
                        | "atomic_and"
                        | "atomic_nand"
                        | "atomic_or"
                        | "atomic_xor"
                        | "atomic_max"
                        | "atomic_min"
                        | "atomic_umax"
                        | "atomic_umin"
                )
                && args.len() >= 2
            {
                let pointer_ty = self.ty(point.frame, args[0].node.ty(index.body, self.tcx));
                let written_ty = self.ty(point.frame, args[1].node.ty(index.body, self.tcx));
                let size = self
                    .tcx
                    .layout_of(TypingEnv::fully_monomorphized().as_query_input(written_ty))
                    .map_err(|_| QueryStop::Unsupported)?
                    .size
                    .bytes();
                let requested_size = self
                    .tcx
                    .layout_of(TypingEnv::fully_monomorphized().as_query_input(requested))
                    .map_err(|_| QueryStop::Unsupported)?
                    .size
                    .bytes();
                let mut values = Vec::new();
                for pointer in self.aliases(point, &args[0].node, pointer_ty, address.object)? {
                    match pointer.value {
                        Value::Pointer(destination, _) if destination.object != address.object => {
                            values.push(Alternative {
                                value: Value::Unchanged,
                                guards: pointer.guards,
                            })
                        }
                        Value::ObjectPointer(object) if object != address.object => {
                            values.push(Alternative {
                                value: Value::Unchanged,
                                guards: pointer.guards,
                            })
                        }
                        Value::Pointer(destination, _)
                            if address.offset >= destination.offset
                                && address.offset + i128::from(requested_size)
                                    <= destination.offset + i128::from(size) =>
                        {
                            let compare =
                                matches!(name.as_str(), "atomic_cxchg" | "atomic_cxchgweak");
                            let replacement = if compare {
                                args.get(2).ok_or(QueryStop::MissingArgument)?
                            } else {
                                &args[1]
                            };
                            let mut replacements = if matches!(
                                name.as_str(),
                                "atomic_xadd"
                                    | "atomic_xsub"
                                    | "atomic_and"
                                    | "atomic_nand"
                                    | "atomic_or"
                                    | "atomic_xor"
                                    | "atomic_max"
                                    | "atomic_min"
                                    | "atomic_umax"
                                    | "atomic_umin"
                            ) {
                                vec![Alternative {
                                    value: Value::Unknown(
                                        "the atomic arithmetic changes the pointer bytes",
                                    ),
                                    guards: Vec::new(),
                                }]
                            } else {
                                self.operand(
                                    point,
                                    &replacement.node,
                                    address.offset - destination.offset,
                                    requested,
                                )?
                            };
                            if compare {
                                replacements.push(Alternative {
                                    value: Value::Unchanged,
                                    guards: Vec::new(),
                                });
                            }
                            for value in replacements {
                                if let Some(guards) = self.merge(&pointer.guards, &value.guards)? {
                                    values.push(Alternative {
                                        value: value.value,
                                        guards,
                                    });
                                }
                            }
                        }
                        Value::Pointer(destination, _)
                            if address.offset >= destination.offset + i128::from(size)
                                || address.offset + i128::from(requested_size)
                                    <= destination.offset =>
                        {
                            values.push(Alternative {
                                value: Value::Unchanged,
                                guards: pointer.guards,
                            })
                        }
                        _ => values.push(Alternative {
                            value: Value::Unknown(
                                "the intrinsic write has an unknown alias or range",
                            ),
                            guards: pointer.guards,
                        }),
                    }
                }
                values
            } else if intrinsic
                && super::intrinsic_effect(self.tcx, instance) == super::IntrinsicEffect::ReadOnly
            {
                vec![Alternative {
                    value: Value::Unchanged,
                    guards: Vec::new(),
                }]
            } else if self.tcx.is_foreign_item(instance.def_id())
                || matches!(
                    instance.def,
                    InstanceKind::Intrinsic(_)
                        | InstanceKind::LlvmIntrinsic(_)
                        | InstanceKind::Virtual(..)
                )
                || !irq_check_rupta::util::type_util::has_instance_mir(self.tcx, instance.def)
            {
                vec![Alternative {
                    value: Value::Unknown("the call has an unsupported storage effect"),
                    guards: Vec::new(),
                }]
            } else if !unwind && let Some(summary) = self.effect_summary(instance, self.frames[point.frame].irq)?
                && summary.stores.iter().all(|store| store.source.valid(self, point, args, index.body)
                    && store.destination.operand(self, point, args, index.body).is_some()
                    && store.source.covers(self.tcx, 0, store.source.ty(), true)
                    && store.conditions.iter().all(|(source, _, _)| source.valid(self, point, args, index.body)
                        && source.covers(self.tcx, 0, source.ty(), false)))
            {
                let requested_size = self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(requested))
                    .map_err(|_| QueryStop::Unsupported)?.size.bytes();
                let singleton = self.singleton_object(address.object)?;
                self.effects.writes += summary.stores.len();
                let mut values = vec![Alternative { value: Value::Unchanged, guards: Vec::new() }];
                for store in &summary.stores {
                    let destination = store.destination.operand(self, point, args, index.body).expect("checked summary destination");
                    let pointer_ty = self.ty(point.frame, destination.ty(index.body, self.tcx));
                    let pointers = self.aliases(point, destination, pointer_ty, address.object)?;
                    let disjoint = !pointers.is_empty() && pointers.iter().all(|pointer| match pointer.value {
                        Value::ObjectPointer(object) => object != address.object,
                        Value::Pointer(target, _) if target.object != address.object => true,
                        Value::Pointer(target, _) => target.offset.checked_add(store.offset).is_some_and(|start|
                            start.checked_add(i128::from(store.bytes)).is_some_and(|end| end <= address.offset)
                                || address.offset.checked_add(i128::from(requested_size)).is_some_and(|end| end <= start)),
                        _ => false,
                    });
                    let writes = if disjoint {
                        pointers.into_iter().map(|pointer| Alternative { value: Value::Unchanged, guards: pointer.guards }).collect()
                    } else {
                    let conditions = effects::Source::conditions(&store.conditions, self, point, args, index.body)?;
                    let mut writes = Vec::new();
                    let pointers = if conditions.iter().any(|condition| condition.value != Value::Bits(0)) {
                        pointers
                    } else { Vec::new() };
                    for pointer in pointers {
                        match pointer.value {
                            Value::Pointer(mut target, _) => {
                                target.offset = target.offset.checked_add(store.offset).ok_or(QueryStop::Unsupported)?;
                                if target.object != address.object || target.offset + i128::from(store.bytes) <= address.offset
                                    || address.offset + i128::from(requested_size) <= target.offset {
                                    writes.push(Alternative { value: Value::Unchanged, guards: pointer.guards });
                                } else if target.offset <= address.offset
                                    && address.offset + i128::from(requested_size) <= target.offset + i128::from(store.bytes) {
                                    for value in store.source.values(self, point, args, index.body, address.offset - target.offset, requested)? {
                                        if let Some(guards) = self.merge(&pointer.guards, &value.guards)? {
                                            writes.push(Alternative { value: value.value, guards });
                                        }
                                    }
                                    if !singleton { writes.push(Alternative { value: Value::Unchanged, guards: pointer.guards }); }
                                } else {
                                    writes.push(Alternative { value: Value::Unknown("a summary write overlaps part of the requested storage"), guards: pointer.guards });
                                }
                            }
                            Value::ObjectPointer(object) if object != address.object => {
                                writes.push(Alternative { value: Value::Unchanged, guards: pointer.guards });
                            }
                            _ => writes.push(Alternative { value: Value::Unknown("a summary write has an unknown storage alias"), guards: pointer.guards }),
                        }
                    }
                    let mut conditional = Vec::new();
                    for condition in conditions {
                        if condition.value != Value::Bits(1) {
                            conditional.push(Alternative { value: Value::Unchanged, guards: condition.guards.clone() });
                        }
                        if condition.value != Value::Bits(0) {
                            for write in &writes {
                                if let Some(guards) = self.merge(&condition.guards, &write.guards)? {
                                    conditional.push(Alternative { value: write.value, guards });
                                }
                            }
                        }
                    }
                    self.bounded(conditional)?
                    };
                    let mut joined = Vec::new();
                    for previous in &values {
                        for write in &writes {
                            if let Some(guards) = self.merge(&previous.guards, &write.guards)? {
                                joined.push(Alternative {
                                    value: if write.value == Value::Unchanged { previous.value } else { write.value }, guards,
                                });
                            }
                        }
                    }
                    values = self.bounded(joined)?;
                }
                values
            } else {
                let child = self.frame(Frame {
                    instance,
                    caller: Some(point),
                    irq: self.frames[point.frame].irq,
                    unknown_entry: false,
                });
                self.with_input(child, point, |solver| {
                let child_index = solver.index(child)?;
                if let Some(&cause) = solver.controls.loop_fallbacks.get(&(instance, solver.frames[child].irq)) {
                    let at = Point { frame: child, location: Location { block: mir::START_BLOCK, statement_index: 0 } };
                    solver.controls.causes.insert(at, cause);
                    solver.record_value_loss(cause.0, "the bounded loop summary cannot exclude a change to this storage",
                        "the loop summary cannot exclude a write to the requested storage");
                    return Ok(vec![Alternative {
                        value: Value::Unknown("the bounded loop summary cannot exclude a change to this storage"),
                        guards: vec![(child, Guard { required: false, proof: GuardProof::Widened,
                            iteration: None, branch: at.location, successor: mir::START_BLOCK,
                            index: None, input: false, phi: true, choice: None })],
                    }]);
                }
                let mut values = Vec::new();
                for block in child_index.body.basic_blocks.indices()
                    .filter(|block| child_index.reachable.contains(block))
                {
                    let data = &child_index.body.basic_blocks[block];
                    if child_index.stops[block.as_usize()].is_some() { continue; }
                    let exit = match &data.terminator().kind {
                        TerminatorKind::Return if !unwind => Some((0, None)),
                        TerminatorKind::UnwindResume if unwind => Some((0, None)),
                        TerminatorKind::TailCall { .. } => Some((1, unwind.then_some(CallEdge::Unwind))),
                        TerminatorKind::Call { unwind: mir::UnwindAction::Continue, .. }
                        | TerminatorKind::Drop { unwind: mir::UnwindAction::Continue, .. }
                        | TerminatorKind::Assert { unwind: mir::UnwindAction::Continue, .. }
                            if unwind => Some((1, Some(CallEdge::Unwind))),
                        _ => None,
                    };
                    if let Some((after, edge)) = exit {
                        let at = Point {
                            frame: child,
                            location: Location { block, statement_index: data.statements.len() + after },
                        };
                        let entered = Self::boundary_addresses(address, child, true)[0];
                        let effects = solver.read_edge(entered, requested, at, true, edge, false)?;
                        values.extend(solver.exit_values(at, point, effects, false)?);
                    }
                }
                Ok(values)
                })?
            };
            for value in values {
                if let Some(guards) = self.merge(&target.guards, &value.guards)? {
                    result.push(Alternative {
                        value: value.value,
                        guards,
                    });
                }
            }
        }
        self.bounded(result)
    }


    fn aliases(
        &mut self,
        point: Point,
        operand: &Operand<'tcx>,
        ty: Ty<'tcx>,
        object: Object,
    ) -> Result<Vec<Alternative<'tcx>>, QueryStop> {
        let alias_object = self.alias_object;
        self.alias_object = Some(object);
        let answer = self.operand(point, operand, 0, ty);
        self.alias_object = alias_object;
        answer
    }

    fn pointer_projection(
        &self,
        mut source: Ty<'tcx>,
        mut destination: Ty<'tcx>,
        mut offset: i128,
    ) -> Option<(i128, Ty<'tcx>, Ty<'tcx>)> {
        let mut source_offset = 0i128;
        let cx = ty::layout::LayoutCx::new(self.tcx, TypingEnv::fully_monomorphized());
        for _ in 0..32 {
            while let ty::Pat(base, _) = *source.kind() {
                source = base;
            }
            while let ty::Pat(base, _) = *destination.kind() {
                destination = base;
            }
            if matches!(source.kind(), ty::Ref(..) | ty::RawPtr(..))
                && matches!(destination.kind(), ty::Ref(..) | ty::RawPtr(..))
            {
                return (offset == 0).then_some((source_offset, source, destination));
            }
            if !matches!((source.kind(), destination.kind()), (ty::Adt(left, _), ty::Adt(right, _)) if left == right && !left.is_enum() && !left.is_union())
            {
                return None;
            }
            let left = self
                .tcx
                .layout_of(TypingEnv::fully_monomorphized().as_query_input(source))
                .ok()?;
            let right = self
                .tcx
                .layout_of(TypingEnv::fully_monomorphized().as_query_input(destination))
                .ok()?;
            let field = (0..right.fields.count()).find(|field| {
                let start = i128::from(right.fields.offset(*field).bytes());
                let size = i128::from(right.field(&cx, *field).size.bytes());
                offset >= start && offset < start + size
            })?;
            source_offset += i128::from(left.fields.offset(field).bytes());
            offset -= i128::from(right.fields.offset(field).bytes());
            source = left.field(&cx, field).ty;
            destination = right.field(&cx, field).ty;
        }
        None
    }

    fn read_only(
        &mut self,
        instance: Instance<'tcx>,
        normal_return: bool,
    ) -> Result<bool, QueryStop> {
        let root = (instance, normal_return);
        if let Some(&result) = self.readonly.get(&root) { return Ok(result); }
        let mut pending = VecDeque::from([root]);
        let mut callers = HashMap::<_, Vec<_>>::new();
        let mut visited = HashSet::new();
        let mut rejected = HashSet::new();
        while let Some(key @ (instance, normal_return)) = pending.pop_front() {
            self.budget.charge(0)?;
            if !visited.insert(key) { continue; }
            if let Some(&result) = self.readonly.get(&key) {
                if !result { rejected.insert(key); }
                continue;
            }
            let instance = if self.tcx.is_foreign_item(instance.def_id())
                && self.trust.checks(instance.def_id()) && !crate::known_intrinsic(self.tcx, instance)
            {
                self.exported.get(self.tcx.symbol_name(instance).name).copied().unwrap_or(instance)
            } else { instance };
            let mut callees = Vec::new();
            let local: Result<bool, QueryStop> = (|| {
            if crate::known_intrinsic(self.tcx, instance)
                || matches!(
                    instance.def,
                    InstanceKind::Shim(ty::ShimKind::DropGlue(_, None))
                )
            {
                return Ok(true);
            }
            if matches!(instance.def, InstanceKind::Intrinsic(_)) {
                let name = self.tcx.item_name(instance.def_id());
                if let Some(requirement) = ty::layout::ValidityRequirement::from_intrinsic(name) {
                    return Ok(self
                        .tcx
                        .check_validity_requirement((
                            requirement,
                            TypingEnv::fully_monomorphized()
                                .as_query_input(instance.args.type_at(0)),
                        ))
                        .is_ok_and(|valid| valid));
                }
                return Ok(super::intrinsic_effect(self.tcx, instance) == super::IntrinsicEffect::ReadOnly);
            }
            if self.tcx.is_foreign_item(instance.def_id())
                || matches!(
                    instance.def,
                    InstanceKind::Intrinsic(_)
                        | InstanceKind::LlvmIntrinsic(_)
                        | InstanceKind::Virtual(..)
                )
                || !irq_check_rupta::util::type_util::has_instance_mir(self.tcx, instance.def)
            {
                return Ok(false);
            }
            let body = self.tcx.instance_mir(instance.def);
            let returning = if normal_return {
                let mut returning = HashSet::new();
                let mut pending: Vec<_> = body
                    .basic_blocks
                    .indices()
                    .filter(|block| {
                        matches!(
                            body.basic_blocks[*block].terminator().kind,
                            TerminatorKind::Return | TerminatorKind::TailCall { .. }
                        )
                    })
                    .collect();
                let predecessors = body.basic_blocks.predecessors();
                while let Some(block) = pending.pop() {
                    self.budget
                        .charge(std::mem::size_of::<mir::BasicBlock>() * 4)?;
                    if returning.insert(block) {
                        pending.extend(predecessors[block].iter().copied());
                    }
                }
                Some(returning)
            } else {
                None
            };
            for (block, data) in body.basic_blocks.iter_enumerated() {
                if returning
                    .as_ref()
                    .is_some_and(|blocks| !blocks.contains(&block))
                {
                    continue;
                }
                self.budget.charge(0)?;
                if matches!(&data.terminator().kind, TerminatorKind::Call { destination, .. } if destination.is_indirect())
                {
                    return Ok(false);
                }
                for statement in &data.statements {
                    match &statement.kind {
                        StatementKind::Assign(value) if value.0.is_indirect() => return Ok(false),
                        StatementKind::SetDiscriminant { place, .. } if place.is_indirect() => {
                            return Ok(false);
                        }
                        StatementKind::Intrinsic(intrinsic)
                            if matches!(
                                &**intrinsic,
                                mir::NonDivergingIntrinsic::CopyNonOverlapping(_)
                            ) =>
                        {
                            return Ok(false);
                        }
                        _ => {}
                    }
                }
                match &data.terminator().kind {
                    TerminatorKind::Call { func, .. } | TerminatorKind::TailCall { func, .. } => {
                        let ty = instance.instantiate_mir_and_normalize_erasing_regions(
                            self.tcx,
                            TypingEnv::fully_monomorphized(),
                            EarlyBinder::bind(self.tcx, func.ty(body, self.tcx)),
                        );
                        let ty::FnDef(def, args) = *ty.kind() else {
                            return Ok(false);
                        };
                        let args = self.tcx.instantiate_bound_regions_with_erased(args);
                        let Some(target) = Instance::try_resolve(
                            self.tcx,
                            TypingEnv::fully_monomorphized(),
                            def,
                            args,
                        )
                        .ok()
                        .flatten() else {
                            return Ok(false);
                        };
                        let normal = normal_return
                            && !matches!(
                                data.terminator().kind,
                                TerminatorKind::Call {
                                    unwind: mir::UnwindAction::Continue
                                        | mir::UnwindAction::Cleanup(_),
                                    ..
                                }
                            );
                        callees.push((target, normal));
                    }
                    TerminatorKind::Drop { place, .. } => {
                        let ty = instance.instantiate_mir_and_normalize_erasing_regions(
                            self.tcx,
                            TypingEnv::fully_monomorphized(),
                            EarlyBinder::bind(self.tcx, place.ty(body, self.tcx).ty),
                        );
                        let normal = normal_return
                            && !matches!(
                                data.terminator().kind,
                                TerminatorKind::Drop {
                                    unwind: mir::UnwindAction::Continue
                                        | mir::UnwindAction::Cleanup(_),
                                    ..
                                }
                            );
                        callees.push((Instance::resolve_drop_glue(self.tcx, ty), normal));
                    }
                    TerminatorKind::InlineAsm { options, operands, .. } => {
                        if !options.intersects(rustc_ast::InlineAsmOptions::NOMEM | rustc_ast::InlineAsmOptions::READONLY)
                            || operands.iter().any(|operand| match operand {
                                mir::InlineAsmOperand::Out { place, .. } => place.is_some_and(|place| place.is_indirect()),
                                mir::InlineAsmOperand::InOut { out_place, .. } => out_place.is_some_and(|place| place.is_indirect()),
                                _ => false,
                            })
                        { return Ok(false); }
                    }
                    TerminatorKind::Assert {
                        unwind: mir::UnwindAction::Continue | mir::UnwindAction::Cleanup(_),
                        ..
                    } => return Ok(false),
                    _ => {}
                }
            }
            Ok(true)

            })();
            if !local? { rejected.insert(key); }
            for callee in callees {
                callers.entry(callee).or_default().push(key);
                pending.push_back(callee);
            }
        }
        pending.extend(rejected.iter().copied());
        while let Some(callee) = pending.pop_front() {
            self.budget.charge(0)?;
            for &caller in callers.get(&callee).into_iter().flatten() {
                if rejected.insert(caller) { pending.push_back(caller); }
            }
        }
        for key in visited { self.readonly.insert(key, !rejected.contains(&key)); }
        Ok(!rejected.contains(&root))
    }
}
