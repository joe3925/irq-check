mod global;
mod query;
mod sparse;

use crate::trust::Trust;
use crate::Edge;
use rustc_middle::mir::{self, Location, TerminatorKind};
use rustc_middle::ty::{self, Instance, InstanceKind, TyCtxt, TypingEnv};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use query::{Guard, QueryStop, ReadOutcome, RequestMode};
use sparse::{Alternative, Frame, Point, Solver, Value};

#[derive(Clone, Eq, PartialEq)]
struct QueryCase {
    indexes: HashMap<(Point, mir::Local), u128>,
    loop_entries: Box<[Point]>,
    cut_recursive_returns: bool,
}

struct QueryContinuation {
    pending: Vec<QueryCase>,
    steps: usize,
    limit: usize,
    fallback_steps: usize,
    scalar_steps: HashMap<Point, usize>,
    attempts: usize,
    cases: usize,
    control_failed: bool,
    advance: bool,
}

pub(super) const LEAF_INTRINSICS: &[&str] = &[
    "abort", "cold_path", "unreachable", "assume", "forget", "breakpoint", "black_box",
    "likely", "unlikely", "select_unpredictable", "size_of", "size_of_val", "align_of_val", "min_align_of", "pref_align_of", "caller_location",
    "compare_bytes", "fabs", "integer_min", "integer_max", "atomic_fence", "atomic_singlethreadfence",
    "prefetch_read_data", "prefetch_write_data", "prefetch_read_instruction", "prefetch_write_instruction",
    "type_id", "needs_drop", "variant_count", "type_name", "discriminant_value", "bswap", "bitreverse",
    "ctlz", "ctlz_nonzero", "cttz", "cttz_nonzero", "ctpop", "rotate_left", "rotate_right",
    "wrapping_add", "wrapping_sub", "wrapping_mul", "saturating_add", "saturating_sub",
    "unchecked_add", "unchecked_sub", "unchecked_mul", "unchecked_div", "unchecked_rem",
    "unchecked_shl", "unchecked_shr", "exact_div", "add_with_overflow", "sub_with_overflow",
    "mul_with_overflow", "carrying_mul_add", "is_val_statically_known", "ptr_mask",
    "atomic_load", "atomic_store", "atomic_xchg", "atomic_cxchg", "atomic_cxchgweak",
    "atomic_xadd", "atomic_xsub", "atomic_and", "atomic_nand", "atomic_or", "atomic_xor",
    "atomic_max", "atomic_min", "atomic_umax", "atomic_umin", "write_bytes", "volatile_set_memory",
    "copy", "copy_nonoverlapping", "volatile_copy_memory", "volatile_copy_nonoverlapping_memory",
    "read_via_copy", "write_via_move", "volatile_load", "unaligned_volatile_load", "volatile_store",
    "unaligned_volatile_store", "offset", "arith_offset", "ptr_offset_from", "ptr_offset_from_unsigned",
    "ptr_guaranteed_cmp", "transmute", "assert_inhabited", "assert_zero_valid", "assert_mem_uninitialized_valid",
    "raw_eq", "type_id_eq", "disjoint_bitor", "typed_swap_nonoverlapping",
    "simd_splat", "simd_eq", "simd_gt", "simd_lt", "simd_or", "simd_bitmask",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum IntrinsicEffect {
    ReadOnly,
    Write { arguments: &'static [usize], fixed: bool },
    Unknown,
}

pub(super) fn intrinsic_effect<'tcx>(tcx: TyCtxt<'tcx>, instance: Instance<'tcx>) -> IntrinsicEffect {
    let name = tcx.item_name(instance.def_id());
    if let Some(requirement) = ty::layout::ValidityRequirement::from_intrinsic(name) {
        return if tcx.check_validity_requirement((requirement,
            TypingEnv::fully_monomorphized().as_query_input(instance.args.type_at(0))))
            .is_ok_and(|valid| valid) { IntrinsicEffect::ReadOnly } else { IntrinsicEffect::Unknown };
    }
    match name.as_str() {
        "copy" | "copy_nonoverlapping" => IntrinsicEffect::Write { arguments: &[1], fixed: false },
        "volatile_copy_memory" | "volatile_copy_nonoverlapping_memory"
        | "write_bytes" | "volatile_set_memory" => IntrinsicEffect::Write { arguments: &[0], fixed: false },
        "typed_swap_nonoverlapping" => IntrinsicEffect::Write { arguments: &[0, 1], fixed: true },
        "write_via_move" | "volatile_store" | "unaligned_volatile_store"
        | "atomic_store" | "atomic_xchg" | "atomic_cxchg" | "atomic_cxchgweak"
        | "atomic_xadd" | "atomic_xsub" | "atomic_and" | "atomic_nand"
        | "atomic_or" | "atomic_xor" | "atomic_max" | "atomic_min"
        | "atomic_umax" | "atomic_umin" => IntrinsicEffect::Write { arguments: &[0], fixed: true },
        _ if LEAF_INTRINSICS.contains(&name.as_str()) => IntrinsicEffect::ReadOnly,
        _ => IntrinsicEffect::Unknown,
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Node<'tcx> {
    pub instance: Instance<'tcx>,
    pub context: usize,
}

pub struct CallSite<'tcx> {
    pub caller: Node<'tcx>,
    pub state: usize,
    pub source: Arc<CallSiteSource>,
    pub diagnostics: Option<Arc<[(rustc_span::Span, String)]>>,
    evidence: QueryEvidence,
}

pub struct CallSiteSource {
    pub location: Location,
    pub scope: mir::SourceScope,
    pub normal: Vec<Location>,
    pub unwind: Option<mir::BasicBlock>,
}

struct QueryEvidence {
    steps: usize,
    attempts: usize,
    cases: usize,
    loop_entries: Arc<[Point]>,
    cut_recursive_returns: bool,
    witness: Option<u128>,
    result: Result<[usize; 4], QueryStop>,
    conservative_safe: bool,
    worker: Option<usize>,
}

pub struct Analysis<'tcx> {
    pub roots: HashMap<Instance<'tcx>, Node<'tcx>>,
    pub graph: HashMap<Node<'tcx>, Vec<Edge<'tcx>>>,
    pub sites: Vec<CallSite<'tcx>>,
    pub free_sites: HashSet<usize>,
    pub incomplete: bool,
    pub limit_detail: String,
    pub precision_loss: HashMap<String, usize>,
    pub safe_summaries: HashSet<Instance<'tcx>>,
    pub specialized_safe: HashSet<Node<'tcx>>,
    pub covered_calls: HashMap<(Instance<'tcx>, Location, &'static str), usize>,
    pub complete_pointer_coverage: bool,
    pub complete_call_coverage: bool,
    pub diagnostic_cutoff: bool,
}

impl<'tcx> Analysis<'tcx> {
    pub fn build(tcx: TyCtxt<'tcx>, trust: &Trust, instances: &[Instance<'tcx>], roots: &[Instance<'tcx>]) -> Self {
        let global::GlobalPointers { exported, roots: discovery_roots, candidates, failure, binding_independent, diagnostic_independent, facts } =
            global::GlobalPointers::build(tcx, trust, instances, roots);
        let mut output = Self {
            roots: HashMap::new(), graph: HashMap::new(), sites: Vec::new(), free_sites: HashSet::new(),
            incomplete: failure.is_some(), limit_detail: failure.unwrap_or_default(), precision_loss: HashMap::new(),
            safe_summaries: facts.safe.clone(), specialized_safe: HashSet::new(), covered_calls: HashMap::new(), complete_pointer_coverage: facts.complete,
            complete_call_coverage: facts.call_complete,
            diagnostic_cutoff: false,
        };
        if output.incomplete { return output; }
        if let Some(directory) = std::env::var_os("IRQ_CHECK_DUMP_DIR") {
            let path = std::path::PathBuf::from(directory).join(format!("{}-phase.txt", tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE)));
            let _ = std::fs::write(path, "global pointer analysis complete; IRQ queries");
        }
        let workers = crate::worker_limit().min(roots.len().max(1));
        let tasks: Vec<_> = (0..workers).collect();
        let results: Vec<_> = rustc_data_structures::sync::par_map(tasks, |worker| {
            let roots: Vec<_> = roots.iter().copied().skip(worker).step_by(workers).collect();
            Self::check_roots(tcx, trust, &exported, &candidates, &binding_independent, &diagnostic_independent, &facts, &discovery_roots, &roots, 1024 * 1024 * 1024 / workers, worker, None)
        });
        for (worker, mut result) in results.into_iter().enumerate() {
            let site_offset = output.sites.len();
            let convert = |node: Node<'tcx>| Node {
                instance: node.instance,
                context: if node.context == usize::MAX { usize::MAX } else { node.context * workers + worker },
            };
            output.roots.extend(result.roots.into_iter().map(|(instance, node)| (instance, convert(node))));
            for site in &mut result.sites {
                site.caller = convert(site.caller);
                site.state += site_offset;
                site.evidence.worker = Some(worker);
            }
            output.sites.extend(result.sites);
            for (node, mut edges) in result.graph {
                for edge in &mut edges {
                    edge.site = edge.site.map(|site| site + site_offset);
                    edge.target = edge.target.map(convert);
                }
                output.graph.entry(convert(node)).or_default().extend(edges);
            }
            output.incomplete |= result.incomplete;
            if result.incomplete { output.limit_detail = result.limit_detail; }
            for (reason, count) in result.precision_loss { *output.precision_loss.entry(reason).or_default() += count; }
            output.diagnostic_cutoff |= result.diagnostic_cutoff;
            output.safe_summaries.extend(result.safe_summaries);
            output.specialized_safe.extend(result.specialized_safe.into_iter().map(convert));
            for (key, count) in result.covered_calls { *output.covered_calls.entry(key).or_default() += count; }
        }
        output
    }

    fn check_roots(
        tcx: TyCtxt<'tcx>,
        trust: &Trust,
        exported: &HashMap<String, Instance<'tcx>>,
        candidates: &HashMap<(Instance<'tcx>, Location), HashSet<Instance<'tcx>>>,
        binding_independent: &HashSet<Instance<'tcx>>,
        diagnostic_independent: &HashSet<Instance<'tcx>>,
        facts: &global::PointerFacts<'tcx>,
        discovery_roots: &[Instance<'tcx>],
        roots: &[Instance<'tcx>],
        cache_limit: usize,
        worker: usize,
        witness_value: Option<u128>,
    ) -> Self {
        let scalar_witness = witness_value.is_some();
        let mut output = Self {
            roots: HashMap::new(), graph: HashMap::new(), sites: Vec::new(), free_sites: HashSet::new(),
            incomplete: false, limit_detail: String::new(), precision_loss: HashMap::new(),
            safe_summaries: HashSet::new(), specialized_safe: HashSet::new(), covered_calls: HashMap::new(), complete_pointer_coverage: facts.complete,
            complete_call_coverage: facts.call_complete,
            diagnostic_cutoff: false,
        };
        let limits = irq_check_rupta::limits::configuration().expect("validated analysis limits");
        let mut solver = Solver::new(tcx, trust, exported, facts);
        solver.discovery_roots = discovery_roots.to_vec();
        solver.cache_limit = cache_limit;
        let mut queue = VecDeque::new();
        let mut deferred = VecDeque::new();
        let mut queued = HashMap::<usize, Vec<Vec<(usize, Guard)>>>::new();
        let mut candidate_sites = HashSet::new();
        let mut diagnostic_frames = HashMap::new();
        let mut diagnostic_inputs = HashMap::<Instance<'tcx>, HashMap<Point, usize>>::new();
        let mut diagnostic_reuse = 0usize;
        let mut diagnostic_widened = 0usize;
        let mut forbidden_distance = HashMap::new();
        let mut callers = HashMap::<_, HashSet<_>>::new();
        for (&(caller, _), targets) in &facts.safe_candidates {
            for &target in targets {
                callers.entry(target).or_default().insert(caller);
                if crate::forbidden(tcx, target).is_some() { forbidden_distance.insert(target, 0usize); }
            }
        }
        let mut pending: VecDeque<_> = forbidden_distance.keys().copied().collect();
        while let Some(target) = pending.pop_front() {
            if !irq_check_rupta::limits::checkpoint() { break; }
            let distance = forbidden_distance[&target] + 1;
            for &caller in callers.get(&target).into_iter().flatten() {
                if let std::collections::hash_map::Entry::Vacant(entry) = forbidden_distance.entry(caller) {
                    entry.insert(distance);
                    pending.push_back(caller);
                }
            }
        }
        drop(callers);
        let mut call_orders = HashMap::new();
        for &instance in roots {
            let context = solver.frame(Frame { instance, caller: None, irq: true, unknown_entry: false });
            if let Some(value) = witness_value { solver.root_scalar_witness.insert(context, value); }
            output.roots.insert(instance, Node { instance, context });
            queue.push_back((context, Vec::<(usize, Guard)>::new(), 0, None::<QueryContinuation>));
        }
        if !scalar_witness && output.roots.values().any(|root| {
            solver.index(root.context).is_ok_and(|index| index.body.args_iter().any(|local|
                matches!(solver.ty(root.context, index.body.local_decls[local].ty).kind(), ty::Int(_) | ty::Uint(_) | ty::Bool)))
        }) {
            for value in [0, 1] {
                let witness = Self::check_roots(tcx, trust, exported, candidates, binding_independent, diagnostic_independent, facts,
                    discovery_roots, roots, cache_limit, worker, Some(value));
                if witness.diagnostic_cutoff && !witness.incomplete { return witness; }
            }
        }
        let mut queries = 0usize;
        let mut skipped_witnesses = 0usize;
        let mut transfers = 0usize;
        let mut safety_checks = 0usize;
        let mut safety_hits = 0usize;
        let mut safety_transfers = 0usize;
        let trace_time = std::env::var_os("IRQ_CHECK_TRACE_TIME").is_some();
        let mut query_times = HashMap::<(Instance<'tcx>, Location), (usize, u128, u128, u128, usize)>::new();
        let mut safety_time = 0u128;
        let mut site_sources = HashMap::<(Instance<'tcx>, Location), Arc<CallSiteSource>>::new();
        let mut evidence_loops = HashSet::<Arc<[Point]>>::new();
        let mut generation = solver.flow.generation;
        loop {
        while let Some((frame, inherited, next_call, mut resume)) = queue.pop_front().or_else(|| deferred.pop_front()) {
            if scalar_witness && transfers >= limits.scalar_steps {
                output.incomplete = true;
                output.limit_detail = "the bounded scalar witness pass reached its allowance".into();
                break;
            }
            if !irq_check_rupta::limits::checkpoint() {
                output.incomplete = true;
                output.limit_detail = irq_check_rupta::limits::stop_reason().unwrap_or("analysis stopped").into();
                break;
            }
            if next_call == 0 && resume.is_none() {
                let previous = queued.entry(frame).or_default();
                if previous.iter().any(|guards| guards.iter().all(|guard| inherited.contains(guard))) { continue; }
                previous.retain(|guards| !inherited.iter().all(|guard| guards.contains(guard)));
                previous.push(inherited.clone());
            }
            let instance = solver.frames[frame].instance;
            let caller = Node { instance, context: frame };
            if facts.safe.contains(&instance) || crate::forbidden(tcx, instance).is_some() || !trust.checks(instance.def_id())
                || crate::known_intrinsic(tcx, instance)
                || matches!(instance.def, InstanceKind::Shim(ty::ShimKind::DropGlue(_, None)))
            { continue; }
            if next_call == 0 && resume.is_none() && !scalar_witness {
                let watch = trace_time.then(std::time::Instant::now);
                solver.begin_query(limits.query_steps.min(4096));
                safety_checks += 1;
                let safe = matches!(solver.scalar_summary(Point { frame, location: Location::START }), Ok((_, true)));
                safety_transfers += solver.budget.steps;
                transfers += solver.budget.steps;
                if let Some(watch) = watch { safety_time += watch.elapsed().as_nanos(); }
                if safe {
                    output.specialized_safe.insert(caller);
                    safety_hits += 1;
                    continue;
                }
            }
            let unsupported = if matches!(instance.def, InstanceKind::Intrinsic(_)) {
                !LEAF_INTRINSICS.contains(&tcx.item_name(instance.def_id()).as_str())
            } else {
                tcx.is_foreign_item(instance.def_id())
                    || matches!(instance.def, InstanceKind::LlvmIntrinsic(_) | InstanceKind::Virtual(..))
                    || !irq_check_rupta::util::type_util::has_instance_mir(tcx, instance.def)
            };
            if unsupported {
                output.graph.entry(caller).or_default().push(Edge {
                    candidate: false, site: None, target: None, span: tcx.def_span(instance.def_id()),
                    kind: "unsupported body", detail: format!("the callable has no checked body: {instance}").into(), trusted: false,
                    allow_incomplete: false, path_proven: false, uncertainty: Vec::new(),
                });
                continue;
            }
            if matches!(instance.def, InstanceKind::Intrinsic(_)) { continue; }
            let index = match solver.index(frame) {
                Ok(index) => index,
                Err(reason) => {
                    output.incomplete = true;
                    output.limit_detail = format!("cannot index {instance}: {reason:?}");
                    break;
                }
            };
            let calls = call_orders.entry(instance).or_insert_with(|| {
                let mut calls = index.calls.clone();
                let order: HashMap<_, _> = mir::traversal::reverse_postorder(index.body).enumerate()
                    .map(|(ordinal, (block, _))| (block, ordinal)).collect();
                calls.sort_by_key(|location| (order.get(&location.block).copied().unwrap_or(usize::MAX),
                    facts.safe_candidates.get(&(instance, *location))
                    .into_iter().flatten().filter_map(|target| forbidden_distance.get(target)).copied()
                    .min().unwrap_or(usize::MAX)));
                calls
            });
            for (call_index, &location) in calls.iter().enumerate().skip(next_call) {
                let point = Point { frame, location };
                let terminator = index.body.basic_blocks[location.block].terminator();
                let span = terminator.source_info.span;
                let trusted = trust.allows(tcx, instance, index.body, terminator.source_info.scope, crate::trust::ScopeMark::Trusted);
                let allow_incomplete = trust.allows(tcx, instance, index.body, terminator.source_info.scope, crate::trust::ScopeMark::AllowIncomplete);
                let mut kind = match &terminator.kind {
                    TerminatorKind::Call { func, .. } | TerminatorKind::TailCall { func, .. } => {
                        if solver.ty(frame, func.ty(index.body, tcx)).is_fn_ptr() { "function-pointer call" } else { "call" }
                    }
                    TerminatorKind::Drop { .. } => "drop",
                    TerminatorKind::Assert { .. } => "assertion panic",
                    TerminatorKind::InlineAsm { .. } => "assembly",
                    _ => continue,
                };
                let direct = match &terminator.kind {
                    TerminatorKind::Call { func, .. } | TerminatorKind::TailCall { func, .. } => {
                        let ty = solver.ty(frame, func.ty(index.body, tcx));
                        if let ty::FnDef(def, args) = *ty.kind() {
                            Instance::try_resolve(tcx, TypingEnv::fully_monomorphized(), def,
                                tcx.instantiate_bound_regions_with_erased(args)).ok().flatten()
                                .filter(|target| !matches!(target.def, InstanceKind::Virtual(..)))
                        } else { None }
                    }
                    TerminatorKind::Drop { place, .. } => {
                        let ty = solver.ty(frame, place.ty(index.body, tcx).ty);
                        (!matches!(ty.kind(), ty::Dynamic(..))).then(|| Instance::resolve_drop_glue(tcx, ty))
                    }
                    _ => None,
                };
                if kind == "call" && direct.is_none() { kind = "virtual call"; }
                let context_contract = facts.context_calls.contains(&(instance, location));
                let context_safe = solver.context_call_safe(frame, location);
                if context_contract || context_safe || facts.covered_calls.contains(&(instance, location)) && facts.safe_candidates.get(&(instance, location)).is_some_and(|targets|
                    !targets.is_empty() && targets.iter().all(|target| facts.safe.contains(target))) {
                    let proof = if context_contract {
                        "trait call uses irq::context; its concrete implementations are separate required roots"
                    } else if context_safe {
                        "complete context-sensitive call coverage; every matching context has an IRQ-safe closure"
                    } else { "complete call-target coverage; every candidate has an IRQ-safe summary" };
                    *output.covered_calls.entry((instance, location, proof)).or_default() += 1;
                    continue;
                }
                let query_watch = trace_time.then(std::time::Instant::now);
                solver.begin_query(limits.query_steps);
                let mut attempts = 0;
                let mut cases = 0usize;
                let mut fallback_steps = 0usize;
                let mut control_failed = false;
                let mut advance = true;
                let mut pending = if let Some(query) = resume.take() {
                    solver.budget.steps = query.steps;
                    solver.budget.limit = query.limit;
                    solver.flow.scalar_steps = query.scalar_steps;
                    attempts = query.attempts;
                    cases = query.cases;
                    fallback_steps = query.fallback_steps;
                    control_failed = query.control_failed;
                    advance = query.advance;
                    query.pending
                } else {
                    queries += 1;
                    let indexes = inherited.iter().filter_map(|(frame, guard)| guard.index.map(|local| {
                        ((Point { frame: *frame, location: guard.branch }, local), guard.successor.as_usize() as u128)
                    })).collect();
                    let mut pending = vec![QueryCase { indexes, loop_entries: Box::new([]), cut_recursive_returns: false }];
                    pending.push(QueryCase { cut_recursive_returns: true, ..pending[0].clone() });
                    {
                        let mut loop_entries = Vec::new();
                        let mut seen = HashSet::new();
                        let mut ancestors = vec![point];
                        while let Some(at) = ancestors.pop() {
                            if !seen.insert(at) { continue; }
                            if let Ok(index) = solver.index(at.frame) {
                                for (&(_, header), blocks) in &index.loops {
                                    if blocks.contains(&at.location.block) {
                                        loop_entries.push(Point { frame: at.frame, location: Location { block: header, statement_index: 0 } });
                                    }
                                }
                            }
                            ancestors.extend(solver.flow.inputs.get(&at.frame).into_iter().flatten().copied());
                        }
                        loop_entries.sort_unstable_by_key(|point| (point.frame, point.location.block));
                        loop_entries.dedup();
                        if !loop_entries.is_empty() {
                            pending.push(QueryCase { indexes: pending[0].indexes.clone(), loop_entries: loop_entries.into_boxed_slice(), cut_recursive_returns: true });
                        }
                    }
                    if inherited.iter().any(|(_, guard)| guard.proof == query::GuardProof::Widened && !guard.phi) {
                        skipped_witnesses += pending.iter().filter(|case| case.cut_recursive_returns).count();
                        pending.retain(|case| !case.cut_recursive_returns);
                    }
                    pending
                };
                solver.budget.scratch = pending.iter().map(|choices|
                    choices.indexes.capacity() * std::mem::size_of::<((Point, mir::Local), u128)>() * 4
                        + choices.loop_entries.len() * std::mem::size_of::<Point>() * 4).sum();
                let starting_steps = solver.budget.steps;
                if scalar_witness {
                    solver.budget.limit = solver.budget.limit.min(starting_steps.saturating_add(limits.scalar_steps.saturating_sub(transfers)));
                }
                if queries % 100 == 0 {
                    if let Some(directory) = std::env::var_os("IRQ_CHECK_DUMP_DIR") {
                        let path = std::path::PathBuf::from(directory).join(format!("{}-queries-{worker}.txt", tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE)));
                        let _ = std::fs::write(path, format!("queries={queries} transfers={transfers} frames={} nodes={} pending={} instance={instance}", solver.frames.len(), output.graph.len(), queue.len()));
                    }
                }
                if queries % 10000 == 0 && std::env::var_os("IRQ_CHECK_TRACE_MEMORY").is_some() {
                    let edges = output.graph.values().map(Vec::len).sum::<usize>();
                    let edge_storage = output.graph.values().map(|edges| edges.capacity() * std::mem::size_of::<Edge<'tcx>>()).sum::<usize>();
                    let details = output.graph.values().flatten().map(|edge| match &edge.detail {
                        std::borrow::Cow::Owned(detail) => detail.capacity(),
                        std::borrow::Cow::Borrowed(_) => 0,
                    }).sum::<usize>();
                    let uncertainty = output.graph.values().flatten().map(|edge|
                        edge.uncertainty.capacity() * std::mem::size_of::<(rustc_span::Span, &'static str)>()).sum::<usize>();
                    let sites = output.sites.capacity() * std::mem::size_of::<CallSite<'tcx>>()
                        + evidence_loops.capacity() * std::mem::size_of::<Arc<[Point]>>()
                        + evidence_loops.iter().map(|entries| entries.len() * std::mem::size_of::<Point>()
                            + std::mem::size_of::<usize>() * 2).sum::<usize>()
                        + site_sources.capacity() * std::mem::size_of::<((Instance<'tcx>, Location), Arc<CallSiteSource>)>()
                        + site_sources.values().map(|source| std::mem::size_of::<CallSiteSource>()
                            + std::mem::size_of::<usize>() * 2
                            + source.normal.capacity() * std::mem::size_of::<Location>()).sum::<usize>();
                    let precision = output.precision_loss.capacity() * std::mem::size_of::<(String, usize)>()
                        + output.precision_loss.keys().map(String::capacity).sum::<usize>();
                    let mut frame_counts = HashMap::<_, usize>::new();
                    for frame in &solver.frames { *frame_counts.entry(frame.instance).or_default() += 1; }
                    let mut frame_counts: Vec<_> = frame_counts.into_iter().collect();
                    frame_counts.sort_unstable_by_key(|(_, count)| std::cmp::Reverse(*count));
                    eprintln!("irq-check: retained worker={worker} queries={queries} frames={} edges={edges} edge-bytes={edge_storage} detail-bytes={details} uncertainty-bytes={uncertainty} site-bytes={sites} precision-bytes={precision} control-bytes={} effect-bytes={}",
                        solver.frames.len(), solver.controls.bytes, solver.effects.bytes);
                    for (instance, count) in frame_counts.into_iter().take(12) {
                        eprintln!("irq-check: retained-frame worker={worker} count={count} instance={instance}");
                    }
                }
                let mut established = HashSet::new();
                let mut control_stop = None;
                let setup_time = query_watch.map_or(0, |watch| watch.elapsed().as_nanos());
                let answer = loop {
                    let answer = (|| {
                        let mut complete = Vec::new();
                        let mut unique = HashSet::new();
                        while let Some(choices) = pending.pop() {
                        if solver.index_choices != choices.indexes || solver.loop_entries != choices.loop_entries
                            || solver.cut_recursive_returns != choices.cut_recursive_returns {
                            solver.index_choices = choices.indexes;
                            solver.loop_entries = choices.loop_entries;
                            solver.cut_recursive_returns = choices.cut_recursive_returns;
                            solver.retain_completed();
                        }
                        solver.input_choices = inherited.iter().filter_map(|(frame, guard)|
                            guard.input.then_some((*frame, guard.successor.as_usize()))).collect();
                        cases += 1;
                        let mode = if solver.cut_recursive_returns { RequestMode::OptionalRefinement }
                            else { RequestMode::RequiredBinding };
                        let revision = solver.flow.generation;
                        let answer = solver.request(point, mode, limits.scalar_steps, |solver| {
                        let paths = solver.control_summary(point)?;
                        control_stop = solver.controls.last_stop.filter(|(stopped, _)| *stopped == point).map(|(_, reason)| reason);
                        if paths.is_empty() { return Ok(Vec::new()); }
                        let mut reaching = Vec::new();
                        for path in paths {
                            if let Some(guards) = solver.merge(&path.guards, &inherited)? {
                                reaching.push(guards);
                            }
                        }
                        if solver.cut_recursive_returns {
                            let before = reaching.len();
                            reaching.retain(|guards| !guards.iter().any(|(_, guard)|
                                guard.proof == query::GuardProof::Widened && !guard.phi));
                            skipped_witnesses += usize::from(before != 0 && reaching.is_empty());
                        }
                        if reaching.is_empty() { return Ok(Vec::new()); }
                        solver.controls.dependency_stop = None;
                        let targets = match solver.request(point, RequestMode::RequiredBinding, 0, |solver| solver.targets(point)) {
                            ReadOutcome::Complete(targets) => targets,
                            ReadOutcome::Stopped(reason) => return Err(reason),
                        };
                        let mut values = Vec::new();
                        for guards in reaching {
                            for target in &targets {
                                if let Some(guards) = solver.merge(&guards, &target.guards)? {
                                    values.push(Alternative { value: target.value, guards });
                                }
                            }
                        }
                        for value in &mut values {
                            for (_, guard) in &mut value.guards { guard.required = true; }
                        }
                        solver.feasible(values)
                        });
                        let answer = match answer {
                            ReadOutcome::Complete(values) => Ok(values),
                            ReadOutcome::Stopped(reason) => Err(reason),
                        };
                        let answer = if revision != solver.flow.generation && answer != Err(QueryStop::ProcessLimit) {
                            Err(QueryStop::ContextChanged)
                        } else { answer };
                        match answer {
                            Ok(values) => {
                                for value in values {
                                    if solver.cut_recursive_returns
                                        && (!matches!(value.value, Value::Function(_)) || !value.path_proven())
                                    { continue; }
                                    solver.budget.charge((std::mem::size_of::<Alternative<'tcx>>()
                                        + value.guards.len() * std::mem::size_of::<(usize, Guard)>()) * 4)?;
                                    if matches!(value.value, Value::Function(_)) { established.insert(value.clone()); }
                                    if unique.insert(value.clone()) { complete.push(value); }
                                }
                                break;
                            }
                            Err(QueryStop::SplitIndex) => {
                                let (point, local, length) = solver.split_index.take().ok_or(QueryStop::Unsupported)?;
                                solver.retain_completed();
                                let length = usize::try_from(length).map_err(|_| QueryStop::ScratchLimit)?;
                                let bytes = length.checked_mul((solver.index_choices.len() + 1)
                                    * std::mem::size_of::<((Point, mir::Local), u128)>() * 4
                                    + solver.loop_entries.len() * std::mem::size_of::<Point>() * 4).ok_or(QueryStop::ScratchLimit)?;
                                solver.budget.charge(bytes)?;
                                for index in (0..length).rev() {
                                    let mut choices = solver.index_choices.clone();
                                    choices.insert((point, local), index as u128);
                                    pending.push(QueryCase { indexes: choices, loop_entries: solver.loop_entries.clone(), cut_recursive_returns: solver.cut_recursive_returns });
                                }
                            }
                            Err(QueryStop::ContextChanged) => {
                                complete.clear();
                                unique.clear();
                                established.clear();
                                pending.clear();
                                pending.push(QueryCase {
                                    indexes: inherited.iter().filter_map(|(frame, guard)| guard.index.map(|local|
                                        ((Point { frame: *frame, location: guard.branch }, local), guard.successor.as_usize() as u128))).collect(),
                                    loop_entries: Box::new([]), cut_recursive_returns: false,
                                });
                                solver.retain_completed();
                            }
                            Err(QueryStop::RefinementLimit | QueryStop::AlternativeLimit | QueryStop::ScratchLimit | QueryStop::ControlMemory)
                                if solver.cut_recursive_returns =>
                            {
                                pending.retain(|case| !case.cut_recursive_returns);
                                break;
                            }
                            Err(reason) => {
                                pending.push(QueryCase { indexes: solver.index_choices.clone(), loop_entries: solver.loop_entries.clone(), cut_recursive_returns: solver.cut_recursive_returns });
                                return Err(reason);
                            }
                        }
                        }
                        Ok(complete)
                    })();
                    if answer == Err(QueryStop::TransferLimit)
                        && !pending.contains(&QueryCase { indexes: solver.index_choices.clone(), loop_entries: solver.loop_entries.clone(), cut_recursive_returns: solver.cut_recursive_returns }) {
                        pending.push(QueryCase { indexes: solver.index_choices.clone(), loop_entries: solver.loop_entries.clone(), cut_recursive_returns: solver.cut_recursive_returns });
                    }
                    if answer == Err(QueryStop::TransferLimit) && attempts == 0 && !scalar_witness {
                        attempts += 1;
                        solver.budget.limit = limits.query_steps * 9;
                        continue;
                    }
                    break answer;
                };
                solver.index_choices.clear();
                let precise_steps = solver.budget.steps;
                let dependency_stop = solver.controls.dependency_stop;
                let diagnostics = match &answer {
                    Err(reason) => solver.explain_read(Some(*reason)),
                    Ok(values) if values.iter().any(|value| matches!(value.value, Value::Unknown(_))) => solver.explain_read(None),
                    _ => Vec::new(),
                };
                let diagnostics = (!diagnostics.is_empty()).then(|| Arc::from(diagnostics));
                let fallback_start = query_watch.map_or(0, |watch| watch.elapsed().as_nanos());
                let fallback = if !scalar_witness && fallback_steps < limits.query_steps
                    && !answer.as_ref().is_ok_and(|values| values.iter()
                    .all(|value| matches!(value.value, Value::Function(_))))
                    && answer != Err(QueryStop::ProcessLimit)
                {
                    solver.conservative_targets(point, limits.query_steps - fallback_steps)
                } else { Err(QueryStop::Unsupported) };
                let fallback_used = solver.budget.steps.saturating_sub(precise_steps);
                fallback_steps += fallback_used;
                transfers += precise_steps.saturating_sub(starting_steps) + fallback_used;
                if let Some(watch) = query_watch {
                    let elapsed = watch.elapsed().as_nanos();
                    let time = query_times.entry((instance, location)).or_default();
                    time.0 += 1;
                    time.1 += elapsed;
                    time.2 += setup_time;
                    time.3 += elapsed.saturating_sub(fallback_start);
                    time.4 += precise_steps.saturating_sub(starting_steps) + fallback_used;
                    if queries % 10000 == 0 {
                        let mut ranked: Vec<_> = query_times.iter().collect();
                        ranked.sort_unstable_by_key(|(_, time)| std::cmp::Reverse(time.1));
                        eprintln!("irq-check: timing worker={worker} queries={queries} safety-checks={safety_checks} safety-hits={safety_hits} safety-ms={} safety-transfers={safety_transfers}", safety_time / 1000000);
                        for ((instance, location), (count, total, setup, fallback, steps)) in ranked.into_iter().take(16) {
                            eprintln!("irq-check: timing-site worker={worker} count={count} total-ms={} setup-ms={} fallback-ms={} steps={steps} {instance} at={location:?}", total / 1000000, setup / 1000000, fallback / 1000000);
                        }
                    }
                }
                solver.budget.steps = precise_steps;
                if fallback == Err(QueryStop::ProcessLimit) {
                    output.incomplete = true;
                    output.limit_detail = irq_check_rupta::limits::stop_reason().unwrap_or("analysis stopped").into();
                    break;
                }
                if answer == Err(QueryStop::ProcessLimit) {
                    output.incomplete = true;
                    output.limit_detail = irq_check_rupta::limits::stop_reason().unwrap_or("analysis stopped").into();
                    break;
                }
                let result = match &answer {
                    Ok(values) => Ok([
                        values.len(), values.iter().filter(|value| matches!(value.value, Value::Function(_))).count(),
                        values.iter().filter(|value| value.path_proven()).count(),
                        values.iter().map(|value| value.guards.len()).sum::<usize>()]),
                    Err(reason) => Err(*reason),
                };
                let loop_entries = if let Some(entries) = evidence_loops.get(solver.loop_entries.as_ref()) {
                    entries.clone()
                } else {
                    let entries: Arc<[Point]> = Arc::from(solver.loop_entries.as_ref());
                    evidence_loops.insert(entries.clone());
                    entries
                };
                let evidence = QueryEvidence {
                    steps: precise_steps + fallback_steps, attempts: attempts + 1, cases,
                    loop_entries, cut_recursive_returns: solver.cut_recursive_returns,
                    witness: witness_value, result, conservative_safe: false, worker: None,
                };
                if std::env::var_os("IRQ_CHECK_TRACE_FLOW_VALUES").is_some() {
                    eprintln!("irq-check: query values frame={frame} at={location:?} result={answer:?}");
                }
                let id = output.sites.len();
                let source = site_sources.entry((instance, location)).or_insert_with(|| Arc::new(CallSiteSource {
                    location, scope: terminator.source_info.scope,
                    normal: terminator.successors().filter(|block| {
                        !matches!(terminator.unwind(), Some(mir::UnwindAction::Cleanup(cleanup)) if cleanup == block)
                    }).map(|block| Location { block, statement_index: 0 }).collect(),
                    unwind: terminator.unwind().and_then(|unwind| match unwind { mir::UnwindAction::Cleanup(block) => Some(*block), _ => None }),
                })).clone();
                output.sites.push(CallSite { caller, state: id, source, evidence, diagnostics });
                if std::env::var_os("IRQ_CHECK_TRACE").is_some() {
                    eprintln!("irq-check: query frame={frame} {instance} at={location:?} {}", output.site_state(id));
                }
                if let Ok(targets) = fallback {
                    if !targets.is_empty() && targets.iter().all(|target| facts.safe.contains(target)) {
                        output.sites[id].evidence.conservative_safe = true;
                        for target in targets {
                            output.graph.entry(caller).or_default().push(Edge {
                                candidate: true, site: Some(id), target: Some(Node { instance: target, context: usize::MAX }),
                                span, kind, detail: "covered by the conservative IRQ-safe target closure".into(), trusted: false,
                                allow_incomplete, path_proven: false, uncertainty: Vec::new(),
                            });
                        }
                        continue;
                    }
                }
                let unfinished = answer.is_ok() && !pending.is_empty();
                let values = match answer {
                    Ok(values) => values,
                    Err(reason) => {
                        let detail = format!("the pointer query has no complete binding: {reason:?}");
                        *output.precision_loss.entry(format!("{instance} at {location:?}: {detail}")).or_default() += 1;
                        let mut uncertainty = Vec::new();
                        if let Some((cause, stop)) = dependency_stop
                            && stop == reason
                            && let Ok(index) = solver.index(cause.frame)
                        {
                            let block = &index.body.basic_blocks[cause.location.block];
                            let source = block.statements.get(cause.location.statement_index)
                                .map_or(block.terminator().source_info.span, |statement| statement.source_info.span);
                            let reason = match stop {
                                QueryStop::ControlMemory => "a possible write to entry storage prevents a control-flow proof here",
                                QueryStop::TransferLimit | QueryStop::RefinementLimit => "analysis step limit reached here",
                                QueryStop::ScratchLimit => "analysis storage limit reached here",
                                QueryStop::AlternativeLimit => "analysis alternative limit reached here",
                                _ => "analysis could not determine this dependency",
                            };
                            uncertainty.push((source.source_callsite(), reason));
                        }
                        output.graph.entry(caller).or_default().push(Edge {
                            candidate: false, site: Some(id), target: None, span, kind, detail: detail.into(), trusted,
                            allow_incomplete, path_proven: false, uncertainty,
                        });
                        established.into_iter().collect()
                    }
                };
                if let Some(reason) = control_stop {
                    *output.precision_loss.entry(format!("{instance} at {location:?}: control refinement stopped after {reason:?}")).or_default() += 1;
                }
                for value in values {
                    let mut uncertainty = Vec::new();
                    for &(frame, guard) in &value.guards {
                        if matches!(guard.proof, query::GuardProof::Established | query::GuardProof::Predicate)
                            || guard.proof != query::GuardProof::Widened
                                && (guard.input || guard.phi || guard.index.is_some() || guard.choice.is_some())
                        { continue; }
                        let point = Point { frame, location: guard.branch };
                        let (cause, reason) = if let Some(&(cause, stop)) = solver.controls.causes.get(&point) {
                            (cause, match stop {
                                QueryStop::ControlMemory => "a possible write to entry storage prevents a control-flow proof here",
                                QueryStop::TransferLimit | QueryStop::RefinementLimit => "analysis step limit reached here",
                                QueryStop::ScratchLimit => "analysis storage limit reached here",
                                QueryStop::AlternativeLimit => "analysis alternative limit reached here",
                                _ => "analysis could not determine this dependency",
                            })
                        } else {
                            (point, if guard.proof == query::GuardProof::Widened {
                                "analysis could not retain the control-flow relation here"
                            } else { "analysis could not determine this condition" })
                        };
                        if let Ok(index) = solver.index(cause.frame) {
                            let block = &index.body.basic_blocks[cause.location.block];
                            let source = block.statements.get(cause.location.statement_index)
                                .map_or(block.terminator().source_info.span, |statement| statement.source_info.span);
                            let cause = (source.source_callsite(), reason);
                            if !uncertainty.contains(&cause) { uncertainty.push(cause); }
                        }
                    }
                    let Value::Function(mut target) = value.value else {
                        output.graph.entry(caller).or_default().push(Edge {
                            candidate: false, site: Some(id), target: None, span, kind,
                            detail: match value.value { Value::Unknown(reason) => reason.into(), _ => "the target has no established binding".into() }, trusted,
                            allow_incomplete, path_proven: false, uncertainty,
                        });
                        continue;
                    };
                    if matches!(target.def, InstanceKind::Intrinsic(_)) {
                        if let Some(requirement) = ty::layout::ValidityRequirement::from_intrinsic(tcx.item_name(target.def_id())) {
                            if tcx.check_validity_requirement((requirement, TypingEnv::fully_monomorphized().as_query_input(target.args.type_at(0))))
                                .is_ok_and(|valid| !valid) {
                                if let Some(def) = tcx.lang_items().get(rustc_hir::attrs::lang_items::LangItem::PanicNounwind) {
                                    target = Instance::mono(tcx, def);
                                }
                            }
                        }
                    }
                    let leaf = facts.safe.contains(&target) || crate::forbidden(tcx, target).is_some() || !trust.checks(target.def_id())
                        || crate::known_intrinsic(tcx, target)
                        || matches!(target.def, InstanceKind::Shim(ty::ShimKind::DropGlue(_, None)))
                        || matches!(target.def, InstanceKind::Intrinsic(_)) && LEAF_INTRINSICS.contains(&tcx.item_name(target.def_id()).as_str());
                    let opaque = !leaf && (tcx.is_foreign_item(target.def_id())
                        || matches!(target.def, InstanceKind::Intrinsic(_) | InstanceKind::LlvmIntrinsic(_) | InstanceKind::Virtual(..))
                        || !irq_check_rupta::util::type_util::has_instance_mir(tcx, target.def));
                    let dependent = !leaf || !binding_independent.contains(&target);
                    let registered = diagnostic_inputs.get(&target).and_then(|inputs| inputs.get(&point)).copied();
                    let widened = !leaf && !opaque && !diagnostic_independent.contains(&target)
                        && registered.is_none() && diagnostic_inputs.get(&target)
                            .is_some_and(|inputs| inputs.len() >= limits.alternatives.clamp(1, 16));
                    let shared = !leaf && !opaque && (diagnostic_independent.contains(&target) || widened);
                    let context = if shared {
                        if let Some(&context) = diagnostic_frames.get(&target) {
                            diagnostic_reuse += 1;
                            context
                        } else {
                            let context = solver.frame(Frame { instance: target, caller: None, irq: true, unknown_entry: true });
                            diagnostic_frames.insert(target, context);
                            context
                        }
                    } else if let Some(context) = registered { context } else {
                        let context = solver.frame(Frame { instance: target, caller: (dependent || opaque).then_some(point), irq: true, unknown_entry: false });
                        if !leaf && !opaque { diagnostic_inputs.entry(target).or_default().insert(point, context); }
                        context
                    };
                    if widened {
                        diagnostic_widened += 1;
                        uncertainty.push((span.source_callsite(), "caller inputs were discarded at this call because the function reached its retained-caller limit"));
                    }
                    let node = Node { instance: target, context };
                    output.graph.entry(caller).or_default().push(Edge {
                        candidate: false, site: Some(id), target: Some(node), span, kind,
                        detail: "the target has an established pointer binding".into(), trusted,
                        allow_incomplete, path_proven: value.path_proven(), uncertainty,
                    });
                    if crate::forbidden(tcx, target).is_some() && generation == solver.flow.generation {
                        let mut pending: Vec<_> = output.roots.values().copied().collect();
                        let mut seen = HashSet::new();
                        while let Some(reached) = pending.pop() {
                            if !seen.insert(reached) { continue; }
                            if reached == node { output.diagnostic_cutoff = true; break; }
                            pending.extend(output.graph.get(&reached).into_iter().flatten()
                                .filter(|edge| !edge.candidate && edge.path_proven).filter_map(|edge| edge.target));
                        }
                    }
                    if opaque {
                        output.graph.entry(node).or_default().push(Edge {
                            candidate: false, site: Some(id), target: None, span, kind: "unsupported body",
                            detail: format!("the target has no checked call model: {target}").into(), trusted,
                            allow_incomplete: false, path_proven: false, uncertainty: Vec::new(),
                        });
                    } else if !leaf {
                        let mut ancestor = (!shared).then_some(frame);
                        let mut depth = 0usize;
                        while let Some(parent) = ancestor {
                            depth += 1;
                            if depth >= limits.call_depth { break; }
                            ancestor = solver.frames[parent].caller.map(|point| point.frame);
                        }
                        if depth >= limits.call_depth {
                            output.graph.entry(node).or_default().push(Edge {
                                candidate: false, site: Some(id), target: None, span, kind: "call depth",
                                detail: "the binding exceeds the call-depth limit".into(), trusted: false,
                                allow_incomplete: false, path_proven: false, uncertainty: Vec::new(),
                            });
                        } else {
                            let guards = if dependent && !shared {
                                vec![solver.entry_guard(context, point)]
                            } else { Vec::new() };
                            queue.push_back((context, guards, 0, None));
                        }
                    }
                }
                if candidate_sites.insert((caller, location)) {
                for &target in candidates.get(&(instance, location)).into_iter().flatten() {
                    output.graph.entry(caller).or_default().push(Edge {
                        candidate: true, site: Some(id), target: Some(Node { instance: target, context: usize::MAX }),
                        span, kind, detail: "global pointer candidate; not an established IRQ binding".into(), trusted: false,
                        allow_incomplete, path_proven: false, uncertainty: Vec::new(),
                    });
                }
                }
                if output.diagnostic_cutoff { break; }
                if unfinished {
                    let query = QueryContinuation {
                        pending, steps: precise_steps, limit: solver.budget.limit, fallback_steps,
                        scalar_steps: std::mem::take(&mut solver.flow.scalar_steps), attempts, cases,
                        control_failed,
                        advance: false,
                    };
                    if query.pending.last().is_some_and(|case| !case.cut_recursive_returns) {
                        deferred.push_back((frame, inherited.clone(), call_index, Some(query)));
                    } else {
                        queue.push_back((frame, inherited.clone(), call_index, Some(query)));
                    }
                }
                if advance && call_index + 1 < calls.len() {
                    queue.push_back((frame, inherited.clone(), call_index + 1, None));
                }
                break;
            }
            if output.incomplete || output.diagnostic_cutoff { break; }
        }
        if output.incomplete || output.diagnostic_cutoff || generation == solver.flow.generation { break; }
        generation = solver.flow.generation;
        output.graph.clear();
        output.sites.clear();
        evidence_loops.clear();
        output.precision_loss.clear();
        output.specialized_safe.clear();
        output.covered_calls.clear();
        queued.clear();
        candidate_sites.clear();
        for &instance in roots {
            let context = output.roots[&instance].context;
            queue.push_back((context, Vec::new(), 0, None));
        }
        }
        if !scalar_witness && !output.incomplete && !output.diagnostic_cutoff
            && output.graph.values().flatten().all(|edge| edge.candidate || edge.target.is_some_and(|target|
                crate::forbidden(tcx, target.instance).is_none()))
        {
            output.safe_summaries.extend(roots.iter().copied());
        }
        if std::env::var_os("IRQ_CHECK_STATS").is_some() {
            eprintln!("irq-check: shared diagnostic summaries functions={} reuse={diagnostic_reuse} widened={diagnostic_widened}", diagnostic_frames.len());
            eprintln!("irq-check: specialized safety checks={safety_checks} hits={safety_hits} transfers={safety_transfers}");
            eprintln!("irq-check: covered call summaries sites={} observations={}", output.covered_calls.len(), output.covered_calls.values().sum::<usize>());
            eprintln!("irq-check: effect summaries hits={} returned={} writes={} loads={} bytes={} cycles={} iterations={}",
                solver.effects.hits, solver.effects.returned, solver.effects.writes, solver.effects.loads, solver.effects.bytes,
                solver.effects.cycles, solver.effects.iterations);
            eprintln!("irq-check: storage impact queries={} hits={} preserved={} unresolved={} stops={} bytes={} local-hits={} local-bytes={}",
                solver.impacts.queries, solver.impacts.hits, solver.impacts.preserved,
                solver.impacts.unresolved, solver.impacts.stops, solver.impacts.bytes,
                solver.impacts.local_hits, solver.impacts.local_bytes);
            eprintln!("irq-check: control summaries evaluations={} hits={} widened={} refinement-stops={} bytes={} condition-hits={}",
                solver.controls.evaluations, solver.controls.hits, solver.controls.widened,
                solver.controls.refinement_stops, solver.controls.bytes, solver.controls.condition_hits);
            eprintln!("irq-check: value-flow evaluations={} changes={} revisions={} caller-locations={} static-allocations={}",
                solver.flow.evaluations, solver.flow.changes, solver.flow.generation,
                solver.caller_locations.len(), solver.static_allocations.len());
            eprintln!("irq-check: value-flow requirements visits={} repairs={} full-nodes={} affected-nodes={} repair-visits={}",
                solver.flow.requirement_visits, solver.flow.requirement_repairs,
                solver.flow.requirement_repair_full, solver.flow.requirement_repair_affected,
                solver.flow.requirement_repair_visits);
            eprintln!("irq-check: IRQ queries={queries} transfers={transfers} frames={} sites={} unknown_coverage={} diagnostic_cutoff={} root_scalar_witness={witness_value:?} skipped_witnesses={skipped_witnesses}",
                solver.frames.len(), output.sites.len(), output.precision_loss.values().sum::<usize>(), output.diagnostic_cutoff);
        }
        output
    }

    pub fn site_state(&self, id: usize) -> String {
        let evidence = &self.sites[id].evidence;
        let result = match evidence.result {
            Ok([values, functions, proven, guards]) => format!(
                "values={values} function_values={functions} proven_paths={proven} guard_entries={guards}"),
            Err(reason) => format!("stopped={reason:?}"),
        };
        let worker = evidence.worker.map_or_else(String::new, |worker| format!("worker={worker} "));
        let suffix = if evidence.conservative_safe { "; complete conservative target closure is IRQ-safe" } else { "" };
        format!("{worker}steps={} attempts={} cases={} loop_entries={:?} cut_recursive_returns={} root_scalar_witness={:?} {result}{suffix}",
            evidence.steps, evidence.attempts, evidence.cases, evidence.loop_entries, evidence.cut_recursive_returns, evidence.witness)
    }
}
