#![feature(rustc_private)]

extern crate rustc_abi;
extern crate rustc_ast;
extern crate rustc_codegen_ssa;
extern crate rustc_data_structures;
extern crate rustc_driver;
extern crate rustc_errors;
extern crate rustc_hir;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_mir_dataflow;
extern crate rustc_span;
extern crate rustc_symbol_mangling;

mod analysis;
mod policy;
mod trust;

use rustc_driver::{Callbacks, Compilation};
use rustc_hir::def_id::DefId;
use rustc_interface::interface::Compiler;
use rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags;
use rustc_middle::mono::MonoItem;
use rustc_middle::ty::{Instance, InstanceKind, TyCtxt};
use rustc_span::{Span, Symbol};
use std::collections::{HashMap, HashSet, VecDeque};
use std::process::Command;

const TOOLCHAIN: &str = env!("IRQ_CHECK_BUILD_RELEASE");
const COMMIT: &str = env!("IRQ_CHECK_BUILD_COMMIT");

struct Checker;

fn worker_limit() -> usize {
    irq_check_rupta::limits::configuration().expect("validated analysis limits").workers
}

#[derive(Clone)]
struct Edge<'tcx> {
    candidate: bool,
    site: Option<usize>,
    target: Option<analysis::Node<'tcx>>,
    span: Span,
    kind: &'static str,
    detail: String,
    trusted: bool,
    allow_incomplete: bool,
    path_proven: bool,
    uncertainty: Vec<(Span, String)>,
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
    if tcx
        .opt_associated_item(instance.def_id())
        .is_some_and(|item| {
            item.trait_item_def_id()
                .is_some_and(|method| marked(tcx, method, "forbidden"))
        })
    {
        return Some(format!("{path} implements a method marked irq::forbidden"));
    }
    None
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

fn known_intrinsic<'tcx>(tcx: TyCtxt<'tcx>, instance: Instance<'tcx>) -> bool {
    (tcx.crate_name(instance.def_id().krate).as_str() == "alloc"
        && tcx.is_foreign_item(instance.def_id())
        && tcx.codegen_fn_attrs(instance.def_id()).flags.contains(
            CodegenFnAttrFlags::RUSTC_STD_INTERNAL_SYMBOL)
        && tcx.opt_item_name(instance.def_id()).is_some_and(|name|
            name.as_str() == rustc_ast::expand::allocator::NO_ALLOC_SHIM_IS_UNSTABLE))
        || tcx.crate_name(instance.def_id().krate).as_str() == "core"
        && matches!(
            tcx.symbol_name(instance).name,
            "llvm.x86.rdtsc" | "llvm.x86.sse2.pause" | "llvm.aarch64.isb"
        )
}

impl Callbacks for Checker {
    fn after_analysis<'tcx>(&mut self, _: &Compiler, tcx: TyCtxt<'tcx>) -> Compilation {
        let callback_started = std::time::Instant::now();
        let phase_file = std::env::var_os("IRQ_CHECK_DUMP_DIR").map(|directory| {
            let directory = std::path::PathBuf::from(directory);
            let _ = std::fs::create_dir_all(&directory);
            directory.join(format!("{}-phase.txt", tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE)))
        });
        if let Some(path) = &phase_file { let _ = std::fs::write(path, "compiler analysis callback"); }
        if std::env::var_os("IRQ_CHECK_STATS").is_some() {
            eprintln!("irq-check: compiler analysis callback");
        }
        let trust = trust::Trust::collect(tcx);
        if let Some(path) = &phase_file { let _ = std::fs::write(path, "annotations collected; concrete instance collection"); }
        if std::env::var_os("IRQ_CHECK_STATS").is_some() {
            eprintln!("irq-check: annotation collection {:.3}s", callback_started.elapsed().as_secs_f64());
        }
        if trust.invalid {
            return Compilation::Stop;
        }
        if !trust.root || !trust.checked.contains(&rustc_hir::def_id::LOCAL_CRATE) {
            return Compilation::Continue;
        }
        let required = std::env::var("IRQ_CHECK_REQUIRED_CRATE").ok().as_deref()
            == Some(tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE).as_str());
        let mut roots = Vec::new();
        for context in &trust.contexts {
            if tcx.generics_of(*context).count() == 0 {
                roots.push(Instance::mono(tcx, *context));
            }
        }
        for local in tcx.hir_body_owners() {
            if context_method(tcx, local.to_def_id()) && tcx.generics_of(local).count() == 0 {
                roots.push(Instance::mono(tcx, local.to_def_id()));
            }
        }
        let labelled = tcx
            .iter_local_def_id()
            .any(|local| context_method(tcx, local.to_def_id()));
        if !labelled && required {
            tcx.dcx()
                .err("the required crate has no irq::context marks");
            return Compilation::Stop;
        }
        if !labelled && trust.contexts.is_empty() {
            return Compilation::Continue;
        }
        let partitions = tcx.collect_and_partition_mono_items(());
        if std::env::var_os("IRQ_CHECK_STATS").is_some() {
            eprintln!("irq-check: concrete instance collection {:.3}s", callback_started.elapsed().as_secs_f64());
        }
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
        for instance in &instances {
            if trust.checks(instance.def_id())
                && context_method(tcx, instance.def_id())
                && !matches!(instance.def, InstanceKind::Virtual(..))
            {
                roots.push(*instance);
            }
        }
        roots.sort_by_key(ToString::to_string);
        roots.dedup();
        if std::env::var_os("IRQ_CHECK_STATS").is_some() {
            let mut checked: Vec<_> = trust
                .checked
                .iter()
                .map(|krate| tcx.crate_name(*krate).to_string())
                .collect();
            checked.sort();
            eprintln!(
                "irq-check: compiler={COMMIT} target={} selected_crates={} initial_roots={}",
                tcx.sess.opts.target_triple,
                checked.join(","),
                roots.len()
            );
            for root in &roots {
                eprintln!("irq-check: initial IRQ root: {root}");
            }
        }
        irq_check_rupta::limits::begin_analysis();
        if let Some(path) = &phase_file { let _ = std::fs::write(path, "global pointer analysis"); }
        let analysis = analysis::Analysis::build(tcx, &trust, &instances, &roots);
        if let Some(path) = &phase_file { let _ = std::fs::write(path, "IRQ queries complete; diagnostics"); }
        if let Some(directory) = std::env::var_os("IRQ_CHECK_DUMP_DIR") {
            let directory = std::path::PathBuf::from(directory);
            let limits = irq_check_rupta::limits::configuration().expect("validated analysis limits");
            let graph = serde_json::json!({
                "compiler": COMMIT,
                "interface": policy::INTERFACE,
                "limits": { "memory_mib": limits.memory_mib, "timeout_seconds": limits.timeout_seconds,
                    "query_steps": limits.query_steps, "scalar_steps": limits.scalar_steps,
                    "alternatives": limits.alternatives, "workers": limits.workers,
                    "call_depth": limits.call_depth, "function_scratch_mib": limits.function_scratch_mib,
                    "query_scratch_mib": limits.query_scratch_mib },
                "target": tcx.sess.opts.target_triple.to_string(),
                "roots": analysis.roots.values().map(|node| serde_json::json!({
                    "instance": node.instance.to_string(), "context": node.context
                })).collect::<Vec<_>>(),
                "checked_crates": trust.checked.iter().map(|krate| tcx.crate_name(*krate).to_string()).collect::<Vec<_>>(),
                "incomplete": analysis.incomplete,
                "diagnostics_complete": !analysis.incomplete && !analysis.diagnostic_cutoff,
                "detail": analysis.limit_detail,
                "precision_loss": analysis.precision_loss,
                "complete_pointer_coverage": analysis.complete_pointer_coverage,
                "complete_call_coverage": analysis.complete_call_coverage,
                "safe_summaries": analysis.safe_summaries.iter().map(ToString::to_string).collect::<Vec<_>>()
            });
            let result = std::fs::create_dir_all(&directory).and_then(|_| {
                use std::io::Write;
                let path = directory.join(format!("{}.json", tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE)));
                let pending = path.with_extension(format!("json.{}.part", std::process::id()));
                let file = std::fs::File::create(&pending)?;
                let mut writer = std::io::BufWriter::new(file);
                let mut header = serde_json::to_vec(&graph).map_err(std::io::Error::other)?;
                header.pop();
                writer.write_all(&header)?;
                if analysis.incomplete {
                    writer.write_all(b",\"sites\":[],\"edges\":[],\"candidates\":[]}")?;
                    writer.flush()?;
                    drop(writer);
                    return std::fs::rename(&pending, &path);
                }
                writer.write_all(b",\"sites\":[")?;
                let mut first = true;
                for (id, site) in analysis.sites.iter().enumerate() {
                    if analysis.free_sites.contains(&id) {
                        continue;
                    }
                    if !first {
                        writer.write_all(b",")?;
                    }
                    first = false;
                    writer.write_all(&serde_json::to_vec(&serde_json::json!({
                        "id": id, "caller": site.caller.instance.to_string(), "context": site.caller.context,
                        "location": format!("{:?}", site.location), "state_id": site.state,
                        "source_scope": format!("{:?}", site.scope), "normal": format!("{:?}", site.normal),
                        "unwind": format!("{:?}", site.unwind), "state": analysis.site_state(id)
                    })).map_err(std::io::Error::other)?)?;
                }
                writer.write_all(b"],\"edges\":[")?;
                first = true;
                for (caller, edges) in &analysis.graph {
                    for edge in edges {
                        if edge.candidate {
                            continue;
                        }
                        if !first {
                            writer.write_all(b",")?;
                        }
                        first = false;
                        writer.write_all(&serde_json::to_vec(&serde_json::json!({
                            "caller": caller.instance.to_string(), "caller_context": caller.context,
                            "callee": edge.target.map(|node| node.instance.to_string()),
                            "callee_context": edge.target.map(|node| node.context),
                            "span": format!("{:?}", edge.span), "kind": edge.kind,
                            "detail": edge.detail, "trusted": edge.trusted, "site": edge.site,
                            "allow_incomplete": edge.allow_incomplete, "path_proven": edge.path_proven,
                            "uncertainty": edge.uncertainty.iter().map(|(span, reason)| serde_json::json!({
                                "span": format!("{span:?}"), "reason": reason,
                            })).collect::<Vec<_>>(),
                            "evidence": if edge.target.is_some() { "bound" } else { "unknown" }
                        })).map_err(std::io::Error::other)?)?;
                    }
                }
                writer.write_all(b"],\"candidates\":[")?;
                first = true;
                for (caller, edges) in &analysis.graph {
                    for edge in edges.iter().filter(|edge| edge.candidate) {
                        if !first { writer.write_all(b",")?; }
                        first = false;
                        writer.write_all(&serde_json::to_vec(&serde_json::json!({
                            "caller": caller.instance.to_string(), "caller_context": caller.context,
                            "callee": edge.target.map(|node| node.instance.to_string()),
                            "callee_context": edge.target.map(|node| node.context),
                            "span": format!("{:?}", edge.span), "kind": edge.kind,
                            "detail": edge.detail, "site": edge.site, "evidence": "candidate"
                        })).map_err(std::io::Error::other)?)?;
                    }
                }
                writer.write_all(b"]}")?;
                writer.flush()?;
                drop(writer);
                std::fs::rename(&pending, &path)
            });
            if let Err(error) = result {
                tcx.dcx()
                    .err(format!("cannot write the analysis evidence: {error}"));
                return Compilation::Stop;
            }
        }
        if analysis.incomplete {
            let mut diagnostic = tcx.dcx().struct_span_err(
                roots
                    .first()
                    .map_or(tcx.def_span(rustc_hir::def_id::CRATE_DEF_ID), |root| {
                        tcx.def_span(root.def_id())
                    }),
                "interrupt call analysis did not reach a complete result",
            );
            diagnostic
                .note("required analysis coverage is incomplete; no partial result is accepted");
            diagnostic.note(analysis.limit_detail.clone());
            diagnostic.emit();
            return Compilation::Stop;
        }
        let mut reported = HashSet::new();
        let mut errors = 0;
        let mut accepted_gaps = Vec::new();
        if analysis.roots.is_empty() && required {
            tcx.dcx()
                .err("the required crate has no concrete irq::context functions");
            return Compilation::Stop;
        }
        let mut root_nodes: Vec<_> = analysis.roots.values().copied().collect();
        root_nodes.sort_by_key(|node| (node.instance.to_string(), node.context));
        for root in root_nodes {
            let mut parents = HashMap::new();
            let root_visit = (root, None::<usize>, true);
            let mut visited = HashSet::from([root_visit]);
            let mut queue = VecDeque::from([root_visit]);
            while let Some(visit @ (current, permission, path_proven)) = queue.pop_front() {
                let mut failures = Vec::new();
                if let Some(reason) = forbidden(tcx, current.instance) {
                    let parent: Option<&(_, Edge<'tcx>)> = parents.get(&visit);
                    let span = parent.map_or(tcx.def_span(current.instance.def_id()), |(_, edge)| edge.span);
                    let site = parent.and_then(|(_, edge)| edge.site);
                    let accepted_by = permission.or_else(|| parent.filter(|(_, edge)| edge.trusted).and_then(|(_, edge)| edge.site));
                    if !path_proven && let Some(mark) = accepted_by {
                        accepted_gaps.push(serde_json::json!({"root": root.instance.to_string(),
                            "instance": current.instance.to_string(), "context": current.context, "site": site,
                            "mark_site": mark, "kind": "unresolved reachability", "detail": reason,
                            "mark": if permission.is_some() { "allow_incomplete" } else { "trusted" }}));
                    } else {
                        failures.push((span, if path_proven { reason } else {
                            format!("the call target is established, but its IRQ path is unresolved: {reason}")
                        }, false, "call", site));
                    }
                }
                for edge in analysis.graph.get(&current).into_iter().flatten() {
                    if edge.candidate {
                        continue;
                    }
                    let permission = if edge.allow_incomplete { edge.site.or(permission) } else { permission };
                    if let Some(target) = edge.target {
                        let next = (target, permission, path_proven && edge.path_proven);
                        if visited.insert(next) {
                            parents.insert(next, (visit, edge.clone()));
                            queue.push_back(next);
                        }
                    } else if edge.trusted || permission.is_some() {
                        accepted_gaps.push(serde_json::json!({"root": root.instance.to_string(),
                            "instance": current.instance.to_string(), "context": current.context, "site": edge.site,
                            "mark_site": permission.or(edge.site), "kind": edge.kind, "detail": edge.detail,
                            "mark": if permission.is_some() { "allow_incomplete" } else { "trusted" }}));
                    } else {
                        failures.push((edge.span, edge.detail.clone(), true, edge.kind, edge.site));
                    }
                }
                for (span, reason, unknown, operation, site) in failures {
                    if !reported.insert((span, reason.clone())) {
                        continue;
                    }
                    let mut cursor = visit;
                    let mut hops = Vec::new();
                    while let Some((parent, edge)) = parents.get(&cursor) {
                        hops.push((*parent, cursor, edge.clone()));
                        cursor = *parent;
                    }
                    hops.reverse();
                    let mut uncertainty = Vec::new();
                    if unknown || !path_proven {
                        for edge in hops.iter().map(|(_, _, edge)| edge).chain(
                            analysis.graph.get(&current).into_iter().flatten()
                                .filter(|edge| edge.site == site && edge.target.is_none()))
                        {
                            for cause in &edge.uncertainty {
                                if !uncertainty.contains(cause) { uncertainty.push(cause.clone()); }
                            }
                        }
                    }
                    let mut primary = span.source_callsite();
                    if primary.is_dummy() || !current.instance.def_id().is_local() {
                        for (parent, _, edge) in &hops {
                            if parent.0.instance.def_id().is_local() {
                                primary = edge.span.source_callsite();
                            }
                        }
                    }
                    let mut chain = vec![tcx.def_path_str(root.instance.def_id())];
                    for (_, target, _) in hops {
                        chain.push(format!("{} [context {}]", target.0.instance, target.0.context));
                    }
                    let message = if unknown {
                        "interrupt call path cannot be fully checked".to_string()
                    } else {
                        "forbidden operation is reachable from interrupt context"
                            .to_string()
                    };
                    let mut diagnostic = tcx.dcx().struct_span_err(primary, message);
                    diagnostic.span_label(
                        primary,
                        if unknown {
                            format!("this {operation} has an unknown call path")
                        } else {
                            format!("this {operation} can reach a forbidden function")
                        },
                    );
                    diagnostic.span_note(
                        tcx.def_span(root.instance.def_id()).source_callsite(),
                        "interrupt context starts here",
                    );
                    if span != primary && !span.is_dummy() {
                        diagnostic.span_note(
                            span,
                            if unknown {
                                "the unchecked operation is here"
                            } else {
                                "the forbidden operation is here"
                            },
                        );
                    }
                    diagnostic.note(reason);
                    if !unknown && !path_proven {
                        diagnostic.note("reachability is uncertain: analysis could not determine whether execution reaches this forbidden call");
                    }
                    for (cause, reason) in &uncertainty {
                        if !cause.is_dummy() {
                            diagnostic.span_note(*cause, format!("cause of uncertainty: {reason}"));
                        }
                    }
                    if !uncertainty.is_empty() {
                        diagnostic.note("checking continued along possible control-flow paths; unresolved values and effects were not assumed safe");
                    }
                    if !unknown {
                        if path_proven {
                            diagnostic.note("the call target and a feasible interrupt path are established");
                        }
                        diagnostic.note("irq::unreachable(unsafe) can exclude a reviewed fatal IRQ path only in an explicit block that cannot return");
                    }
                    if let Some(id) = site {
                        let site = &analysis.sites[id];
                        diagnostic.note(format!(
                            "analysis site {id}: {} [context {}], {:?}, state {}, scope {:?}",
                            site.caller.instance,
                            site.caller.context,
                            site.location,
                            site.state,
                            site.scope
                        ));
                        if std::env::var_os("IRQ_CHECK_EXPLAIN").is_some() {
                            diagnostic.note(format!(
                                "argument and storage state:\n{}",
                                analysis.site_state(id)
                            ));
                        }
                    }
                    diagnostic.note(format!(
                        "call path:\n{}",
                        chain
                            .into_iter()
                            .enumerate()
                            .map(|(index, name)| if index == 0 {
                                format!("  {name}")
                            } else {
                                format!("  -> {name}")
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    ));
                    if unknown {
                        diagnostic.help("keep a typed call target, or check this operation and put it in #[cfg_attr(irq_check, irq::trusted(unsafe))] unsafe { ... }");
                        diagnostic.note("trust applies only to unknown operations in that unsafe block; it does not allow known forbidden calls or pass to returned values");
                        diagnostic.note("check all actual callback targets and mark the callback function or trait method with irq::context");
                    }
                    diagnostic.emit();
                    errors += 1;
                }
            }
        }
        if let Some(directory) = std::env::var_os("IRQ_CHECK_DUMP_DIR") {
            let path = std::path::PathBuf::from(directory).join(format!("{}-accepted-gaps.json",
                tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE)));
            let result = serde_json::to_vec_pretty(&accepted_gaps).map_err(std::io::Error::other)
                .and_then(|bytes| std::fs::write(path, bytes));
            if let Err(error) = result {
                tcx.dcx().err(format!("cannot write accepted gap evidence: {error}"));
                return Compilation::Stop;
            }
        }
        if errors == 0 {
            if analysis.diagnostic_cutoff {
                tcx.dcx().err("interrupt call checking stopped before all required paths were checked");
                Compilation::Stop
            } else {
                Compilation::Continue
            }
        } else {
            Compilation::Stop
        }
    }
}

fn main() -> std::process::ExitCode {
    let mut args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|arg| arg == "--build-identity") {
        print!(
            "{}",
            include_str!(concat!(env!("OUT_DIR"), "/driver-identity.txt"))
        );
        return std::process::ExitCode::SUCCESS;
    }
    if args
        .get(1)
        .is_some_and(|argument| argument == "--irq-check-arguments")
    {
        let arguments = args
            .get(2)
            .ok_or_else(|| "missing driver argument file".to_owned())
            .and_then(|path| std::fs::read(path).map_err(|error| error.to_string()))
            .and_then(|bytes| {
                serde_json::from_slice::<Vec<String>>(&bytes).map_err(|error| error.to_string())
            });
        match arguments {
            Ok(arguments) => {
                args.truncate(1);
                args.extend(arguments);
            }
            Err(error) => {
                eprintln!("error: cannot read driver arguments: {error}");
                return std::process::ExitCode::from(2);
            }
        }
    }
    if args.get(1).is_some_and(|arg| arg == "--version") {
        println!(
            "irq-check {} {TOOLCHAIN} {COMMIT} {}",
            env!("CARGO_PKG_VERSION"),
            policy::INTERFACE
        );
        return std::process::ExitCode::SUCCESS;
    }
    if args.len() < 2 {
        eprintln!("error: use irq-check as RUSTC_WRAPPER; required compiler: {TOOLCHAIN}");
        std::process::exit(2);
    }
    if let Err(error) = irq_check_rupta::limits::install_process_limit() {
        eprintln!("error: {error}");
        return std::process::ExitCode::from(2);
    }
    let rustc = args.remove(1);
    let version = Command::new(&rustc)
        .arg("-Vv")
        .output()
        .expect("cannot run rustc");
    if !version.status.success()
        || String::from_utf8_lossy(&version.stdout).trim()
            != include_str!(concat!(env!("OUT_DIR"), "/compiler-identity.txt")).trim()
    {
        eprintln!(
            "error: irq-check was built with {TOOLCHAIN} ({COMMIT})\nhelp: rebuild and install both checker binaries with the selected nightly"
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
    let workers = worker_limit();
    args.extend([
        "-Zunstable-options".into(),
        format!("--jobs={workers}"),
        format!("--jobs-frontend={workers}"),
    ]);
    if std::env::var_os("IRQ_CHECK_STATS").is_some() {
        eprintln!("irq-check: worker-limit={workers}");
    }
    args.extend([
        "-Zalways-encode-mir".into(),
        "-Zmir-opt-level=0".into(),
        "-Zmir-enable-passes=-Inline,-ForceInline".into(),
        "-Zmaximal-hir-to-mir-coverage".into(),
        "-Clink-dead-code=yes".into(),
        "-Zcrate-attr=feature(register_tool,stmt_expr_attributes)".into(),
        "-Zcrate-attr=register_tool(irq)".into(),
        "--cfg=irq_check".into(),
        "--check-cfg=cfg(irq_check)".into(),
    ]);
    rustc_driver::catch_with_exit_code(|| rustc_driver::run_compiler(&args, &mut Checker))
}
