//! This module is concerned with finding methods that a given type provides.
//! For details about how this works in rustc, see the method lookup page in the
//! [rustc guide](https://rust-lang.github.io/rustc-guide/method-lookup.html)
//! and the corresponding code mostly in librustc_typeck/check/method/probe.rs.
use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::{
    HirDatabase, Module, Crate, Name, Function, Trait,
    impl_block::{ImplId, ImplBlock, ImplItem},
    ty::{Ty, TypeCtor},
    nameres::CrateModuleId,
    resolve::Resolver,
    traits::TraitItem,
    generics::HasGenericParams,
};
use super::{TraitRef, Substs};

/// This is used as a key for indexing impls.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum TyFingerprint {
    Apply(TypeCtor),
}

impl TyFingerprint {
    /// Creates a TyFingerprint for looking up an impl. Only certain types can
    /// have impls: if we have some `struct S`, we can have an `impl S`, but not
    /// `impl &S`. Hence, this will return `None` for reference types and such.
    fn for_impl(ty: &Ty) -> Option<TyFingerprint> {
        match ty {
            Ty::Apply(a_ty) => Some(TyFingerprint::Apply(a_ty.ctor)),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct CrateImplBlocks {
    /// To make sense of the CrateModuleIds, we need the source root.
    krate: Crate,
    impls: FxHashMap<TyFingerprint, Vec<(CrateModuleId, ImplId)>>,
    impls_by_trait: FxHashMap<Trait, Vec<(CrateModuleId, ImplId)>>,
}

impl CrateImplBlocks {
    pub fn lookup_impl_blocks<'a>(&'a self, ty: &Ty) -> impl Iterator<Item = ImplBlock> + 'a {
        let fingerprint = TyFingerprint::for_impl(ty);
        fingerprint.and_then(|f| self.impls.get(&f)).into_iter().flat_map(|i| i.iter()).map(
            move |(module_id, impl_id)| {
                let module = Module { krate: self.krate, module_id: *module_id };
                ImplBlock::from_id(module, *impl_id)
            },
        )
    }

    pub fn lookup_impl_blocks_for_trait<'a>(
        &'a self,
        tr: &Trait,
    ) -> impl Iterator<Item = ImplBlock> + 'a {
        self.impls_by_trait.get(&tr).into_iter().flat_map(|i| i.iter()).map(
            move |(module_id, impl_id)| {
                let module = Module { krate: self.krate, module_id: *module_id };
                ImplBlock::from_id(module, *impl_id)
            },
        )
    }

    fn collect_recursive(&mut self, db: &impl HirDatabase, module: &Module) {
        let module_impl_blocks = db.impls_in_module(module.clone());

        for (impl_id, _) in module_impl_blocks.impls.iter() {
            let impl_block = ImplBlock::from_id(module_impl_blocks.module, impl_id);

            let target_ty = impl_block.target_ty(db);

            if let Some(tr) = impl_block.target_trait_ref(db) {
                self.impls_by_trait
                    .entry(tr.trait_)
                    .or_insert_with(Vec::new)
                    .push((module.module_id, impl_id));
            } else {
                if let Some(target_ty_fp) = TyFingerprint::for_impl(&target_ty) {
                    self.impls
                        .entry(target_ty_fp)
                        .or_insert_with(Vec::new)
                        .push((module.module_id, impl_id));
                }
            }
        }

        for child in module.children(db) {
            self.collect_recursive(db, &child);
        }
    }

    pub(crate) fn impls_in_crate_query(
        db: &impl HirDatabase,
        krate: Crate,
    ) -> Arc<CrateImplBlocks> {
        let mut crate_impl_blocks = CrateImplBlocks {
            krate,
            impls: FxHashMap::default(),
            impls_by_trait: FxHashMap::default(),
        };
        if let Some(module) = krate.root_module(db) {
            crate_impl_blocks.collect_recursive(db, &module);
        }
        Arc::new(crate_impl_blocks)
    }
}

fn def_crate(db: &impl HirDatabase, ty: &Ty) -> Option<Crate> {
    match ty {
        Ty::Apply(a_ty) => match a_ty.ctor {
            TypeCtor::Adt(def_id) => def_id.krate(db),
            _ => None,
        },
        _ => None,
    }
}

impl Ty {
    /// Look up the method with the given name, returning the actual autoderefed
    /// receiver type (but without autoref applied yet).
    pub(crate) fn lookup_method(
        self,
        db: &impl HirDatabase,
        name: &Name,
        resolver: &Resolver,
    ) -> Option<(Ty, Function)> {
        self.iterate_method_candidates(db, resolver, Some(name), |ty, f| Some((ty.clone(), f)))
    }

    // This would be nicer if it just returned an iterator, but that runs into
    // lifetime problems, because we need to borrow temp `CrateImplBlocks`.
    pub(crate) fn iterate_method_candidates<T>(
        self,
        db: &impl HirDatabase,
        resolver: &Resolver,
        name: Option<&Name>,
        mut callback: impl FnMut(&Ty, Function) -> Option<T>,
    ) -> Option<T> {
        // For method calls, rust first does any number of autoderef, and then one
        // autoref (i.e. when the method takes &self or &mut self). We just ignore
        // the autoref currently -- when we find a method matching the given name,
        // we assume it fits.

        // Also note that when we've got a receiver like &S, even if the method we
        // find in the end takes &self, we still do the autoderef step (just as
        // rustc does an autoderef and then autoref again).

        for derefed_ty in self.autoderef(db) {
            if let Some(result) = derefed_ty.iterate_inherent_methods(db, name, &mut callback) {
                return Some(result);
            }
            if let Some(result) =
                derefed_ty.iterate_trait_method_candidates(db, resolver, name, &mut callback)
            {
                return Some(result);
            }
        }
        None
    }

    fn iterate_trait_method_candidates<T>(
        &self,
        db: &impl HirDatabase,
        resolver: &Resolver,
        name: Option<&Name>,
        mut callback: impl FnMut(&Ty, Function) -> Option<T>,
    ) -> Option<T> {
        'traits: for t in resolver.traits_in_scope() {
            let data = t.trait_data(db);
            // we'll be lazy about checking whether the type implements the
            // trait, but if we find out it doesn't, we'll skip the rest of the
            // iteration
            let mut known_implemented = false;
            for item in data.items() {
                match item {
                    &TraitItem::Function(m) => {
                        let sig = m.signature(db);
                        if name.map_or(true, |name| sig.name() == name) && sig.has_self_param() {
                            if !known_implemented {
                                let trait_ref = TraitRef {
                                    trait_: t,
                                    substs: fresh_substs_for_trait(db, t, self.clone()),
                                };
                                let (trait_ref, _) = super::traits::canonicalize(trait_ref);
                                if db.implements(trait_ref).is_none() {
                                    continue 'traits;
                                }
                            }
                            known_implemented = true;
                            if let Some(result) = callback(self, m) {
                                return Some(result);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        None
    }

    fn iterate_inherent_methods<T>(
        &self,
        db: &impl HirDatabase,
        name: Option<&Name>,
        mut callback: impl FnMut(&Ty, Function) -> Option<T>,
    ) -> Option<T> {
        let krate = match def_crate(db, self) {
            Some(krate) => krate,
            None => return None,
        };
        let impls = db.impls_in_crate(krate);

        for impl_block in impls.lookup_impl_blocks(self) {
            for item in impl_block.items(db) {
                match item {
                    ImplItem::Method(f) => {
                        let sig = f.signature(db);
                        if name.map_or(true, |name| sig.name() == name) && sig.has_self_param() {
                            if let Some(result) = callback(self, f) {
                                return Some(result);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        None
    }

    // This would be nicer if it just returned an iterator, but that runs into
    // lifetime problems, because we need to borrow temp `CrateImplBlocks`.
    pub fn iterate_impl_items<T>(
        self,
        db: &impl HirDatabase,
        mut callback: impl FnMut(ImplItem) -> Option<T>,
    ) -> Option<T> {
        let krate = def_crate(db, &self)?;
        let impls = db.impls_in_crate(krate);

        for impl_block in impls.lookup_impl_blocks(&self) {
            for item in impl_block.items(db) {
                if let Some(result) = callback(item) {
                    return Some(result);
                }
            }
        }
        None
    }
}

/// This creates Substs for a trait with the given Self type and type variables
/// for all other parameters. This is kind of a hack since these aren't 'real'
/// type variables; the resulting trait reference is just used for the
/// preliminary method candidate check.
fn fresh_substs_for_trait(db: &impl HirDatabase, tr: Trait, self_ty: Ty) -> Substs {
    let mut substs = Vec::new();
    let generics = tr.generic_params(db);
    substs.push(self_ty);
    substs.extend(generics.params_including_parent().into_iter().skip(1).enumerate().map(
        |(i, _p)| Ty::Infer(super::infer::InferTy::TypeVar(super::infer::TypeVarId(i as u32))),
    ));
    substs.into()
}
