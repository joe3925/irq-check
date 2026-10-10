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
mod pointer;
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

#[derive(Clone)]
struct Edge<'tcx> {
    site: Option<usize>,
    target: Option<analysis::Node<'tcx>>,
    span: Span,
    kind: &'static str,
    detail: String,
    trusted: bool,
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
    if tcx.codegen_fn_attrs(instance.def_id()).flags.intersects(
        CodegenFnAttrFlags::ALLOCATOR
            | CodegenFnAttrFlags::ALLOCATOR_ZEROED
            | CodegenFnAttrFlags::REALLOCATOR
            | CodegenFnAttrFlags::DEALLOCATOR,
    ) {
        return Some(format!("allocator entry {path}"));
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
    tcx.crate_name(instance.def_id().krate).as_str() == "core"
        && matches!(
            tcx.symbol_name(instance).name,
            "llvm.x86.rdtsc" | "llvm.x86.sse2.pause" | "llvm.aarch64.isb"
        )
}

impl Callbacks for Checker {
    fn after_analysis<'tcx>(&mut self, _: &Compiler, tcx: TyCtxt<'tcx>) -> Compilation {
        let trust = trust::Trust::collect(tcx);
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
        let analysis = analysis::Analysis::build(tcx, &trust, &instances, &roots);
        if let Some(directory) = std::env::var_os("IRQ_CHECK_DUMP_DIR") {
            let directory = std::path::PathBuf::from(directory);
            let graph = serde_json::json!({
                "compiler": COMMIT,
                "target": tcx.sess.opts.target_triple.to_string(),
                "roots": analysis.roots.values().map(|node| serde_json::json!({
                    "instance": node.instance.to_string(), "context": node.context
                })).collect::<Vec<_>>(),
                "checked_crates": trust.checked.iter().map(|krate| tcx.crate_name(*krate).to_string()).collect::<Vec<_>>(),
                "incomplete": analysis.incomplete,
                "detail": analysis.limit_detail,
                "precision_loss": analysis.precision_loss
            });
            let result = std::fs::create_dir_all(&directory).and_then(|_| {
                use std::io::Write;
                let file = std::fs::File::create(
                    directory.join(format!(
                        "{}.json",
                        tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE)
                    )),
                )?;
                let mut writer = std::io::BufWriter::new(file);
                let mut header = serde_json::to_vec(&graph).map_err(std::io::Error::other)?;
                header.pop();
                writer.write_all(&header)?;
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
                        if !first {
                            writer.write_all(b",")?;
                        }
                        first = false;
                        writer.write_all(&serde_json::to_vec(&serde_json::json!({
                            "caller": caller.instance.to_string(), "caller_context": caller.context,
                            "callee": edge.target.map(|node| node.instance.to_string()),
                            "callee_context": edge.target.map(|node| node.context),
                            "span": format!("{:?}", edge.span), "kind": edge.kind,
                            "detail": edge.detail, "trusted": edge.trusted, "site": edge.site
                        })).map_err(std::io::Error::other)?)?;
                    }
                }
                writer.write_all(b"]}")?;
                writer.flush()
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
        if analysis.roots.is_empty() && required {
            tcx.dcx()
                .err("the required crate has no concrete irq::context functions");
            return Compilation::Stop;
        }
        let mut root_nodes: Vec<_> = analysis.roots.values().copied().collect();
        root_nodes.sort_by_key(|node| (node.instance.to_string(), node.context));
        for root in root_nodes {
            let mut parents: HashMap<analysis::Node<'tcx>, (analysis::Node<'tcx>, Edge<'tcx>)> =
                HashMap::new();
            let mut visited = HashSet::from([root]);
            let mut queue = VecDeque::from([root]);
            while let Some(current) = queue.pop_front() {
                let mut failures = Vec::new();
                if let Some(reason) = forbidden(tcx, current.instance) {
                    failures.push((
                        parents
                            .get(&current)
                            .map_or(tcx.def_span(current.instance.def_id()), |(_, edge)| {
                                edge.span
                            }),
                        reason,
                        false,
                        "call",
                        parents.get(&current).and_then(|(_, edge)| edge.site),
                    ));
                }
                for edge in analysis.graph.get(&current).into_iter().flatten() {
                    if let Some(target) = edge.target {
                        if visited.insert(target) {
                            parents.insert(target, (current, edge.clone()));
                            queue.push_back(target);
                        }
                    } else if !edge.trusted {
                        failures.push((edge.span, edge.detail.clone(), true, edge.kind, edge.site));
                    }
                }
                for (span, reason, unknown, operation, site) in failures {
                    if !reported.insert((span, reason.clone())) {
                        continue;
                    }
                    let mut cursor = current;
                    let mut hops = Vec::new();
                    while let Some((parent, edge)) = parents.get(&cursor) {
                        hops.push((*parent, cursor, edge.clone()));
                        cursor = *parent;
                    }
                    hops.reverse();
                    let mut primary = span.source_callsite();
                    if primary.is_dummy() || !current.instance.def_id().is_local() {
                        for (parent, _, edge) in &hops {
                            if parent.instance.def_id().is_local() {
                                primary = edge.span.source_callsite();
                            }
                        }
                    }
                    let mut chain = vec![tcx.def_path_str(root.instance.def_id())];
                    for (_, target, _) in hops {
                        chain.push(format!("{} [context {}]", target.instance, target.context));
                    }
                    let message = if unknown {
                        "interrupt call path cannot be fully checked".to_string()
                    } else {
                        "forbidden operation is possibly reachable from interrupt context"
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
                    }
                    diagnostic.emit();
                    errors += 1;
                }
            }
        }
        if errors == 0 {
            Compilation::Continue
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
    let workers = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    args.extend([
        "-Zunstable-options".into(),
        format!("--jobs={workers}"),
        format!("--jobs-frontend={workers}"),
    ]);
    if std::env::var_os("IRQ_CHECK_STATS").is_some() {
        eprintln!("irq-check: worker-limit={workers} hardware-parallelism");
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
