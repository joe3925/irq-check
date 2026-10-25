use crate::trust::Trust;
use rustc_middle::mir::visit::{PlaceContext, Visitor};
use rustc_middle::mir::{self, Body, Location, Place, StatementKind, TerminatorKind};
use rustc_middle::ty::{Instance, TyCtxt};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum DefinitionKind {
    Assignment,
    CallReturn,
    Uninitialized,
    MemoryEffect,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct Definition<'tcx> {
    pub location: Location,
    pub place: Option<Place<'tcx>>,
    pub kind: DefinitionKind,
    pub normal_target: Option<mir::BasicBlock>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum GuardProof {
    Pending,
    Unresolved,
    Predicate,
    Established,
    Widened,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct Guard {
    pub required: bool,
    pub proof: GuardProof,
    pub iteration: Option<usize>,
    pub branch: Location,
    pub successor: mir::BasicBlock,
    pub index: Option<mir::Local>,
    pub input: bool,
    pub phi: bool,
    pub choice: Option<(usize, usize)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum QueryStop {
    ContextChanged,
    ControlMemory,
    TransferLimit,
    RefinementLimit,
    AlternativeLimit,
    ScratchLimit,
    ProcessLimit,
    Unsupported,
    MissingArgument,
    SplitIndex,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RequestMode {
    RequiredBinding,
    OptionalRefinement,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RequestState {
    Pending,
    Complete,
    Stopped(QueryStop),
}

pub(super) enum ReadOutcome<T> {
    Complete(T),
    Stopped(QueryStop),
}

pub(super) struct QueryBudget {
    pub steps: usize,
    pub limit: usize,
    pub scratch: usize,
    pub refinement_limit: Option<usize>,
    pub limit_usage: Option<(QueryStop, &'static str, usize, usize, usize)>,
}

impl QueryBudget {
    pub fn record_limit(&mut self, reason: QueryStop, resource: &'static str, used: usize, limit: usize) {
        self.limit_usage = Some((reason, resource, used, limit, self.steps));
    }

    pub fn charge(&mut self, bytes: usize) -> Result<(), QueryStop> {
        if !irq_check_rupta::limits::checkpoint() {
            return Err(QueryStop::ProcessLimit);
        }
        if self.steps >= self.limit {
            self.record_limit(QueryStop::TransferLimit, "query transfer steps", self.steps, self.limit);
            return Err(QueryStop::TransferLimit);
        }
        if self.refinement_limit.is_some_and(|limit| self.steps >= limit) {
            self.record_limit(QueryStop::RefinementLimit, "refinement transfer steps", self.steps, self.refinement_limit.unwrap());
            return Err(QueryStop::RefinementLimit);
        }
        self.steps += 1;
        self.reserve(bytes)
    }

    pub fn reserve(&mut self, bytes: usize) -> Result<(), QueryStop> {
        self.scratch = self
            .scratch
            .checked_add(bytes)
            .ok_or(QueryStop::ScratchLimit)?;
        if self.scratch > irq_check_rupta::limits::configuration().expect("validated analysis limits").query_scratch_mib * 1024 * 1024 {
            self.record_limit(QueryStop::ScratchLimit, "query scratch bytes", self.scratch,
                irq_check_rupta::limits::configuration().expect("validated analysis limits").query_scratch_mib * 1024 * 1024);
            if std::env::var_os("IRQ_CHECK_TRACE").is_some() {
                eprintln!("irq-check: query scratch limit bytes={} added={bytes} steps={}", self.scratch, self.steps);
            }
            return Err(QueryStop::ScratchLimit);
        }
        Ok(())
    }
}

pub(super) struct BodyIndex<'tcx> {
    pub body: &'tcx Body<'tcx>,
    pub stops: Vec<Option<usize>>,
    pub reachable: HashSet<mir::BasicBlock>,
    pub predecessors: Vec<Vec<(mir::BasicBlock, Option<Guard>)>>,
    pub definitions: HashMap<(mir::Local, mir::BasicBlock), Vec<Definition<'tcx>>>,
    pub local_definitions: HashMap<mir::Local, Vec<Definition<'tcx>>>,
    pub memory: HashMap<mir::BasicBlock, Vec<Definition<'tcx>>>,
    pub clobbers: HashMap<mir::BasicBlock, Vec<Definition<'tcx>>>,
    pub calls: Vec<Location>,
    pub address_taken: HashSet<mir::Local>,
    pub used_arguments: HashSet<mir::Local>,
    pub loops: HashMap<(mir::BasicBlock, mir::BasicBlock), HashSet<mir::BasicBlock>>,
    pub assumptions: HashMap<Location, mir::Operand<'tcx>>,
}

struct ArgumentUses<'a>(&'a mut HashSet<mir::Local>, usize);

impl<'tcx> Visitor<'tcx> for ArgumentUses<'_> {
    fn visit_local(&mut self, local: mir::Local, _: PlaceContext, _: Location) {
        if local.as_usize() > 0 && local.as_usize() <= self.1 {
            self.0.insert(local);
        }
    }
}

impl<'tcx> BodyIndex<'tcx> {
    pub fn build(
        tcx: TyCtxt<'tcx>,
        trust: &Trust,
        instance: Instance<'tcx>,
        irq: bool,
    ) -> Result<Self, QueryStop> {
        let body = tcx.instance_mir(instance.def);
        if std::env::var("IRQ_CHECK_TRACE_FUNCTION")
            .is_ok_and(|filter| !filter.is_empty() && instance.to_string().contains(&filter))
        {
            eprintln!("irq-check: indexed MIR {instance} irq={irq}\n{body:#?}");
        }
        let mut index = Self {
            body,
            stops: vec![None; body.basic_blocks.len()],
            reachable: HashSet::new(),
            predecessors: vec![Vec::new(); body.basic_blocks.len()],
            definitions: HashMap::new(),
            local_definitions: HashMap::new(),
            memory: HashMap::new(),
            clobbers: HashMap::new(),
            calls: Vec::new(),
            address_taken: HashSet::new(),
            used_arguments: HashSet::new(),
            loops: HashMap::new(),
            assumptions: HashMap::new(),
        };
        let mut required_arguments = HashMap::<Instance<'tcx>, Option<usize>>::new();
        let mut pending = VecDeque::from([mir::START_BLOCK]);
        while let Some(block) = pending.pop_front() {
            if !irq_check_rupta::limits::checkpoint() {
                return Err(QueryStop::ProcessLimit);
            }
            if !index.reachable.insert(block) {
                continue;
            }
            let data = &body.basic_blocks[block];
            for (statement_index, statement) in data.statements.iter().enumerate() {
                if irq && trust.allows(tcx, instance, body, statement.source_info.scope, crate::trust::ScopeMark::Unreachable) {
                    index.stops[block.as_usize()] = Some(statement_index);
                    break;
                }
                if !irq_check_rupta::limits::checkpoint() {
                    return Err(QueryStop::ProcessLimit);
                }
                let location = Location {
                    block,
                    statement_index,
                };
                if !matches!(
                    statement.kind,
                    StatementKind::StorageLive(_) | StatementKind::StorageDead(_)
                ) {
                    ArgumentUses(&mut index.used_arguments, body.arg_count)
                        .visit_statement(statement, location);
                }
                match &statement.kind {
                    StatementKind::Assign(assignment) => {
                        let (place, value) = &**assignment;
                        if let mir::Rvalue::Ref(_, kind, source) = value {
                            let ty = instance.instantiate_mir_and_normalize_erasing_regions(
                                tcx,
                                rustc_middle::ty::TypingEnv::fully_monomorphized(),
                                rustc_middle::ty::EarlyBinder::bind(
                                    tcx,
                                    body.local_decls[source.local].ty,
                                ),
                            );
                            if !source.is_indirect()
                                && (!matches!(kind, mir::BorrowKind::Shared)
                                    || !ty.is_freeze(
                                        tcx,
                                        rustc_middle::ty::TypingEnv::fully_monomorphized(),
                                    ))
                            {
                                index.address_taken.insert(source.local);
                            }
                        } else if let mir::Rvalue::RawPtr(_, source)
                        | mir::Rvalue::Reborrow(_, _, source) = value
                        {
                            if !source.is_indirect() {
                                index.address_taken.insert(source.local);
                            }
                        }
                        let definition = Definition {
                            location,
                            place: Some(*place),
                            kind: DefinitionKind::Assignment,
                            normal_target: None,
                        };
                        if place.is_indirect() {
                            index.memory.entry(block).or_default().push(definition);
                        } else {
                            index
                                .definitions
                                .entry((place.local, block))
                                .or_default()
                                .push(definition);
                        }
                    }
                    StatementKind::StorageLive(local) | StatementKind::StorageDead(local) => {
                        index
                            .definitions
                            .entry((*local, block))
                            .or_default()
                            .push(Definition {
                                location,
                                place: Some(Place::from(*local)),
                                kind: DefinitionKind::Uninitialized,
                                normal_target: None,
                            });
                    }
                    StatementKind::SetDiscriminant { place, .. } => {
                        let definition = Definition {
                            location,
                            place: Some(**place),
                            kind: DefinitionKind::MemoryEffect,
                            normal_target: None,
                        };
                        if place.is_indirect() {
                            index.memory.entry(block).or_default().push(definition);
                        } else {
                            index
                                .definitions
                                .entry((place.local, block))
                                .or_default()
                                .push(definition);
                        }
                    }
                    StatementKind::Intrinsic(intrinsic) => {
                        if let mir::NonDivergingIntrinsic::Assume(condition) = &**intrinsic {
                            index.assumptions.insert(location, condition.clone());
                        }
                        index.memory.entry(block).or_default().push(Definition {
                            location,
                            place: None,
                            kind: DefinitionKind::MemoryEffect,
                            normal_target: None,
                        });
                    }
                    _ => {}
                }
            }
            if index.stops[block.as_usize()].is_some() {
                continue;
            }
            let terminator = data.terminator();
            let location = Location {
                block,
                statement_index: data.statements.len(),
            };
            if irq && trust.allows(tcx, instance, body, terminator.source_info.scope, crate::trust::ScopeMark::Unreachable) {
                index.stops[block.as_usize()] = Some(location.statement_index);
                continue;
            }
            ArgumentUses(&mut index.used_arguments, body.arg_count)
                .visit_terminator(terminator, location);
            if let TerminatorKind::Call { func, args, target: Some(_), .. } = &terminator.kind {
                let callee_ty = instance.instantiate_mir_and_normalize_erasing_regions(
                    tcx, rustc_middle::ty::TypingEnv::fully_monomorphized(),
                    rustc_middle::ty::EarlyBinder::bind(tcx, func.ty(body, tcx)));
                if let rustc_middle::ty::FnDef(def, substitutions) = *callee_ty.kind()
                    && let Ok(Some(callee)) = Instance::try_resolve(tcx,
                        rustc_middle::ty::TypingEnv::fully_monomorphized(), def,
                        tcx.instantiate_bound_regions_with_erased(substitutions))
                {
                    let required = *required_arguments.entry(callee).or_insert_with(|| {
                        if !irq_check_rupta::util::type_util::has_instance_mir(tcx, callee.def) { return None; }
                        let called = tcx.instance_mir(callee.def);
                        let returns: Vec<_> = called.basic_blocks.iter_enumerated()
                            .filter_map(|(block, data)| matches!(data.terminator().kind, TerminatorKind::Return).then_some(block)).collect();
                        if returns.is_empty() || called.basic_blocks.iter().any(|data|
                            matches!(data.terminator().kind, TerminatorKind::TailCall { .. }
                                | TerminatorKind::InlineAsm { .. } | TerminatorKind::Yield { .. }))
                        { return None; }
                        let dominators = called.basic_blocks.dominators();
                        for (block, data) in called.basic_blocks.iter_enumerated() {
                            if !irq_check_rupta::limits::checkpoint() { return None; }
                            if !returns.iter().all(|&exit| dominators.dominates(block, exit)) { continue; }
                            for statement in &data.statements {
                                let StatementKind::Intrinsic(intrinsic) = &statement.kind else { continue; };
                                let mir::NonDivergingIntrinsic::Assume(condition) = &**intrinsic else { continue; };
                                let (mir::Operand::Copy(place) | mir::Operand::Move(place)) = condition else { continue; };
                                if !place.projection.is_empty() || place.local.as_usize() == 0
                                    || place.local.as_usize() > called.arg_count
                                    || !called.local_decls[place.local].ty.is_bool()
                                { continue; }
                                let local = place.local;
                                let changed = called.basic_blocks.iter().any(|data| {
                                    let written = match &data.terminator().kind {
                                        TerminatorKind::Call { destination, .. } => destination.local == local && !destination.is_indirect(),
                                        _ => false,
                                    };
                                    written || data.statements.iter().any(|statement| match &statement.kind {
                                        StatementKind::Assign(assignment) => {
                                            let (destination, value) = &**assignment;
                                            destination.local == local && !destination.is_indirect()
                                                || match value {
                                                    mir::Rvalue::Ref(_, _, source) | mir::Rvalue::RawPtr(_, source)
                                                        | mir::Rvalue::Reborrow(_, _, source) => source.local == local && !source.is_indirect(),
                                                    _ => false,
                                                }
                                        }
                                        StatementKind::SetDiscriminant { place, .. } => place.local == local && !place.is_indirect(),
                                        StatementKind::StorageLive(source) | StatementKind::StorageDead(source) => *source == local,
                                        _ => false,
                                    })
                                });
                                if !changed { return Some(local.as_usize() - 1); }
                            }
                        }
                        None
                    });
                    if let Some(argument) = required.and_then(|argument| args.get(argument)) {
                        index.assumptions.insert(location, argument.node.clone());
                    }
                }
            }
            match &terminator.kind {
                TerminatorKind::Call {
                    destination,
                    target,
                    ..
                } => {
                    index.calls.push(location);
                    index.memory.entry(block).or_default().push(Definition {
                        location,
                        place: None,
                        kind: DefinitionKind::MemoryEffect,
                        normal_target: None,
                    });
                    if let Some(target) = target {
                        let definition = Definition {
                            location,
                            place: Some(*destination),
                            kind: DefinitionKind::CallReturn,
                            normal_target: Some(*target),
                        };
                        if destination.is_indirect() {
                            index.memory.entry(block).or_default().push(definition);
                        } else {
                            index
                                .definitions
                                .entry((destination.local, block))
                                .or_default()
                                .push(definition);
                        }
                    }
                }
                TerminatorKind::Assert { .. } => {
                    index.calls.push(location);
                    index.memory.entry(block).or_default().push(Definition {
                        location,
                        place: None,
                        kind: DefinitionKind::MemoryEffect,
                        normal_target: None,
                    });
                }
                TerminatorKind::InlineAsm { .. } => {
                    index.calls.push(location);
                    index.clobbers.entry(block).or_default().push(Definition {
                        location,
                        place: None,
                        kind: DefinitionKind::MemoryEffect,
                        normal_target: None,
                    });
                }
                TerminatorKind::Drop { .. } | TerminatorKind::TailCall { .. } => {
                    index.calls.push(location);
                    index.memory.entry(block).or_default().push(Definition {
                        location,
                        place: None,
                        kind: DefinitionKind::MemoryEffect,
                        normal_target: None,
                    });
                    if matches!(terminator.kind, TerminatorKind::TailCall { .. }) {
                        index.definitions.entry((mir::RETURN_PLACE, block)).or_default().push(Definition {
                            location, place: Some(Place::from(mir::RETURN_PLACE)),
                            kind: DefinitionKind::CallReturn, normal_target: None,
                        });
                    }
                }
                _ => {}
            }
            let successors = irq_check_rupta::util::type_util::constant_successors(
                tcx,
                body,
                block,
                instance.args,
            );
            let guarded = matches!(terminator.kind, TerminatorKind::SwitchInt { .. }) && successors.len() > 1
                || matches!(terminator.kind, TerminatorKind::Assert { .. })
                || index.assumptions.contains_key(&location);
            for successor in successors {
                let normal_assumption = !index.assumptions.contains_key(&location)
                    || matches!(&terminator.kind, TerminatorKind::Call { target, .. } if *target == Some(successor));
                let guard = (guarded && normal_assumption).then_some(Guard {
                    required: true, iteration: None, proof: GuardProof::Pending,
                    phi: false,
                    branch: location,
                    successor,
                    index: None,
                    input: false,
                    choice: None,
                });
                let predecessors = &mut index.predecessors[successor.as_usize()];
                if !predecessors.contains(&(block, guard)) {
                    predecessors.push((block, guard));
                }
                pending.push_back(successor);
            }
        }
        for ((local, _), definitions) in &index.definitions {
            index.local_definitions.entry(*local).or_default().extend(
                definitions
                    .iter()
                    .copied()
                    .filter(|definition| definition.kind != DefinitionKind::Uninitialized),
            );
        }
        let dominators = body.basic_blocks.dominators();
        for &header in &index.reachable {
            for &(tail, _) in &index.predecessors[header.as_usize()] {
                if !dominators.dominates(header, tail) {
                    continue;
                }
                let mut blocks = HashSet::from([header]);
                let mut pending = vec![tail];
                while let Some(block) = pending.pop() {
                    if !irq_check_rupta::limits::checkpoint() {
                        return Err(QueryStop::ProcessLimit);
                    }
                    if !blocks.insert(block) {
                        continue;
                    }
                    pending.extend(
                        index.predecessors[block.as_usize()]
                            .iter()
                            .map(|(block, _)| *block),
                    );
                }
                index.loops.insert((tail, header), blocks);
            }
        }
        Ok(index)
    }
}
