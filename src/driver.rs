#![feature(rustc_private)]

extern crate rustc_abi;
extern crate rustc_driver;
extern crate rustc_errors;
extern crate rustc_hir;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_span;

mod analysis;
mod policy;
mod trust;

use rustc_driver::{Callbacks, Compilation};
use rustc_hir::def_id::DefId;
use rustc_interface::interface::Compiler;
use rustc_middle::mir::mono::MonoItem;
use rustc_middle::ty::{Instance, InstanceKind, TyCtxt};
use rustc_span::{Span, Symbol};
use std::collections::{HashMap, HashSet, VecDeque};
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
    trusted: bool,
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
    if tcx
        .opt_associated_item(instance.def_id())
        .is_some_and(|item| {
            item.trait_item_def_id()
                .is_some_and(|method| marked(tcx, method, "forbidden"))
        })
    {
        return Some(format!("{path} implements a method marked irq::forbidden"));
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
        let analysis = analysis::Analysis::build(tcx, &trust, &instances, &roots);
        if analysis.incomplete {
            let mut diagnostic = tcx.dcx().struct_span_err(
                roots
                    .first()
                    .map_or(tcx.def_span(rustc_hir::def_id::CRATE_DEF_ID), |root| {
                        tcx.def_span(root.def_id())
                    }),
                "interrupt call analysis did not reach a complete result",
            );
            diagnostic.note("the call-target or memory analysis limit was reached; no partial result is accepted");
            diagnostic.emit();
            return Compilation::Stop;
        }
        let mut reported = HashSet::new();
        let mut errors = 0;
        roots.extend(analysis.graph.keys().copied().filter(|instance| {
            trust.checks(instance.def_id())
                && context_method(tcx, instance.def_id())
                && !matches!(instance.def, InstanceKind::Virtual(..))
        }));
        roots.sort_by_key(ToString::to_string);
        roots.dedup();
        if roots.is_empty() && required {
            tcx.dcx()
                .err("the required crate has no concrete irq::context functions");
            return Compilation::Stop;
        }
        for root in roots {
            let mut parents: HashMap<Instance<'tcx>, (Instance<'tcx>, Edge<'tcx>)> = HashMap::new();
            let mut visited = HashSet::from([root]);
            let mut queue = VecDeque::from([root]);
            while let Some(current) = queue.pop_front() {
                let mut failures = Vec::new();
                if let Some(reason) = forbidden(tcx, current) {
                    failures.push((
                        parents
                            .get(&current)
                            .map_or(tcx.def_span(current.def_id()), |(_, edge)| edge.span),
                        reason,
                        false,
                        "call",
                    ));
                }
                for edge in analysis.graph.get(&current).into_iter().flatten() {
                    if let Some(target) = edge.target {
                        if visited.insert(target) {
                            parents.insert(target, (current, edge.clone()));
                            queue.push_back(target);
                        }
                    } else if !edge.trusted {
                        failures.push((edge.span, edge.detail.clone(), true, edge.kind));
                    }
                }
                for (span, reason, unknown, operation) in failures {
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
                    if primary.is_dummy() || !current.def_id().is_local() {
                        for (parent, _, edge) in &hops {
                            if parent.def_id().is_local() {
                                primary = edge.span.source_callsite();
                            }
                        }
                    }
                    let mut chain = vec![tcx.def_path_str(root.def_id())];
                    let mut library_steps = 0;
                    for (_, target, _) in hops {
                        let name = tcx.crate_name(target.def_id().krate);
                        if matches!(name.as_str(), "core" | "alloc" | "std") && target != current {
                            library_steps += 1;
                        } else {
                            if library_steps != 0 {
                                chain.push(format!("{library_steps} library or drop steps"));
                                library_steps = 0;
                            }
                            chain.push(tcx.def_path_str(target.def_id()));
                        }
                    }
                    let message = if unknown {
                        "interrupt call path cannot be fully checked".to_string()
                    } else {
                        "forbidden operation is reachable from interrupt context".to_string()
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
                        tcx.def_span(root.def_id()).source_callsite(),
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
    if !version.status.success() || !String::from_utf8_lossy(&version.stdout).contains(COMMIT) {
        eprintln!(
            "error: irq-check requires {TOOLCHAIN} ({COMMIT})\nhelp: select that toolchain and rebuild dependencies"
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
