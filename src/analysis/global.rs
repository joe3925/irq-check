use crate::trust::Trust;
use irq_check_rupta::mir::function::GenericArgE;
use irq_check_rupta::pta::PointerAnalysis;
use irq_check_rupta::pts_set::points_to::PointsToSet;
use rustc_middle::mir::Location;
use rustc_middle::ty::{self, Instance, TyCtxt};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum PointerObject<'tcx> {
    Local(Instance<'tcx>, rustc_middle::mir::Local),
    Heap(Instance<'tcx>, Location),
    Static(rustc_hir::def_id::DefId),
}

pub(super) struct PointerNode<'tcx> {
    pub object: Option<PointerObject<'tcx>>,
    pub context: Option<usize>,
    pub projection: Vec<irq_check_rupta::mir::path::PathSelector>,
    pub offset: usize,
    pub ty: Option<ty::Ty<'tcx>>,
    pub function: Option<Instance<'tcx>>,
    pub targets: Vec<usize>,
    pub complete: bool,
}

pub(super) struct PointerFacts<'tcx> {
    pub nodes: Vec<PointerNode<'tcx>>,
    pub local_nodes: HashMap<(usize, rustc_middle::mir::Local), Vec<usize>>,
    pub complete: bool,
    pub call_complete: bool,
    pub covered_calls: HashSet<(Instance<'tcx>, Location)>,
    pub pointer_calls: HashSet<(Instance<'tcx>, Location)>,
    pub context_calls: HashSet<(Instance<'tcx>, Location)>,
    pub instance_contexts: HashMap<Instance<'tcx>, Vec<usize>>,
    pub context_bindings: HashMap<(usize, Location), Vec<(Instance<'tcx>, usize)>>,
    pub safe_contexts: HashSet<usize>,
    pub safe_candidates: HashMap<(Instance<'tcx>, Location), HashSet<Instance<'tcx>>>,
    pub safe: HashSet<Instance<'tcx>>,
    pub exclusions: HashSet<Instance<'tcx>>,
    pub pointees: HashMap<(Instance<'tcx>, rustc_middle::mir::Local), HashSet<PointerObject<'tcx>>>,
    pub pointee_ranges: HashMap<(Instance<'tcx>, rustc_middle::mir::Local), HashSet<(PointerObject<'tcx>, Option<(u64, u64)>)>>,
}

pub(super) struct GlobalPointers<'tcx> {
    pub exported: HashMap<String, Instance<'tcx>>,
    pub roots: Vec<Instance<'tcx>>,
    pub candidates: HashMap<(Instance<'tcx>, Location), HashSet<Instance<'tcx>>>,
    pub failure: Option<String>,
    pub binding_independent: HashSet<Instance<'tcx>>,
    pub diagnostic_independent: HashSet<Instance<'tcx>>,
    pub facts: PointerFacts<'tcx>,
}

impl<'tcx> GlobalPointers<'tcx> {
    pub fn build(
        tcx: TyCtxt<'tcx>,
        trust: &Trust,
        instances: &[Instance<'tcx>],
        roots: &[Instance<'tcx>],
    ) -> Self {
        let mut callable_instances: HashSet<_> = instances.iter().copied().collect();
        let mut backend = irq_check_rupta::pta::context_sensitive::ContextSensitivePTA::new(
            irq_check_rupta::mir::analysis_context::AnalysisContext::new(
                tcx.sess,
                tcx,
                irq_check_rupta::util::options::AnalysisOptions {
                    context_depth: 0,
                    rceus: true,
                    dump_stats: false,
                    ..Default::default()
                },
                Vec::new(),
                tcx.crates(())
                    .iter()
                    .copied()
                    .chain(std::iter::once(rustc_hir::def_id::LOCAL_CRATE))
                    .collect(),
            ),
            irq_check_rupta::pta::strategies::context_strategy::RCEUSCallSiteSensitive::new(0),
        );
        let entry = tcx.entry_fn(()).map(|(def, _)| def);
        let mut functions: HashSet<_> = instances
            .iter()
            .copied()
            .filter(|instance| {
                trust.checks(instance.def_id())
                    && (entry == Some(instance.def_id())
                        || crate::context_method(tcx, instance.def_id())
                        || matches!(
                            tcx.def_kind(instance.def_id()),
                            rustc_hir::def::DefKind::Fn | rustc_hir::def::DefKind::AssocFn
                        ) && tcx
                            .codegen_fn_attrs(instance.def_id())
                            .contains_extern_indicator()
                            && tcx.lang_items().panic_impl() != Some(instance.def_id())
                            && !(tcx.codegen_fn_attrs(instance.def_id()).flags.contains(
                                rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags::RUSTC_STD_INTERNAL_SYMBOL)
                                && tcx.codegen_fn_attrs(instance.def_id()).flags.intersects(
                                    rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags::ALLOCATOR
                                        | rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags::ALLOCATOR_ZEROED
                                        | rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags::DEALLOCATOR
                                        | rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags::REALLOCATOR)))
            })
            .collect();
        functions.extend(roots.iter().copied());
        if std::env::var_os("IRQ_CHECK_TRACE_HAVOC").is_some() {
            for instance in &functions {
                eprintln!("irq-check: discovery root: {instance}");
            }
        }
        let mut exported: HashMap<_, _> = instances
            .iter()
            .filter(|instance| {
                !tcx.is_foreign_item(instance.def_id())
                    && matches!(
                        tcx.def_kind(instance.def_id()),
                        rustc_hir::def::DefKind::Fn | rustc_hir::def::DefKind::AssocFn
                    )
            })
            .map(|instance| (tcx.symbol_name(*instance).name.to_owned(), *instance))
            .collect();
        {
            use rustc_ast::expand::allocator::{default_fn_name, global_fn_name};
            use rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags;
            use rustc_middle::middle::exported_symbols::ExportedSymbol;
            use rustc_symbol_mangling::mangle_internal_symbol;
            let local_definitions = exported.len();
            let dependencies = tcx.dependency_formats(());
            for &krate in tcx.crates(()) {
                if !dependencies.values().any(|list| {
                    list.get(krate).is_some_and(|linkage| {
                        *linkage != rustc_middle::middle::dependency_format::Linkage::NotLinked
                    })
                }) {
                    continue;
                }
                for &(symbol, _) in tcx.exported_non_generic_symbols(krate) {
                    let ExportedSymbol::NonGeneric(def) = symbol else {
                        continue;
                    };
                    if !matches!(
                        tcx.def_kind(def),
                        rustc_hir::def::DefKind::Fn | rustc_hir::def::DefKind::AssocFn
                    ) || tcx.is_foreign_item(def)
                    {
                        continue;
                    }
                    let instance = Instance::mono(tcx, def);
                    exported
                        .entry(tcx.symbol_name(instance).name.to_owned())
                        .or_insert(instance);
                }
            }
            if std::env::var_os("IRQ_CHECK_STATS").is_some() {
                eprintln!(
                    "irq-check: linked Rust definitions local={local_definitions} dependency={}",
                    exported.len() - local_definitions
                );
            }
            let methods = rustc_codegen_ssa::base::allocator_kind_for_codegen(tcx)
                .map(|kind| rustc_codegen_ssa::base::allocator_shim_contents(tcx, kind))
                .unwrap_or_default();
            for method in methods.into_iter().map(|method| method.name) {
                let default_name = default_fn_name(method);
                let destination = mangle_internal_symbol(tcx, &default_name);
                let mut target = exported.get(&destination).copied();
                if target.is_none() {
                    for &krate in tcx.crates(()) {
                        if !matches!(tcx.crate_name(krate).as_str(), "alloc" | "std") {
                            continue;
                        }
                        let mut modules = vec![krate.as_def_id()];
                        let mut visited = HashSet::new();
                        while let Some(module) = modules.pop() {
                            if !visited.insert(module) {
                                continue;
                            }
                            for child in tcx.module_children(module) {
                                let Some(def) = child.res.opt_def_id() else {
                                    continue;
                                };
                                if def.krate != krate {
                                    continue;
                                }
                                if tcx.def_kind(def) == rustc_hir::def::DefKind::Mod {
                                    modules.push(def);
                                    continue;
                                }
                                if tcx.def_kind(def) != rustc_hir::def::DefKind::Fn
                                    || tcx.is_foreign_item(def)
                                    || !tcx
                                        .opt_item_name(def)
                                        .is_some_and(|name| name.as_str() == default_name)
                                    || !tcx
                                        .codegen_fn_attrs(def)
                                        .flags
                                        .contains(CodegenFnAttrFlags::RUSTC_STD_INTERNAL_SYMBOL)
                                {
                                    continue;
                                }
                                let instance = Instance::mono(tcx, def);
                                if tcx.symbol_name(instance).name == destination {
                                    target = Some(instance);
                                    break;
                                }
                            }
                            if target.is_some() {
                                break;
                            }
                        }
                        if target.is_some() {
                            break;
                        }
                    }
                }
                if let Some(target) = target {
                    let source = mangle_internal_symbol(tcx, &global_fn_name(method));
                    if std::env::var_os("IRQ_CHECK_STATS").is_some() {
                        eprintln!("irq-check: compiler allocator shim {source} -> {target}");
                    }
                    exported.entry(source).or_insert(target);
                }
            }
        }
        let mut ordered: Vec<_> = functions.iter().copied().collect();
        callable_instances.extend(exported.values().copied());
        ordered.sort_by_key(ToString::to_string);
        backend.acx.discovery_roots = ordered.clone();
        backend.acx.linked_definitions = exported
            .iter()
            .filter(|(_, instance)| !tcx.is_foreign_item(instance.def_id()))
            .map(|(symbol, instance)| (symbol.clone(), *instance))
            .collect();
        backend.pre_analysis();
        backend.initialize();
        backend.propagate();
        let mut scalar_writes = HashMap::<_, Vec<_>>::new();
        for (base, projection) in &backend.acx.scalar_writes {
            scalar_writes
                .entry(base.clone())
                .or_default()
                .push(projection);
        }
        if !backend.acx.offset_risks.is_empty() {
            use irq_check_rupta::graph::pag::PAGEdgeEnum;
            let mut uses = HashMap::<_, Vec<_>>::new();
            let mut sensitive: HashSet<_> = backend
                .assoc_calls
                .dynamic_dispatch_calls
                .keys()
                .chain(backend.assoc_calls.dynamic_fntrait_calls.keys())
                .chain(backend.assoc_calls.fnptr_calls.keys())
                .copied()
                .collect();
            for edge in backend.pag.graph().edge_indices() {
                if !irq_check_rupta::limits::checkpoint() {
                    break;
                }
                let (source, destination) = backend.pag.graph().edge_endpoints(edge).unwrap();
                match backend.pag.get_edge(edge).kind {
                    PAGEdgeEnum::DirectPAGEdge
                    | PAGEdgeEnum::CastPAGEdge
                    | PAGEdgeEnum::OffsetPAGEdge
                    | PAGEdgeEnum::GepPAGEdge(_) => {
                        uses.entry(source).or_default().push(destination);
                    }
                    PAGEdgeEnum::LoadPAGEdge(_) => {
                        sensitive.insert(source);
                    }
                    PAGEdgeEnum::StorePAGEdge(_) => {
                        sensitive.insert(destination);
                    }
                    PAGEdgeEnum::AddrPAGEdge => {}
                }
            }
            let mut pending = Vec::new();
            for node in backend.pag.graph().node_indices() {
                let path = &backend.pag.node_path(node).path;
                if scalar_writes.contains_key(path) {
                    sensitive.insert(node);
                }
                if backend.acx.offset_risks.contains(path) {
                    pending.push(node);
                }
            }
            let mut seen = HashSet::new();
            while let Some(node) = pending.pop() {
                if !irq_check_rupta::limits::checkpoint() {
                    break;
                }
                if !seen.insert(node) {
                    continue;
                }
                if sensitive.contains(&node) {
                    backend.acx.unmodeled_flows.insert(format!(
                        "subobject offset reaches a write or a pointer-dependent operation: {:?}",
                        backend.pag.node_path(node)
                    ));
                }
                pending.extend(uses.get(&node).into_iter().flatten().copied());
            }
        }
        if let Ok(path) = std::env::var("IRQ_CHECK_RUPTA_POINTS") {
            backend.acx.analysis_options.pts_output = Some(path);
            backend.finalize();
        }
        let mut failure = irq_check_rupta::limits::stop_reason().map(str::to_owned);
        let instances: HashMap<_, _> = backend
            .acx
            .functions
            .iter_enumerated()
            .filter(|(_, reference)| reference.promoted.is_none())
            .map(|(id, reference)| {
                (
                    id,
                    Instance {
                        def: reference.kind,
                        args: tcx.mk_args_from_iter(reference.generic_args.iter().map(|arg| {
                            match arg {
                                GenericArgE::Region => {
                                    ty::GenericArg::from(tcx.lifetimes.re_erased)
                                }
                                GenericArgE::Const(value) => ty::GenericArg::from(*value),
                                GenericArgE::Type(value) => ty::GenericArg::from(*value),
                            }
                        })),
                    },
                )
            })
            .collect();
        callable_instances.extend(instances.values().copied());
        let mut candidates = HashMap::<_, HashSet<_>>::new();
        for sites in backend.call_sites.values() {
            for site in sites {
                let Some(&caller) = instances.get(&site.func.func_id) else {
                    continue;
                };
                let targets = candidates.entry((caller, site.location)).or_default();
                for callee in backend.call_graph.get_callees(&site.into()) {
                    if let Some(&target) = instances.get(&callee.func_id) {
                        targets.insert(target);
                    }
                }
            }
        }
        let covered = |(function, _, _): &(
            irq_check_rupta::mir::function::FuncId,
            Option<Location>,
            String,
        )| {
            let Some(&instance) = instances.get(function) else {
                return false;
            };
            if crate::known_intrinsic(tcx, instance) {
                return true;
            }
            if !matches!(instance.def, ty::InstanceKind::Intrinsic(_)) {
                return false;
            }
            let name = tcx.item_name(instance.def_id());
            if super::intrinsic_effect(tcx, instance) != super::IntrinsicEffect::ReadOnly {
                return false;
            }
            if let Some(requirement) = ty::layout::ValidityRequirement::from_intrinsic(name) {
                return tcx
                    .check_validity_requirement((
                        requirement,
                        ty::TypingEnv::fully_monomorphized()
                            .as_query_input(instance.args.type_at(0)),
                    ))
                    .is_ok_and(|valid| valid);
            }
            let result = tcx
                .normalize_erasing_regions(
                    ty::TypingEnv::fully_monomorphized(),
                    tcx.fn_sig(instance.def_id())
                        .instantiate(tcx, instance.args),
                )
                .skip_binder()
                .output();
            !result.is_any_ptr()
                && irq_check_rupta::util::type_util::get_pointer_projections(tcx, result).is_empty()
        };
        let mut complete = backend.acx.coverage_gaps.iter().all(&covered);
        let mut call_complete = backend.acx.coverage_gaps.iter().all(|gap| {
            covered(gap)
                || matches!(
                    gap.2.as_str(),
                    "pointer bits are converted to non-pointer storage"
                        | "transmute does not establish all pointer fields"
                        | "constant allocation contains unmodeled pointer storage"
                        | "constant pointer bytes require flow refinement"
                )
        });
        let unresolved_sites = backend
            .call_sites
            .values()
            .flatten()
            .filter(|site| backend.call_graph.get_callees(&(*site).into()).is_empty())
            .count();
        complete &= unresolved_sites == 0 && backend.acx.unmodeled_flows.is_empty();
        call_complete &= backend.acx.unmodeled_flows.iter().all(|gap| {
            gap.starts_with("subobject offset reaches a write or a pointer-dependent operation:")
                || gap.starts_with("heap storage has no initial pointer type:")
        });
        for &root in &ordered {
            if !irq_check_rupta::util::type_util::has_instance_mir(tcx, root.def) {
                complete = false;
                call_complete = false;
                continue;
            }
            let body = tcx.instance_mir(root.def);
            for local in body.args_iter() {
                let argument = root.instantiate_mir_and_normalize_erasing_regions(
                    tcx,
                    ty::TypingEnv::fully_monomorphized(),
                    ty::EarlyBinder::bind(tcx, body.local_decls[local].ty),
                );
                if argument.is_any_ptr()
                    || !irq_check_rupta::util::type_util::get_pointer_projections(tcx, argument)
                        .is_empty()
                {
                    complete = false;
                    call_complete = false;
                }
            }
        }
        let mut pointees = HashMap::<_, HashSet<_>>::new();
        let mut pointee_ranges = HashMap::<_, HashSet<_>>::new();
        let mut uncertain = HashSet::new();
        let (mut covered_calls, context_coverage, uncertain_nodes, range_offsets) = {
            use irq_check_rupta::graph::pag::PAGEdgeEnum;
            use irq_check_rupta::mir::path::{Path, PathEnum};
            use rustc_middle::mir::{StatementKind, TerminatorKind};
            let base_path = |path: &std::rc::Rc<Path>| {
                let mut base = path;
                while let PathEnum::QualifiedPath { base: parent, .. }
                | PathEnum::OffsetPath { base: parent, .. } = &base.value
                {
                    base = parent;
                }
                base.clone()
            };
            let mut groups = HashMap::new();
            let mut node_groups = vec![0; backend.pag.graph().node_count()];
            for node in backend.pag.graph().node_indices() {
                let context = backend.pag.node_path(node).cid;
                let mut path = backend.pag.node_path(node).path.clone();
                let next = groups.len();
                node_groups[node.index()] = *groups.entry((context, path.clone())).or_insert(next);
                while let PathEnum::QualifiedPath { base, .. } | PathEnum::OffsetPath { base, .. } =
                    &path.value
                {
                    path = base.clone();
                    let next = groups.len();
                    groups.entry((context, path.clone())).or_insert(next);
                }
            }
            let mut path_groups = HashMap::<_, Vec<_>>::new();
            for ((_, path), &group) in &groups {
                path_groups.entry(path.clone()).or_default().push(group);
            }
            let mut uses = vec![Vec::new(); groups.len() * 2];
            for ((context, path), &group) in &groups {
                if let PathEnum::QualifiedPath { base, .. } | PathEnum::OffsetPath { base, .. } =
                    &path.value
                {
                    let parent = groups[&(*context, base.clone())];
                    uses[parent * 2].push(group * 2);
                    uses[parent * 2 + 1].push(group * 2 + 1);
                }
            }
            let mut pending = Vec::new();
            let mut no_provenance = HashSet::new();
            let mut value_uses = HashMap::<_, Vec<_>>::new();
            let mut no_provenance_pending = Vec::new();
            for node in backend.pag.graph().node_indices() {
                if backend
                    .acx
                    .no_provenance_paths
                    .contains(&backend.pag.node_path(node).path)
                {
                    no_provenance_pending.push(node);
                }
            }
            for edge in backend.pag.graph().edge_indices() {
                if matches!(
                    backend.pag.get_edge(edge).kind,
                    PAGEdgeEnum::DirectPAGEdge
                        | PAGEdgeEnum::CastPAGEdge
                        | PAGEdgeEnum::OffsetPAGEdge
                        | PAGEdgeEnum::GepPAGEdge(_)
                ) {
                    let (source, destination) = backend.pag.graph().edge_endpoints(edge).unwrap();
                    value_uses.entry(source).or_default().push(destination);
                }
            }
            while let Some(node) = no_provenance_pending.pop() {
                if !irq_check_rupta::limits::checkpoint() {
                    break;
                }
                if !no_provenance.insert(node) {
                    continue;
                }
                if backend
                    .acx
                    .get_path_rustc_type(&backend.pag.node_path(node).path)
                    .is_some_and(|ty| ty.is_fn_ptr())
                {
                    pending.push(node_groups[node.index()]);
                }
                no_provenance_pending.extend(value_uses.get(&node).into_iter().flatten().copied());
            }
            let mut offset_nodes = HashSet::new();
            let mut offset_pending: Vec<_> = backend
                .pag
                .graph()
                .node_indices()
                .filter(|node| {
                    backend
                        .acx
                        .offset_risks
                        .contains(&backend.pag.node_path(*node).path)
                })
                .collect();
            while let Some(node) = offset_pending.pop() {
                if !irq_check_rupta::limits::checkpoint() {
                    break;
                }
                if offset_nodes.insert(node) {
                    offset_pending.extend(value_uses.get(&node).into_iter().flatten().copied());
                }
            }
            let mut range_offsets = offset_nodes.clone();
            let mut offset_pending: Vec<_> = backend
                .pag
                .graph()
                .edge_indices()
                .filter(|edge| {
                    matches!(backend.pag.get_edge(*edge).kind, PAGEdgeEnum::OffsetPAGEdge)
                })
                .map(|edge| backend.pag.graph().edge_endpoints(edge).unwrap().1)
                .collect();
            while let Some(node) = offset_pending.pop() {
                if !irq_check_rupta::limits::checkpoint() {
                    break;
                }
                if range_offsets.insert(node) {
                    offset_pending.extend(value_uses.get(&node).into_iter().flatten().copied());
                }
            }
            drop(value_uses);
            let mut memory = HashSet::new();
            let mut parameters = HashSet::new();
            let mut write_addresses = HashSet::new();
            let mut call_receivers = HashSet::new();
            let mut unknown_memory = false;
            let mut unknown_calls = false;
            let mut gap_functions = HashSet::new();
            let mut gap_sites = HashSet::new();
            let root_instances: HashSet<_> = ordered.iter().copied().collect();
            for ((_, path), &group) in &groups {
                match path.value {
                    PathEnum::Parameter { func_id, .. } => {
                        parameters.insert(group);
                        if instances
                            .get(&func_id)
                            .is_none_or(|instance| root_instances.contains(instance))
                        {
                            pending.push(group);
                        }
                    }
                    PathEnum::HeapObj { .. }
                    | PathEnum::StaticVariable { .. }
                    | PathEnum::PromotedConstant { .. }
                    | PathEnum::PromotedStrRefArray
                    | PathEnum::PromotedArgumentV1Array => {
                        memory.insert(group);
                    }
                    _ => {}
                }
            }
            for path in &backend.acx.unmodeled_paths {
                let mut affected = path;
                while !path_groups.contains_key(affected) {
                    match &affected.value {
                        PathEnum::QualifiedPath { base, .. } | PathEnum::OffsetPath { base, .. } => affected = base,
                        _ => break,
                    }
                }
                if let Some(groups) = path_groups.get(affected) {
                    pending.extend(groups.iter().copied());
                } else {
                    if std::env::var_os("IRQ_CHECK_TRACE_COVERAGE").is_some() {
                        eprintln!("irq-check: coverage untracked path {path:?}");
                    }
                    unknown_memory = true;
                    unknown_calls = true;
                }
            }
            let mut integer_groups = HashSet::new();
            let mut function_groups = HashSet::new();
            for node in backend.pag.graph().node_indices() {
                if let Some(ty) = backend.acx.get_path_rustc_type(&backend.pag.node_path(node).path) {
                    let group = node_groups[node.index()];
                    if irq_check_rupta::util::type_util::is_integer_carrier(tcx, ty) {
                        integer_groups.insert(group);
                    } else if ty.is_fn_ptr() {
                        function_groups.insert(group);
                    }
                }
            }
            let mut integer_uses = HashMap::<_, Vec<_>>::new();
            for edge in backend.pag.graph().edge_indices() {
                if !irq_check_rupta::limits::checkpoint() { break; }
                if matches!(backend.pag.get_edge(edge).kind, PAGEdgeEnum::DirectPAGEdge) {
                    let (source, destination) = backend.pag.graph().edge_endpoints(edge).unwrap();
                    let from = node_groups[source.index()];
                    let to = node_groups[destination.index()];
                    if integer_groups.contains(&from)
                        && (integer_groups.contains(&to) || function_groups.contains(&to))
                    {
                        integer_uses.entry(from).or_default().push(to);
                    }
                }
            }
            for path in &backend.acx.value_unknown_paths {
                if !irq_check_rupta::limits::checkpoint() { break; }
                let Some(ty) = backend.acx.get_path_rustc_type(path) else {
                    if let Some(groups) = path_groups.get(path) {
                        pending.extend(groups.iter().copied());
                    }
                    continue;
                };
                if ty.is_any_ptr() {
                    if let Some(groups) = path_groups.get(path) {
                        pending.extend(groups.iter().copied());
                    }
                    continue;
                }
                let projections = backend.acx.value_projs_cache.get(&ty).cloned()
                    .unwrap_or_else(|| irq_check_rupta::util::type_util::get_value_projections(tcx, ty, true));
                for (projection, _) in projections {
                    let field = Path::append_projection(path, &projection);
                    if let Some(groups) = path_groups.get(&field) {
                        pending.extend(groups.iter().copied());
                    }
                }
            }
            for gap in &backend.acx.coverage_gaps {
                if covered(gap) {
                    continue;
                }
                let (function, location, _) = gap;
                if let Some(location) = location {
                    gap_sites.insert((*function, *location));
                } else {
                    gap_functions.insert(*function);
                }
                if let Some(instance) = instances
                    .get(function)
                    .filter(|instance| matches!(instance.def, ty::InstanceKind::Intrinsic(_)))
                {
                    let write_arguments = match super::intrinsic_effect(tcx, *instance) {
                        super::IntrinsicEffect::ReadOnly => Some(&[][..]),
                        super::IntrinsicEffect::Write { arguments, .. } => Some(arguments),
                        super::IntrinsicEffect::Unknown => None,
                    };
                    if let Some(write_arguments) = write_arguments {
                        for sites in backend.call_sites.values() {
                            for site in sites {
                                if !backend
                                    .call_graph
                                    .get_callees(&site.into())
                                    .iter()
                                    .any(|callee| callee.func_id == *function)
                                {
                                    continue;
                                }
                                if let Some(destination) =
                                    backend.pag.get_node_id(&site.destination)
                                {
                                    pending.push(node_groups[destination.index()]);
                                }
                                for &argument in write_arguments {
                                    if let Some(node) = site
                                        .args
                                        .get(argument)
                                        .and_then(|path| backend.pag.get_node_id(path))
                                    {
                                        write_addresses.insert(node_groups[node.index()]);
                                        if let Some(points) = backend
                                            .get_pt_data()
                                            .get_propa_pts(node)
                                            .filter(|points| !points.is_empty())
                                        {
                                            pending.extend(
                                                points
                                                    .iter()
                                                    .map(|target| node_groups[target.index()]),
                                            );
                                        } else if !no_provenance.contains(&node) {
                                            if std::env::var_os("IRQ_CHECK_TRACE_COVERAGE")
                                                .is_some()
                                            {
                                                eprintln!(
                                                    "irq-check: coverage empty intrinsic write {instance} at {site:?}"
                                                );
                                            }
                                            unknown_memory = true;
                                        }
                                    } else {
                                        if std::env::var_os("IRQ_CHECK_TRACE_COVERAGE").is_some() {
                                            eprintln!(
                                                "irq-check: coverage missing intrinsic write {instance} at {site:?}"
                                            );
                                        }
                                        unknown_memory = true;
                                    }
                                }
                            }
                        }
                        continue;
                    }
                }
                let body = instances.get(function).and_then(|instance| {
                    if matches!(
                        instance.def,
                        ty::InstanceKind::Intrinsic(_)
                            | ty::InstanceKind::LlvmIntrinsic(_)
                            | ty::InstanceKind::Virtual(..)
                    ) || !irq_check_rupta::util::type_util::has_instance_mir(tcx, instance.def)
                    {
                        None
                    } else {
                        Some(tcx.instance_mir(instance.def))
                    }
                });
                if let (Some(body), Some(location)) = (body, location) {
                    if let Some(statement) = body.basic_blocks[location.block]
                        .statements
                        .get(location.statement_index)
                    {
                        if let StatementKind::Assign(assignment) = &statement.kind {
                            let place = assignment.0;
                            let path = Path::new_local_parameter_or_result(
                                *function,
                                place.local.as_usize(),
                                body.arg_count,
                            );
                            if let Some(groups) = path_groups.get(&path) {
                                pending.extend(groups.iter().copied());
                            }
                            if place.is_indirect() {
                                unknown_memory = true;
                            }
                            continue;
                        }
                    }
                    if let TerminatorKind::InlineAsm {
                        options, operands, ..
                    } = &body.basic_blocks[location.block].terminator().kind
                    {
                        for operand in operands {
                            let place = match operand {
                                rustc_middle::mir::InlineAsmOperand::Out { place, .. } => *place,
                                rustc_middle::mir::InlineAsmOperand::InOut {
                                    out_place, ..
                                } => *out_place,
                                _ => None,
                            };
                            if let Some(place) = place {
                                let path = Path::new_local_parameter_or_result(
                                    *function,
                                    place.local.as_usize(),
                                    body.arg_count,
                                );
                                if let Some(groups) = path_groups.get(&path) {
                                    pending.extend(groups.iter().copied());
                                }
                                if place.is_indirect() {
                                    unknown_memory = true;
                                }
                            }
                        }
                        if !options.intersects(
                            rustc_ast::InlineAsmOptions::NOMEM
                                | rustc_ast::InlineAsmOptions::READONLY,
                        ) {
                            unknown_memory = true;
                        }
                        continue;
                    }
                }
                for ((_, path), &group) in &groups {
                    if matches!(path.value, PathEnum::LocalVariable { func_id, .. } | PathEnum::Parameter { func_id, .. }
                        | PathEnum::ReturnValue { func_id } | PathEnum::Auxiliary { func_id, .. } if func_id == *function)
                    {
                        pending.push(group);
                    }
                }
                if std::env::var_os("IRQ_CHECK_TRACE_COVERAGE").is_some() {
                    eprintln!(
                        "irq-check: coverage unknown body {:?}: {}",
                        instances.get(function),
                        gap.2
                    );
                }
                unknown_memory = true;
                unknown_calls = true;
            }
            for edge in backend.pag.graph().edge_indices() {
                if !irq_check_rupta::limits::checkpoint() {
                    break;
                }
                let (source, destination) = backend.pag.graph().edge_endpoints(edge).unwrap();
                let from = node_groups[source.index()];
                let to = node_groups[destination.index()];
                match backend.pag.get_edge(edge).kind {
                    PAGEdgeEnum::AddrPAGEdge => {
                        if !matches!(
                            base_path(&backend.pag.node_path(source).path).value,
                            PathEnum::Function(_) | PathEnum::Type(_)
                        ) {
                            memory.insert(groups[&(backend.pag.node_path(source).cid, base_path(&backend.pag.node_path(source).path))]);
                        }
                    }
                    PAGEdgeEnum::DirectPAGEdge
                    | PAGEdgeEnum::CastPAGEdge
                    | PAGEdgeEnum::GepPAGEdge(_)
                    | PAGEdgeEnum::OffsetPAGEdge => {
                        uses[from * 2].push(to * 2);
                        let fixed_vtable = backend.acx.fixed_vtable_edges.contains(&(
                            backend.pag.node_path(source).path.clone(),
                            backend.pag.node_path(destination).path.clone(),
                        )) && backend
                            .acx
                            .get_path_rustc_type(&backend.pag.node_path(source).path)
                            .and_then(|ty| ty.builtin_deref(true))
                            .is_some_and(|concrete| {
                                backend
                                    .get_pt_data()
                                    .get_propa_pts(source)
                                    .is_some_and(|points| {
                                        points.iter().any(|pointee| {
                                            backend
                                                .acx
                                                .get_path_rustc_type(
                                                    &backend.pag.node_path(pointee).path,
                                                )
                                                .is_some_and(|ty| {
                                                    tcx.erase_and_anonymize_regions(ty)
                                                        == tcx.erase_and_anonymize_regions(concrete)
                                                })
                                        })
                                    })
                            });
                        if !fixed_vtable {
                            uses[from * 2 + 1].push(to * 2 + 1);
                        }
                    }
                    PAGEdgeEnum::LoadPAGEdge(_) => {
                        uses[from * 2].extend([to * 2, to * 2 + 1]);
                        if offset_nodes.contains(&source) {
                            pending.push(to);
                        }
                        if backend
                            .get_pt_data()
                            .get_propa_pts(source)
                            .is_none_or(|points| points.is_empty())
                            && !no_provenance.contains(&source)
                        {
                            pending.push(to);
                        }
                    }
                    PAGEdgeEnum::StorePAGEdge(_) => {
                        write_addresses.insert(to);
                        if offset_nodes.contains(&destination)
                            && let Some(points) = backend.get_pt_data().get_propa_pts(destination)
                        {
                            for target in points.iter() {
                                pending
                                    .push(groups[&(backend.pag.node_path(target).cid, base_path(&backend.pag.node_path(target).path))]);
                            }
                        }
                        if backend
                            .get_pt_data()
                            .get_propa_pts(destination)
                            .is_none_or(|points| points.is_empty())
                            && !no_provenance.contains(&destination)
                        {
                            if std::env::var_os("IRQ_CHECK_TRACE_COVERAGE").is_some() {
                                eprintln!(
                                    "irq-check: coverage empty store {:?}",
                                    backend.pag.node_path(destination)
                                );
                            }
                            unknown_memory = true;
                        }
                    }
                }
            }
            for node in backend.pag.graph().node_indices() {
                let Some(projections) = scalar_writes.get(&backend.pag.node_path(node).path) else {
                    continue;
                };
                write_addresses.insert(node_groups[node.index()]);
                if let Some(points) = backend
                    .get_pt_data()
                    .get_propa_pts(node)
                    .filter(|points| !points.is_empty())
                {
                    for target in points.iter() {
                        if offset_nodes.contains(&node) {
                            pending.push(groups[&(backend.pag.node_path(target).cid, base_path(&backend.pag.node_path(target).path))]);
                            continue;
                        }
                        for projection in projections {
                            let stored = Path::append_projection(
                                &backend.pag.node_path(target).path,
                                projection,
                            );
                            if let Some(&group) = groups.get(&(backend.pag.node_path(target).cid, stored)) {
                                pending.push(group);
                            }
                        }
                    }
                } else if !no_provenance.contains(&node) {
                    if std::env::var_os("IRQ_CHECK_TRACE_COVERAGE").is_some() {
                        eprintln!(
                            "irq-check: coverage empty scalar write {:?}",
                            backend.pag.node_path(node)
                        );
                    }
                    unknown_memory = true;
                }
            }
            let mut receivers = HashMap::<_, HashSet<_>>::new();
            for (node, sites) in backend
                .assoc_calls
                .dynamic_dispatch_calls
                .iter()
                .chain(backend.assoc_calls.dynamic_fntrait_calls.iter())
                .chain(backend.assoc_calls.fnptr_calls.iter())
            {
                let group = node_groups[node.index()];
                call_receivers.insert(group);
                if offset_nodes.contains(node) && backend.assoc_calls.fnptr_calls.contains_key(node)
                {
                    pending.push(group);
                }
                for site in sites {
                    receivers
                        .entry((site.func, site.location))
                        .or_default()
                        .insert(group);
                    if let Some(destination) = backend.pag.get_node_id(&site.destination) {
                        let destination = node_groups[destination.index()];
                        uses[group * 2 + 1].extend([destination * 2, destination * 2 + 1]);
                    }
                }
            }
            for sites in backend.call_sites.values() {
                for site in sites {
                    if backend.call_graph.get_callees(&site.into()).is_empty() {
                        if std::env::var_os("IRQ_CHECK_TRACE_COVERAGE").is_some() {
                            eprintln!(
                                "irq-check: coverage empty call {:?} at {site:?}",
                                instances.get(&site.func.func_id)
                            );
                        }
                        unknown_memory = true;
                        unknown_calls = true;
                        if let Some(destination) = backend.pag.get_node_id(&site.destination) {
                            pending.push(node_groups[destination.index()]);
                        }
                    }
                }
            }
            for edges in &mut uses {
                edges.sort_unstable();
                edges.dedup();
            }
            let mut pending: Vec<_> = pending
                .into_iter()
                .flat_map(|group| [group * 2, group * 2 + 1])
                .collect();
            let mut unknown = vec![false; uses.len()];
            let seeds: HashSet<_> = pending.iter().copied().collect();
            let mut origins = vec![None; uses.len()];
            let mut memory_invalidated = false;
            let mut parameters_invalidated = false;
            loop {
                if unknown_calls && !parameters_invalidated {
                    parameters_invalidated = true;
                    pending.extend(
                        parameters
                            .iter()
                            .flat_map(|group| [group * 2, group * 2 + 1]),
                    );
                }
                if (unknown_memory || unknown_calls) && !memory_invalidated {
                    memory_invalidated = true;
                    pending.extend(memory.iter().flat_map(|group| [group * 2, group * 2 + 1]));
                }
                let Some(group) = pending.pop() else { break };
                if !irq_check_rupta::limits::checkpoint() {
                    break;
                }
                if std::mem::replace(&mut unknown[group], true) {
                    continue;
                }
                if integer_groups.contains(&(group / 2)) {
                    for &next in integer_uses.get(&(group / 2)).into_iter().flatten() {
                        for next in [next * 2, next * 2 + 1] {
                            if !unknown[next] && !seeds.contains(&next) && origins[next].is_none() {
                                origins[next] = Some(group);
                            }
                            pending.push(next);
                        }
                    }
                    continue;
                }
                if std::env::var_os("IRQ_CHECK_TRACE_COVERAGE").is_some()
                    && (!unknown_memory && group % 2 == 0 && write_addresses.contains(&(group / 2))
                        || !unknown_calls
                            && group % 2 == 1
                            && call_receivers.contains(&(group / 2)))
                {
                    eprintln!(
                        "irq-check: coverage unknown consumer {:?}",
                        groups
                            .iter()
                            .find(|(_, id)| **id == group / 2)
                            .map(|(path, _)| path)
                    );
                    let mut source = Some(group);
                    for _ in 0..256 {
                        let Some(current) = source else { break };
                        let path = groups
                            .iter()
                            .find(|(_, id)| **id == current / 2)
                            .map(|(path, _)| path);
                        let instance = path.and_then(|path| match base_path(&path.1).value {
                            PathEnum::Parameter { func_id, .. }
                            | PathEnum::LocalVariable { func_id, .. }
                            | PathEnum::ReturnValue { func_id }
                            | PathEnum::Auxiliary { func_id, .. }
                            | PathEnum::HeapObj { func_id, .. } => instances.get(&func_id),
                            _ => None,
                        });
                        eprintln!(
                            "irq-check: coverage source {path:?} {instance:?} binding={}",
                            current % 2 == 1
                        );
                        source = origins[current];
                    }
                }
                unknown_memory |= group % 2 == 0 && write_addresses.contains(&(group / 2));
                unknown_calls |= group % 2 == 1 && call_receivers.contains(&(group / 2));
                for &next in &uses[group] {
                    if !unknown[next] && !seeds.contains(&next) && origins[next].is_none() {
                        origins[next] = Some(group);
                    }
                }
                pending.extend(uses[group].iter().copied());
            }
            let mut covered_calls = HashSet::new();
            let mut context_coverage = HashSet::new();
            let mut uncovered_calls = HashSet::new();
            for sites in backend.call_sites.values() {
                for site in sites {
                    let Some(&instance) = instances.get(&site.func.func_id) else {
                        continue;
                    };
                    let key = (instance, site.location);
                    let gap = gap_functions.contains(&site.func.func_id)
                        || gap_sites.contains(&(site.func.func_id, site.location));
                    let callees = backend.call_graph.get_callees(&site.into());
                    if gap
                        || callees.is_empty()
                        || callees
                            .iter()
                            .any(|callee| !instances.contains_key(&callee.func_id))
                        || receivers
                            .get(&(site.func, site.location))
                            .is_some_and(|groups| {
                                groups.iter().any(|group| unknown[*group * 2 + 1])
                            })
                    {
                        uncovered_calls.insert(key);
                    } else {
                        covered_calls.insert(key);
                        if let Some(node) = backend.call_graph.func_nodes.get(&site.func) {
                            context_coverage.insert((node.index(), site.location));
                        }
                    }
                }
            }
            covered_calls.retain(|key| !uncovered_calls.contains(key));
            let uncertain_nodes: HashSet<_> = backend
                .pag
                .graph()
                .node_indices()
                .filter(|node| unknown[node_groups[node.index()] * 2])
                .collect();
            if std::env::var_os("IRQ_CHECK_STATS").is_some() {
                let indirect = receivers
                    .keys()
                    .filter(|(function, location)| {
                        instances
                            .get(&function.func_id)
                            .is_some_and(|instance| covered_calls.contains(&(*instance, *location)))
                    })
                    .count();
                eprintln!(
                    "irq-check: local pointer coverage covered_calls={} covered_indirect_calls={indirect} uncertain_nodes={} total_nodes={} unknown_memory={memory_invalidated} unknown_calls={parameters_invalidated}",
                    covered_calls.len(),
                    uncertain_nodes.len(),
                    node_groups.len()
                );
            }
            if let Some(directory) = std::env::var_os("IRQ_CHECK_DUMP_DIR") {
                let mut evidence: Vec<_> = receivers
                    .iter()
                    .filter_map(|((function, location), groups)| {
                        let instance = instances.get(&function.func_id)?;
                        Some(format!(
                            "{instance} at {location:?}: covered={} unknown_receiver={}",
                            covered_calls.contains(&(*instance, *location)),
                            groups.iter().any(|group| unknown[*group * 2 + 1])
                        ))
                    })
                    .collect();
                evidence.sort_unstable();
                let directory = std::path::PathBuf::from(directory);
                if let Err(error) = std::fs::create_dir_all(&directory).and_then(|_| {
                    std::fs::write(
                        directory.join(format!(
                            "{}-call-coverage.txt",
                            tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE)
                        )),
                        evidence.join("\n"),
                    )
                }) {
                    failure = Some(format!(
                        "cannot write local call coverage evidence: {error}"
                    ));
                }
            }
            (covered_calls, context_coverage, uncertain_nodes, range_offsets)
        };
        complete &= !uncertain_nodes.iter().any(|node| {
            backend.acx.get_path_rustc_type(&backend.pag.node_path(*node).path)
                .is_some_and(|ty| ty.is_any_ptr())
        });
        {
            use irq_check_rupta::mir::path::{PathEnum, PathSelector};
            let cx = ty::layout::LayoutCx::new(tcx, ty::TypingEnv::fully_monomorphized());
            for node in backend.pag.graph().node_indices() {
                if !irq_check_rupta::limits::checkpoint() {
                    break;
                }
                let path = &backend.pag.node_path(node).path;
                if backend.acx.get_path_rustc_type(path).is_some_and(|ty|
                    irq_check_rupta::util::type_util::is_integer_carrier(tcx, ty))
                {
                    continue;
                }
                let mut base = path;
                while let PathEnum::QualifiedPath { base: parent, .. }
                | PathEnum::OffsetPath { base: parent, .. } = &base.value
                {
                    base = parent;
                }
                let (function, local) = match base.value {
                    PathEnum::LocalVariable { func_id, ordinal }
                    | PathEnum::Parameter { func_id, ordinal } => {
                        (func_id, rustc_middle::mir::Local::from_usize(ordinal))
                    }
                    PathEnum::ReturnValue { func_id } => (func_id, rustc_middle::mir::RETURN_PLACE),
                    _ => continue,
                };
                let Some(&instance) = instances.get(&function) else {
                    continue;
                };
                let key = (instance, local);
                if uncertain_nodes.contains(&node) {
                    uncertain.insert(key);
                    continue;
                }
                let Some(points) = backend
                    .get_pt_data()
                    .get_propa_pts(node)
                    .filter(|points| !points.is_empty())
                else {
                    uncertain.insert(key);
                    continue;
                };
                for target in points {
                    let mut path = &backend.pag.node_path(target).path;
                    let mut projections = Vec::new();
                    while let PathEnum::QualifiedPath { base, .. }
                    | PathEnum::OffsetPath { base, .. } = &path.value
                    {
                        projections.push(&path.value);
                        path = base;
                    }
                    let object = match path.value {
                        PathEnum::LocalVariable { func_id, ordinal }
                        | PathEnum::Parameter { func_id, ordinal } => {
                            instances.get(&func_id).map(|instance| {
                                PointerObject::Local(
                                    *instance,
                                    rustc_middle::mir::Local::from_usize(ordinal),
                                )
                            })
                        }
                        PathEnum::ReturnValue { func_id } => {
                            instances.get(&func_id).map(|instance| {
                                PointerObject::Local(*instance, rustc_middle::mir::RETURN_PLACE)
                            })
                        }
                        PathEnum::HeapObj { func_id, location } => instances
                            .get(&func_id)
                            .map(|instance| PointerObject::Heap(*instance, location)),
                        PathEnum::StaticVariable { def_id } => Some(PointerObject::Static(def_id)),
                        _ => None,
                    };
                    if let Some(object) = object {
                        pointees.entry(key).or_default().insert(object);
                        let range = (|| {
                            if range_offsets.contains(&node) {
                                return None;
                            }
                            let root_ty = backend.acx.get_path_rustc_type(path)?;
                            let mut layout = tcx
                                .layout_of(
                                    ty::TypingEnv::fully_monomorphized().as_query_input(root_ty),
                                )
                                .ok()?;
                            let extent = layout.size.bytes();
                            let mut offset = 0u64;
                            for projection in projections.iter().rev() {
                                match projection {
                                    PathEnum::OffsetPath { offset: bytes, .. } => {
                                        offset = offset.checked_add(u64::try_from(*bytes).ok()?)?;
                                    }
                                    PathEnum::QualifiedPath { projection, .. } => {
                                        for part in projection {
                                            match *part {
                                                PathSelector::Field(field)
                                                | PathSelector::UnionField(field) => {
                                                    if field >= layout.fields.count() {
                                                        return None;
                                                    }
                                                    offset = offset.checked_add(
                                                        layout.fields.offset(field).bytes(),
                                                    )?;
                                                    layout = layout.field(&cx, field);
                                                }
                                                PathSelector::Downcast(variant) => {
                                                    let ty::Adt(def, _) = layout.ty.kind() else {
                                                        return None;
                                                    };
                                                    if variant >= def.variants().len() {
                                                        return None;
                                                    }
                                                    layout = layout.for_variant(
                                                        &cx,
                                                        rustc_abi::VariantIdx::from_usize(variant),
                                                    );
                                                }
                                                PathSelector::Cast(index) => {
                                                    layout = tcx
                                                        .layout_of(
                                                            ty::TypingEnv::fully_monomorphized()
                                                                .as_query_input(
                                                                    backend
                                                                        .acx
                                                                        .get_type_by_index(index)?,
                                                                ),
                                                        )
                                                        .ok()?;
                                                }
                                                _ => return None,
                                            }
                                        }
                                    }
                                    _ => return None,
                                }
                            }
                            (offset <= extent).then_some((offset, extent))
                        })();
                        pointee_ranges
                            .entry(key)
                            .or_default()
                            .insert((object, range));
                    } else {
                        uncertain.insert(key);
                    }
                }
            }
            pointees.retain(|key, values| !uncertain.contains(key) && !values.is_empty());
            pointee_ranges.retain(|key, _| pointees.contains_key(key));
        }
        let mut nodes = Vec::with_capacity(backend.pag.graph().node_count());
        let mut local_nodes = HashMap::<_, Vec<_>>::new();
        for node in backend.pag.graph().node_indices() {
            if !irq_check_rupta::limits::checkpoint() { break; }
            use irq_check_rupta::mir::path::PathEnum;
            let source = backend.pag.node_path(node);
            let mut base = &source.path;
            let mut pieces = Vec::new();
            let mut offset = 0usize;
            loop {
                match &base.value {
                    PathEnum::QualifiedPath { base: parent, projection } => {
                        pieces.push(projection.as_slice());
                        base = parent;
                    }
                    PathEnum::OffsetPath { base: parent, offset: bytes } => {
                        offset = offset.saturating_add(*bytes);
                        base = parent;
                    }
                    _ => break,
                }
            }
            let mut projection = Vec::new();
            for piece in pieces.into_iter().rev() {
                projection.extend_from_slice(piece);
            }
            let (function_id, object, function) = match base.value {
                PathEnum::LocalVariable { func_id, ordinal }
                | PathEnum::Parameter { func_id, ordinal } => (
                    Some(func_id),
                    instances.get(&func_id).map(|instance| PointerObject::Local(
                        *instance, rustc_middle::mir::Local::from_usize(ordinal))),
                    None,
                ),
                PathEnum::ReturnValue { func_id } => (
                    Some(func_id),
                    instances.get(&func_id).map(|instance| PointerObject::Local(
                        *instance, rustc_middle::mir::RETURN_PLACE)),
                    None,
                ),
                PathEnum::HeapObj { func_id, location } => (
                    Some(func_id),
                    instances.get(&func_id).map(|instance| PointerObject::Heap(*instance, location)),
                    None,
                ),
                PathEnum::Auxiliary { func_id, .. } => (Some(func_id), None, None),
                PathEnum::StaticVariable { def_id } => (None, Some(PointerObject::Static(def_id)), None),
                PathEnum::Function(func_id) => (None, None, instances.get(&func_id).copied()),
                _ => (None, None, None),
            };
            let context = function_id.and_then(|func_id| backend.call_graph.func_nodes.get(
                &irq_check_rupta::mir::function::CSFuncId::new(source.cid, func_id)
            )).map(|node| node.index());
            let targets: Vec<_> = backend.get_pt_data().get_propa_pts(node)
                .into_iter().flat_map(|points| points.iter()).map(|target| target.index()).collect();
            if let (Some(context), Some(PointerObject::Local(_, local))) = (context, object) {
                local_nodes.entry((context, local)).or_default().push(node.index());
            }
            nodes.push(PointerNode {
                object, context, projection, offset,
                ty: backend.acx.get_path_rustc_type(&source.path),
                function,
                complete: !uncertain_nodes.contains(&node) && !targets.is_empty(),
                targets,
            });
        }
        let mut pending: Vec<_> = instances
            .values()
            .chain(exported.values())
            .copied()
            .collect();
        let mut visited = HashSet::new();
        let mut callers = HashMap::<_, HashSet<_>>::new();
        let mut binding_dependent = HashSet::new();
        let mut diagnostic_dependent = HashSet::new();
        while let Some(instance) = pending.pop() {
            if !irq_check_rupta::limits::checkpoint() {
                break;
            }
            if !visited.insert(instance)
                || !trust.checks(instance.def_id())
                || crate::forbidden(tcx, instance).is_some()
                || crate::known_intrinsic(tcx, instance)
                || tcx.is_foreign_item(instance.def_id())
                || matches!(
                    instance.def,
                    ty::InstanceKind::Intrinsic(_)
                        | ty::InstanceKind::LlvmIntrinsic(_)
                        | ty::InstanceKind::Virtual(..)
                )
                || !irq_check_rupta::util::type_util::has_instance_mir(tcx, instance.def)
            {
                continue;
            }
            let index = match super::query::BodyIndex::build(tcx, trust, instance, true) {
                Ok(index) => index,
                Err(_) => {
                    binding_dependent.insert(instance);
                    continue;
                }
            };
            if index.stops.iter().any(Option::is_some) || !index.assumptions.is_empty()
                || index.reachable.iter().any(|&block| match &index.body.basic_blocks[block].terminator().kind {
                    rustc_middle::mir::TerminatorKind::Goto { .. }
                    | rustc_middle::mir::TerminatorKind::Return
                    | rustc_middle::mir::TerminatorKind::Unreachable => false,
                    rustc_middle::mir::TerminatorKind::Call { unwind, .. }
                    | rustc_middle::mir::TerminatorKind::Drop { unwind, .. } =>
                        matches!(unwind, rustc_middle::mir::UnwindAction::Cleanup(_)),
                    rustc_middle::mir::TerminatorKind::TailCall { .. } => false,
                    rustc_middle::mir::TerminatorKind::InlineAsm { targets, unwind, .. } =>
                        targets.len() > 1 || matches!(unwind, rustc_middle::mir::UnwindAction::Cleanup(_)),
                    _ => true,
                })
            {
                diagnostic_dependent.insert(instance);
            }
            for location in index.calls {
                use rustc_middle::mir::TerminatorKind;
                let target = match &index.body.basic_blocks[location.block].terminator().kind {
                    TerminatorKind::Call { func, .. } | TerminatorKind::TailCall { func, .. } => {
                        let ty = instance.instantiate_mir_and_normalize_erasing_regions(
                            tcx,
                            ty::TypingEnv::fully_monomorphized(),
                            ty::EarlyBinder::bind(tcx, func.ty(index.body, tcx)),
                        );
                        if let ty::FnDef(def, args) = *ty.kind() {
                            Instance::try_resolve(
                                tcx,
                                ty::TypingEnv::fully_monomorphized(),
                                def,
                                tcx.instantiate_bound_regions_with_erased(args),
                            )
                            .ok()
                            .flatten()
                        } else {
                            None
                        }
                    }
                    TerminatorKind::Drop { place, .. } => {
                        let ty = instance.instantiate_mir_and_normalize_erasing_regions(
                            tcx,
                            ty::TypingEnv::fully_monomorphized(),
                            ty::EarlyBinder::bind(tcx, place.ty(index.body, tcx).ty),
                        );
                        if matches!(ty.kind(), ty::Dynamic(..)) {
                            None
                        } else {
                            Some(Instance::resolve_drop_glue(tcx, ty))
                        }
                    }
                    TerminatorKind::Assert { msg, .. } => {
                        use rustc_hir::attrs::lang_items::LangItem;
                        let item = match &**msg {
                            rustc_middle::mir::AssertKind::BoundsCheck { .. } => {
                                LangItem::PanicBoundsCheck
                            }
                            rustc_middle::mir::AssertKind::MisalignedPointerDereference {
                                ..
                            } => LangItem::PanicMisalignedPointerDereference,
                            rustc_middle::mir::AssertKind::NullPointerDereference => {
                                LangItem::PanicNullPointerDereference
                            }
                            rustc_middle::mir::AssertKind::NullReferenceConstructed => {
                                LangItem::PanicNullReferenceConstructed
                            }
                            rustc_middle::mir::AssertKind::InvalidEnumConstruction(_) => {
                                LangItem::PanicInvalidEnumConstruction
                            }
                            other => other.panic_function(),
                        };
                        tcx.lang_items()
                            .get(item)
                            .map(|def| Instance::mono(tcx, def))
                    }
                    TerminatorKind::InlineAsm { .. } => continue,
                    _ => None,
                };
                if let Some(mut target) =
                    target.filter(|target| !matches!(target.def, ty::InstanceKind::Virtual(..)))
                {
                    if tcx.is_foreign_item(target.def_id())
                        && trust.checks(target.def_id())
                        && crate::forbidden(tcx, target).is_none()
                    {
                        if let Some(linked) = exported.get(tcx.symbol_name(target).name) {
                            target = *linked;
                        }
                    }
                    callers.entry(target).or_default().insert(instance);
                    pending.push(target);
                } else {
                    binding_dependent.insert(instance);
                }
            }
        }
        let mut changed: Vec<_> = binding_dependent.iter().copied().collect();
        while let Some(instance) = changed.pop() {
            if !irq_check_rupta::limits::checkpoint() {
                break;
            }
            for &caller in callers.get(&instance).into_iter().flatten() {
                if binding_dependent.insert(caller) {
                    changed.push(caller);
                }
            }
        }
        diagnostic_dependent.extend(binding_dependent.iter().copied());
        let mut changed: Vec<_> = diagnostic_dependent.iter().copied().collect();
        while let Some(instance) = changed.pop() {
            if !irq_check_rupta::limits::checkpoint() { break; }
            for &caller in callers.get(&instance).into_iter().flatten() {
                if diagnostic_dependent.insert(caller) { changed.push(caller); }
            }
        }
        let diagnostic_independent = visited.difference(&diagnostic_dependent).copied().collect();
        if let Some(reason) = irq_check_rupta::limits::stop_reason() {
            failure = Some(reason.into());
        }
        if let Some(directory) = std::env::var_os("IRQ_CHECK_DUMP_DIR") {
            let directory = std::path::PathBuf::from(directory);
            let mut gaps: Vec<_> = backend
                .acx
                .coverage_gaps
                .iter()
                .map(|(function, location, reason)| {
                    let function = backend.acx.get_function_reference(*function);
                    format!(
                        "{} {:?} {:?} at {location:?}: {reason}",
                        tcx.def_path_str(function.def_id),
                        function.kind,
                        function.generic_args
                    )
                })
                .collect();
            gaps.extend(backend.acx.unmodeled_flows.iter().cloned());
            gaps.sort_unstable();
            let result = std::fs::create_dir_all(&directory).and_then(|_| {
                std::fs::write(
                    directory.join(format!(
                        "{}-backend-coverage.txt",
                        tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE)
                    )),
                    gaps.join("\n"),
                )
            });
            if let Err(error) = result {
                failure = Some(format!("cannot write backend coverage evidence: {error}"));
            }
        }
        if std::env::var_os("IRQ_CHECK_STATS").is_some() {
            eprintln!(
                "irq-check: global pointer graph contexts={} call_sites={} call_bindings={} coverage_gaps={} points-to-set-bytes={}",
                backend.call_graph.func_nodes.len(),
                candidates.len(),
                backend.call_bindings.len(),
                backend.acx.coverage_gaps.len(),
                backend.get_pt_data().storage_bytes()
            );
        }
        let binding_independent = visited.difference(&binding_dependent).copied().collect();
        let mut safe = HashSet::new();
        let mut exclusions = HashSet::new();
        let mut pointer_calls = covered_calls.clone();
        let mut safe_candidates = candidates.clone();
        let mut context_calls = HashSet::new();
        let mut signature_candidates = HashMap::<_, HashSet<_>>::new();
        let mut rejected = HashSet::new();
        let mut safety_callers = HashMap::<_, HashSet<_>>::new();
        let summary_facts = PointerFacts {
            nodes: Vec::new(),
            local_nodes: HashMap::new(),
            complete: false,
            call_complete: false,
            safe_candidates: HashMap::new(),
            safe: HashSet::new(),
            exclusions: HashSet::new(),
            covered_calls: HashSet::new(),
            pointer_calls: HashSet::new(),
            context_calls: HashSet::new(),
            instance_contexts: HashMap::new(),
            context_bindings: HashMap::new(),
            safe_contexts: HashSet::new(),
            pointees: HashMap::new(),
            pointee_ranges: HashMap::new(),
        };
        let mut summary_solver = super::sparse::Solver::new(tcx, trust, &exported, &summary_facts);
        summary_solver.cache_limit = 16 * 1024 * 1024;
        let mut summary_queries = 0usize;
        let mut summary_transfers = 0usize;
        let mut summary_gaps = Vec::new();
        let mut pending: Vec<_> = visited.iter().copied().collect();
        while let Some(instance) = pending.pop() {
            if !irq_check_rupta::limits::checkpoint() {
                break;
            }
            if !safe.insert(instance) {
                continue;
            }
            if crate::forbidden(tcx, instance).is_some() {
                rejected.insert(instance);
                continue;
            }
            if !trust.checks(instance.def_id())
                || crate::known_intrinsic(tcx, instance)
                || matches!(
                    instance.def,
                    ty::InstanceKind::Shim(ty::ShimKind::DropGlue(_, None))
                )
            {
                continue;
            }
            if matches!(instance.def, ty::InstanceKind::Intrinsic(_)) {
                if !super::LEAF_INTRINSICS.contains(&tcx.item_name(instance.def_id()).as_str()) {
                    rejected.insert(instance);
                }
                if let Some(requirement) = ty::layout::ValidityRequirement::from_intrinsic(
                    tcx.item_name(instance.def_id()),
                ) {
                    if !tcx
                        .check_validity_requirement((
                            requirement,
                            ty::TypingEnv::fully_monomorphized()
                                .as_query_input(instance.args.type_at(0)),
                        ))
                        .is_ok_and(|valid| valid)
                    {
                        rejected.insert(instance);
                    }
                }
                continue;
            }
            if tcx.is_foreign_item(instance.def_id())
                || matches!(
                    instance.def,
                    ty::InstanceKind::LlvmIntrinsic(_) | ty::InstanceKind::Virtual(..)
                )
                || !irq_check_rupta::util::type_util::has_instance_mir(tcx, instance.def)
            {
                rejected.insert(instance);
                continue;
            }
            let Ok(index) = super::query::BodyIndex::build(tcx, trust, instance, true) else {
                rejected.insert(instance);
                continue;
            };
            if index.stops.iter().any(Option::is_some) {
                exclusions.insert(instance);
            }
            for location in index.calls {
                let terminator = index.body.basic_blocks[location.block].terminator();
                if matches!(terminator.kind, rustc_middle::mir::TerminatorKind::InlineAsm { .. })
                    && [crate::trust::ScopeMark::Trusted, crate::trust::ScopeMark::AllowIncomplete].into_iter()
                        .any(|mark| trust.allows(tcx, instance, index.body, terminator.source_info.scope, mark))
                { rejected.insert(instance); continue; }
                if let rustc_middle::mir::TerminatorKind::Call { func, .. }
                    | rustc_middle::mir::TerminatorKind::TailCall { func, .. } =
                    &index.body.basic_blocks[location.block].terminator().kind {
                    let function = instance.instantiate_mir_and_normalize_erasing_regions(
                        tcx, ty::TypingEnv::fully_monomorphized(),
                        ty::EarlyBinder::bind(tcx, func.ty(index.body, tcx)));
                    if let ty::FnDef(def, args) = *function.kind()
                        && crate::context_method(tcx, def)
                        && let Ok(Some(target)) = Instance::try_resolve(tcx,
                            ty::TypingEnv::fully_monomorphized(), def,
                            tcx.instantiate_bound_regions_with_erased(args))
                        && matches!(target.def, ty::InstanceKind::Virtual(..))
                        && crate::forbidden(tcx, target).is_none() {
                        let method = tcx.opt_associated_item(def)
                            .and_then(|item| item.trait_item_def_id()).unwrap_or(def);
                        let implementations: Vec<_> = callable_instances.iter().copied().filter(|target|
                            !matches!(target.def, ty::InstanceKind::Virtual(..))
                                && tcx.opt_associated_item(target.def_id())
                                    .and_then(|item| item.trait_item_def_id()) == Some(method)).collect();
                        if !implementations.is_empty() && implementations.iter().all(|target|
                            !trust.checks(target.def_id()) || roots.contains(target)) {
                            context_calls.insert((instance, location));
                            continue;
                        }
                    }
                }
                if call_complete && !pointer_calls.contains(&(instance, location)) {
                    let mut fallback = HashSet::new();
                    match &index.body.basic_blocks[location.block].terminator().kind {
                        rustc_middle::mir::TerminatorKind::Call { func, .. }
                        | rustc_middle::mir::TerminatorKind::TailCall { func, .. } => {
                            let function = instance.instantiate_mir_and_normalize_erasing_regions(
                                tcx,
                                ty::TypingEnv::fully_monomorphized(),
                                ty::EarlyBinder::bind(tcx, func.ty(index.body, tcx)),
                            );
                            if function.is_fn_ptr() {
                                let signature = function.fn_sig(tcx);
                                if !signature_candidates.contains_key(&signature) {
                                    let mut targets = HashSet::new();
                                    let extra = tcx.mk_type_list(&[]);
                                    let caller_abi = tcx.fn_abi_of_fn_ptr(
                                        ty::TypingEnv::fully_monomorphized()
                                            .as_query_input((signature, extra)),
                                    );
                                    for &target in &callable_instances {
                                        if !irq_check_rupta::limits::checkpoint() {
                                            break;
                                        }
                                        if !matches!(
                                            target.def,
                                            ty::InstanceKind::Item(_) | ty::InstanceKind::Shim(_)
                                        ) {
                                            continue;
                                        }
                                        if matches!(target.def, ty::InstanceKind::Item(_))
                                            && !matches!(tcx.def_kind(target.def_id()), rustc_hir::def::DefKind::Fn | rustc_hir::def::DefKind::AssocFn | rustc_hir::def::DefKind::Closure | rustc_hir::def::DefKind::SyntheticCoroutineBody) { continue; }
                                        let callee_abi = tcx.fn_abi_of_instance_no_deduced_attrs(
                                            ty::TypingEnv::fully_monomorphized()
                                                .as_query_input((target, extra)),
                                        );
                                        let compatible = match (caller_abi, callee_abi) {
                                            (Ok(caller), Ok(callee)) => {
                                                caller.conv == callee.conv
                                                    && caller.c_variadic == callee.c_variadic
                                                    && caller.args.len() == callee.args.len()
                                                    && caller
                                                        .args
                                                        .iter()
                                                        .zip(callee.args.iter())
                                                        .all(|(source, destination)| {
                                                            source.eq_abi(destination)
                                                        })
                                                    && (callee.ret.layout.ty.is_never()
                                                        || caller.ret.eq_abi(&callee.ret))
                                            }
                                            _ => true,
                                        };
                                        if compatible {
                                            targets.insert(target);
                                        }
                                    }
                                    signature_candidates.insert(signature, targets);
                                }
                                fallback.extend(signature_candidates[&signature].iter().copied());
                            } else if let ty::FnDef(def, args) = *function.kind() {
                                if Instance::try_resolve(
                                    tcx,
                                    ty::TypingEnv::fully_monomorphized(),
                                    def,
                                    tcx.instantiate_bound_regions_with_erased(args),
                                )
                                .ok()
                                .flatten()
                                .is_some_and(|target| {
                                    matches!(target.def, ty::InstanceKind::Virtual(..))
                                }) {
                                    let method = tcx
                                        .opt_associated_item(def)
                                        .and_then(|item| item.trait_item_def_id())
                                        .unwrap_or(def);
                                    if backend.acx.is_std_ops_fntrait_call(def) {
                                        fallback.extend(callable_instances.iter().copied().filter(|target|
                                            matches!(target.def, ty::InstanceKind::Shim(_))
                                            || matches!(target.def, ty::InstanceKind::Item(_))
                                                && matches!(tcx.def_kind(target.def_id()), rustc_hir::def::DefKind::Fn | rustc_hir::def::DefKind::AssocFn | rustc_hir::def::DefKind::Closure | rustc_hir::def::DefKind::SyntheticCoroutineBody)));
                                    } else {
                                        fallback.extend(callable_instances.iter().copied().filter(
                                            |target| {
                                                !matches!(target.def, ty::InstanceKind::Virtual(..))
                                                    && tcx
                                                        .opt_associated_item(target.def_id())
                                                        .and_then(|item| item.trait_item_def_id())
                                                        .unwrap_or(target.def_id())
                                                        == method
                                            },
                                        ));
                                    }
                                }
                            }
                        }
                        rustc_middle::mir::TerminatorKind::Drop { place, .. } => {
                            let ty = instance.instantiate_mir_and_normalize_erasing_regions(
                                tcx,
                                ty::TypingEnv::fully_monomorphized(),
                                ty::EarlyBinder::bind(tcx, place.ty(index.body, tcx).ty),
                            );
                            if matches!(ty.kind(), ty::Dynamic(..)) {
                                fallback.extend(callable_instances.iter().copied().filter(
                                    |target| {
                                        matches!(
                                            target.def,
                                            ty::InstanceKind::Shim(ty::ShimKind::DropGlue(..))
                                        )
                                    },
                                ));
                            }
                        }
                        _ => {}
                    }
                    if !fallback.is_empty() {
                        safe_candidates
                            .entry((instance, location))
                            .or_default()
                            .extend(fallback);
                        covered_calls.insert((instance, location));
                    }
                }
                let targets = safe_candidates.get(&(instance, location));
                let direct = match &index.body.basic_blocks[location.block].terminator().kind {
                    rustc_middle::mir::TerminatorKind::Call { func, .. }
                    | rustc_middle::mir::TerminatorKind::TailCall { func, .. } => {
                        let function = instance.instantiate_mir_and_normalize_erasing_regions(
                            tcx,
                            ty::TypingEnv::fully_monomorphized(),
                            ty::EarlyBinder::bind(tcx, func.ty(index.body, tcx)),
                        );
                        if let ty::FnDef(def, args) = *function.kind() {
                            Instance::try_resolve(
                                tcx,
                                ty::TypingEnv::fully_monomorphized(),
                                def,
                                tcx.instantiate_bound_regions_with_erased(args),
                            )
                            .ok()
                            .flatten()
                            .filter(|target| !matches!(target.def, ty::InstanceKind::Virtual(..)))
                        } else {
                            None
                        }
                    }
                    rustc_middle::mir::TerminatorKind::Drop { place, .. } => {
                        let ty = instance.instantiate_mir_and_normalize_erasing_regions(
                            tcx,
                            ty::TypingEnv::fully_monomorphized(),
                            ty::EarlyBinder::bind(tcx, place.ty(index.body, tcx).ty),
                        );
                        (!matches!(ty.kind(), ty::Dynamic(..)))
                            .then(|| Instance::resolve_drop_glue(tcx, ty))
                    }
                    rustc_middle::mir::TerminatorKind::Assert { msg, .. } => {
                        use rustc_hir::attrs::lang_items::LangItem;
                        let item = match &**msg {
                            rustc_middle::mir::AssertKind::BoundsCheck { .. } => {
                                LangItem::PanicBoundsCheck
                            }
                            rustc_middle::mir::AssertKind::MisalignedPointerDereference {
                                ..
                            } => LangItem::PanicMisalignedPointerDereference,
                            rustc_middle::mir::AssertKind::NullPointerDereference => {
                                LangItem::PanicNullPointerDereference
                            }
                            rustc_middle::mir::AssertKind::NullReferenceConstructed => {
                                LangItem::PanicNullReferenceConstructed
                            }
                            rustc_middle::mir::AssertKind::InvalidEnumConstruction(_) => {
                                LangItem::PanicInvalidEnumConstruction
                            }
                            other => other.panic_function(),
                        };
                        tcx.lang_items()
                            .get(item)
                            .map(|def| Instance::mono(tcx, def))
                    }
                    _ => None,
                };
                if let Some(mut target) = direct {
                    if tcx.is_foreign_item(target.def_id())
                        && crate::forbidden(tcx, target).is_none()
                        && trust.checks(target.def_id())
                    {
                        if let Some(&linked) = exported.get(tcx.symbol_name(target).name) {
                            target = linked;
                        }
                    }
                    safety_callers.entry(target).or_default().insert(instance);
                    pending.push(target);
                } else if covered_calls.contains(&(instance, location))
                    && targets.is_some_and(|targets| !targets.is_empty())
                {
                    for &target in targets.unwrap() {
                        safety_callers.entry(target).or_default().insert(instance);
                        pending.push(target);
                    }
                } else {
                    let frame = summary_solver.frame(super::sparse::Frame {
                        instance,
                        caller: None,
                        irq: true,
                        unknown_entry: true,
                    });
                    summary_solver.begin_query(4096);
                    summary_solver.index_choices.clear();
                    summary_solver.split_index = None;
                    let point = super::sparse::Point { frame, location };
                    let mut values = summary_solver.targets(point);
                    if !values.as_ref().is_ok_and(|values| !values.is_empty()
                        && values.iter().all(|value| matches!(value.value, super::sparse::Value::Function(_))))
                        && values != Err(super::query::QueryStop::ProcessLimit)
                    {
                        if let Ok(targets) = summary_solver.conservative_targets(point, 4096) {
                            values = Ok(targets.into_iter().map(|target| super::sparse::Alternative {
                                value: super::sparse::Value::Function(target), guards: Vec::new(),
                            }).collect());
                        }
                    }
                    summary_queries += 1;
                    summary_transfers += summary_solver.budget.steps;
                    let detail = format!("{instance} at {location:?}: {values:?}");
                    if let Ok(values) = values {
                        if !values.is_empty()
                            && values.iter().all(|value| {
                                matches!(value.value, super::sparse::Value::Function(_))
                            })
                        {
                            for value in values {
                                if let super::sparse::Value::Function(target) = value.value {
                                    safety_callers.entry(target).or_default().insert(instance);
                                    pending.push(target);
                                }
                            }
                            continue;
                        }
                    }
                    summary_gaps.push(detail);
                    rejected.insert(instance);
                }
            }
        }
        let mut pending: Vec<_> = rejected.iter().copied().collect();
        while let Some(target) = pending.pop() {
            if !irq_check_rupta::limits::checkpoint() {
                break;
            }
            for &caller in safety_callers.get(&target).into_iter().flatten() {
                if rejected.insert(caller) {
                    pending.push(caller);
                }
            }
        }
        safe.retain(|instance| !rejected.contains(instance));
        for (&(caller, _), targets) in &safe_candidates {
            for &target in targets { safety_callers.entry(target).or_default().insert(caller); }
        }
        let mut pending: Vec<_> = exclusions.iter().copied().collect();
        while let Some(target) = pending.pop() {
            if !irq_check_rupta::limits::checkpoint() { break; }
            for &caller in safety_callers.get(&target).into_iter().flatten() {
                if exclusions.insert(caller) { pending.push(caller); }
            }
        }
        drop(summary_solver);
        let mut instance_contexts = HashMap::<_, Vec<_>>::new();
        let mut context_nodes = HashMap::new();
        for (function, node) in &backend.call_graph.func_nodes {
            if let Some(&instance) = instances.get(&function.func_id) {
                instance_contexts.entry(instance).or_default().push(node.index());
                context_nodes.insert(node.index(), instance);
            }
        }
        for contexts in instance_contexts.values_mut() { contexts.sort_unstable(); }
        let mut context_bindings = HashMap::<_, Vec<_>>::new();
        for sites in backend.call_sites.values() {
            for site in sites {
                if !instances.contains_key(&site.func.func_id) { continue; }
                let Some(node) = backend.call_graph.func_nodes.get(&site.func) else { continue; };
                if !context_coverage.contains(&(node.index(), site.location)) { continue; }
                let targets = context_bindings.entry((node.index(), site.location)).or_default();
                for callee in backend.call_graph.get_callees(&site.into()) {
                    if let Some(&target) = instances.get(&callee.func_id)
                        && let Some(node) = backend.call_graph.func_nodes.get(&callee)
                    {
                        let target = (target, node.index());
                        if !targets.contains(&target) { targets.push(target); }
                    }
                }
            }
        }
        let mut context_indexes = HashMap::new();
        let mut rejected_contexts = HashSet::new();
        let mut context_callers = HashMap::<_, HashSet<_>>::new();
        for (&node, &instance) in &context_nodes {
            if !irq_check_rupta::limits::checkpoint() { break; }
            if crate::forbidden(tcx, instance).is_some() {
                rejected_contexts.insert(node);
                continue;
            }
            if safe.contains(&instance) || !trust.checks(instance.def_id()) { continue; }
            if tcx.is_foreign_item(instance.def_id())
                || !irq_check_rupta::util::type_util::has_instance_mir(tcx, instance.def)
            {
                rejected_contexts.insert(node);
                continue;
            }
            if !context_indexes.contains_key(&instance) {
                match super::query::BodyIndex::build(tcx, trust, instance, true) {
                    Ok(index) => { context_indexes.insert(instance, index); }
                    Err(_) => { rejected_contexts.insert(node); continue; }
                }
            }
            let index = &context_indexes[&instance];
            for &location in &index.calls {
                let terminator = index.body.basic_blocks[location.block].terminator();
                if matches!(terminator.kind, rustc_middle::mir::TerminatorKind::InlineAsm { .. })
                    && [crate::trust::ScopeMark::Trusted, crate::trust::ScopeMark::AllowIncomplete].into_iter()
                        .any(|mark| trust.allows(tcx, instance, index.body, terminator.source_info.scope, mark))
                { rejected_contexts.insert(node); continue; }
                if context_calls.contains(&(instance, location))
                    || covered_calls.contains(&(instance, location))
                        && safe_candidates.get(&(instance, location)).is_some_and(|targets|
                            !targets.is_empty() && targets.iter().all(|target| safe.contains(target)))
                { continue; }
                let Some(targets) = context_bindings.get(&(node, location)).filter(|targets| !targets.is_empty()) else {
                    rejected_contexts.insert(node);
                    continue;
                };
                for (_, target) in targets {
                    context_callers.entry(*target).or_default().insert(node);
                }
            }
        }
        let mut pending: Vec<_> = rejected_contexts.iter().copied().collect();
        while let Some(target) = pending.pop() {
            if !irq_check_rupta::limits::checkpoint() { break; }
            for &caller in context_callers.get(&target).into_iter().flatten() {
                if rejected_contexts.insert(caller) { pending.push(caller); }
            }
        }
        let mut safe_contexts: HashSet<_> = context_nodes.keys().copied()
            .filter(|node| !rejected_contexts.contains(node)).collect();
        if std::env::var_os("IRQ_CHECK_STATS").is_some() {
            eprintln!("irq-check: context safety summaries safe={} total={} call_sites={}",
                safe_contexts.len(), context_nodes.len(), context_bindings.len());
        }
        if let Some(directory) = std::env::var_os("IRQ_CHECK_DUMP_DIR") {
            summary_gaps.sort();
            let path = std::path::PathBuf::from(directory).join(format!(
                "{}-summary-gaps.txt",
                tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE)
            ));
            if let Err(error) = std::fs::write(path, summary_gaps.join("\n")) {
                failure = Some(format!("cannot write summary coverage evidence: {error}"));
            }
        }
        if std::env::var_os("IRQ_CHECK_STATS").is_some() {
            eprintln!(
                "irq-check: local safety summaries queries={summary_queries} transfers={summary_transfers} gaps={}",
                summary_gaps.len()
            );
        }
        if let Some(reason) = irq_check_rupta::limits::stop_reason() {
            failure = Some(reason.into());
        }
        if failure.is_some() {
            nodes.clear();
            local_nodes.clear();
            complete = false;
            call_complete = false;
            covered_calls.clear();
            pointer_calls.clear();
            context_calls.clear();
            instance_contexts.clear();
            context_bindings.clear();
            safe_contexts.clear();
            safe.clear();
            pointees.clear();
            pointee_ranges.clear();
        }
        if std::env::var_os("IRQ_CHECK_STATS").is_some() {
            eprintln!(
                "irq-check: pointer coverage complete={complete} call_complete={call_complete} unresolved_sites={unresolved_sites} safe_summaries={} alias_summaries={} alias_ranges={}",
                safe.len(),
                pointees.len(),
                pointee_ranges.len()
            );
        }
        let facts = PointerFacts {
            nodes,
            local_nodes,
            complete,
            call_complete,
            covered_calls,
            pointer_calls,
            context_calls,
            instance_contexts,
            context_bindings,
            safe_contexts,
            safe_candidates,
            safe,
            exclusions,
            pointees,
            pointee_ranges,
        };
        Self {
            exported,
            roots: ordered,
            candidates,
            failure,
            binding_independent,
            diagnostic_independent,
            facts,
        }
    }
}
