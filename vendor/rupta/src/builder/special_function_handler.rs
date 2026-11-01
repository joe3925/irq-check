// Copyright (c) 2024 <Wei Li>.
//
// This source code is licensed under the GNU license found in the
// LICENSE file in the root directory of this source tree.

//! Provides special handling for a set of functions.

use std::rc::Rc;
use rustc_hir::def_id::DefId;
use rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags;
use rustc_middle::mir;
use rustc_middle::ty::{GenericArgsRef, TyKind};
use crate::builder::fpag_builder::FuncPAGBuilder;
use crate::mir::analysis_context::AnalysisContext;
use crate::mir::path::Path;

pub(crate) enum LibraryModel {
    SliceIterator,
    SliceIndex,
    AllocatorResize,
    ArcFromRaw,
}

pub(crate) fn library_model<'tcx>(acx: &AnalysisContext<'tcx, '_>, def_id: DefId,
    gen_args: GenericArgsRef<'tcx>) -> Option<LibraryModel> {
    if acx.tcx.def_kind(def_id) != rustc_hir::def::DefKind::AssocFn
        || !matches!(acx.tcx.item_name(def_id).as_str(),
            "next" | "next_back" | "get" | "get_mut" | "get_unchecked" | "get_unchecked_mut"
                | "index" | "index_mut" | "grow" | "grow_zeroed" | "shrink" | "from_raw" | "from_raw_in")
    {
        return None;
    }
    let (def_id, _) = crate::builder::call_graph_builder::try_to_devirtualize(acx.tcx, def_id, gen_args)
        .unwrap_or((def_id, gen_args));
    let implementation = acx.tcx.impl_of_assoc(def_id)?;
    let name = acx.tcx.item_name(def_id);
    let crate_name = acx.tcx.crate_name(def_id.krate);
    let self_ty = acx.tcx.type_of(implementation).skip_binder();
    if crate_name.as_str() == "alloc" && matches!(name.as_str(), "from_raw" | "from_raw_in")
        && let TyKind::Adt(definition, _) = self_ty.kind()
        && acx.tcx.def_path_str(definition.did()) == "alloc::sync::Arc"
        && gen_args.types().next().is_some_and(|ty| acx.tcx.layout_of(
            rustc_middle::ty::TypingEnv::fully_monomorphized().as_query_input(ty)).is_ok_and(|layout| layout.is_sized()))
        && gen_args.types().nth(1).is_none_or(|ty| matches!(ty.kind(), TyKind::Adt(definition, _)
            if acx.tcx.def_path_str(definition.did()) == "alloc::alloc::Global"))
    {
        return Some(LibraryModel::ArcFromRaw);
    }
    if crate_name.as_str() == "core" {
        if matches!(name.as_str(), "next" | "next_back")
            && let TyKind::Adt(definition, _) = self_ty.kind()
            && matches!(acx.tcx.def_path_str(definition.did()).as_str(),
                "core::slice::Iter" | "core::slice::IterMut"
                    | "core::slice::iter::Iter" | "core::slice::iter::IterMut")
        {
            return Some(LibraryModel::SliceIterator);
        }
        if matches!(name.as_str(), "get" | "get_mut" | "get_unchecked" | "get_unchecked_mut" | "index" | "index_mut")
            && let Some(implementation) = acx.tcx.trait_impl_of_assoc(def_id)
            && acx.tcx.is_diagnostic_item(rustc_span::Symbol::intern("SliceIndex"),
                acx.tcx.impl_trait_ref(implementation).skip_binder().def_id)
            && acx.tcx.impl_trait_ref(implementation).skip_binder().args.types().nth(1)
                .is_some_and(|ty| matches!(ty.kind(), TyKind::Slice(_)))
        {
            return Some(LibraryModel::SliceIndex);
        }
    } else if crate_name.as_str() == "alloc"
        && matches!(name.as_str(), "grow" | "grow_zeroed" | "shrink")
        && let TyKind::Adt(definition, _) = self_ty.kind()
        && acx.tcx.def_path_str(definition.did()) == "alloc::alloc::Global"
    {
        return Some(LibraryModel::AllocatorResize);
    }
    None
}

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
    model_body: bool,
) -> bool {
    if !model_body && !is_specially_handled_function(fpb.acx, *callee_def_id) {
        return false;
    }
    let tcx = fpb.acx.tcx;
    if model_body && let Some(model) = library_model(fpb.acx, *callee_def_id, *gen_args) {
        let modeled = (|| {
            match model {
            LibraryModel::ArcFromRaw => {
                let source = args.first()?;
                let source_ty = fpb.acx.get_path_rustc_type(source)?;
                let element = source_ty.builtin_deref(true)?;
                let destination_ty = fpb.acx.get_path_rustc_type(destination)?;
                let fields = fpb.acx.get_pointer_projections(destination_ty).clone();
                let [(projection, pointer_ty)] = fields.as_slice() else { return None; };
                let inner_ty = pointer_ty.builtin_deref(true)?;
                let TyKind::Adt(definition, substitutions) = inner_ty.kind() else { return None; };
                if tcx.def_path_str(definition.did()) != "alloc::sync::ArcInner" { return None; }
                let (field, data) = definition.non_enum_variant().fields.iter().enumerate()
                    .find(|(_, field)| field.name.as_str() == "data")?;
                if crate::util::type_util::field_ty(tcx, data, substitutions) != element { return None; }
                let destination = Path::append_projection(destination, projection);
                fpb.acx.set_path_rustc_type(destination.clone(), *pointer_ty);
                fpb.acx.container_casts.insert((source.clone(), destination.clone()), field);
                fpb.add_cast_edge(source.clone(), destination);
            }
            LibraryModel::SliceIterator => {
            let receiver = args.first()?;
            let iterator_ty = fpb.acx.get_path_rustc_type(receiver)?.builtin_deref(true)?;
            let TyKind::Adt(definition, substitutions) = iterator_ty.kind() else { return None; };
            let (index, field) = definition.non_enum_variant().fields.iter().enumerate()
                .find(|(_, field)| field.name.as_str() == "ptr")?;
            let field_ty = crate::util::type_util::field_ty(tcx, field, substitutions);
            let fields = fpb.acx.get_pointer_projections(field_ty).clone();
            let [(projection, pointer_ty)] = fields.as_slice() else { return None; };
            let destination_ty = fpb.acx.get_path_rustc_type(destination)?;
            let TyKind::Adt(option, option_args) = destination_ty.kind() else { return None; };
            if !tcx.is_diagnostic_item(rustc_span::sym::Option, option.did()) { return None; }
            let returned_ty = option_args.types().next()?;
            if !returned_ty.is_ref() || returned_ty.builtin_deref(true) != pointer_ty.builtin_deref(true) {
                return None;
            }
            let iterator = Path::new_deref(receiver.clone());
            fpb.acx.set_path_rustc_type(iterator.clone(), iterator_ty);
            let field = Path::new_field(iterator, index);
            fpb.acx.set_path_rustc_type(field.clone(), field_ty);
            let pointer = Path::append_projection(&field, projection);
            fpb.acx.set_path_rustc_type(pointer.clone(), *pointer_ty);
            let returned = Path::new_field(Path::new_downcast(destination.clone(), 1), 0);
            fpb.acx.set_path_rustc_type(returned.clone(), returned_ty);
            fpb.copy_and_transmute(pointer, *pointer_ty, returned, returned_ty);
            }
            LibraryModel::SliceIndex => {
                let source = args.get(1)?;
                let source_ty = fpb.acx.get_path_rustc_type(source)?;
                let TyKind::Slice(element) = source_ty.builtin_deref(true)?.kind() else { return None; };
                let destination_ty = fpb.acx.get_path_rustc_type(destination)?;
                let (returned, returned_ty) = if let TyKind::Adt(option, substitutions) = destination_ty.kind()
                    && tcx.is_diagnostic_item(rustc_span::sym::Option, option.did())
                {
                    (Path::new_field(Path::new_downcast(destination.clone(), 1), 0), substitutions.types().next()?)
                } else {
                    (destination.clone(), destination_ty)
                };
                let returned_element = returned_ty.builtin_deref(true)?;
                if returned_element != *element && returned_element != source_ty.builtin_deref(true)? {
                    return None;
                }
                fpb.acx.set_path_rustc_type(returned.clone(), returned_ty);
                fpb.copy_and_transmute(source.clone(), source_ty, returned, returned_ty);
            }
            LibraryModel::AllocatorResize => {
                let source = args.get(1)?;
                let source_ty = fpb.acx.get_path_rustc_type(source)?;
                let sources = fpb.acx.get_pointer_projections(source_ty).clone();
                let [(source_projection, source_pointer_ty)] = sources.as_slice() else { return None; };
                if source_pointer_ty.builtin_deref(true) != Some(tcx.types.u8) { return None; }
                let destination_ty = fpb.acx.get_path_rustc_type(destination)?;
                let destinations = fpb.acx.get_pointer_projections(destination_ty).clone();
                let [(destination_projection, destination_pointer_ty)] = destinations.as_slice() else { return None; };
                let TyKind::Slice(element) = destination_pointer_ty.builtin_deref(true)?.kind() else { return None; };
                if *element != tcx.types.u8 { return None; }
                let source = Path::append_projection(source, source_projection);
                let destination = Path::append_projection(destination, destination_projection);
                fpb.acx.set_path_rustc_type(source.clone(), *source_pointer_ty);
                fpb.acx.set_path_rustc_type(destination.clone(), *destination_pointer_ty);
                fpb.copy_and_transmute(source, *source_pointer_ty, destination.clone(), *destination_pointer_ty);
                let heap_object = Path::new_heap_obj(fpb.fpag.func_id, location);
                fpb.acx.set_path_rustc_type(heap_object.clone(), tcx.types.u8);
                fpb.add_addr_edge(heap_object, destination);
            }
            }
            Some(())
        })().is_some();
        if !modeled {
            fpb.acx.coverage_gaps.insert((fpb.func_id, Some(location),
                "standard library pointer model has an unsupported representation".into()));
        }
        return modeled;
    }
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
                if name.as_str() == "arith_offset" {
                    fpb.acx.wrapping_offsets.insert((source.clone(), destination.clone()));
                }
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
