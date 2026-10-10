use crate::policy::{self, CrateData, Policy, ScopeMarks};
use rustc_hir::def_id::{CrateNum, DefId, DefIndex, LOCAL_CRATE};
use rustc_hir::intravisit::{self, Visitor};
use rustc_hir::{BlockCheckMode, ExprKind, HirId, Node, UnsafeSource};
use rustc_middle::mir::{Body, SourceScope};
use rustc_middle::ty::{Instance, InstanceKind, TyCtxt};
use rustc_span::{Span, Symbol};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

pub struct Trust {
    pub checked: HashSet<CrateNum>,
    pub scopes: HashMap<u64, HashMap<u64, ScopeMarks>>,
    pub contexts: Vec<DefId>,
    pub root: bool,
    pub invalid: bool,
}

#[derive(Default)]
struct ErrorPath {
    targets: HashSet<HirId>,
    exits: Vec<Span>,
}

impl<'hir> Visitor<'hir> for ErrorPath {
    fn visit_expr(&mut self, expression: &'hir rustc_hir::Expr<'hir>) {
        let block = match expression.kind {
            ExprKind::Loop(block, ..) | ExprKind::Block(block, _) => Some(block.hir_id),
            _ => None,
        };
        if let Some(block) = block {
            self.targets.insert(expression.hir_id);
            self.targets.insert(block);
        }
        match expression.kind {
            ExprKind::Ret(_) => self.exits.push(expression.span),
            ExprKind::Break(destination, _) | ExprKind::Continue(destination)
                if !destination
                    .target_id
                    .is_ok_and(|target| self.targets.contains(&target)) =>
            {
                self.exits.push(expression.span);
            }
            _ => {}
        }
        intravisit::walk_expr(self, expression);
        if let Some(block) = block {
            self.targets.remove(&expression.hir_id);
            self.targets.remove(&block);
        }
    }
}

impl Trust {
    pub fn collect(tcx: TyCtxt<'_>) -> Self {
        let mut invalid = false;
        let manifest = std::env::var_os("CARGO_MANIFEST_DIR")
            .and_then(|path| std::fs::canonicalize(PathBuf::from(path).join("Cargo.toml")).ok());
        let mut root = true;
        let selected = match std::env::var("IRQ_CHECK_POLICY") {
            Ok(value) => match serde_json::from_str::<Policy>(&value) {
                Ok(policy) => {
                    root = manifest.as_ref().is_some_and(|manifest| {
                        policy.packages.iter().any(|package| {
                            package.id == policy.root && package.manifest == *manifest
                        })
                    });
                    manifest.is_some_and(|manifest| {
                        policy
                            .packages
                            .iter()
                            .any(|package| package.manifest == manifest)
                    })
                }
                Err(error) => {
                    tcx.dcx()
                        .err(format!("cannot read the interrupt check policy: {error}"));
                    invalid = true;
                    false
                }
            },
            Err(_) => true,
        };
        let local_name = tcx.crate_name(LOCAL_CRATE);
        let selected = selected || matches!(local_name.as_str(), "core" | "alloc" | "std");
        let mut blocks = HashMap::<HirId, u8>::new();
        if selected {
            for owner in tcx.hir_crate_items(()).owners() {
                for (local_id, attributes) in tcx.hir_attr_map(owner).map.iter() {
                    let id = HirId {
                        owner,
                        local_id: *local_id,
                    };
                    for attribute in *attributes {
                        let unreachable = attribute
                            .path_matches(&[Symbol::intern("irq"), Symbol::intern("unreachable")]);
                        if unreachable
                            || attribute
                                .path_matches(&[Symbol::intern("irq"), Symbol::intern("trusted")])
                        {
                            let name = if unreachable {
                                "unreachable"
                            } else {
                                "trusted"
                            };
                            let valid_argument = attribute.meta_item_list().is_some_and(|list| {
                                list.len() == 1
                                    && list[0].has_name(Symbol::intern("unsafe"))
                                    && list[0].is_word()
                            });
                            let expression = match tcx.hir_node(id) {
                                Node::Expr(expression) => Some(expression),
                                Node::Stmt(statement) => match statement.kind {
                                    rustc_hir::StmtKind::Expr(expression)
                                    | rustc_hir::StmtKind::Semi(expression) => Some(expression),
                                    _ => None,
                                },
                                _ => None,
                            };
                            let valid_block = expression.is_some_and(|expression| {
                                matches!(expression.kind, ExprKind::Block(block, _)
                                    if block.rules == BlockCheckMode::UnsafeBlock(UnsafeSource::UserProvided))
                            });
                            if !valid_argument || !valid_block {
                                let mut diagnostic = tcx.dcx().struct_span_err(
                                    attribute.span(),
                                    format!(
                                        "irq::{name}(unsafe) requires an explicit unsafe block"
                                    ),
                                );
                                diagnostic.help(
                                format!("use #[cfg_attr(irq_check, irq::{name}(unsafe))] unsafe {{ ... }}"),
                            );
                                diagnostic.note("functions, safe blocks, and marks without (unsafe) are not accepted");
                                diagnostic.emit();
                                invalid = true;
                            } else if unreachable
                                && !tcx
                                    .typeck(id.owner.def_id)
                                    .expr_ty(expression.unwrap())
                                    .is_never()
                            {
                                let mut diagnostic = tcx.dcx().struct_span_err(
                                    attribute.span(),
                                    "irq::unreachable(unsafe) requires a block that cannot return",
                                );
                                diagnostic.help("use this mark only on a fatal error path that ends in panic, abort, or a non-returning call");
                                diagnostic.emit();
                                invalid = true;
                            } else {
                                if unreachable {
                                    let mut error_path = ErrorPath::default();
                                    error_path.visit_expr(expression.unwrap());
                                    if !error_path.exits.is_empty() {
                                        let mut diagnostic = tcx.dcx().struct_span_err(attribute.span(), "irq::unreachable(unsafe) cannot return or jump outside its error block");
                                        for span in error_path.exits {
                                            diagnostic.span_label(
                                                span,
                                                "this exit can continue normal execution",
                                            );
                                        }
                                        diagnostic.help("end the error path in panic, abort, or a non-returning call");
                                        diagnostic.emit();
                                        invalid = true;
                                        continue;
                                    }
                                }
                                let mark = if unreachable { 2 } else { 1 };
                                *blocks.entry(id).or_default() |= mark;
                                *blocks.entry(expression.unwrap().hir_id).or_default() |= mark;
                            }
                        }
                        if attribute
                            .path_matches(&[Symbol::intern("irq"), Symbol::intern("context")])
                            && !matches!(tcx.hir_node(id), Node::Item(item)
                            if matches!(item.kind, rustc_hir::ItemKind::Fn { .. } | rustc_hir::ItemKind::Trait { .. }))
                            && !matches!(tcx.hir_node(id), Node::ImplItem(item)
                            if matches!(item.kind, rustc_hir::ImplItemKind::Fn(..)))
                            && !matches!(tcx.hir_node(id), Node::TraitItem(item)
                            if matches!(item.kind, rustc_hir::TraitItemKind::Fn(..)))
                        {
                            tcx.dcx().span_err(
                            attribute.span(),
                            "irq::context is allowed only on functions, trait methods, and traits",
                        );
                            invalid = true;
                        }
                        if attribute
                            .path_matches(&[Symbol::intern("irq"), Symbol::intern("forbidden")])
                            && !matches!(tcx.hir_node(id), Node::Item(item)
                                if matches!(item.kind, rustc_hir::ItemKind::Fn { .. }))
                            && !matches!(tcx.hir_node(id), Node::ImplItem(item)
                                if matches!(item.kind, rustc_hir::ImplItemKind::Fn(..)))
                            && !matches!(tcx.hir_node(id), Node::TraitItem(item)
                                if matches!(item.kind, rustc_hir::TraitItemKind::Fn(..)))
                            && !matches!(tcx.hir_node(id), Node::ForeignItem(item)
                                if matches!(item.kind, rustc_hir::ForeignItemKind::Fn(..)))
                        {
                            tcx.dcx().span_err(
                                attribute.span(),
                                "irq::forbidden is allowed only on functions and trait methods",
                            );
                            invalid = true;
                        }
                    }
                }
            }
        }
        let policy_hash =
            std::env::var("IRQ_CHECK_POLICY_HASH").unwrap_or_else(|_| policy::INTERFACE.into());
        let mut checked = HashSet::new();
        let mut scopes = HashMap::new();
        if selected {
            checked.insert(LOCAL_CRATE);
        }
        let mut local_scopes = HashMap::new();
        if !blocks.is_empty() && !invalid {
            for owner in tcx.hir_body_owners() {
                if !matches!(
                    tcx.def_kind(owner),
                    rustc_hir::def::DefKind::Fn
                        | rustc_hir::def::DefKind::AssocFn
                        | rustc_hir::def::DefKind::Closure
                ) || !tcx.is_mir_available(owner)
                {
                    continue;
                }
                let body = tcx.optimized_mir(owner);
                let mut allowed = ScopeMarks::default();
                for (scope, _) in body.source_scopes.iter_enumerated() {
                    let Some(mut id) = scope.lint_root(&body.source_scopes) else {
                        continue;
                    };
                    let mut marks = 0;
                    loop {
                        if let Some(mark) = blocks.get(&id) {
                            marks |= mark;
                        }
                        if matches!(tcx.hir_node(id), Node::Expr(expression) if matches!(expression.kind, ExprKind::Closure(..)))
                        {
                            break;
                        }
                        let parent = tcx.parent_hir_id(id);
                        if parent == id || parent.owner != id.owner {
                            break;
                        }
                        id = parent;
                    }
                    if marks & 1 != 0 {
                        allowed.trusted.push(scope.as_usize());
                    }
                    if marks & 2 != 0 {
                        allowed.unreachable.push(scope.as_usize());
                    }
                }
                if !allowed.trusted.is_empty() || !allowed.unreachable.is_empty() {
                    local_scopes.insert(
                        tcx.def_path_hash(owner.to_def_id()).local_hash().as_u64(),
                        allowed,
                    );
                }
            }
        }
        scopes.insert(
            tcx.stable_crate_id(LOCAL_CRATE).as_u64(),
            local_scopes.clone(),
        );
        let contexts: Vec<_> = tcx
            .hir_body_owners()
            .filter(|owner| crate::context_method(tcx, owner.to_def_id()))
            .map(|owner| owner.local_def_index.as_u32())
            .collect();
        if let Some(data_file) = std::env::var_os("IRQ_CHECK_DATA_FILE") {
            let data = CrateData {
                interface: policy::INTERFACE.into(),
                compiler: crate::COMMIT.into(),
                policy: policy_hash.clone(),
                crate_id: tcx.stable_crate_id(LOCAL_CRATE).as_u64(),
                checked: selected,
                metadata: std::env::var_os("IRQ_CHECK_METADATA_FILE")
                    .map(PathBuf::from)
                    .unwrap_or_default(),
                metadata_hash: 0,
                scopes: local_scopes,
                contexts,
            };
            let result = serde_json::to_vec(&data)
                .map_err(|error| error.to_string())
                .and_then(|data| {
                    std::fs::write(data_file, data).map_err(|error| error.to_string())
                });
            if let Err(error) = result {
                tcx.dcx()
                    .err(format!("cannot save interrupt check metadata: {error}"));
                invalid = true;
            }
        }
        if !root {
            return Self {
                checked,
                scopes,
                contexts: Vec::new(),
                root,
                invalid,
            };
        }
        let mut directories: Vec<PathBuf> = std::env::var("IRQ_CHECK_CACHE_DIRS")
            .ok()
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or_default();
        for crate_num in tcx.crates(()).iter().copied() {
            for path in tcx.used_crate_source(crate_num).paths() {
                if let Some(parent) = path.parent() {
                    directories.push(parent.to_path_buf());
                }
            }
        }
        directories.sort();
        directories.dedup();
        let mut contexts = Vec::new();
        let mut loaded = HashSet::new();
        for directory in directories {
            let Ok(entries) = std::fs::read_dir(directory) else {
                continue;
            };
            for entry in entries.flatten() {
                if entry.path().extension().and_then(|value| value.to_str()) != Some("irq-data") {
                    continue;
                }
                let Some(data) = std::fs::read(entry.path())
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<CrateData>(&bytes).ok())
                else {
                    continue;
                };
                if data.interface != policy::INTERFACE
                    || data.compiler != crate::COMMIT
                    || data.policy != policy_hash
                {
                    continue;
                }
                let Some(crate_num) =
                    tcx.crates(()).iter().copied().find(|crate_num| {
                        tcx.stable_crate_id(*crate_num).as_u64() == data.crate_id
                    })
                else {
                    continue;
                };
                let Some(metadata) = std::fs::canonicalize(&data.metadata).ok() else {
                    continue;
                };
                if !tcx
                    .used_crate_source(crate_num)
                    .paths()
                    .any(|path| std::fs::canonicalize(path).is_ok_and(|path| path == metadata))
                {
                    continue;
                }
                if !std::fs::read(&data.metadata)
                    .is_ok_and(|bytes| policy::hash(bytes) == data.metadata_hash)
                {
                    tcx.dcx().err(format!(
                        "interrupt check metadata does not match {}",
                        data.metadata.display()
                    ));
                    invalid = true;
                    continue;
                }
                if data.checked {
                    checked.insert(crate_num);
                    contexts.extend(data.contexts.iter().map(|index| DefId {
                        krate: crate_num,
                        index: DefIndex::from_u32(*index),
                    }));
                }
                loaded.insert(crate_num);
                scopes.insert(data.crate_id, data.scopes);
            }
        }
        for crate_num in tcx.crates(()).iter().copied() {
            let source = tcx.used_crate_source(crate_num);
            let sysroot = source
                .paths()
                .any(|path| path.starts_with(tcx.sess.opts.sysroot.path().join("lib/rustlib")));
            let children = tcx.module_children(DefId {
                krate: crate_num,
                index: DefIndex::from_u32(0),
            });
            let macros_only = !children.is_empty()
                && children.iter().all(|child| {
                    matches!(
                        child.res,
                        rustc_hir::def::Res::Def(rustc_hir::def::DefKind::Macro(..), _)
                    )
                });
            if matches!(tcx.crate_name(crate_num).as_str(), "core" | "alloc" | "std") {
                checked.insert(crate_num);
            } else if std::env::var_os("IRQ_CHECK_POLICY").is_some()
                && root
                && !loaded.contains(&crate_num)
                && !macros_only
                && !sysroot
            {
                tcx.dcx().err(format!("interrupt check metadata is missing for {}; rebuild this dependency with irq-check; crate identity {}; compiler sources: {:?}", tcx.crate_name(crate_num), tcx.stable_crate_id(crate_num).as_u64(), source.paths().collect::<Vec<_>>()));
                invalid = true;
            }
        }
        Self {
            checked,
            scopes,
            contexts,
            root,
            invalid,
        }
    }

    pub fn allows(
        &self,
        tcx: TyCtxt<'_>,
        instance: Instance<'_>,
        body: &Body<'_>,
        scope: SourceScope,
        unreachable: bool,
    ) -> bool {
        if !matches!(instance.def, InstanceKind::Item(_))
            || body.source_scopes[scope].inlined.is_some()
        {
            return false;
        }
        self.scopes
            .get(&tcx.stable_crate_id(instance.def_id().krate).as_u64())
            .and_then(|bodies| {
                bodies.get(&tcx.def_path_hash(instance.def_id()).local_hash().as_u64())
            })
            .is_some_and(|scopes| {
                if unreachable {
                    &scopes.unreachable
                } else {
                    &scopes.trusted
                }
                .contains(&scope.as_usize())
            })
    }

    pub fn checks(&self, def: DefId) -> bool {
        self.checked.contains(&def.krate)
    }
}
