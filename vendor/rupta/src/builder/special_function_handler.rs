// Copyright (c) 2024 <Wei Li>.
//
// This source code is licensed under the GNU license found in the
// LICENSE file in the root directory of this source tree.

//! Provides special handling for a set of functions.

use std::rc::Rc;
use rustc_hir::def_id::DefId;
use rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags;
use rustc_middle::mir;
use rustc_middle::ty::GenericArgsRef;
use crate::builder::fpag_builder::FuncPAGBuilder;
use crate::mir::analysis_context::AnalysisContext;
use crate::mir::path::Path;

pub fn is_specially_handled_function(acx: &mut AnalysisContext, def_id: DefId) -> bool {
    if acx.tcx.is_mir_available(def_id) || crate::util::is_trait_method(acx.tcx, def_id) {
        return false;
    }
    acx.tcx.codegen_fn_attrs(def_id).flags.intersects(
        CodegenFnAttrFlags::ALLOCATOR | CodegenFnAttrFlags::ALLOCATOR_ZEROED
            | CodegenFnAttrFlags::REALLOCATOR | CodegenFnAttrFlags::DEALLOCATOR,
    ) || is_specially_handled_precision_critical_function(acx, def_id)
}

pub fn is_specially_handled_precision_critical_function(acx: &mut AnalysisContext, def_id: DefId) -> bool {
    !acx.tcx.is_mir_available(def_id)
        && acx.tcx.intrinsic(def_id).is_some_and(|intrinsic|
            matches!(intrinsic.name.as_str(), "transmute" | "offset" | "arith_offset"))
}

pub fn handled_as_special_function_call<'tcx>(
    fpb: &mut FuncPAGBuilder<'_, 'tcx, '_>,
    callee_def_id: &DefId,
    gen_args: &GenericArgsRef<'tcx>,
    args: &Vec<Rc<Path>>,
    destination: &Rc<Path>,
    location: mir::Location,
) -> bool {
    if !is_specially_handled_function(fpb.acx, *callee_def_id) {
        return false;
    }
    let tcx = fpb.acx.tcx;
    let flags = tcx.codegen_fn_attrs(*callee_def_id).flags;
    if flags.intersects(CodegenFnAttrFlags::ALLOCATOR | CodegenFnAttrFlags::ALLOCATOR_ZEROED) {
        let heap_object = Path::new_heap_obj(fpb.fpag.func_id, location);
        fpb.acx.set_path_rustc_type(heap_object.clone(), tcx.types.u8);
        fpb.add_addr_edge(heap_object, destination.clone());
        return true;
    }
    if flags.contains(CodegenFnAttrFlags::REALLOCATOR) {
        if let Some(source) = args.first() {
            fpb.add_direct_edge(source.clone(), destination.clone());
            return true;
        }
        fpb.acx.coverage_gaps.insert((fpb.func_id, Some(location),
            "allocator model has no source pointer".into()));
        return false;
    }
    if flags.contains(CodegenFnAttrFlags::DEALLOCATOR) {
        return true;
    }
    match tcx.intrinsic(*callee_def_id).map(|intrinsic| intrinsic.name) {
        Some(name) if name.as_str() == "transmute" => {
            if let (Some(source), Some(source_ty), Some(target_ty)) = (
                args.first(),
                gen_args.types().next(),
                fpb.acx.get_path_rustc_type(destination),
            ) {
                fpb.copy_and_transmute(source.clone(), source_ty, destination.clone(), target_ty);
                true
            } else {
                fpb.acx.coverage_gaps.insert((fpb.func_id, Some(location),
                    "transmute model has an unsupported signature".into()));
                false
            }
        }
        Some(name) if matches!(name.as_str(), "offset" | "arith_offset") => {
            if let Some(source) = args.first() {
                fpb.add_offset_edge(source.clone(), destination.clone());
                true
            } else {
                fpb.acx.coverage_gaps.insert((fpb.func_id, Some(location),
                    "offset model has no source pointer".into()));
                false
            }
        }
        _ => false,
    }
}
