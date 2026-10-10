// Copyright (c) 2024 <Wei Li>.
//
// This source code is licensed under the GNU license found in the
// LICENSE file in the root directory of this source tree.

use rustc_hir::def_id::DefId;
use rustc_middle::mir;
use rustc_middle::ty::{GenericArgsRef, TyCtxt, TyKind};
use std::rc::Rc;

use crate::mir::function::GenericArgE;
use crate::mir::analysis_context::AnalysisContext;
use crate::mir::path::{Path, PathEnum, PathSelector};

pub mod bit_vec;
pub mod call_graph_stat;
pub mod chunked_queue;
pub mod dot;
pub mod index_tree;
pub mod options;
pub mod pta_statistics;
pub mod results_dumper;
pub mod type_util;
pub mod unsafe_statistics;


/// Returns true if the function identified by `def_id` is defined as part of a trait.
#[inline]
pub fn is_trait_method(tcx: TyCtxt<'_>, def_id: DefId) -> bool {
    if tcx.trait_of_assoc(def_id).is_some() {
        true
    } else {
        false
    }
}

/// Returns true if the function identified by `def_id` is defined in the Rust Standard Library.
pub fn is_std_lib_func(tcx: TyCtxt<'_>, def_id: DefId) -> bool {
    let crate_name = tcx.crate_name(def_id.krate);
    match crate_name.as_str() {
        "alloc" | "core" | "std" => true,
        _ => false,
    }
}

/// Returns true if the function has an explicit `self` (either `self` or `&(mut) self`) as its first 
/// parameter, allowing method calls.
#[inline]
pub fn has_self_parameter(tcx: TyCtxt<'_>, def_id: DefId) -> bool {
    if !tcx.is_mir_available(def_id) {
        return false;
    }
    if let Some(associated_item) = tcx.opt_associated_item(def_id) {
        matches!(associated_item.kind, rustc_middle::ty::AssocKind::Fn { has_self: true, .. })
    } else {
        false
    }
}

/// Returns true if the function has an explicit `&(mut) self` as its first parameter, allowing method calls.
pub fn has_self_ref_parameter(tcx: TyCtxt<'_>, def_id: DefId) -> bool {
    if has_self_parameter(tcx, def_id) {
        let mir = tcx.optimized_mir(def_id);
        if let Some(decl) = mir.local_decls.get(mir::Local::from(1usize)) {
            decl.ty.is_ref()
        } else {
            false
        }
    } else {
        false
    }
}

/// Returns true if the call to (`callee_def_id`, `callee_substs`) is a dynamic call.
#[inline]
pub fn is_dynamic_call<'tcx>(
    tcx: TyCtxt<'tcx>,
    callee_def_id: DefId,
    callee_substs: GenericArgsRef<'tcx>,
) -> bool {
    if !is_trait_method(tcx, callee_def_id) {
        return false;
    }
    let arg0_ty = callee_substs
        .types()
        .next()
        .expect("Expect `Self` substition in trait method invocation");
    if matches!(arg0_ty.kind(), TyKind::Dynamic(..)) {
        true
    } else {
        false
    }
}


#[inline]
pub fn customize_generic_args<'tcx>(_tcx: TyCtxt<'tcx>, generic_args: GenericArgsRef<'tcx>) -> Vec<GenericArgE<'tcx>> {
    generic_args.iter().map(|arg| GenericArgE::from(&arg)).collect()
}

/// Returns an `offset_path` equivalent to the `qualified_path`.
pub fn qualified_path_to_offset_path(acx: &mut AnalysisContext, path: Rc<Path>) -> Rc<Path> {
    if let PathEnum::QualifiedPath { base, projection } = &path.value {
        let base_ty = acx.get_path_rustc_type(base).unwrap();
        match projection[0] {
            PathSelector::Deref => {
                if projection.len() > 1 {
                    let deref_path = Path::new_deref(base.clone());
                    let deref_ty = type_util::get_dereferenced_type(base_ty);
                    let offset = acx.get_field_byte_offset(deref_ty, &projection[1..].to_vec());
                    Path::new_offset(deref_path, offset)
                } else {
                    path
                }
            }
            _ => {
                let offset = acx.get_field_byte_offset(base_ty, &projection);
                Path::new_offset(base.clone(), offset)
            }
        }
    } else {
        path
    }
}
