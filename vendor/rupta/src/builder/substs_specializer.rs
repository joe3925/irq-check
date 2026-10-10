// Copyright (c) 2024 <Wei Li>.
//
// This source code is licensed under the GNU license found in the
// LICENSE file in the root directory of this source tree.

//! Specializes generic types to concrete types.
//! 
//! Adapted primarily from the code in [MIRAI](<https://github.com/facebookexperimental/MIRAI>).
//! 
//! For example:
//!
//! ```no_run
//! fn foo<T>(t: T) {}
//! fn bar<U, V>(u: U, v: V) { foo(u); foo(v); }
//! fn main() { bar(3, 4.0); }
//! ```
//!
//! The function `bar` is invoked in `main` with generic arguments `[i32, f64]`.
//! During analysis, we specialize the types of `u` and `v` in `bar` to `i32` and `f64` respectively.
//! The calls to `foo(u)` and `foo(v)` can therefore be resolved to `foo::<i32>(u)` and 
//! `foo::<f64>(v)` respectively.

use rustc_middle::ty::{EarlyBinder, GenericArg, GenericArgsRef, Ty, TyCtxt, TypeVisitableExt, TypingEnv};
use crate::mir::function::GenericArgE;

pub struct SubstsSpecializer<'tcx> {
    pub tcx: TyCtxt<'tcx>,
    pub generic_args: GenericArgsRef<'tcx>,
}

impl<'tcx> SubstsSpecializer<'tcx> {
    pub fn new(tcx: TyCtxt<'tcx>, generic_args: Vec<GenericArgE<'tcx>>) -> Self {
        let generic_args = tcx.mk_args_from_iter(generic_args.into_iter().map(|arg| match arg {
            GenericArgE::Region => GenericArg::from(tcx.lifetimes.re_erased),
            GenericArgE::Const(value) => GenericArg::from(value),
            GenericArgE::Type(ty) => GenericArg::from(ty),
        }));
        Self { tcx, generic_args }
    }

    pub fn specialize_generic_args(&self, args: GenericArgsRef<'tcx>) -> GenericArgsRef<'tcx> {
        let value = EarlyBinder::bind(self.tcx, args).instantiate(self.tcx, self.generic_args);
        if value.skip_norm_wip().has_non_region_param() || value.skip_norm_wip().has_escaping_bound_vars() {
            self.tcx.erase_and_anonymize_regions(value.skip_norm_wip())
        } else {
            self.tcx.normalize_erasing_regions(TypingEnv::fully_monomorphized(), value)
        }
    }

    pub fn specialize_generic_argument_type(&self, ty: Ty<'tcx>) -> Ty<'tcx> {
        let value = EarlyBinder::bind(self.tcx, ty).instantiate(self.tcx, self.generic_args);
        if value.skip_norm_wip().has_non_region_param() || value.skip_norm_wip().has_escaping_bound_vars() {
            self.tcx.erase_and_anonymize_regions(value.skip_norm_wip())
        } else {
            self.tcx.normalize_erasing_regions(TypingEnv::fully_monomorphized(), value)
        }
    }
}
