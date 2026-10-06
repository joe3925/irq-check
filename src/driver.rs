#![feature(rustc_private)]

extern crate rustc_driver;
extern crate rustc_errors;
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
    severity: &'static str,
    category: &'static str,
    reason: String,
    path: Vec<Step>,
}

#[derive(Serialize)]
struct Report {
    checker_version: &'static str,
    rustc_commit: &'static str,
    crate_name: String,
    findings: Vec<Finding>,
}

struct InterruptDiagnostic {
    primary: Span,
    context: Span,
    message: String,
    label: String,
    chain: String,
    report_path: Option<PathBuf>,
}

fn emit_interrupt<G: rustc_errors::EmissionGuarantee>(
    mut diagnostic: rustc_errors::Diag<'_, G>,
    finding: &InterruptDiagnostic,
) {
    diagnostic.span_label(finding.primary, finding.label.clone());
    if finding.context != finding.primary {
        diagnostic.span_note(finding.context, "interrupt context starts here");
    }
    diagnostic.note(format!("call path:\n{}", finding.chain));
    diagnostic.note("this check follows possible branches; it does not prove initialization state or reference counts");
    if let Some(path) = &finding.report_path {
        diagnostic.note(format!(
            "full call path and concrete types: {}",
            path.display()
        ));
    } else {
        diagnostic.help("set IRQ_CHECK_REPORT_DIR to save the full call paths and concrete types");
    }
    diagnostic.emit();
}

fn marked(tcx: TyCtxt<'_>, def: DefId, name: &str) -> bool {
    if def.as_local().is_some_and(|local| {
        matches!(
            tcx.hir_crate(()).owner(tcx, local),
            rustc_hir::MaybeOwner::Phantom
        )
    }) {
        return false;
    }
    tcx.get_attrs_by_path(def, &[Symbol::intern("irq"), Symbol::intern(name)])
        .next()
        .is_some()
}

fn forbidden(tcx: TyCtxt<'_>, instance: Instance<'_>) -> Option<String> {
    let path = tcx.def_path_str(instance.def_id());
    if marked(tcx, instance.def_id(), "forbidden") {
        return Some(format!("{path} is marked irq::forbidden"));
    }
    if tcx.crate_name(instance.def_id().krate).as_str() == "alloc"
        && tcx.is_foreign_item(instance.def_id())
        && [
            "__rust_alloc",
            "__rust_alloc_zeroed",
            "__rust_realloc",
            "__rust_dealloc",
        ]
        .iter()
        .any(|name| {
            tcx.opt_item_name(instance.def_id())
                .is_some_and(|item| item.as_str() == *name)
        })
    {
        return Some(format!("allocator entry {path}"));
    }
    None
}

fn location(tcx: TyCtxt<'_>, span: Span) -> String {
    tcx.sess.source_map().span_to_diagnostic_string(span)
}

fn context_method(tcx: TyCtxt<'_>, def: DefId) -> bool {
    marked(tcx, def, "context")
        || tcx.opt_associated_item(def).is_some_and(|item| {
            item.trait_item_def_id().is_some_and(|method| {
                marked(tcx, method, "context") || marked(tcx, tcx.parent(method), "context")
            }) || item
                .trait_container(tcx)
                .is_some_and(|trait_id| marked(tcx, trait_id, "context"))
        })
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
        operation: "interrupt context".into(),
    });
    path.reverse();
    path
}

impl Callbacks for Checker {
    fn after_analysis<'tcx>(&mut self, _: &Compiler, tcx: TyCtxt<'tcx>) -> Compilation {
        let mut invalid_labels = false;
        for local in tcx.iter_local_def_id() {
            if marked(tcx, local.to_def_id(), "context")
                && !matches!(
                    tcx.def_kind(local),
                    rustc_hir::def::DefKind::Fn
                        | rustc_hir::def::DefKind::AssocFn
                        | rustc_hir::def::DefKind::Trait
                )
            {
                tcx.dcx().span_err(tcx.def_span(local), "irq::context is allowed only on functions, trait methods, and traits; mark the callback function, not its field");
                invalid_labels = true;
            }
        }
        if invalid_labels {
            return Compilation::Stop;
        }
        let mut roots = Vec::new();
        for local in tcx.hir_body_owners() {
            if context_method(tcx, local.to_def_id()) {
                if tcx.generics_of(local).count() == 0 {
                    roots.push(Instance::mono(tcx, local.to_def_id()));
                }
            }
        }
        if roots.is_empty()
            && !tcx
                .iter_local_def_id()
                .any(|local| context_method(tcx, local.to_def_id()))
        {
            if std::env::var("IRQ_CHECK_REQUIRED_CRATE").ok().as_deref()
                == Some(tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE).as_str())
            {
                tcx.dcx()
                    .err("The required crate has no irq::context labels.");
                return Compilation::Stop;
            }
            return Compilation::Continue;
        }
        roots.sort_by_key(|instance| instance.to_string());
        let typing_env = TypingEnv::fully_monomorphized();
        let partitions = tcx.collect_and_partition_mono_items(());
        let mut instances = HashSet::new();
        for cgu in partitions.codegen_units {
            for item in cgu.items().keys() {
                if let MonoItem::Fn(instance) = item {
                    instances.insert(*instance);
                }
            }
        }
        let mut instances: Vec<_> = instances.into_iter().collect();
        instances.sort_by_key(ToString::to_string);
        let mut exported_functions = HashMap::new();
        for instance in &instances {
            if context_method(tcx, instance.def_id())
                && !matches!(instance.def, InstanceKind::Virtual(..))
            {
                roots.push(*instance);
            }
            if matches!(
                tcx.def_kind(instance.def_id()),
                rustc_hir::def::DefKind::Fn | rustc_hir::def::DefKind::AssocFn
            ) {
                exported_functions.insert(tcx.symbol_name(*instance).name.to_owned(), *instance);
            }
        }
        roots.sort_by_key(ToString::to_string);
        roots.dedup();
        if roots.is_empty() {
            if std::env::var("IRQ_CHECK_REQUIRED_CRATE").ok().as_deref()
                == Some(tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE).as_str())
            {
                tcx.dcx()
                    .err("The required crate has no concrete irq::context functions.");
                return Compilation::Stop;
            }
            return Compilation::Continue;
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
                    edges.push(Edge {
                        target: None,
                        span: tcx.def_span(def),
                        kind: "unknown",
                        detail: format!("dynamic dispatch target is unresolved: {instance}"),
                    });
                }
                InstanceKind::Item(def) if tcx.is_foreign_item(def) => {
                    let symbol = tcx.symbol_name(instance).name;
                    if symbol.starts_with("llvm.") {
                        graph.insert(instance, edges);
                        continue;
                    }
                    let target = exported_functions.get(symbol).copied();
                    edges.push(Edge {
                        target,
                        span: tcx.def_span(def),
                        kind: if target.is_some() {
                            "linked Rust function"
                        } else {
                            "external"
                        },
                        detail: format!(
                            "external symbol has no Rust body in this compilation: {symbol}"
                        ),
                    });
                }
                InstanceKind::Item(def)
                    if !tcx.is_mir_available(def)
                        && !matches!(tcx.def_kind(def), rustc_hir::def::DefKind::Ctor(..)) =>
                {
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
                                    edges.push(Edge {
                                        target: None,
                                        span,
                                        kind: "unknown",
                                        detail: format!(
                                            "function pointer target is unresolved: {callee_ty}"
                                        ),
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
        let mut findings = Vec::new();
        let mut coverage_warnings = 0usize;
        let mut diagnostics = Vec::new();
        for root in &roots {
            let mut parents: HashMap<Instance<'tcx>, (Instance<'tcx>, Edge<'tcx>)> = HashMap::new();
            let mut visited = HashSet::from([*root]);
            let mut queue = VecDeque::from([*root]);
            let mut reported_unknown = HashSet::new();
            while let Some(current) = queue.pop_front() {
                for edge in &graph[&current] {
                    if let Some(target) = edge.target {
                        if visited.insert(target) {
                            parents.insert(target, (current, edge.clone()));
                            queue.push_back(target);
                        }
                    } else if reported_unknown.insert((edge.span, edge.detail.clone())) {
                        coverage_warnings += 1;
                        let mut path = trace(tcx, *root, current, &parents);
                        let span = if matches!(current.def, InstanceKind::Virtual(..)) {
                            parents
                                .get(&current)
                                .map_or(edge.span, |(_, call)| call.span)
                        } else {
                            edge.span
                        };
                        path.push(Step {
                            function: current.to_string(),
                            location: location(tcx, span),
                            operation: if edge.kind == "external" {
                                "external call"
                            } else {
                                "unresolved call"
                            }
                            .into(),
                        });
                        findings.push(Finding {
                            severity: "warning",
                            category: "coverage gap",
                            reason: edge.detail.clone(),
                            path,
                        });
                    }
                }
                if let Some(reason) = forbidden(tcx, current) {
                    let path = trace(tcx, *root, current, &parents);
                    let finding = Finding {
                        severity: "error",
                        category: "forbidden",
                        reason,
                        path,
                    };
                    let mut cursor = current;
                    let mut hops = Vec::new();
                    while let Some((parent, edge)) = parents.get(&cursor) {
                        hops.push((*parent, cursor, edge.clone()));
                        cursor = *parent;
                    }
                    hops.reverse();
                    let mut primary = tcx.def_span(root.def_id());
                    let mut operation = "call";
                    let mut chain = vec![tcx.def_path_str(root.def_id())];
                    let mut library_steps = 0;
                    for (parent, target, edge) in &hops {
                        if parent.def_id().is_local() {
                            primary = edge.span.source_callsite();
                            operation = edge.kind;
                        }
                        let crate_name = tcx.crate_name(target.def_id().krate);
                        if matches!(crate_name.as_str(), "core" | "alloc" | "std")
                            && *target != current
                        {
                            library_steps += 1;
                            continue;
                        }
                        if library_steps != 0 {
                            chain.push(format!("{library_steps} library or generated-drop steps"));
                            library_steps = 0;
                        }
                        chain.push(tcx.def_path_str(target.def_id()));
                    }
                    let action = match tcx
                        .opt_item_name(current.def_id())
                        .map(|name| name.as_str().to_owned())
                        .as_deref()
                    {
                        Some("__rust_alloc" | "__rust_alloc_zeroed") => "heap allocation",
                        Some("__rust_realloc") => "heap reallocation",
                        Some("__rust_dealloc") => "heap deallocation",
                        _ => "a forbidden operation",
                    };
                    diagnostics.push(InterruptDiagnostic {
                        primary,
                        context: tcx.def_span(root.def_id()).source_callsite(),
                        message: format!("{action} is reachable from interrupt context"),
                        label: format!("this {operation} can reach {action}"),
                        chain: chain
                            .into_iter()
                            .enumerate()
                            .map(|(index, name)| {
                                if index == 0 {
                                    format!("  {name}")
                                } else {
                                    format!("  -> {name}")
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                        report_path: std::env::var_os("IRQ_CHECK_REPORT_DIR").map(|directory| {
                            PathBuf::from(directory).join(format!(
                                "{}.json",
                                tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE)
                            ))
                        }),
                    });
                    findings.push(finding);
                }
            }
        }
        let report = Report {
            checker_version: env!("CARGO_PKG_VERSION"),
            rustc_commit: COMMIT,
            crate_name: tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE).to_string(),
            findings,
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
        for finding in &diagnostics {
            emit_interrupt(
                tcx.dcx()
                    .struct_span_err(finding.primary, finding.message.clone()),
                finding,
            );
        }
        eprintln!(
            "irq-check: {} interrupt contexts, {} instances, {} errors, {} warnings (see report)",
            roots.len(),
            graph.len(),
            diagnostics.len(),
            coverage_warnings
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
