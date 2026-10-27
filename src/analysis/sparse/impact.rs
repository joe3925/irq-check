use super::*;
use super::super::global::PointerObject;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct EffectCause<'tcx> {
    pub instance: Instance<'tcx>,
    pub location: Location,
    pub reason: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WriteImpact<'tcx> {
    Preserved,
    MayWrite(EffectCause<'tcx>),
    Unresolved(EffectCause<'tcx>),
}

type ImpactKey<'tcx> = (Instance<'tcx>, PointerObject<'tcx>, Option<(u64, u64)>);

struct LocalImpact<'tcx> {
    direct: WriteImpact<'tcx>,
    callees: Box<[Instance<'tcx>]>,
}

#[derive(Default)]
pub(crate) struct ImpactCache<'tcx> {
    completed: HashMap<ImpactKey<'tcx>, WriteImpact<'tcx>>,
    local: HashMap<ImpactKey<'tcx>, LocalImpact<'tcx>>,
    pub(super) stopped: HashMap<ImpactKey<'tcx>, (usize, usize)>,
    revision: usize,
    pub bytes: usize,
    pub local_bytes: usize,
    pub local_hits: usize,
    pub queries: usize,
    pub hits: usize,
    pub preserved: usize,
    pub unresolved: usize,
    pub stops: usize,
}

impl<'a, 'tcx> Solver<'a, 'tcx> {
    fn place_write_impact(&mut self, instance: Instance<'tcx>, body: &mir::Body<'tcx>,
        place: Place<'tcx>, object: PointerObject<'tcx>, range: Option<(u64, u64)>,
        bytes: Option<u64>, location: Location) -> Result<WriteImpact<'tcx>, QueryStop>
    {
        self.budget.charge(0)?;
        let unknown = WriteImpact::Unresolved(EffectCause { instance, location,
            reason: "the write address has no complete alias information for this storage" });
        if !place.is_indirect() { return Ok(WriteImpact::Preserved); }
        if place.projection.first() != Some(&ProjectionElem::Deref)
            || place.projection.iter().skip(1).any(|part| matches!(part, ProjectionElem::Deref)) {
            return Ok(unknown);
        }
        let ty = instance.instantiate_mir_and_normalize_erasing_regions(self.tcx,
            TypingEnv::fully_monomorphized(), EarlyBinder::bind(self.tcx, body.local_decls[place.local].ty));
        let offset = (|| {
            let ty = ty.builtin_deref(true)?;
            let mut layout = self.tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(ty)).ok()?;
            let cx = ty::layout::LayoutCx::new(self.tcx, TypingEnv::fully_monomorphized());
            let mut offset = 0u64;
            for part in place.projection.iter().skip(1) {
                match part {
                    ProjectionElem::Field(field, _) => {
                        if field.as_usize() >= layout.fields.count() { return None; }
                        offset = offset.checked_add(layout.fields.offset(field.as_usize()).bytes())?;
                        layout = layout.field(&cx, field.as_usize());
                    }
                    ProjectionElem::Downcast(_, variant) => layout = layout.for_variant(&cx, variant),
                    _ => return None,
                }
            }
            Some(offset)
        })();
        let Some(targets) = self.facts.pointee_ranges.get(&(instance, place.local)).filter(|targets| !targets.is_empty())
            else { return Ok(unknown); };
        for &(target, bounds) in targets {
            self.budget.charge(0)?;
            if target != object { continue; }
            let written = (|| {
                let (base, extent) = bounds?;
                let start = base.checked_add(offset?)?;
                let end = start.checked_add(bytes?)?;
                (end <= extent).then_some((start, end))
            })();
            if !written.zip(range).is_some_and(|((start, end), (read_start, read_end))|
                end <= read_start || read_end <= start) {
                return Ok(WriteImpact::MayWrite(EffectCause { instance, location,
                    reason: "the write may overlap the requested storage" }));
            }
        }
        Ok(WriteImpact::Preserved)
    }

    pub(super) fn write_impact(&mut self, instance: Instance<'tcx>, object: PointerObject<'tcx>, range: Option<(u64, u64)>)
        -> Result<WriteImpact<'tcx>, QueryStop>
    {
        let root = (instance, object, range);
        self.impacts.queries += 1;
        if let Some(&impact) = self.impacts.completed.get(&root) {
            self.impacts.hits += 1;
            return Ok(impact);
        }
        self.budget.charge(0)?;
        let before = self.budget.steps;
        let scratch = self.budget.scratch;
        let refinement = self.budget.refinement_limit;
        let limit_usage = self.budget.limit_usage;
        let revision = self.impacts.revision;
        let allowance = irq_check_rupta::limits::configuration().expect("validated analysis limits").scalar_steps.min(4096)
            .min(refinement.unwrap_or(self.budget.limit).min(self.budget.limit).saturating_sub(before) / 2);
        if self.impacts.stopped.get(&root).is_some_and(|&(previous, generation)|
            previous >= allowance && generation == revision) { return Err(QueryStop::RefinementLimit); }
        let limit = self.budget.steps.saturating_add(allowance);
        self.budget.refinement_limit = Some(refinement.map_or(limit, |old| old.min(limit)));
        let answer = (|| {
            let mut pending = VecDeque::from([instance]);
            let mut impacts = HashMap::new();
            let mut callers = HashMap::<Instance<'tcx>, Vec<Instance<'tcx>>>::new();
            while let Some(instance) = pending.pop_front() {
                self.budget.charge(0)?;
                if impacts.contains_key(&instance) { continue; }
                self.budget.reserve(std::mem::size_of::<(ImpactKey<'tcx>, WriteImpact<'tcx>)>() * 8)?;
                if let Some(&impact) = self.impacts.completed.get(&(instance, object, range)) {
                    impacts.insert(instance, impact);
                    continue;
                }
                let mut callees = Vec::new();
                let local = if let Some(local) = self.impacts.local.get(&(instance, object, range)) {
                    self.budget.reserve(local.callees.len().saturating_mul(std::mem::size_of::<Instance<'tcx>>() * 4))?;
                    self.impacts.local_hits += 1;
                    callees.extend(local.callees.iter().copied());
                    local.direct
                } else {
                let direct = (|| {
                    let unknown = |location, reason| WriteImpact::Unresolved(EffectCause { instance, location, reason });
                    if crate::known_intrinsic(self.tcx, instance)
                        || matches!(instance.def, InstanceKind::Shim(ty::ShimKind::DropGlue(_, None))) {
                        return Ok(WriteImpact::Preserved);
                    }
                    if matches!(instance.def, InstanceKind::Intrinsic(_)) {
                        return Ok(if super::super::intrinsic_effect(self.tcx, instance) == super::super::IntrinsicEffect::ReadOnly {
                            WriteImpact::Preserved
                        } else { unknown(Location::START, "the intrinsic write needs its call arguments") });
                    }
                    if self.tcx.is_foreign_item(instance.def_id()) {
                        if let Some(linked) = self.linked_target(instance).filter(|target| *target != instance) {
                            self.budget.reserve(std::mem::size_of::<Instance<'tcx>>() * 4)?;
                            callees.push(linked);
                            return Ok(WriteImpact::Preserved);
                        }
                        return Ok(unknown(Location::START, "the external call has no storage-effect model"));
                    }
                    if matches!(instance.def, InstanceKind::LlvmIntrinsic(_) | InstanceKind::Virtual(..))
                        || !irq_check_rupta::util::type_util::has_instance_mir(self.tcx, instance.def) {
                        return Ok(unknown(Location::START, "the callable has no storage-effect model"));
                    }
                    let body = self.tcx.instance_mir(instance.def);
                    let tcx = self.tcx;
                    let normalize = |ty| instance.instantiate_mir_and_normalize_erasing_regions(tcx,
                        TypingEnv::fully_monomorphized(), EarlyBinder::bind(tcx, ty));
                    let size = |place: Place<'tcx>| tcx.layout_of(TypingEnv::fully_monomorphized()
                        .as_query_input(normalize(place.ty(body, tcx).ty))).ok().map(|layout| layout.size.bytes());
                    for (block, data) in body.basic_blocks.iter_enumerated() {
                        self.budget.charge(0)?;
                        for (statement_index, statement) in data.statements.iter().enumerate() {
                            self.budget.charge(0)?;
                            let at = Location { block, statement_index };
                            let write = match &statement.kind {
                                StatementKind::Assign(value) => Some((value.0, size(value.0))),
                                StatementKind::SetDiscriminant { place, .. } => Some((**place, size(**place))),
                                StatementKind::Intrinsic(intrinsic) => match &**intrinsic {
                                    mir::NonDivergingIntrinsic::CopyNonOverlapping(copy) => {
                                        let (Operand::Copy(place) | Operand::Move(place)) = &copy.dst else {
                                            return Ok(unknown(at, "the copy destination has no complete storage binding"));
                                        };
                                        if !place.projection.is_empty() {
                                            return Ok(unknown(at, "the copy destination requires a projected pointer read"));
                                        }
                                        Some((Place { local: place.local, projection: tcx.mk_place_elems(&[ProjectionElem::Deref]) }, None))
                                    }
                                    _ => None,
                                },
                                _ => None,
                            };
                            if let Some((place, bytes)) = write {
                                let impact = self.place_write_impact(instance, body, place, object, range, bytes, at)?;
                                if impact != WriteImpact::Preserved { return Ok(impact); }
                            }
                        }
                        let at = Location { block, statement_index: data.statements.len() };
                        if let TerminatorKind::Call { destination, .. } = &data.terminator().kind {
                            let impact = self.place_write_impact(instance, body, *destination, object, range, size(*destination), at)?;
                            if impact != WriteImpact::Preserved { return Ok(impact); }
                        }
                        match &data.terminator().kind {
                            TerminatorKind::Call { func, args, .. } | TerminatorKind::TailCall { func, args, .. } => {
                                let target = if let ty::FnDef(def, substitutions) = *normalize(func.ty(body, tcx)).kind() {
                                    Instance::try_resolve(tcx, TypingEnv::fully_monomorphized(), def,
                                        tcx.instantiate_bound_regions_with_erased(substitutions)).ok().flatten()
                                        .filter(|target| !matches!(target.def, InstanceKind::Virtual(..)))
                                } else { None };
                                if let Some(target) = target {
                                    if matches!(target.def, InstanceKind::Intrinsic(_)) {
                                        match super::super::intrinsic_effect(tcx, target) {
                                            super::super::IntrinsicEffect::ReadOnly => continue,
                                            super::super::IntrinsicEffect::Write { arguments, fixed } => {
                                                for &argument in arguments {
                                                    let Some(argument) = args.get(argument) else {
                                                        return Ok(unknown(at, "the intrinsic write argument is missing"));
                                                    };
                                                    let (Operand::Copy(place) | Operand::Move(place)) = &argument.node else {
                                                        return Ok(unknown(at, "the intrinsic write address is unresolved"));
                                                    };
                                                    if !place.projection.is_empty() {
                                                        return Ok(unknown(at, "the intrinsic write requires a projected pointer read"));
                                                    }
                                                    let destination = Place { local: place.local, projection: tcx.mk_place_elems(&[ProjectionElem::Deref]) };
                                                    let impact = self.place_write_impact(instance, body, destination, object, range,
                                                        if fixed { size(destination) } else { None }, at)?;
                                                    if impact != WriteImpact::Preserved { return Ok(impact); }
                                                }
                                                continue;
                                            }
                                            super::super::IntrinsicEffect::Unknown =>
                                                return Ok(unknown(at, "the intrinsic has no complete storage-effect model")),
                                        }
                                    }
                                    self.budget.reserve(std::mem::size_of::<Instance<'tcx>>() * 4)?;
                                    callees.push(target);
                                } else if self.facts.pointer_calls.contains(&(instance, at)) {
                                    let Some(targets) = self.facts.safe_candidates.get(&(instance, at)).filter(|targets| !targets.is_empty())
                                        else { return Ok(unknown(at, "the indirect call has no complete target set")); };
                                    for &target in targets {
                                        self.budget.charge(std::mem::size_of::<Instance<'tcx>>() * 4)?;
                                        callees.push(target);
                                    }
                                } else { return Ok(unknown(at, "the indirect call has no complete target set")); }
                            }
                            TerminatorKind::Drop { place, .. } => {
                                self.budget.reserve(std::mem::size_of::<Instance<'tcx>>() * 4)?;
                                callees.push(Instance::resolve_drop_glue(tcx, normalize(place.ty(body, tcx).ty)));
                            }
                            TerminatorKind::InlineAsm { options, operands, .. } => {
                                if !options.intersects(rustc_ast::InlineAsmOptions::NOMEM | rustc_ast::InlineAsmOptions::READONLY) {
                                    return Ok(unknown(at, "inline assembly can change the requested storage"));
                                }
                                for operand in operands {
                                    let place = match operand {
                                        mir::InlineAsmOperand::Out { place, .. } => *place,
                                        mir::InlineAsmOperand::InOut { out_place, .. } => *out_place,
                                        _ => None,
                                    };
                                    if let Some(place) = place {
                                        let impact = self.place_write_impact(instance, body, place, object, range, size(place), at)?;
                                        if impact != WriteImpact::Preserved { return Ok(impact); }
                                    }
                                }
                            }
                            TerminatorKind::Assert { unwind, .. } if !matches!(unwind,
                                mir::UnwindAction::Unreachable | mir::UnwindAction::Terminate(_)) =>
                                return Ok(unknown(at, "the assertion unwind has no storage-effect model")),
                            TerminatorKind::Yield { .. } => return Ok(unknown(at, "the suspension has no storage-effect model")),
                            _ => {}
                        }
                    }
                    Ok(WriteImpact::Preserved)
                })()?;
                if direct != WriteImpact::Preserved { callees.clear(); }
                let bytes = std::mem::size_of::<(ImpactKey<'tcx>, LocalImpact<'tcx>)>() * 4
                    + callees.len().saturating_mul(std::mem::size_of::<Instance<'tcx>>() * 2);
                if bytes <= self.cache_limit / 128 {
                    if self.impacts.local_bytes.saturating_add(bytes) > self.cache_limit / 128 {
                        self.impacts.local.clear();
                        self.impacts.local_bytes = 0;
                    }
                    self.budget.reserve(bytes)?;
                    self.impacts.local.insert((instance, object, range), LocalImpact {
                        direct, callees: callees.clone().into_boxed_slice(),
                    });
                    self.impacts.local_bytes += bytes;
                    self.impacts.revision = self.impacts.revision.saturating_add(1);
                }
                direct
                };
                impacts.insert(instance, local);
                if local == WriteImpact::Preserved {
                    for callee in callees {
                        self.budget.charge(std::mem::size_of::<Instance<'tcx>>() * 4)?;
                        callers.entry(callee).or_default().push(instance);
                        pending.push_back(callee);
                    }
                }
            }
            pending.extend(impacts.iter().filter_map(|(&instance, impact)| (*impact != WriteImpact::Preserved).then_some(instance)));
            while let Some(callee) = pending.pop_front() {
                self.budget.charge(0)?;
                let impact = impacts[&callee];
                for &caller in callers.get(&callee).into_iter().flatten() {
                    if impacts[&caller] == WriteImpact::Preserved {
                        impacts.insert(caller, impact);
                        pending.push_back(caller);
                    }
                }
            }
            let result = impacts[&instance];
            let bytes = impacts.len().saturating_mul(std::mem::size_of::<(ImpactKey<'tcx>, WriteImpact<'tcx>)>() * 4);
            if bytes <= self.cache_limit / 128 {
                if self.impacts.bytes.saturating_add(bytes) > self.cache_limit / 128 {
                    self.impacts.completed.clear();
                    self.impacts.bytes = 0;
                }
                for (instance, impact) in impacts { self.impacts.completed.insert((instance, object, range), impact); }
                self.impacts.bytes += bytes;
            }
            Ok(result)
        })();
        self.budget.refinement_limit = refinement;
        self.budget.scratch = scratch;
        match answer {
            Ok(WriteImpact::Preserved) => self.impacts.preserved += 1,
            Ok(WriteImpact::Unresolved(_)) => self.impacts.unresolved += 1,
            Err(QueryStop::RefinementLimit) => {
                self.budget.limit_usage = limit_usage;
                self.impacts.stops += 1;
                if self.impacts.stopped.len().saturating_add(1).saturating_mul(std::mem::size_of::<ImpactKey<'tcx>>() * 4)
                    <= self.cache_limit / 64 { self.impacts.stopped.insert(root, (allowance, revision)); }
            }
            _ => {}
        }
        answer
    }
}
