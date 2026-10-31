// Copyright (c) 2024 <Wei Li>.
//
// This source code is licensed under the GNU license found in the
// LICENSE file in the root directory of this source tree.

use std::collections::HashSet;
use std::rc::Rc;

use super::pag::PAGEdgeEnum;
use crate::mir::call_site::CallSite;
use crate::mir::function::FuncId;
use crate::mir::path::Path;
use crate::util::chunked_queue::{self, ChunkedQueue};

/// An edge consists of the source path, the destination path and the PAG edge type.
pub type InternalEdge = (Rc<Path>, Rc<Path>, PAGEdgeEnum);

/// A function's pointer assignment graph, part of the whole program's PAG.
pub struct FuncPAG {
    pub(crate) func_id: FuncId,
    pub(crate) internal_edges: ChunkedQueue<InternalEdge>,
    pub(crate) static_variables_involved: HashSet<Rc<Path>>,

    // Calls that can be statically resolved, including the Fn* trait calls that can 
    // be directly resolved. 
    pub(crate) static_dispatch_callsites: Vec<(Rc<CallSite>, FuncId)>,
    // Special calls like alloc().
    pub(crate) special_callsites: Vec<(Rc<CallSite>, FuncId)>,
    // Pairs of dynamic type receivers and their corresponding callsites.
    pub(crate) dynamic_dispatch_callsites: Vec<(Rc<Path>, Rc<CallSite>)>,
    // Pairs of the first arguments of dynamic `Fn::call`, `FnMut::call_mut`, 
    // `FnOnce::call_once` calls and the callsites.
    pub(crate) dynamic_fntrait_callsites: Vec<(Rc<Path>, Rc<CallSite>)>,
    // Pairs of function pointers and their callsites, including the fnptr
    // callsites that are speciallized from a Fn* trait callsite.
    pub(crate) fnptr_callsites: Vec<(Rc<Path>, Rc<CallSite>)>,
}

impl FuncPAG {
    pub fn new(func_id: FuncId) -> Self {
        FuncPAG {
            func_id,
            internal_edges: ChunkedQueue::new(),
            static_variables_involved: HashSet::new(),
            static_dispatch_callsites: Vec::new(),
            special_callsites: Vec::new(),
            dynamic_fntrait_callsites: Vec::new(),
            dynamic_dispatch_callsites: Vec::new(),
            fnptr_callsites: Vec::new(),
        }
    }

    pub fn add_internal_edge(&mut self, src: Rc<Path>, dst: Rc<Path>, kind: PAGEdgeEnum) {
        self.internal_edges.push((src, dst, kind));
    }

    pub fn add_constant_allocation<'tcx>(
        &mut self,
        acx: &mut crate::mir::analysis_context::AnalysisContext<'tcx, '_>,
        value: Rc<Path>,
        ty: rustc_middle::ty::Ty<'tcx>,
        allocation: rustc_middle::mir::interpret::ConstAllocation<'tcx>,
        offset: rustc_abi::Size,
    ) {
        use rustc_middle::mir::interpret::{AllocError, AllocRange, GlobalAlloc, Scalar};
        use rustc_middle::ty::{Binder, Ty, TyKind, TypingEnv};
        use crate::util::type_util;

        let tcx = acx.tcx;
        acx.set_path_rustc_type(value.clone(), ty);
        let environment = TypingEnv::fully_monomorphized();
        let layout_context = rustc_middle::ty::layout::LayoutCx::new(tcx, environment);
        let mut pending = vec![(value, ty, offset, 1u64, rustc_abi::Size::ZERO)];
        let mut incomplete = false;
        while let Some((field, field_ty, field_offset, count, stride)) = pending.pop() {
            if !crate::limits::checkpoint() { return; }
            if count > 1 {
                pending.push((field.clone(), field_ty, field_offset + stride, count - 1, stride));
            }
            if !field_ty.is_any_ptr() && !type_util::is_integer_carrier(tcx, field_ty) {
                if acx.get_value_projections(field_ty).is_empty() {
                    continue;
                }
                if let Ok(layout) = tcx.layout_of(environment.as_query_input(field_ty)) {
                    match field_ty.kind() {
                        TyKind::Array(element, length) => {
                            let length = type_util::get_array_length(tcx, environment, length);
                            if length != 0 {
                                let stride = rustc_abi::Size::from_bytes(type_util::size_of(tcx, environment, *element));
                                pending.push((Path::new_index(field), *element, field_offset, length as u64, stride));
                            }
                            continue;
                        }
                        TyKind::Adt(definition, args) => {
                            for (variant_index, variant) in definition.variants().iter_enumerated() {
                                let (base, layout) = if definition.is_enum() {
                                    (Path::new_downcast(field.clone(), variant_index.as_usize()),
                                        layout.for_variant(&layout_context, variant_index))
                                } else {
                                    (field.clone(), layout)
                                };
                                for (index, definition_field) in variant.fields.iter_enumerated() {
                                    let path = if definition.is_union() {
                                        Path::new_union_field(base.clone(), index.as_usize())
                                    } else {
                                        Path::new_field(base.clone(), index.as_usize())
                                    };
                                    pending.push((path, type_util::field_ty(tcx, definition_field, args),
                                        field_offset + layout.fields.offset(index.as_usize()),
                                        1, rustc_abi::Size::ZERO));
                                }
                            }
                            continue;
                        }
                        TyKind::Tuple(types) => {
                            for (index, ty) in types.iter().enumerate() {
                                pending.push((Path::new_field(field.clone(), index), ty,
                                    field_offset + layout.fields.offset(index), 1, rustc_abi::Size::ZERO));
                            }
                            continue;
                        }
                        _ => {}
                    }
                }
                acx.value_unknown_paths.insert(field);
                incomplete = true;
                continue;
            }
            acx.set_path_rustc_type(field.clone(), field_ty);
            let scalar = field_offset.bytes().checked_add(tcx.data_layout.pointer_size().bytes()).and_then(|end| {
                let range = AllocRange {
                    start: field_offset,
                    size: tcx.data_layout.pointer_size(),
                };
                (end <= allocation.inner().len() as u64)
                    .then(|| allocation.inner().read_scalar(&tcx, range, true))
            });
            match scalar {
                Some(Err(AllocError::InvalidUninitBytes(_))) => continue,
                Some(Ok(Scalar::Int(bits))) if bits.to_bits(bits.size()) == 0 => continue,
                Some(Ok(Scalar::Ptr(pointer, _))) if pointer.prov_and_relative_offset().1.bytes() == 0 => {
                    match tcx.try_get_global_alloc(pointer.provenance.alloc_id()) {
                        Some(GlobalAlloc::Function { instance }) => {
                            let function = Path::new_function(acx.get_instance_id(instance));
                            acx.set_path_rustc_type(function.clone(), Ty::new_fn_def(
                                tcx, instance.def_id(), Binder::dummy(instance.args),
                            ));
                            self.add_internal_edge(function, field, PAGEdgeEnum::AddrPAGEdge);
                            continue;
                        }
                        Some(GlobalAlloc::Static(def_id))
                            if tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(field_ty))
                                .is_ok_and(|layout| layout.size == tcx.data_layout.pointer_size()) =>
                        {
                            let target = Path::new_static_variable(def_id);
                            acx.set_path_rustc_type(target.clone(), tcx.type_of(def_id).skip_binder());
                            self.add_static_variables_involved(target.clone());
                            self.add_internal_edge(target, field, PAGEdgeEnum::AddrPAGEdge);
                            continue;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            acx.value_unknown_paths.insert(field);
            incomplete |= field_ty.is_any_ptr();
        }
        if incomplete {
            acx.coverage_gaps.insert((self.func_id, None,
                "constant allocation contains unmodeled pointer storage".into()));
        }
    }

    pub fn internal_edges_iter(&self) -> chunked_queue::Iter<'_, InternalEdge> {
        self.internal_edges.iter()
    }

    pub fn add_static_variables_involved(&mut self, static_variable: Rc<Path>) {
        self.static_variables_involved.insert(static_variable);
    }

    pub fn add_static_dispatch_callsite(&mut self, callsite: Rc<CallSite>, callee: FuncId) {
        self.static_dispatch_callsites.push((callsite, callee));
    }

    pub fn add_dynamic_fntrait_callsite(
        &mut self,
        dynamic_fn_obj: Rc<Path>,
        std_ops_callsite: Rc<CallSite>,
    ) {
        self.dynamic_fntrait_callsites.push((dynamic_fn_obj, std_ops_callsite));
    }

    pub fn add_dynamic_dispatch_callsite(
        &mut self,
        dynamic_obj: Rc<Path>,
        dyn_callsite: Rc<CallSite>,
    ) {
        self.dynamic_dispatch_callsites.push((dynamic_obj, dyn_callsite));
    }

    pub fn add_fnptr_callsite(&mut self, fn_ptr: Rc<Path>, callsite: Rc<CallSite>) {
        self.fnptr_callsites.push((fn_ptr, callsite));
    }

    pub fn add_special_callsite(&mut self, callsite: Rc<CallSite>, callee: FuncId) {
        self.special_callsites.push((callsite, callee));
    }
}
