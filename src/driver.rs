#![feature(rustc_private)]

extern crate rustc_driver;
extern crate rustc_hir;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_span;

use rustc_driver::{Callbacks, Compilation};
use rustc_hir::def_id::DefId;
use rustc_interface::interface::Compiler;
use rustc_middle::mir::TerminatorKind;
use rustc_middle::mir::mono::MonoItem;
use rustc_middle::ty::{self, EarlyBinder, Instance, InstanceKind, TyCtxt, TypingEnv};
use rustc_span::{Span, Symbol};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::process::Command;

const TOOLCHAIN: &str = "nightly-2026-04-07";
const COMMIT: &str = "bcded331651b60a0383b3ff51db4f24c4495ac53";

struct Checker;

#[derive(Clone)]
struct Edge<'tcx> {
    target: Option<Instance<'tcx>>,
    span: Span,
    kind: &'static str,
    detail: String,
}

#[derive(Serialize)]
struct Step {
    function: String,
    location: String,
    operation: String,
}

#[derive(Serialize)]
struct Finding {
    category: &'static str,
    reason: String,
    path: Vec<Step>,
}

#[derive(Serialize)]
struct Report {
    checker_version: &'static str,
    rustc_commit: &'static str,
    crate_name: String,
    roots: Vec<String>,
    checked_instances: usize,
    findings: Vec<Finding>,
    allocation_sites: Vec<Finding>,
}

fn marked(tcx: TyCtxt<'_>, def: DefId, name: &str) -> bool {
    tcx.get_attrs_by_path(def, &[Symbol::intern("irq"), Symbol::intern(name)])
        .next()
        .is_some()
}

fn forbidden(tcx: TyCtxt<'_>, instance: Instance<'_>) -> Option<String> {
    let path = tcx.def_path_str(instance.def_id());
    if marked(tcx, instance.def_id(), "forbidden") {
        return Some(format!("{path} is marked irq::forbidden"));
    }
    if [
        "__rust_alloc",
        "__rust_alloc_zeroed",
        "__rust_realloc",
        "__rust_dealloc",
    ]
    .iter()
    .any(|name| {
        tcx.opt_item_name(instance.def_id())
            .is_some_and(|item| item.as_str() == *name)
    }) {
        return Some(format!("allocator entry {path}"));
    }
    None
}

fn location(tcx: TyCtxt<'_>, span: Span) -> String {
    tcx.sess.source_map().span_to_diagnostic_string(span)
}

fn trace<'tcx>(
    tcx: TyCtxt<'tcx>,
    root: Instance<'tcx>,
    mut current: Instance<'tcx>,
    parents: &HashMap<Instance<'tcx>, (Instance<'tcx>, Edge<'tcx>)>,
) -> Vec<Step> {
    let mut path = Vec::new();
    while let Some((parent, edge)) = parents.get(&current) {
        path.push(Step {
            function: current.to_string(),
            location: location(tcx, edge.span),
            operation: edge.kind.into(),
        });
        current = *parent;
    }
    path.push(Step {
        function: root.to_string(),
        location: location(tcx, tcx.def_span(root.def_id())),
        operation: "handler".into(),
    });
    path.reverse();
    path
}

impl Callbacks for Checker {
    fn after_analysis<'tcx>(&mut self, _: &Compiler, tcx: TyCtxt<'tcx>) -> Compilation {
        let mut roots = Vec::new();
        for local in tcx.hir_body_owners() {
            if marked(tcx, local.to_def_id(), "handler") {
                if tcx.generics_of(local).count() != 0 {
                    tcx.dcx().span_err(
                        tcx.def_span(local),
                        "irq::handler must have no generic parameters",
                    );
                } else {
                    roots.push(Instance::mono(tcx, local.to_def_id()));
                }
            }
        }
        if roots.is_empty() {
            if std::env::var("IRQ_CHECK_REQUIRED_CRATE").ok().as_deref()
                == Some(tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE).as_str())
            {
                tcx.dcx()
                    .err("The required crate has no irq::handler entry points.");
                return Compilation::Stop;
            }
            return Compilation::Continue;
        }
        roots.sort_by_key(|instance| instance.to_string());
        let typing_env = TypingEnv::fully_monomorphized();
        let partitions = tcx.collect_and_partition_mono_items(());
        let mut candidate_set = HashSet::new();
        for cgu in partitions.codegen_units {
            for item in cgu.items().keys() {
                if let MonoItem::Fn(instance) = item {
                    candidate_set.insert(*instance);
                }
            }
        }
        let mut candidates: Vec<_> = candidate_set.into_iter().collect();
        candidates.sort_by_key(ToString::to_string);
        let mut pointer_candidates: HashMap<ty::Ty<'tcx>, Vec<Instance<'tcx>>> = HashMap::new();
        for candidate in &candidates {
            if matches!(
                tcx.def_kind(candidate.def_id()),
                rustc_hir::def::DefKind::Fn | rustc_hir::def::DefKind::AssocFn
            ) {
                let function_ty = candidate.ty(tcx, typing_env);
                if matches!(function_ty.kind(), ty::FnDef(..)) {
                    let signature =
                        tcx.normalize_erasing_regions(typing_env, function_ty.fn_sig(tcx));
                    let pointer_ty = ty::Ty::new_fn_ptr(tcx, signature);
                    pointer_candidates
                        .entry(pointer_ty)
                        .or_default()
                        .push(*candidate);
                }
            }
        }
        let mut graph: HashMap<Instance<'tcx>, Vec<Edge<'tcx>>> = HashMap::new();
        let mut pending: VecDeque<_> = roots.iter().copied().collect();
        while let Some(instance) = pending.pop_front() {
            if graph.contains_key(&instance) {
                continue;
            }
            let mut edges = Vec::new();
            if forbidden(tcx, instance).is_some() || marked(tcx, instance.def_id(), "trusted") {
                graph.insert(instance, edges);
                continue;
            }
            match instance.def {
                InstanceKind::Intrinsic(_) | InstanceKind::DropGlue(_, None) => {
                    graph.insert(instance, edges);
                    continue;
                }
                InstanceKind::Virtual(def, _) => {
                    for candidate in &candidates {
                        if tcx
                            .opt_associated_item(candidate.def_id())
                            .and_then(|item| item.trait_item_def_id())
                            == Some(def)
                        {
                            edges.push(Edge {
                                target: Some(*candidate),
                                span: tcx.def_span(candidate.def_id()),
                                kind: "virtual candidate",
                                detail: String::new(),
                            });
                        }
                    }
                    edges.push(Edge {
                        target: None,
                        span: tcx.def_span(def),
                        kind: "unknown",
                        detail: format!("dynamic dispatch requires a contract: {instance}"),
                    });
                }
                InstanceKind::Item(def) if !tcx.is_mir_available(def) => {
                    edges.push(Edge {
                        target: None,
                        span: tcx.def_span(def),
                        kind: "unknown",
                        detail: format!("MIR is unavailable for {instance}"),
                    });
                }
                _ => {
                    let body = tcx.instance_mir(instance.def);
                    let mut blocks = VecDeque::from([rustc_middle::mir::START_BLOCK]);
                    let mut seen = HashSet::new();
                    while let Some(block) = blocks.pop_front() {
                        if !seen.insert(block) {
                            continue;
                        }
                        let terminator = body.basic_blocks[block].terminator();
                        blocks.extend(terminator.successors());
                        let span = terminator.source_info.span;
                        match &terminator.kind {
                            TerminatorKind::Call { func, .. }
                            | TerminatorKind::TailCall { func, .. } => {
                                let callee_ty = instance
                                    .instantiate_mir_and_normalize_erasing_regions(
                                        tcx,
                                        typing_env,
                                        EarlyBinder::bind(func.ty(body, tcx)),
                                    );
                                if let ty::FnDef(def, args) = *callee_ty.kind() {
                                    let target = Instance::try_resolve(tcx, typing_env, def, args)
                                        .ok()
                                        .flatten();
                                    edges.push(Edge {
                                        target,
                                        span,
                                        kind: "call",
                                        detail: format!("cannot resolve {callee_ty}"),
                                    });
                                } else {
                                    for candidate in
                                        pointer_candidates.get(&callee_ty).into_iter().flatten()
                                    {
                                        edges.push(Edge {
                                            target: Some(*candidate),
                                            span,
                                            kind: "function pointer candidate",
                                            detail: String::new(),
                                        });
                                    }
                                    edges.push(Edge {
                                        target: None,
                                        span,
                                        kind: "unknown",
                                        detail: format!("function pointer requires a contract: {callee_ty}; {} candidate targets", pointer_candidates.get(&callee_ty).map_or(0, Vec::len)),
                                    });
                                }
                            }
                            TerminatorKind::Drop { place, .. } => {
                                let dropped_ty = instance
                                    .instantiate_mir_and_normalize_erasing_regions(
                                        tcx,
                                        typing_env,
                                        EarlyBinder::bind(place.ty(body, tcx).ty),
                                    );
                                edges.push(Edge {
                                    target: Some(Instance::resolve_drop_in_place(tcx, dropped_ty)),
                                    span,
                                    kind: "drop",
                                    detail: String::new(),
                                });
                            }
                            TerminatorKind::InlineAsm { .. } => edges.push(Edge {
                                target: None,
                                span,
                                kind: "unknown",
                                detail: "assembly requires an irq::trusted boundary".into(),
                            }),
                            _ => {}
                        }
                    }
                }
            }
            for edge in &edges {
                if let Some(target) = edge.target {
                    if !graph.contains_key(&target) {
                        pending.push_back(target);
                    }
                }
            }
            graph.insert(instance, edges);
        }
        let mut reverse: HashMap<Instance<'tcx>, Vec<(Instance<'tcx>, Edge<'tcx>)>> =
            HashMap::new();
        let mut sink_queue = VecDeque::new();
        let mut sinks = HashSet::new();
        let mut toward_sink: HashMap<Instance<'tcx>, (Instance<'tcx>, Edge<'tcx>)> = HashMap::new();
        let mut ordered: Vec<_> = graph.keys().copied().collect();
        ordered.sort_by_key(ToString::to_string);
        for current in ordered {
            if forbidden(tcx, current).is_some() {
                sinks.insert(current);
                sink_queue.push_back(current);
            }
            for edge in &graph[&current] {
                if let Some(target) = edge.target {
                    reverse
                        .entry(target)
                        .or_default()
                        .push((current, edge.clone()));
                }
            }
        }
        while let Some(target) = sink_queue.pop_front() {
            for (current, edge) in reverse.get(&target).into_iter().flatten() {
                if !sinks.contains(current) && !toward_sink.contains_key(current) {
                    toward_sink.insert(*current, (target, edge.clone()));
                    sink_queue.push_back(*current);
                }
            }
        }
        let mut allocation_sites = Vec::new();
        let mut reported_sites = HashSet::new();
        let mut findings = Vec::new();
        let mut diagnostics = Vec::new();
        for root in &roots {
            let mut parents: HashMap<Instance<'tcx>, (Instance<'tcx>, Edge<'tcx>)> = HashMap::new();
            let mut visited = HashSet::from([*root]);
            let mut queue = VecDeque::from([*root]);
            let mut reported_unknown = HashSet::new();
            while let Some(current) = queue.pop_front() {
                let blocked = forbidden(tcx, current);
                let mut leaves = Vec::new();
                if let Some(reason) = blocked {
                    leaves.push(("forbidden", reason, tcx.def_span(current.def_id())));
                }
                for edge in &graph[&current] {
                    if let Some(target) = edge.target {
                        if visited.insert(target) {
                            parents.insert(target, (current, edge.clone()));
                            queue.push_back(target);
                        }
                        let from_crate = tcx.crate_name(current.def_id().krate);
                        let into_crate = tcx.crate_name(target.def_id().krate);
                        if ((!matches!(from_crate.as_str(), "core" | "alloc" | "std")
                            && into_crate.as_str() == "alloc")
                            || (current.def_id().is_local()
                                && matches!(
                                    edge.kind,
                                    "function pointer candidate" | "virtual candidate"
                                )))
                            && toward_sink.contains_key(&target)
                            && reported_sites.insert((*root, edge.span, target.def_id()))
                        {
                            let mut path = trace(tcx, *root, current, &parents);
                            path.push(Step {
                                function: target.to_string(),
                                location: location(tcx, edge.span),
                                operation: edge.kind.into(),
                            });
                            let mut cursor = target;
                            while let Some((next, hop)) = toward_sink.get(&cursor) {
                                path.push(Step {
                                    function: next.to_string(),
                                    location: location(tcx, hop.span),
                                    operation: hop.kind.into(),
                                });
                                cursor = *next;
                            }
                            allocation_sites.push(Finding {
                                category: "allocation site",
                                reason: forbidden(tcx, cursor).unwrap(),
                                path,
                            });
                        }
                    } else if reported_unknown.insert((edge.span, edge.detail.clone())) {
                        leaves.push(("unknown", edge.detail.clone(), edge.span));
                    }
                }
                for (category, reason, span) in leaves {
                    let path = trace(tcx, *root, current, &parents);
                    let trace = path
                        .iter()
                        .map(|step| {
                            format!(
                                "  {} [{}; {}]",
                                step.function, step.operation, step.location
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    diagnostics.push((
                        category,
                        span,
                        format!("interrupt {category}: {reason}\n{trace}"),
                    ));
                    findings.push(Finding {
                        category,
                        reason,
                        path,
                    });
                }
            }
        }
        let report = Report {
            checker_version: env!("CARGO_PKG_VERSION"),
            rustc_commit: COMMIT,
            crate_name: tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE).to_string(),
            roots: roots.iter().map(ToString::to_string).collect(),
            checked_instances: graph.len(),
            findings,
            allocation_sites,
        };
        if let Some(directory) = std::env::var_os("IRQ_CHECK_REPORT_DIR") {
            let directory = PathBuf::from(directory);
            let result = std::fs::create_dir_all(&directory).and_then(|_| {
                std::fs::write(
                    directory.join(format!("{}.json", report.crate_name)),
                    serde_json::to_vec_pretty(&report).unwrap(),
                )
            });
            if let Err(error) = result {
                tcx.dcx()
                    .err(format!("cannot write interrupt report: {error}"));
            }
        }
        diagnostics.sort_by_key(|(category, _, _)| *category);
        for (_, span, message) in &diagnostics {
            tcx.dcx().span_err(*span, message.clone());
        }
        eprintln!(
            "irq-check: {} handlers, {} instances, {} findings",
            roots.len(),
            graph.len(),
            diagnostics.len()
        );
        if diagnostics.is_empty() {
            Compilation::Continue
        } else {
            Compilation::Stop
        }
    }
}

fn main() -> std::process::ExitCode {
    let mut args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|arg| arg == "--version") {
        println!(
            "irq-check {} {TOOLCHAIN} {COMMIT}",
            env!("CARGO_PKG_VERSION")
        );
        return std::process::ExitCode::SUCCESS;
    }
    if args.len() < 2 {
        eprintln!("Use irq-check as RUSTC_WRAPPER. Required compiler: {TOOLCHAIN}.");
        std::process::exit(2);
    }
    let rustc = args.remove(1);
    let version = Command::new(&rustc)
        .arg("-Vv")
        .output()
        .expect("cannot run rustc");
    if !version.status.success() || !String::from_utf8_lossy(&version.stdout).contains(COMMIT) {
        eprintln!(
            "irq-check requires {TOOLCHAIN} ({COMMIT}). Select that toolchain and rebuild dependencies."
        );
        std::process::exit(2);
    }
    let sysroot = Command::new(&rustc)
        .args(["--print", "sysroot"])
        .output()
        .expect("cannot locate rustc sysroot");
    args.extend([
        "--sysroot".into(),
        String::from_utf8(sysroot.stdout).unwrap().trim().into(),
    ]);
    args.extend([
        "-Zalways-encode-mir".into(),
        "-Zmir-opt-level=3".into(),
        "-Zmir-enable-passes=-Inline,-ForceInline".into(),
        "-Clink-dead-code=yes".into(),
        "-Zcrate-attr=feature(register_tool)".into(),
        "-Zcrate-attr=register_tool(irq)".into(),
        "--cfg=irq_check".into(),
        "--check-cfg=cfg(irq_check)".into(),
    ]);
    let result =
        rustc_driver::catch_with_exit_code(|| rustc_driver::run_compiler(&args, &mut Checker));
    result
}
