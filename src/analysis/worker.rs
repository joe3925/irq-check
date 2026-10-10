use super::*;

impl<'tcx> Analysis<'tcx> {
    pub(super) fn process_node(
        &mut self,
        node: Node<'tcx>,
        trust: &Trust,
        roots: &[Instance<'tcx>],
        exported: &HashMap<String, Instance<'tcx>>,
        live_blocks: &mut HashMap<Instance<'tcx>, HashMap<mir::BasicBlock, HashSet<usize>>>,
        loop_headers: &mut HashMap<Instance<'tcx>, HashSet<mir::BasicBlock>>,
        progress: bool,
        progress_file: &Option<std::path::PathBuf>,
        started: std::time::Instant,
        last_progress: &mut std::time::Instant,
        scans: &mut usize,
        block_visits: &mut usize,
    ) {
        let analysis = self;
        let tcx = analysis.tcx;
        let instance = node.instance;
        analysis.queued.remove(&node);
        analysis.active = Some(node);
        analysis.read_subscriptions.clear();
        analysis.havoc_subscribed = false;
        analysis.active_location = mir::Location::START;
        analysis.local_context = node.context;
        analysis.singleton_storage = node
            .context
            .checked_sub(1)
            .and_then(|index| {
                analysis
                    .contexts
                    .get(&instance)
                    .and_then(|contexts| contexts.get(index))
            })
            .map(|input| input.singletons.clone())
            .unwrap_or_default();
        if let Some(edges) = analysis.graph.get(&node) {
            for site in edges.iter().filter_map(|edge| edge.site) {
                if analysis.free_sites.insert(site) {
                    analysis.free_site_slots.push(site);
                }
            }
        }
        analysis.states.remove(&node);
        analysis.site_index.remove(&node);
        *scans += 1;
        if progress && last_progress.elapsed().as_secs() >= 10 {
            let status = format!(
                "elapsed={:.1}s scans={scans} contexts={} policy_roots={} pending={} facts={} current={instance} opaque_calls={} opaque_addresses={} opaque_seconds={:.3} opaque_read_hits={} opaque_read_misses={} opaque_parallel_batches={} opaque_parallel_addresses={} opaque_parallel_misses={} opaque_workers={} traversal_hits={} saved_visits={} effective_view_hits={} subscription_hits={}\n",
                started.elapsed().as_secs_f64(),
                analysis.graph.len(),
                analysis.roots.len(),
                analysis.pending.len(),
                analysis.revision,
                analysis.opaque_calls,
                analysis.opaque_addresses,
                analysis.opaque_elapsed.as_secs_f64(),
                analysis.opaque_read_hits,
                analysis.opaque_read_misses,
                analysis.opaque_parallel_batches,
                analysis.opaque_parallel_addresses,
                analysis.opaque_parallel_misses,
                analysis.opaque_workers.len(),
                analysis.opaque_traversal_hits,
                analysis.opaque_saved_visits,
                analysis.effective_view_hits.get(),
                analysis.read_subscription_hits
            );
            eprint!("irq-check: {status}");
            if let Some(path) = &progress_file {
                let _ = std::fs::write(path, status);
                let _ = std::fs::write(
                    path.with_extension("contexts.txt"),
                    analysis.context_details(),
                );
            }
            *last_progress = std::time::Instant::now();
        }
        let mut edges = Vec::new();
        if forbidden(tcx, instance).is_some() && !tcx.is_mir_available(instance.def_id())
            || !trust.checks(instance.def_id())
            || known_intrinsic(tcx, instance)
        {
            analysis.graph.insert(node, edges);
            return;
        }
        if matches!(
            instance.def,
            InstanceKind::Intrinsic(_) | InstanceKind::Shim(ShimKind::DropGlue(_, None))
        ) {
            analysis.graph.insert(node, edges);
            return;
        }
        if tcx.is_foreign_item(instance.def_id()) {
            let symbol = tcx.symbol_name(instance).name;
            edges.push(Edge {
                site: None,
                target: exported
                    .get(symbol)
                    .map(|instance| analysis.context(*instance, Vec::new()).0),
                span: tcx.def_span(instance.def_id()),
                kind: "external call",
                detail: format!("external symbol has no Rust body: {symbol}"),
                trusted: false,
            });
        } else if matches!(instance.def, InstanceKind::Virtual(..))
            || !tcx.is_mir_available(instance.def_id())
                && matches!(instance.def, InstanceKind::Item(_))
        {
            edges.push(Edge {
                site: None,
                target: None,
                span: tcx.def_span(instance.def_id()),
                kind: "unresolved call",
                detail: format!("Rust body is not available for {instance}"),
                trusted: false,
            });
        } else {
            let body = tcx.instance_mir(instance.def);
            let loop_headers = loop_headers.entry(instance).or_insert_with(|| {
                let dominators = body.basic_blocks.dominators();
                body.basic_blocks
                    .iter_enumerated()
                    .filter(|(block, _)| dominators.is_reachable(*block))
                    .flat_map(|(block, data)| {
                        data.terminator()
                            .successors()
                            .filter(move |target| dominators.dominates(*target, block))
                    })
                    .collect()
            });
            analysis.published_locals = analysis
                .address_taken
                .entry(instance)
                .or_insert_with(|| {
                    rustc_mir_dataflow::impls::borrowed_locals(body)
                        .iter()
                        .map(|local| local.as_usize())
                        .collect()
                })
                .clone();
            analysis.published_locals.insert(0);
            let live_blocks = live_blocks.entry(instance).or_insert_with(|| {
                let mut cursor = rustc_mir_dataflow::impls::MaybeLiveLocals
                    .iterate_to_fixpoint(tcx, body, None)
                    .into_results_cursor(body);
                body.basic_blocks
                    .indices()
                    .map(|block| {
                        cursor.seek_to_block_start(block);
                        let mut live: HashSet<_> =
                            cursor.get().iter().map(|local| local.as_usize()).collect();
                        live.insert(0);
                        (block, live)
                    })
                    .collect()
            });
            let mut initial = node
                .context
                .checked_sub(1)
                .and_then(|index| {
                    analysis
                        .contexts
                        .get(&instance)
                        .and_then(|contexts| contexts.get(index))
                })
                .map(|input| input.memory.clone())
                .unwrap_or_default();
            initial.extend((1..=body.arg_count).map(|local| {
                (
                    Base::Local(instance, node.context, local),
                    analysis.memory_tree(&Base::Local(instance, node.context, local)),
                )
            }));
            let mut pending = VecDeque::from([(mir::Location::START, initial)]);
            let mut partitions = crate::pointer::ContextCache::<ContextKey<'tcx>>::default();
            let mut seen = HashMap::<(mir::Location, usize), HashSet<ContextKey<'tcx>>>::new();
            let mut widened = HashMap::<(mir::Location, usize), FlowState<'tcx>>::new();
            let mut live_reachable = VecDeque::new();
            let mut live_visited = HashSet::new();
            let mut live = HashSet::new();
            'states: while let Some((location, mut locals)) = pending.pop_front() {
                let block = location.block;
                *block_visits += 1;
                if progress && last_progress.elapsed().as_secs() >= 10 {
                    let status = format!(
                        "elapsed={:.1}s scans={scans} contexts={} policy_roots={} pending={} facts={} blocks={block_visits} current={instance} context={} block={block:?} flow_pending={} partitions={} stored_states={} current_cells={} opaque_calls={} opaque_addresses={} opaque_seconds={:.3} opaque_read_hits={} opaque_read_misses={} opaque_parallel_batches={} opaque_parallel_addresses={} opaque_parallel_misses={} opaque_workers={} traversal_hits={} saved_visits={} effective_view_hits={} subscription_hits={}\n",
                        started.elapsed().as_secs_f64(),
                        analysis.graph.len(),
                        analysis.roots.len(),
                        analysis.pending.len(),
                        analysis.revision,
                        node.context,
                        pending.len(),
                        partitions.context_list.len(),
                        seen.values().map(HashSet::len).sum::<usize>(),
                        locals.len(),
                        analysis.opaque_calls,
                        analysis.opaque_addresses,
                        analysis.opaque_elapsed.as_secs_f64(),
                        analysis.opaque_read_hits,
                        analysis.opaque_read_misses,
                        analysis.opaque_parallel_batches,
                        analysis.opaque_parallel_addresses,
                        analysis.opaque_parallel_misses,
                        analysis.opaque_workers.len(),
                        analysis.opaque_traversal_hits,
                        analysis.opaque_saved_visits,
                        analysis.effective_view_hits.get(),
                        analysis.read_subscription_hits
                    );
                    eprint!("irq-check: {status}");
                    if let Some(path) = &progress_file {
                        let _ = std::fs::write(path, &status);
                        let _ = std::fs::write(
                            path.with_extension("contexts.txt"),
                            analysis.context_details(),
                        );
                    }
                    *last_progress = std::time::Instant::now();
                }
                if location.statement_index == 0 {
                    live.clone_from(&live_blocks[&block]);
                    let reachable = &mut live_reachable;
                    let visited = &mut live_visited;
                    reachable.clear();
                    visited.clear();
                    for base in locals.keys() {
                        if let Base::Local(owner, context, local) = base {
                            if *owner == instance && *context == node.context {
                                if analysis.escaped.contains(base)
                                    || locals.contains_key(&Base::Havoc)
                                        && analysis.published_locals.contains(local)
                                {
                                    live.insert(*local);
                                }
                                if !live.contains(local) {
                                    continue;
                                }
                            }
                        }
                        if visited.insert(base.clone()) {
                            reachable.push_back(base.clone());
                        }
                    }
                    while let Some(base) = reachable.pop_front() {
                        for referenced in locals
                            .get(&base)
                            .into_iter()
                            .flat_map(|tree| tree.references())
                        {
                            if let Base::Local(owner, context, local) = referenced {
                                if *owner == instance && *context == node.context {
                                    live.insert(*local);
                                }
                            }
                            if locals.contains_key(referenced) && visited.insert(referenced.clone()) {
                                reachable.push_back(referenced.clone());
                            }
                        }
                    }
                    locals.retain(|base, _| match base {
                        Base::Local(owner, context, local)
                            if *owner == instance && *context == node.context =>
                        {
                            live.contains(local)
                        }
                        _ => true,
                    });
                }
                let mut signature = FlowState::new();
                for (base, tree) in &locals {
                    if !matches!(base, Base::Local(owner, context, _) if *owner == instance && *context == node.context)
                    {
                        continue;
                    }
                    for (path, facts) in tree {
                        for fact in facts {
                            if let Fact::Reference(address) = fact {
                                if let Base::Allocation(id, offset) = address.base {
                                    if let GlobalAlloc::Memory(memory) = tcx.global_alloc(id) {
                                        if memory.inner().mutability == rustc_hir::Mutability::Not {
                                            let position =
                                                i128::from(offset).checked_add(address.byte_offset);
                                            let value = if position.is_some_and(|position| {
                                                position >= 0
                                                    && position <= memory.inner().len() as i128
                                            }) {
                                                fact.clone()
                                            } else {
                                                Fact::Unknown
                                            };
                                            signature
                                                .entry(base.clone())
                                                .or_default()
                                                .entry(path.clone())
                                                .or_default()
                                                .insert(value);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                let partition = partitions.get_context_id(std::borrow::Cow::Owned(ContextKey(Vec::new(), signature)));
                let state_key = (location, partition);
                let states = seen.entry(state_key).or_default();
                let candidate = ContextKey(Vec::new(), locals);
                if states.contains(&candidate) {
                    continue;
                }
                let ContextKey(_, mut locals) = candidate;
                if states.len()
                    >= if node.context == 0
                        || location.statement_index == 0 && loop_headers.contains(&block)
                    {
                        1
                    } else {
                        128
                    }
                    || widened.contains_key(&state_key)
                {
                    let previous = widened.get(&state_key).cloned();
                    let mut joined = previous.clone().unwrap_or_default();
                    for state in states
                        .iter()
                        .filter(|_| previous.is_none())
                        .map(|state| &state.1)
                        .chain(std::iter::once(&locals))
                    {
                        for (local, tree) in state {
                            if joined.get(local).is_some_and(|output| output == tree && output.is_normalized()) {
                                continue;
                            }
                            for (path, facts) in tree {
                                let output = joined
                                    .entry(local.clone())
                                    .or_default()
                                    .entry(path.clone())
                                    .or_default();
                                let mut references = output
                                    .iter()
                                    .filter(|fact| matches!(fact, Fact::Reference(_)))
                                    .count();
                                for fact in facts {
                                    if matches!(fact, Fact::Reference(_)) && !output.contains(fact)
                                    {
                                        if references >= 16 {
                                            output.insert(Fact::Unknown);
                                            continue;
                                        }
                                        references += 1;
                                    }
                                    output.insert(fact.clone());
                                }
                                join_facts(output, std::iter::empty());
                            }
                        }
                    }
                    if previous.as_ref() == Some(&joined) {
                        continue;
                    }
                    widened.insert(state_key, joined.clone());
                    analysis.precision_loss.insert(format!(
                        "joined program-point alternatives in {instance} at {block:?}"
                    ));
                    states.clear();
                    locals = joined;
                }
                states.insert(ContextKey(Vec::new(), locals.clone()));
                analysis.flow_locals = Some(locals);
                let data = &body.basic_blocks[block];
                for (statement_index, statement) in data
                    .statements
                    .iter()
                    .enumerate()
                    .skip(location.statement_index)
                {
                    if trust.allows(tcx, instance, body, statement.source_info.scope, true) {
                        continue;
                    }
                    let first_edge = edges.len();
                    let incoming = matches!(&statement.kind,
                        StatementKind::Assign(assignment) if matches!(&assignment.1,
                            Rvalue::Reborrow(..) | Rvalue::Cast(CastKind::Transmute, ..)))
                        .then(|| analysis.flow_locals.clone().unwrap_or_default());
                    let current = mir::Location {
                        block,
                        statement_index,
                    };
                    analysis.active_location = current;
                    analysis.pending_bindings.clear();
                    if node.context != 0
                        && !widened.keys().any(|(location, _)| *location == current)
                    {
                        let mut indexed = IndexedPlaces::default();
                        indexed.visit_statement(statement, current);
                        if let Some(alternatives) = analysis.index_states(node, body, indexed.0) {
                            pending.extend(alternatives.into_iter().map(|state| (current, state)));
                            continue 'states;
                        }
                    }
                    if let StatementKind::Assign(assignment) = &statement.kind {
                        let (destination, value) = &**assignment;
                        let mut tree = Tree::new();
                        match value {
                            Rvalue::Reborrow(destination_ty, _, place) => {
                                let source_ty = analysis.ty(instance, place.ty(body, tcx).ty);
                                let destination_ty = analysis.ty(instance, *destination_ty);
                                if source_ty == destination_ty {
                                    tree = analysis.place_value(instance, body, *place);
                                } else {
                                    tree.insert(Vec::new(), HashSet::from([Fact::Unknown]));
                                    edges.push(Edge {
                                        site: None,
                                        target: None,
                                        span: statement.source_info.span,
                                        kind: "unsupported reborrow layout",
                                        detail: format!("cannot map compiler reborrow from {source_ty} to {destination_ty}"),
                                        trusted: trust.allows(tcx, instance, body, statement.source_info.scope, false),
                                    });
                                }
                            }
                            Rvalue::Use(operand, _) | Rvalue::WrapUnsafeBinder(operand, _) => {
                                tree = analysis.operand(instance, body, operand)
                            }
                            Rvalue::CopyForDeref(place) => {
                                tree = analysis.place_value(instance, body, *place)
                            }
                            Rvalue::Ref(_, _, place) | Rvalue::RawPtr(_, place) => {
                                let mut values: HashSet<_> = analysis
                                    .places(instance, body, *place)
                                    .into_iter()
                                    .map(|mut address| {
                                        address.pointee = address.pointee.or(Some(
                                            analysis.ty(instance, place.ty(body, tcx).ty),
                                        ));
                                        Fact::Reference(address)
                                    })
                                    .collect();
                                let referent = analysis.ty(instance, place.ty(body, tcx).ty);
                                for (index, projection) in place.projection.iter().enumerate() {
                                    if !matches!(projection, ProjectionElem::Deref) {
                                        continue;
                                    }
                                    let pointer = Place {
                                        local: place.local,
                                        projection: tcx.mk_place_elems(&place.projection[..index]),
                                    };
                                    for fact in analysis
                                        .place_value(instance, body, pointer)
                                        .values()
                                        .flatten()
                                    {
                                        if matches!(fact, Fact::Unknown)
                                            || index + 1 == place.projection.len()
                                                && (matches!(referent.kind(), ty::Dynamic(..))
                                                    && matches!(fact, Fact::Concrete(_))
                                                    || matches!(
                                                        referent.kind(),
                                                        ty::Slice(_) | ty::Str
                                                    ) && matches!(fact, Fact::Length(_)))
                                        {
                                            values.insert(fact.clone());
                                        }
                                    }
                                }
                                tree.insert(Vec::new(), values);
                            }
                            Rvalue::ThreadLocalRef(def) => {
                                tree.insert(
                                    Vec::new(),
                                    HashSet::from([Fact::Reference(Address {
                                        base: Base::Static(*def),
                                        fields: Vec::new(),
                                        byte_offset: 0,
                                        pointee: Some(analysis.ty(
                                            instance,
                                            tcx.normalize_erasing_regions(
                                                TypingEnv::fully_monomorphized(),
                                                tcx.type_of(*def).instantiate_identity(),
                                            ),
                                        )),
                                    })]),
                                );
                            }
                            Rvalue::Cast(kind, operand, destination_ty) => {
                                let source_ty = analysis.ty(instance, operand.ty(body, tcx));
                                let destination_ty = analysis.ty(instance, *destination_ty);
                                tree = analysis.operand(instance, body, operand);
                                match kind {
                                    CastKind::IntToInt => {
                                        let layouts = tcx
                                            .layout_of(
                                                TypingEnv::fully_monomorphized()
                                                    .as_query_input(source_ty),
                                            )
                                            .ok()
                                            .zip(
                                                tcx.layout_of(
                                                    TypingEnv::fully_monomorphized()
                                                        .as_query_input(destination_ty),
                                                )
                                                .ok(),
                                            );
                                        tree = Tree::from([(
                                            Vec::new(),
                                            tree.values()
                                                .flatten()
                                                .map(|fact| {
                                                    if let (
                                                        Fact::ExposedPointer(origin, width),
                                                        Some((_, destination)),
                                                    ) = (fact, layouts)
                                                    {
                                                        return if destination.size.bits()
                                                            >= tcx.data_layout.pointer_size().bits()
                                                            && *width
                                                                >= tcx
                                                                    .data_layout
                                                                    .pointer_size()
                                                                    .bits()
                                                        {
                                                            Fact::ExposedPointer(
                                                                origin.clone(),
                                                                destination.size.bits(),
                                                            )
                                                        } else {
                                                            Fact::Unknown
                                                        };
                                                    }
                                                    let (
                                                        Fact::Scalar(value),
                                                        Some((source, destination)),
                                                    ) = (fact, layouts)
                                                    else {
                                                        return Fact::Unknown;
                                                    };
                                                    let source_bits = source.size.bits();
                                                    let destination_bits = destination.size.bits();
                                                    if source_bits == 0
                                                        || source_bits > 128
                                                        || destination_bits == 0
                                                        || destination_bits > 128
                                                    {
                                                        return Fact::Unknown;
                                                    }
                                                    let value =
                                                        if matches!(source_ty.kind(), ty::Int(_))
                                                            && source_bits < destination_bits
                                                        {
                                                            (((*value << (128 - source_bits))
                                                                as i128)
                                                                >> (128 - source_bits))
                                                                as u128
                                                        } else {
                                                            *value
                                                        };
                                                    Fact::Scalar(if destination_bits == 128 {
                                                        value
                                                    } else {
                                                        value & ((1u128 << destination_bits) - 1)
                                                    })
                                                })
                                                .collect(),
                                        )]);
                                    }
                                    CastKind::FloatToInt
                                    | CastKind::IntToFloat
                                    | CastKind::FloatToFloat => {
                                        tree = Tree::from([(
                                            Vec::new(),
                                            HashSet::from([Fact::Unknown]),
                                        )]);
                                    }
                                    CastKind::PointerCoercion(
                                        PointerCoercion::ReifyFnPointer(_),
                                        _,
                                    ) => {
                                        if let ty::FnDef(def, args) = *source_ty.kind() {
                                            let args =
                                                tcx.instantiate_bound_regions_with_erased(args);
                                            let target = Instance::resolve_for_fn_ptr(
                                                tcx,
                                                TypingEnv::fully_monomorphized(),
                                                def,
                                                args,
                                            );
                                            tree.insert(
                                                Vec::new(),
                                                HashSet::from([
                                                    target.map_or(Fact::Unknown, Fact::Function)
                                                ]),
                                            );
                                        }
                                    }
                                    CastKind::PointerCoercion(
                                        PointerCoercion::ClosureFnPointer(_),
                                        _,
                                    ) => {
                                        if let ty::Closure(def, args) = *source_ty.kind() {
                                            tree.insert(
                                                Vec::new(),
                                                HashSet::from([Fact::Function(
                                                    Instance::resolve_closure(
                                                        tcx,
                                                        def,
                                                        args,
                                                        ty::ClosureKind::FnOnce,
                                                    ),
                                                )]),
                                            );
                                        }
                                    }
                                    CastKind::PointerCoercion(PointerCoercion::Unsize, _) => {
                                        if let Some(source) = source_ty.builtin_deref(true) {
                                            if let ty::Array(_, count) = source.kind() {
                                                if destination_ty.builtin_deref(true).is_some_and(
                                                    |ty| matches!(ty.kind(), ty::Slice(_)),
                                                ) {
                                                    tree.entry(Vec::new()).or_default().insert(
                                                        count
                                                            .try_to_target_usize(tcx)
                                                            .map_or(Fact::Unknown, |count| {
                                                                Fact::Length(count as u128)
                                                            }),
                                                    );
                                                }
                                            }
                                            if destination_ty.builtin_deref(true).is_some_and(
                                                |ty| matches!(ty.kind(), ty::Dynamic(..)),
                                            ) && !matches!(source.kind(), ty::Dynamic(..))
                                            {
                                                let path = if source_ty.is_box() {
                                                    vec![0, 0, 0]
                                                } else {
                                                    Vec::new()
                                                };
                                                let values = tree.entry(path).or_default();
                                                if values.remove(&Fact::Unknown) {
                                                    values.insert(Fact::Reference(Address {
                                                        base: Base::Unknown(source),
                                                        fields: Vec::new(),
                                                        byte_offset: 0,
                                                        pointee: Some(source),
                                                    }));
                                                }
                                                values.insert(Fact::Concrete(source));
                                            }
                                        }
                                    }
                                    CastKind::PointerExposeProvenance
                                    | CastKind::PointerWithExposedProvenance => {
                                        tree = analysis
                                            .provenance_cast(
                                                &tree,
                                                source_ty,
                                                destination_ty,
                                                false,
                                            )
                                            .unwrap_or_else(|| {
                                                Tree::from([(
                                                    Vec::new(),
                                                    HashSet::from([Fact::Unknown]),
                                                )])
                                            });
                                    }
                                    CastKind::Transmute if source_ty != destination_ty => {
                                        if let ty::Array(element, count) = source_ty.kind() {
                                            if *element == tcx.types.u8
                                                && destination_ty.is_integral()
                                            {
                                                let count = count.try_to_target_usize(tcx);
                                                let size = tcx
                                                    .layout_of(
                                                        TypingEnv::fully_monomorphized()
                                                            .as_query_input(destination_ty),
                                                    )
                                                    .ok()
                                                    .map(|layout| layout.size.bytes());
                                                if let Some(count) = count.filter(|count| {
                                                    Some(*count) == size
                                                        && *count > 0
                                                        && *count <= 16
                                                }) {
                                                    let mut value = 0u128;
                                                    let mut known =
                                                        !tree.get(&Vec::new()).is_some_and(
                                                            |facts| facts.contains(&Fact::Unknown),
                                                        );
                                                    for index in 0..count {
                                                        let byte = tree
                                                            .get(&vec![index as u32])
                                                            .filter(|facts| facts.len() == 1)
                                                            .and_then(|facts| facts.iter().next());
                                                        if let Some(Fact::Scalar(byte)) = byte.filter(|fact| matches!(fact, Fact::Scalar(value) if *value <= 255)) {
                                                            let position = if tcx.data_layout.endian == rustc_abi::Endian::Little { index } else { count - index - 1 };
                                                            value |= *byte << (position * 8);
                                                        } else { known = false; }
                                                    }
                                                    tree = Tree::from([(
                                                        Vec::new(),
                                                        HashSet::from([if known {
                                                            Fact::Scalar(value)
                                                        } else {
                                                            Fact::Unknown
                                                        }]),
                                                    )]);
                                                    analysis.write_place(
                                                        instance,
                                                        body,
                                                        *destination,
                                                        &tree,
                                                    );
                                                    continue;
                                                }
                                            }
                                        }
                                        if let ty::Array(element, count) = destination_ty.kind() {
                                            if *element == tcx.types.u8 && source_ty.is_integral() {
                                                let count = count.try_to_target_usize(tcx);
                                                let size = tcx
                                                    .layout_of(
                                                        TypingEnv::fully_monomorphized()
                                                            .as_query_input(source_ty),
                                                    )
                                                    .ok()
                                                    .map(|layout| layout.size.bytes());
                                                if let Some(count) = count.filter(|count| {
                                                    Some(*count) == size
                                                        && *count > 0
                                                        && *count <= 16
                                                }) {
                                                    let mut converted = Tree::new();
                                                    for index in 0..count {
                                                        let position = if tcx.data_layout.endian
                                                            == rustc_abi::Endian::Little
                                                        {
                                                            index
                                                        } else {
                                                            count - index - 1
                                                        };
                                                        let facts = tree
                                                            .get(&Vec::new())
                                                            .into_iter()
                                                            .flatten()
                                                            .map(|fact| match fact {
                                                                Fact::Scalar(value) => {
                                                                    Fact::Scalar(
                                                                        (value >> (position * 8))
                                                                            & 255,
                                                                    )
                                                                }
                                                                _ => Fact::Unknown,
                                                            })
                                                            .collect();
                                                        converted.insert(vec![index as u32], facts);
                                                    }
                                                    analysis.write_place(
                                                        instance,
                                                        body,
                                                        *destination,
                                                        &converted,
                                                    );
                                                    continue;
                                                }
                                            }
                                        }
                                        if let Some(converted) = analysis.provenance_cast(
                                            &tree,
                                            source_ty,
                                            destination_ty,
                                            true,
                                        ) {
                                            analysis.write_place(
                                                instance,
                                                body,
                                                *destination,
                                                &converted,
                                            );
                                            continue;
                                        }
                                        if let (
                                            Some((source_path, source)),
                                            Some((destination_path, target)),
                                        ) = (
                                            analysis.pointer_layout(source_ty),
                                            analysis.pointer_layout(destination_ty),
                                        ) {
                                            let mut values = HashSet::new();
                                            for (path, facts) in &tree {
                                                if *path == source_path {
                                                    values.extend(facts.iter().cloned());
                                                } else if source_path.starts_with(path)
                                                    && facts.contains(&Fact::Unknown)
                                                {
                                                    values.insert(Fact::Unknown);
                                                }
                                            }
                                            let values =
                                                analysis.cast_pointer(values, source, target);
                                            tree = Tree::from([(destination_path, values)]);
                                            analysis.write_place(
                                                instance,
                                                body,
                                                *destination,
                                                &tree,
                                            );
                                            continue;
                                        }
                                        if source_ty.is_box() && destination_ty.is_box() {
                                            if let Some(path) = source_ty
                                                .builtin_deref(true)
                                                .zip(destination_ty.builtin_deref(true))
                                                .and_then(|(source, destination)| {
                                                    analysis.inner_path(source, destination, 0)
                                                })
                                            {
                                                for values in tree.values_mut() {
                                                    *values = values
                                                        .drain()
                                                        .map(|fact| match fact {
                                                            Fact::Reference(mut address) => {
                                                                address.fields.extend(&path);
                                                                address.pointee = destination_ty
                                                                    .builtin_deref(true);
                                                                Fact::Reference(address)
                                                            }
                                                            other => other,
                                                        })
                                                        .collect();
                                                }
                                                analysis.write_place(
                                                    instance,
                                                    body,
                                                    *destination,
                                                    &tree,
                                                );
                                                continue;
                                            }
                                        }
                                        if let Some(path) =
                                            analysis.inner_path(source_ty, destination_ty, 0)
                                        {
                                            let mut projected = Tree::new();
                                            for (fields, facts) in tree {
                                                if fields.len() < path.len()
                                                    && path.starts_with(&fields)
                                                    && facts.contains(&Fact::Unknown)
                                                {
                                                    projected
                                                        .entry(Vec::new())
                                                        .or_default()
                                                        .insert(Fact::Unknown);
                                                } else if let Some(fields) =
                                                    fields.strip_prefix(path.as_slice())
                                                {
                                                    projected
                                                        .entry(fields.to_vec())
                                                        .or_default()
                                                        .extend(facts);
                                                }
                                            }
                                            tree = projected;
                                            analysis.write_place(
                                                instance,
                                                body,
                                                *destination,
                                                &tree,
                                            );
                                            continue;
                                        }
                                        if let Some(path) =
                                            analysis.inner_path(destination_ty, source_ty, 0)
                                        {
                                            tree = tree
                                                .into_iter()
                                                .map(|(fields, facts)| {
                                                    let mut output = path.clone();
                                                    output.extend(fields);
                                                    (output, facts)
                                                })
                                                .collect();
                                            analysis.write_place(
                                                instance,
                                                body,
                                                *destination,
                                                &tree,
                                            );
                                            continue;
                                        }
                                        let typed_pointer =
                                            matches!(source_ty.kind(), ty::FnPtr(..))
                                                && matches!(destination_ty.kind(), ty::FnPtr(..));
                                        if !typed_pointer {
                                            if !matches!(
                                                source_ty.kind(),
                                                ty::FnPtr(..) | ty::Int(_) | ty::Uint(_)
                                            ) || !matches!(
                                                destination_ty.kind(),
                                                ty::FnPtr(..) | ty::Int(_) | ty::Uint(_)
                                            ) {
                                                for facts in tree.values_mut() {
                                                    facts.retain(|fact| {
                                                        matches!(fact, Fact::Function(_))
                                                    });
                                                }
                                                tree.retain(|_, facts| !facts.is_empty());
                                            }
                                            tree.entry(Vec::new())
                                                .or_default()
                                                .insert(Fact::Unknown);
                                            if matches!(
                                                destination_ty.kind(),
                                                ty::FnPtr(..) | ty::Dynamic(..)
                                            ) {
                                                edges.push(Edge { site: None,
                                                        target: None,
                                                        span: statement.source_info.span,
                                                        kind: "raw-address conversion",
                                                        detail: "the transmute has no known typed call target".into(),
                                                        trusted: trust.allows(tcx, instance, body, statement.source_info.scope, false),
                                                    });
                                            }
                                        }
                                    }
                                    CastKind::PtrToPtr
                                        if source_ty.builtin_deref(true)
                                            != destination_ty.builtin_deref(true) =>
                                    {
                                        if let Some((source, target)) = source_ty
                                            .builtin_deref(true)
                                            .zip(destination_ty.builtin_deref(true))
                                        {
                                            for values in tree.values_mut() {
                                                *values = analysis.cast_pointer(
                                                    std::mem::take(values),
                                                    source,
                                                    target,
                                                );
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            Rvalue::Aggregate(kind, operands) => {
                                let variant = match &**kind {
                                    AggregateKind::Adt(def, variant, _, _, _)
                                        if tcx.adt_def(*def).is_enum() =>
                                    {
                                        tree.insert(
                                            vec![DISCRIMINANT],
                                            HashSet::from([Fact::Scalar(
                                                tcx.adt_def(*def)
                                                    .discriminant_for_variant(tcx, *variant)
                                                    .val,
                                            )]),
                                        );
                                        Some(variant.as_u32())
                                    }
                                    _ => None,
                                };
                                for (field, operand) in operands.iter_enumerated() {
                                    if matches!(&**kind, AggregateKind::RawPtr(..))
                                        && field.as_usize() != 0
                                    {
                                        let metadata_ty =
                                            analysis.ty(instance, operand.ty(body, tcx));
                                        if !metadata_ty.is_unit() {
                                            let metadata =
                                                analysis.operand(instance, body, operand);
                                            let values = tree.entry(Vec::new()).or_default();
                                            for fact in metadata.values().flatten() {
                                                values.insert(match fact {
                                                    Fact::Scalar(value)
                                                        if metadata_ty.is_integral() =>
                                                    {
                                                        Fact::Length(*value)
                                                    }
                                                    Fact::Concrete(_) | Fact::Length(_) => {
                                                        fact.clone()
                                                    }
                                                    _ => Fact::Unknown,
                                                });
                                            }
                                        }
                                        continue;
                                    }
                                    let field = match &**kind {
                                        AggregateKind::Array(_) => field.as_u32(),
                                        AggregateKind::Adt(_, _, _, _, Some(field)) => {
                                            field.as_u32()
                                        }
                                        _ => field.as_u32(),
                                    };
                                    for (path, mut values) in
                                        analysis.operand(instance, body, operand)
                                    {
                                        if let AggregateKind::RawPtr(pointee, _) = &**kind {
                                            let pointee = analysis.ty(instance, *pointee);
                                            values = values
                                                .into_iter()
                                                .map(|fact| match fact {
                                                    Fact::Reference(mut address) => {
                                                        address.pointee = Some(pointee);
                                                        Fact::Reference(address)
                                                    }
                                                    other => other,
                                                })
                                                .collect();
                                        }
                                        let mut output =
                                            if matches!(&**kind, AggregateKind::RawPtr(..)) {
                                                Vec::new()
                                            } else {
                                                let mut path = variant
                                                    .map_or_else(Vec::new, |variant| {
                                                        vec![VARIANT, variant]
                                                    });
                                                path.push(field);
                                                path
                                            };
                                        output.extend(path);
                                        tree.entry(output).or_default().extend(values);
                                    }
                                }
                            }
                            Rvalue::Repeat(operand, _) => {
                                for (path, values) in analysis.operand(instance, body, operand) {
                                    let mut output = vec![u32::MAX];
                                    output.extend(path);
                                    tree.insert(output, values);
                                }
                            }
                            Rvalue::BinaryOp(mir::BinOp::Offset, operands) => {
                                let pointers = analysis.operand(instance, body, &operands.0);
                                let counts = analysis.operand(instance, body, &operands.1);
                                let element = analysis
                                    .ty(instance, operands.0.ty(body, tcx))
                                    .builtin_deref(true);
                                tree = analysis.offset_pointer(
                                    pointers,
                                    &counts,
                                    analysis.ty(instance, operands.1.ty(body, tcx)),
                                    element,
                                    false,
                                );
                            }
                            Rvalue::BinaryOp(operation, operands) => {
                                let left = analysis.operand(instance, body, &operands.0);
                                let right = analysis.operand(instance, body, &operands.1);
                                if !analysis.settling_unknowns
                                    && (left.values().all(HashSet::is_empty)
                                        || right.values().all(HashSet::is_empty))
                                {
                                    continue 'states;
                                }
                                let operand_ty = analysis.ty(instance, operands.0.ty(body, tcx));
                                let bits = tcx
                                    .layout_of(
                                        TypingEnv::fully_monomorphized().as_query_input(operand_ty),
                                    )
                                    .ok()
                                    .map(|layout| layout.size.bits());
                                let overflow_result = matches!(
                                    operation,
                                    mir::BinOp::AddWithOverflow
                                        | mir::BinOp::SubWithOverflow
                                        | mir::BinOp::MulWithOverflow
                                );
                                let output_path =
                                    if overflow_result { vec![0] } else { Vec::new() };
                                if left.is_empty() || right.is_empty() {
                                    tree.entry(output_path.clone())
                                        .or_default()
                                        .insert(Fact::Unknown);
                                    if overflow_result {
                                        tree.entry(vec![1]).or_default().insert(Fact::Unknown);
                                    }
                                }
                                for left in left.values().flatten() {
                                    for right in right.values().flatten() {
                                        let mut result = None;
                                        let mut overflow = false;
                                        if matches!(operation, mir::BinOp::Eq | mir::BinOp::Ne)
                                            && bits == Some(tcx.data_layout.pointer_size().bits())
                                        {
                                            if let (Fact::Reference(left), Fact::Reference(right)) =
                                                (left, right)
                                            {
                                                let same_storage = match (&left.base, &right.base) {
                                                    (
                                                        Base::Allocation(left, _),
                                                        Base::Allocation(right, _),
                                                    ) => left == right,
                                                    (Base::Static(left), Base::Static(right)) => {
                                                        left == right
                                                    }
                                                    _ => false,
                                                };
                                                if same_storage {
                                                    if let (Some((_, left)), Some((_, right))) = (
                                                        analysis.storage_position(left),
                                                        analysis.storage_position(right),
                                                    ) {
                                                        let width =
                                                            tcx.data_layout.pointer_size().bits();
                                                        let mask = if width == 128 {
                                                            u128::MAX
                                                        } else {
                                                            (1u128 << width) - 1
                                                        };
                                                        let equal = left as u128 & mask
                                                            == right as u128 & mask;
                                                        result = Some(
                                                            (equal
                                                                == matches!(
                                                                    operation,
                                                                    mir::BinOp::Eq
                                                                ))
                                                                as u128,
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                        if matches!(operation, mir::BinOp::BitAnd) {
                                            let pointer_mask = match (left, right) {
                                                (
                                                    Fact::ExposedPointer(
                                                        PointerOrigin::Storage(address),
                                                        _,
                                                    ),
                                                    Fact::Scalar(mask),
                                                )
                                                | (
                                                    Fact::Scalar(mask),
                                                    Fact::ExposedPointer(
                                                        PointerOrigin::Storage(address),
                                                        _,
                                                    ),
                                                ) => Some((address, *mask)),
                                                _ => None,
                                            };
                                            if let Some((address, mask)) = pointer_mask {
                                                if let Some((alignment, offset)) =
                                                    analysis.storage_position(address)
                                                {
                                                    if mask & !(u128::from(alignment) - 1) == 0 {
                                                        result = Some(offset as u128 & mask);
                                                    }
                                                }
                                            }
                                        }
                                        if let (
                                            Fact::Scalar(left),
                                            Fact::Scalar(right),
                                            Some(bits),
                                        ) = (left, right, bits)
                                        {
                                            if bits > 0
                                                && bits <= 128
                                                && matches!(
                                                    operand_ty.kind(),
                                                    ty::Int(_) | ty::Uint(_) | ty::Bool | ty::Char
                                                )
                                            {
                                                let mask = if bits == 128 {
                                                    u128::MAX
                                                } else {
                                                    (1u128 << bits) - 1
                                                };
                                                let a = *left & mask;
                                                let b = *right & mask;
                                                let signed =
                                                    matches!(operand_ty.kind(), ty::Int(_));
                                                let sa =
                                                    ((a << (128 - bits)) as i128) >> (128 - bits);
                                                let sb =
                                                    ((b << (128 - bits)) as i128) >> (128 - bits);
                                                let ordering =
                                                    if signed { sa.cmp(&sb) } else { a.cmp(&b) };
                                                result = match operation {
                                                    mir::BinOp::Eq => Some((a == b) as u128),
                                                    mir::BinOp::Ne => Some((a != b) as u128),
                                                    mir::BinOp::Lt => {
                                                        Some(ordering.is_lt() as u128)
                                                    }
                                                    mir::BinOp::Le => {
                                                        Some(ordering.is_le() as u128)
                                                    }
                                                    mir::BinOp::Gt => {
                                                        Some(ordering.is_gt() as u128)
                                                    }
                                                    mir::BinOp::Ge => {
                                                        Some(ordering.is_ge() as u128)
                                                    }
                                                    mir::BinOp::BitAnd => Some(a & b),
                                                    mir::BinOp::BitOr => Some(a | b),
                                                    mir::BinOp::BitXor => Some(a ^ b),
                                                    mir::BinOp::Shl | mir::BinOp::ShlUnchecked
                                                        if *right < bits as u128 =>
                                                    {
                                                        Some((a << *right) & mask)
                                                    }
                                                    mir::BinOp::Shr | mir::BinOp::ShrUnchecked
                                                        if *right < bits as u128 =>
                                                    {
                                                        Some(if signed {
                                                            (sa >> *right) as u128 & mask
                                                        } else {
                                                            a >> *right
                                                        })
                                                    }
                                                    mir::BinOp::Div if b != 0 => {
                                                        if signed {
                                                            sa.checked_div(sb)
                                                                .map(|value| value as u128 & mask)
                                                        } else {
                                                            Some(a / b)
                                                        }
                                                    }
                                                    mir::BinOp::Rem if b != 0 => {
                                                        if signed {
                                                            sa.checked_rem(sb)
                                                                .map(|value| value as u128 & mask)
                                                        } else {
                                                            Some(a % b)
                                                        }
                                                    }
                                                    mir::BinOp::Add
                                                    | mir::BinOp::AddUnchecked
                                                    | mir::BinOp::AddWithOverflow
                                                    | mir::BinOp::Sub
                                                    | mir::BinOp::SubUnchecked
                                                    | mir::BinOp::SubWithOverflow
                                                    | mir::BinOp::Mul
                                                    | mir::BinOp::MulUnchecked
                                                    | mir::BinOp::MulWithOverflow => {
                                                        let addition = matches!(
                                                            operation,
                                                            mir::BinOp::Add
                                                                | mir::BinOp::AddUnchecked
                                                                | mir::BinOp::AddWithOverflow
                                                        );
                                                        let subtraction = matches!(
                                                            operation,
                                                            mir::BinOp::Sub
                                                                | mir::BinOp::SubUnchecked
                                                                | mir::BinOp::SubWithOverflow
                                                        );
                                                        let (value, wide_overflow) = if addition {
                                                            a.overflowing_add(b)
                                                        } else if subtraction {
                                                            a.overflowing_sub(b)
                                                        } else {
                                                            a.overflowing_mul(b)
                                                        };
                                                        overflow = if signed {
                                                            let wide = if addition {
                                                                sa.checked_add(sb)
                                                            } else if subtraction {
                                                                sa.checked_sub(sb)
                                                            } else {
                                                                sa.checked_mul(sb)
                                                            };
                                                            wide.is_none_or(|value| {
                                                                bits < 128
                                                                    && (value
                                                                        < -(1i128 << (bits - 1))
                                                                        || value
                                                                            >= 1i128 << (bits - 1))
                                                            })
                                                        } else {
                                                            wide_overflow || value > mask
                                                        };
                                                        Some(value & mask)
                                                    }
                                                    _ => None,
                                                };
                                            }
                                        }
                                        tree.entry(output_path.clone())
                                            .or_default()
                                            .insert(result.map_or(Fact::Unknown, Fact::Scalar));
                                        if overflow_result {
                                            tree.entry(vec![1]).or_default().insert(
                                                result.map_or(Fact::Unknown, |_| {
                                                    Fact::Scalar(overflow as u128)
                                                }),
                                            );
                                        }
                                    }
                                }
                            }
                            Rvalue::UnaryOp(mir::UnOp::PtrMetadata, operand) => {
                                for facts in analysis.operand(instance, body, operand).values() {
                                    tree.entry(Vec::new()).or_default().extend(
                                        facts.iter().filter_map(|fact| match fact {
                                            Fact::Concrete(_) | Fact::Unknown => Some(fact.clone()),
                                            Fact::Length(value) => Some(Fact::Scalar(*value)),
                                            _ => None,
                                        }),
                                    );
                                }
                            }
                            Rvalue::UnaryOp(mir::UnOp::Not, operand)
                                if analysis.ty(instance, operand.ty(body, tcx)).is_bool() =>
                            {
                                for fact in
                                    analysis.operand(instance, body, operand).values().flatten()
                                {
                                    tree.entry(Vec::new()).or_default().insert(match fact {
                                        Fact::Scalar(value) if *value <= 1 => {
                                            Fact::Scalar(1 - value)
                                        }
                                        _ => Fact::Unknown,
                                    });
                                }
                            }
                            Rvalue::UnaryOp(_, _) => {
                                tree.entry(Vec::new()).or_default().insert(Fact::Unknown);
                            }
                            Rvalue::Discriminant(place) => {
                                let value = analysis.place_value(instance, body, *place);
                                if !analysis.settling_unknowns
                                    && value.values().all(HashSet::is_empty)
                                {
                                    continue 'states;
                                }
                                let mut facts =
                                    value.get(&vec![DISCRIMINANT]).cloned().unwrap_or_default();
                                if value
                                    .get(&Vec::new())
                                    .is_some_and(|facts| facts.contains(&Fact::Unknown))
                                    || facts.is_empty()
                                {
                                    facts.insert(Fact::Unknown);
                                }
                                tree.insert(Vec::new(), facts);
                            }
                        }
                        analysis.write_place(instance, body, *destination, &tree);
                    } else if let StatementKind::SetDiscriminant {
                        place,
                        variant_index,
                    } = &statement.kind
                    {
                        let ty = analysis.ty(instance, place.ty(body, tcx).ty);
                        if let ty::Adt(adt, _) = ty.kind() {
                            let values = Tree::from([(
                                vec![DISCRIMINANT],
                                HashSet::from([Fact::Scalar(
                                    adt.discriminant_for_variant(tcx, *variant_index).val,
                                )]),
                            )]);
                            for mut address in analysis.places(instance, body, **place) {
                                address.fields.push(DISCRIMINANT);
                                let previous = analysis.strong_update;
                                analysis.strong_update = place.projection.is_empty();
                                analysis.write(
                                    &address,
                                    &Tree::from([(
                                        Vec::new(),
                                        values[&vec![DISCRIMINANT]].clone(),
                                    )]),
                                );
                                analysis.strong_update = previous;
                            }
                        }
                    } else if let StatementKind::Intrinsic(intrinsic) = &statement.kind {
                        if let mir::NonDivergingIntrinsic::CopyNonOverlapping(copy) = &**intrinsic {
                            let sources = analysis.operand(instance, body, &copy.src);
                            let destinations = analysis.operand(instance, body, &copy.dst);
                            let element = analysis
                                .ty(instance, copy.src.ty(body, tcx))
                                .builtin_deref(true);
                            let count = analysis.operand(instance, body, &copy.count);
                            let previous_effects = analysis.record_effects;
                            analysis.record_effects = true;
                            analysis.copy_memory(&sources, &destinations, element, &count);
                            analysis.record_effects = previous_effects;
                        }
                    }
                    if edges.len() != first_edge {
                        analysis.record_site(
                        node,
                        body,
                        mir::Location {
                            block,
                            statement_index,
                        },
                        incoming.expect("a statement call edge requires its input state"),
                        &mut edges[first_edge..],
                        );
                    }
                }
                let terminator = data.terminator();
                if trust.allows(tcx, instance, body, terminator.source_info.scope, true) {
                    continue;
                }
                let current = mir::Location {
                    block,
                    statement_index: data.statements.len(),
                };
                analysis.active_location = current;
                analysis.pending_bindings.clear();
                if node.context != 0 && !widened.keys().any(|(location, _)| *location == current) {
                    let mut indexed = IndexedPlaces::default();
                    indexed.visit_terminator(terminator, current);
                    if let Some(alternatives) = analysis.index_states(node, body, indexed.0) {
                        pending.extend(alternatives.into_iter().map(|state| (current, state)));
                        continue 'states;
                    }
                    if let TerminatorKind::Call {
                        args, destination, ..
                    } = &terminator.kind
                    {
                        if analysis
                            .ty(instance, destination.ty(body, tcx).ty)
                            .is_bool()
                        {
                            for argument in args {
                                let values = analysis.operand(instance, body, &argument.node);
                                for fact in values.values().flatten() {
                                    let Fact::Reference(address) = fact else {
                                        continue;
                                    };
                                    if !address
                                        .pointee
                                        .is_some_and(|ty| analysis.can_replace(address, ty))
                                    {
                                        continue;
                                    }
                                    for (path, facts) in analysis.read(address) {
                                        let alternatives: Vec<_> = facts
                                            .iter()
                                            .filter(|fact| {
                                                matches!(
                                                    fact,
                                                    Fact::Reference(_)
                                                        | Fact::Function(_)
                                                        | Fact::ExposedPointer(..)
                                                        | Fact::IntegerPointer(_)
                                                        | Fact::Unknown
                                                )
                                            })
                                            .cloned()
                                            .collect();
                                        if alternatives.len() < 2 {
                                            continue;
                                        }
                                        let mut storage_path = address.fields.clone();
                                        storage_path.extend(path);
                                        if storage_path.contains(&u32::MAX) {
                                            continue;
                                        }
                                        let state = analysis.flow_locals.as_ref().unwrap();
                                        if state
                                            .get(&address.base)
                                            .and_then(|tree| tree.get(&storage_path))
                                            .is_none()
                                        {
                                            continue;
                                        }
                                        for alternative in &alternatives {
                                            let mut refined = state.clone();
                                            let selected = refined
                                                .get_mut(&address.base)
                                                .unwrap()
                                                .get_mut(&storage_path)
                                                .unwrap();
                                            selected.retain(|fact| !alternatives.contains(fact));
                                            selected.insert(alternative.clone());
                                            pending.push_back((current, refined));
                                        }
                                        continue 'states;
                                    }
                                }
                            }
                        }
                    }
                }
                let trusted =
                    trust.allows(tcx, instance, body, terminator.source_info.scope, false);
                let span = terminator.source_info.span;
                let first_edge = edges.len();
                let incoming = matches!(&terminator.kind,
                    TerminatorKind::Call { .. } | TerminatorKind::TailCall { .. }
                        | TerminatorKind::Drop { .. } | TerminatorKind::Assert { .. }
                        | TerminatorKind::InlineAsm { .. })
                    .then(|| analysis.flow_locals.clone().unwrap_or_default());
                let mut call_states = None;
                let mut unwind_states = Vec::new();
                match &terminator.kind {
                    TerminatorKind::Call { func, args, .. }
                    | TerminatorKind::TailCall { func, args, .. } => {
                        call_states = Some(Vec::new());
                        let entry_state = analysis.flow_locals.clone();
                        let callee_ty = analysis.ty(instance, func.ty(body, tcx));
                        let mut targets = HashSet::new();
                        let mut unknown = false;
                        let mut virtual_call = false;
                        if let ty::FnDef(def, generic_args) = *callee_ty.kind() {
                            let generic_args =
                                tcx.instantiate_bound_regions_with_erased(generic_args);
                            let resolved = Instance::try_resolve(
                                tcx,
                                TypingEnv::fully_monomorphized(),
                                def,
                                generic_args,
                            )
                            .ok()
                            .flatten();
                            if let Some(target) = resolved {
                                if forbidden(tcx, target).is_some() {
                                    targets.insert(target);
                                } else if matches!(target.def, InstanceKind::Virtual(..)) {
                                    virtual_call = true;
                                    if let Some(receiver) = args.first() {
                                        let values =
                                            analysis.operand(instance, body, &receiver.node);
                                        for fact in values.values().flatten() {
                                            if let Fact::Concrete(concrete) = fact {
                                                let args = tcx.mk_args_from_iter(
                                                    generic_args.iter().enumerate().map(
                                                        |(index, arg)| {
                                                            if index == 0 {
                                                                (*concrete).into()
                                                            } else {
                                                                arg
                                                            }
                                                        },
                                                    ),
                                                );
                                                if let Some(target) = Instance::try_resolve(
                                                    tcx,
                                                    TypingEnv::fully_monomorphized(),
                                                    def,
                                                    args,
                                                )
                                                .ok()
                                                .flatten()
                                                .filter(|target| {
                                                    !matches!(target.def, InstanceKind::Virtual(..))
                                                }) {
                                                    targets.insert(target);
                                                } else {
                                                    unknown = true;
                                                }
                                            } else if matches!(fact, Fact::Unknown) {
                                                unknown = true;
                                            }
                                        }
                                    }
                                } else {
                                    targets.insert(target);
                                }
                            } else {
                                unknown = true;
                            }
                        } else {
                            for fact in analysis.operand(instance, body, func).values().flatten() {
                                match fact {
                                    Fact::Function(target) => {
                                        targets.insert(*target);
                                    }
                                    Fact::Unknown
                                    | Fact::IntegerPointer(_)
                                    | Fact::ExposedPointer(..) => unknown = true,
                                    _ => {}
                                }
                            }
                        }
                        if targets.is_empty() && analysis.settling_unknowns {
                            unknown = true;
                        }
                        if unknown {
                            if let Some(candidates) = analysis.backend_calls.get(&(instance, location)) {
                                targets.extend(candidates.iter().copied());
                            }
                        }
                        if targets.is_empty() || unknown {
                            edges.push(Edge { site: None,
                                target: None,
                                span,
                                kind: "unresolved call",
                                detail: format!("call target is not fully known: {callee_ty}; alternatives: {:?}", analysis.operand(instance, body, func)),
                                trusted,
                            });
                        }
                        let destination = match &terminator.kind {
                            TerminatorKind::Call { destination, .. } => Some(*destination),
                            TerminatorKind::TailCall { .. } => Some(Place::from(mir::RETURN_PLACE)),
                            _ => None,
                        };
                        for mut target in targets {
                            analysis.flow_locals = entry_state.clone();
                            let mut validity_panic = false;
                            if matches!(target.def, InstanceKind::Intrinsic(_)) {
                                if let Some(requirement) = tcx
                                    .opt_item_name(target.def_id())
                                    .and_then(ty::layout::ValidityRequirement::from_intrinsic)
                                {
                                    if tcx
                                        .check_validity_requirement((
                                            requirement,
                                            TypingEnv::fully_monomorphized()
                                                .as_query_input(target.args.type_at(0)),
                                        ))
                                        .is_ok_and(|valid| !valid)
                                    {
                                        if let Some(panic) =
                                            tcx.lang_items().get(LangItem::PanicNounwind)
                                        {
                                            target = Instance::mono(tcx, panic);
                                            validity_panic = true;
                                        }
                                    }
                                }
                            }
                            let intrinsic = known_intrinsic(tcx, target);
                            let mut linked = false;
                            if tcx.is_foreign_item(target.def_id())
                                && trust.checks(target.def_id())
                                && forbidden(tcx, target).is_none()
                            {
                                if let Some(function) = exported.get(tcx.symbol_name(target).name) {
                                    target = *function;
                                    linked = true;
                                }
                            }
                            let opaque = !trust.checks(target.def_id())
                                || tcx.is_foreign_item(target.def_id())
                                || matches!(
                                    target.def,
                                    InstanceKind::Intrinsic(..) | InstanceKind::Virtual(..)
                                )
                                || !tcx.is_mir_available(target.def_id())
                                    && matches!(target.def, InstanceKind::Item(..));
                            let mut arguments: Vec<_> = args
                                .iter()
                                .map(|argument| {
                                    let ty = analysis.ty(instance, argument.node.ty(body, tcx));
                                    let values = analysis.operand(instance, body, &argument.node);
                                    (ty, values)
                                })
                                .collect();
                            if linked || matches!(callee_ty.kind(), ty::FnPtr(..)) {
                                let compatible = irq_check_rupta::util::type_util::call_abi_compatible(
                                    tcx, callee_ty.fn_sig(tcx), target);
                                if !compatible {
                                    edges.push(Edge {
                                        site: None,
                                        target: None,
                                        span,
                                        kind: "unsupported call ABI",
                                        detail: format!(
                                            "cannot bind arguments from {callee_ty} to the original callable {target} under the selected target ABI"
                                        ),
                                        trusted,
                                    });
                                    edges.extend(
                                        analysis.opaque_effects(&arguments, span, true, true, true),
                                    );
                                    for (_, values) in &mut arguments {
                                        *values = Tree::from([(
                                            Vec::new(),
                                            HashSet::from([Fact::Unknown]),
                                        )]);
                                    }
                                }
                            }
                            if validity_panic {
                                arguments = tcx
                                    .fn_sig(target.def_id())
                                    .instantiate(tcx, target.args)
                                    .skip_binder()
                                    .inputs()
                                    .iter()
                                    .map(|ty| {
                                        (
                                            analysis.ty(target, *ty),
                                            Tree::from([(
                                                Vec::new(),
                                                HashSet::from([Fact::Unknown]),
                                            )]),
                                        )
                                    })
                                    .collect();
                            }
                            if virtual_call && !opaque && !arguments.is_empty() {
                                let target_body = tcx.instance_mir(target.def);
                                if target_body.arg_count != 0 {
                                    let parameter_ty = analysis.ty(
                                        target,
                                        target_body.local_decls[mir::Local::from_usize(1)].ty,
                                    );
                                    if let (Some((path, source)), Some((_, concrete))) = (
                                        analysis.pointer_layout(arguments[0].0),
                                        analysis.pointer_layout(parameter_ty),
                                    ) {
                                        if matches!(source.kind(), ty::Dynamic(..))
                                            && !matches!(concrete.kind(), ty::Dynamic(..))
                                        {
                                            let values = arguments[0].1.entry(path).or_default();
                                            let unknown_data = values.remove(&Fact::Unknown);
                                            values.retain(|fact| match fact {
                                                Fact::Concrete(ty) => *ty == concrete,
                                                Fact::Reference(address) => {
                                                    address.pointee.is_none_or(|ty| {
                                                        ty == concrete
                                                            || matches!(ty.kind(), ty::Dynamic(..))
                                                    })
                                                }
                                                _ => true,
                                            });
                                            if unknown_data
                                                || !values
                                                    .iter()
                                                    .any(|fact| matches!(fact, Fact::Reference(_)))
                                            {
                                                values.insert(Fact::Reference(Address {
                                                    base: Base::Unknown(concrete),
                                                    fields: Vec::new(),
                                                    byte_offset: 0,
                                                    pointee: Some(concrete),
                                                }));
                                            }
                                        }
                                    }
                                }
                            }
                            let previous_effects = analysis.record_effects;
                            analysis.record_effects = true;
                            let projection = if !opaque
                                && matches!(target.def, InstanceKind::Item(_))
                                && !crate::context_method(tcx, target.def_id())
                                && !roots.contains(&target)
                            {
                                analysis.return_projections.entry(target).or_insert_with(|| {
                                    let body = tcx.instance_mir(target.def);
                                    if body.basic_blocks.len() != 1 {
                                        return None;
                                    }
                                    let block = &body.basic_blocks[mir::START_BLOCK];
                                    if !matches!(block.terminator().kind, TerminatorKind::Return)
                                        || trust.allows(tcx, target, body, block.terminator().source_info.scope, true)
                                    {
                                        return None;
                                    }
                                    let mut values = vec![None; body.local_decls.len()];
                                    for argument in 0..body.arg_count {
                                        values[argument + 1] = Some((argument, Vec::new(), None));
                                    }
                                    for statement in &block.statements {
                                        if trust.allows(tcx, target, body, statement.source_info.scope, true) {
                                            return None;
                                        }
                                        match &statement.kind {
                                            StatementKind::Assign(assignment) => {
                                                let (destination, value) = &**assignment;
                                                if !destination.projection.is_empty() {
                                                    return None;
                                                }
                                                let (operand, cast) = match value {
                                                    Rvalue::Use(operand, _) => (operand, None),
                                                    Rvalue::Cast(kind @ (CastKind::PtrToPtr | CastKind::Transmute), operand, _) => (operand, Some(*kind)),
                                                    _ => return None,
                                                };
                                                let (Operand::Copy(source) | Operand::Move(source)) = operand else {
                                                    return None;
                                                };
                                                if matches!(operand, Operand::Move(_)) && !source.projection.is_empty() {
                                                    return None;
                                                }
                                                let (argument, mut fields, mut pointer_view) = values[source.local.as_usize()].clone()?;
                                                let mut source_ty = body.local_decls[source.local].ty;
                                                for projection in source.projection {
                                                    if matches!(source_ty.kind(), ty::Adt(adt, _) if adt.is_union()) {
                                                        return None;
                                                    }
                                                    let ProjectionElem::Field(field, ty) = projection else {
                                                        return None;
                                                    };
                                                    fields.push(field.as_u32());
                                                    source_ty = ty;
                                                }
                                                if let Some(cast) = cast {
                                                    let source_ty = target.instantiate_mir_and_normalize_erasing_regions(
                                                        tcx, TypingEnv::fully_monomorphized(), EarlyBinder::bind(tcx, source_ty));
                                                    let destination_ty = target.instantiate_mir_and_normalize_erasing_regions(
                                                        tcx, TypingEnv::fully_monomorphized(), EarlyBinder::bind(tcx, destination.ty(body, tcx).ty));
                                                    let ty::RawPtr(destination_pointee, _) = destination_ty.kind() else {
                                                        return None;
                                                    };
                                                    let mut pointer_ty = match source_ty.kind() {
                                                        ty::RawPtr(..) => source_ty,
                                                        ty::Adt(adt, args)
                                                            if matches!(cast, CastKind::Transmute)
                                                                && adt.is_struct()
                                                                && adt.repr().transparent()
                                                                && adt.non_enum_variant().fields.len() == 1 =>
                                                        {
                                                            let layout = tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(source_ty)).ok()?;
                                                            let destination_layout = tcx.layout_of(TypingEnv::fully_monomorphized().as_query_input(destination_ty)).ok()?;
                                                            if layout.size != destination_layout.size || layout.fields.offset(0).bytes() != 0 {
                                                                return None;
                                                            }
                                                            fields.push(0);
                                                            tcx.normalize_erasing_regions(TypingEnv::fully_monomorphized(),
                                                                adt.non_enum_variant().fields[rustc_abi::FieldIdx::from_usize(0)].ty(tcx, args))
                                                        }
                                                        _ => return None,
                                                    };
                                                    while let ty::Pat(inner, _) = pointer_ty.kind() {
                                                        pointer_ty = *inner;
                                                    }
                                                    if !matches!(pointer_ty.kind(), ty::RawPtr(source_pointee, _) if source_pointee == destination_pointee) {
                                                        return None;
                                                    }
                                                    if matches!(cast, CastKind::Transmute) {
                                                        pointer_view = Some(*destination_pointee);
                                                    }
                                                }
                                                if matches!(operand, Operand::Move(_)) {
                                                    values[source.local.as_usize()] = None;
                                                }
                                                values[destination.local.as_usize()] = Some((argument, fields, pointer_view));
                                            }
                                            StatementKind::StorageLive(local) | StatementKind::StorageDead(local) => {
                                                values[local.as_usize()] = None;
                                            }
                                            StatementKind::Nop => {}
                                            _ => return None,
                                        }
                                    }
                                    values[mir::RETURN_PLACE.as_usize()].clone()
                                }).clone()
                            } else {
                                None
                            };
                            let mut projection_applied = false;
                            let storage = destination.and_then(|destination| {
                                let result_ty = analysis.ty(instance, destination.ty(body, tcx).ty);
                                if let Some((argument, fields, pointer_view)) = &projection {
                                    if let Some((_, values)) = arguments.get(*argument) {
                                        let mut values = values.project(fields);
                                        if let Some(pointee) = pointer_view {
                                            values = Tree::from([(Vec::new(), analysis.cast_pointer(
                                                values.get(&Vec::new()).cloned().unwrap_or_default(),
                                                *pointee, *pointee,
                                            ))]);
                                        }
                                        if result_ty.is_bool() {
                                            if let Some(facts) = values.get_mut(&Vec::new()) {
                                                if facts.remove(&Fact::Unknown) {
                                                    facts.extend([Fact::Scalar(0), Fact::Scalar(1)]);
                                                }
                                            }
                                        }
                                        projection_applied = true;
                                        return Some(values);
                                    }
                                }
                                analysis
                                    .storage_call(instance, block, target, &arguments, result_ty)
                            });
                            analysis.record_effects = previous_effects;
                            let mut inputs = Vec::new();
                            if !opaque && !projection_applied {
                                inputs.extend(arguments.iter().map(|(_, values)| values.clone()));
                            }
                            let (target_node, binding) = if opaque {
                                let target_node = Node { instance: target, context: 0 };
                                if !analysis.graph.contains_key(&target_node) {
                                    analysis.schedule(target_node);
                                }
                                (target_node, HashMap::new())
                            } else {
                                analysis.context(target, inputs)
                            };
                            let mut caller_binding: HashMap<_, _> = binding
                                .iter()
                                .map(|(caller, parameter)| (parameter.clone(), caller.clone()))
                                .collect();
                            if let Some(input) = target_node.context.checked_sub(1)
                                .and_then(|index| analysis.contexts.get(&target)
                                    .and_then(|contexts| contexts.get(index)))
                            {
                                for base in input.memory.keys() {
                                    if let Base::Parameter(owner, context, _, ty) = base {
                                        if *owner == target && *context == target_node.context {
                                            caller_binding
                                                .entry(base.clone())
                                                .or_insert(Base::Unknown(*ty));
                                        }
                                    }
                                }
                            }
                            if matches!(target.def, InstanceKind::Intrinsic(_)) && storage.is_none()
                            {
                                edges.push(Edge { site: None, target: None, span, kind: "unmodelled intrinsic", detail: format!("the selected compiler intrinsic has no effect model: {target}"), trusted });
                            } else if tcx.is_foreign_item(target.def_id())
                                && trust.checks(target.def_id())
                                && forbidden(tcx, target).is_none()
                                && !intrinsic
                            {
                                let symbol = tcx.symbol_name(target).name;
                                edges.push(Edge {
                                    site: None,
                                    target: None,
                                    span,
                                    kind: "external call",
                                    detail: format!("external symbol has no Rust body: {symbol}"),
                                    trusted,
                                });
                            } else if trust.checks(target.def_id())
                                && !tcx.is_mir_available(target.def_id())
                                && matches!(target.def, InstanceKind::Item(..))
                                && forbidden(tcx, target).is_none()
                                && !intrinsic
                            {
                                edges.push(Edge {
                                    site: None,
                                    target: None,
                                    span,
                                    kind: "unresolved call",
                                    detail: format!("Rust body is not available for {target}"),
                                    trusted,
                                });
                            } else {
                                if virtual_call || matches!(callee_ty.kind(), ty::FnPtr(..)) {
                                    let feedback = (instance, location, target_node.instance);
                                    if analysis.backend_observed.insert(feedback)
                                        && !analysis.backend_calls.get(&(instance, location))
                                            .is_some_and(|targets| targets.contains(&target_node.instance))
                                    {
                                        analysis.backend_feedback.insert(feedback);
                                    }
                                }
                                edges.push(Edge {
                                    site: None,
                                    target: Some(target_node),
                                    span,
                                    kind: if validity_panic {
                                        "compiler validity panic"
                                    } else if linked {
                                        "linked Rust call"
                                    } else if virtual_call {
                                        "virtual call"
                                    } else if matches!(callee_ty.kind(), ty::FnPtr(..)) {
                                        "function-pointer call"
                                    } else {
                                        "call"
                                    },
                                    detail: if projection_applied {
                                        "return value follows the verified MIR argument projection".into()
                                    } else {
                                        String::new()
                                    },
                                    trusted: false,
                                });
                            }
                            if storage.is_some() {
                                if !opaque && !projection_applied {
                                    analysis.apply_call_effects(node, target_node, &caller_binding);
                                    let target_body = tcx.instance_mir(target.def);
                                    analysis.local_context = target_node.context;
                                    for (index, (_, values)) in
                                        arguments.iter().take(target_body.arg_count).enumerate()
                                    {
                                        analysis.write_place(
                                            target,
                                            target_body,
                                            Place::from(mir::Local::from_usize(index + 1)),
                                            &bind_tree(values, &binding),
                                        );
                                    }
                                    analysis.local_context = node.context;
                                }
                                if let (Some(destination), Some(values)) = (destination, &storage) {
                                    analysis.write_place(instance, body, destination, values);
                                }
                                call_states
                                    .as_mut()
                                    .unwrap()
                                    .push(analysis.flow_locals.clone().unwrap_or_default());
                                unwind_states
                                    .push(analysis.flow_locals.clone().unwrap_or_default());
                                continue;
                            }
                            if opaque {
                                if let Some(destination) = destination {
                                    analysis.write_place(
                                        instance,
                                        body,
                                        destination,
                                        &Tree::from([(Vec::new(), HashSet::from([Fact::Unknown]))]),
                                    );
                                }
                                if !intrinsic {
                                    edges.extend(analysis.opaque_effects(
                                        &arguments,
                                        span,
                                        trust.checks(target.def_id()),
                                        true,
                                        true,
                                    ));
                                }
                                call_states
                                    .as_mut()
                                    .unwrap()
                                    .push(analysis.flow_locals.clone().unwrap_or_default());
                                unwind_states
                                    .push(analysis.flow_locals.clone().unwrap_or_default());
                                continue;
                            }
                            let target_body = tcx.instance_mir(target.def);
                            analysis.apply_call_effects(node, target_node, &caller_binding);
                            unwind_states.push(analysis.flow_locals.clone().unwrap_or_default());
                            let untuple = if matches!(
                                tcx.def_kind(target.def_id()),
                                rustc_hir::def::DefKind::Closure
                            ) {
                                args.last().and_then(|argument| {
                                    let argument_ty =
                                        analysis.ty(instance, argument.node.ty(body, tcx));
                                    let ty::Tuple(fields) = argument_ty.kind() else {
                                        return None;
                                    };
                                    (target_body.arg_count == args.len() - 1 + fields.len()
                                        && fields.iter().enumerate().all(|(index, ty)| {
                                            analysis.ty(
                                                target,
                                                target_body.local_decls
                                                    [mir::Local::from_usize(args.len() + index)]
                                                .ty,
                                            ) == ty
                                        }))
                                    .then_some(*fields)
                                })
                            } else {
                                None
                            };
                            for index in 0..args.len().min(target_body.arg_count) {
                                let mut values = bind_tree(&arguments[index].1, &binding);
                                let parameter_ty = analysis.ty(
                                    target,
                                    target_body.local_decls[mir::Local::from_usize(index + 1)].ty,
                                );
                                if matches!(callee_ty.kind(), ty::FnPtr(..))
                                    && arguments[index].0 != parameter_ty
                                {
                                    if let (
                                        Some((source_path, source)),
                                        Some((destination_path, destination)),
                                    ) = (
                                        analysis.pointer_layout(arguments[index].0),
                                        analysis.pointer_layout(parameter_ty),
                                    ) {
                                        let mut facts = HashSet::new();
                                        for (path, values) in &values {
                                            if *path == source_path {
                                                facts.extend(values.iter().cloned());
                                            } else if source_path.starts_with(path)
                                                && values.contains(&Fact::Unknown)
                                            {
                                                facts.insert(Fact::Unknown);
                                            }
                                        }
                                        values = Tree::from([(
                                            destination_path,
                                            analysis.cast_pointer(facts, source, destination),
                                        )]);
                                    } else {
                                        values.entry(Vec::new()).or_default().insert(Fact::Unknown);
                                    }
                                }
                                if index + 1 == args.len() {
                                    if let Some(fields) = untuple {
                                        for field in 0..fields.len() {
                                            let mut projected = Tree::new();
                                            for (path, facts) in &values {
                                                if let Some(path) =
                                                    path.strip_prefix(&[field as u32])
                                                {
                                                    projected
                                                        .entry(path.to_vec())
                                                        .or_default()
                                                        .extend(facts.iter().cloned());
                                                } else if path.is_empty()
                                                    && facts.contains(&Fact::Unknown)
                                                {
                                                    projected
                                                        .entry(Vec::new())
                                                        .or_default()
                                                        .insert(Fact::Unknown);
                                                }
                                            }
                                            analysis.local_context = target_node.context;
                                            analysis.write_place(
                                                target,
                                                target_body,
                                                Place::from(mir::Local::from_usize(
                                                    index + field + 1,
                                                )),
                                                &projected,
                                            );
                                            analysis.local_context = node.context;
                                        }
                                        continue;
                                    }
                                }
                                analysis.local_context = target_node.context;
                                analysis.write_place(
                                    target,
                                    target_body,
                                    Place::from(mir::Local::from_usize(index + 1)),
                                    &values,
                                );
                                if crate::context_method(tcx, target.def_id())
                                    || roots.contains(&target)
                                {
                                    analysis.local_context = 0;
                                    analysis.write_place(
                                        target,
                                        target_body,
                                        Place::from(mir::Local::from_usize(index + 1)),
                                        &bind_tree(&values, &caller_binding),
                                    );
                                }
                                analysis.local_context = node.context;
                            }
                            if let Some(destination) = destination.filter(|_| storage.is_none()) {
                                let before_return =
                                    analysis.flow_locals.clone().unwrap_or_default();
                                for alternative in analysis
                                    .returns
                                    .get(&target_node)
                                    .cloned()
                                    .unwrap_or_default()
                                {
                                    let mut state = before_return.clone();
                                    state.extend(bind_memory(&alternative.memory, &caller_binding));
                                    analysis.flow_locals = Some(state);
                                    analysis.write_place(
                                        instance,
                                        body,
                                        destination,
                                        &bind_tree(&alternative.value, &caller_binding),
                                    );
                                    call_states
                                        .as_mut()
                                        .unwrap()
                                        .push(analysis.flow_locals.clone().unwrap_or_default());
                                }
                            }
                        }
                        if unknown {
                            analysis.flow_locals = entry_state;
                            let arguments: Vec<_> = args
                                .iter()
                                .map(|argument| {
                                    (
                                        analysis.ty(instance, argument.node.ty(body, tcx)),
                                        analysis.operand(instance, body, &argument.node),
                                    )
                                })
                                .collect();
                            edges.extend(
                                analysis.opaque_effects(&arguments, span, true, true, true),
                            );
                            if let Some(destination) = destination {
                                analysis.write_place(
                                    instance,
                                    body,
                                    destination,
                                    &Tree::from([(Vec::new(), HashSet::from([Fact::Unknown]))]),
                                );
                            }
                            call_states
                                .as_mut()
                                .unwrap()
                                .push(analysis.flow_locals.clone().unwrap_or_default());
                            unwind_states.push(analysis.flow_locals.clone().unwrap_or_default());
                        }
                    }
                    TerminatorKind::Return => {
                        let mut value =
                            analysis.place_value(instance, body, Place::from(mir::RETURN_PLACE));
                        if value.is_empty()
                            && analysis.settling_unknowns
                            && !analysis.ty(instance, body.return_ty()).is_unit()
                        {
                            value.insert(Vec::new(), HashSet::from([Fact::Unknown]));
                        }
                        if !value.is_empty() || analysis.ty(instance, body.return_ty()).is_unit() {
                            analysis.record_return(
                                node,
                                value,
                                analysis.flow_locals.clone().unwrap_or_default(),
                            );
                        }
                    }
                    TerminatorKind::Drop { place, .. } => {
                        let ty = analysis.ty(instance, place.ty(body, tcx).ty);
                        let target = Instance::resolve_drop_glue(tcx, ty);
                        let mut values = if !matches!(
                            target.def,
                            InstanceKind::Shim(ShimKind::DropGlue(_, None))
                        ) && analysis.tracks(ty, &mut HashSet::new())
                        {
                            Tree::from([(
                                Vec::new(),
                                analysis
                                    .places(instance, body, *place)
                                    .into_iter()
                                    .map(Fact::Reference)
                                    .collect(),
                            )])
                        } else {
                            Tree::new()
                        };
                        if matches!(ty.kind(), ty::Dynamic(..)) {
                            for (index, projection) in place.projection.iter().enumerate() {
                                if matches!(projection, ProjectionElem::Deref) {
                                    let pointer = Place {
                                        local: place.local,
                                        projection: tcx.mk_place_elems(&place.projection[..index]),
                                    };
                                    for fact in analysis
                                        .place_value(instance, body, pointer)
                                        .values()
                                        .flatten()
                                    {
                                        if matches!(fact, Fact::Concrete(_) | Fact::Unknown) {
                                            values
                                                .entry(Vec::new())
                                                .or_default()
                                                .insert(fact.clone());
                                        }
                                    }
                                }
                            }
                        }
                        let (target_node, binding) = analysis.context(target, vec![values.clone()]);
                        let caller_binding: HashMap<_, _> = binding
                            .iter()
                            .map(|(caller, parameter)| (parameter.clone(), caller.clone()))
                            .collect();
                        analysis.apply_call_effects(node, target_node, &caller_binding);
                        edges.push(Edge {
                            site: None,
                            target: Some(target_node),
                            span,
                            kind: "drop",
                            detail: String::new(),
                            trusted: false,
                        });
                        analysis.write(
                            &Address {
                                base: Base::Local(target, target_node.context, 1),
                                fields: Vec::new(),
                                byte_offset: 0,
                                pointee: None,
                            },
                            &bind_tree(&values, &binding),
                        );
                    }
                    TerminatorKind::Assert {
                        cond,
                        expected,
                        msg,
                        ..
                    } => {
                        let condition = analysis.operand(instance, body, cond);
                        let known = condition.get(&Vec::new());
                        if !analysis.settling_unknowns && known.is_none_or(HashSet::is_empty) {
                            continue 'states;
                        }
                        let can_fail = known.is_none_or(|facts| facts.is_empty() || facts.iter().any(|fact| !matches!(fact, Fact::Scalar(value) if *value == *expected as u128)));
                        if can_fail {
                            let (lang_item, operands) = match &**msg {
                                mir::AssertKind::BoundsCheck { len, index } => {
                                    (LangItem::PanicBoundsCheck, vec![index, len])
                                }
                                mir::AssertKind::MisalignedPointerDereference {
                                    required,
                                    found,
                                } => (
                                    LangItem::PanicMisalignedPointerDereference,
                                    vec![required, found],
                                ),
                                mir::AssertKind::NullPointerDereference => {
                                    (LangItem::PanicNullPointerDereference, Vec::new())
                                }
                                mir::AssertKind::NullReferenceConstructed => {
                                    (LangItem::PanicNullReferenceConstructed, Vec::new())
                                }
                                mir::AssertKind::InvalidEnumConstruction(_) => {
                                    (LangItem::PanicInvalidEnumConstruction, Vec::new())
                                }
                                other => (other.panic_function(), Vec::new()),
                            };
                            if let Some(def) = tcx.lang_items().get(lang_item) {
                                let target = Instance::mono(tcx, def);
                                let mut arguments: Vec<_> = operands
                                    .into_iter()
                                    .map(|operand| analysis.operand(instance, body, operand))
                                    .collect();
                                if tcx.is_mir_available(def) {
                                    arguments.resize_with(
                                        tcx.instance_mir(target.def).arg_count,
                                        || {
                                            Tree::from([(
                                                Vec::new(),
                                                HashSet::from([Fact::Unknown]),
                                            )])
                                        },
                                    );
                                }
                                let (target_node, binding) =
                                    analysis.context(target, arguments.clone());
                                for (index, values) in arguments.iter().enumerate() {
                                    analysis.write(
                                        &Address {
                                            base: Base::Local(
                                                target,
                                                target_node.context,
                                                index + 1,
                                            ),
                                            fields: Vec::new(),
                                            pointee: None,
                                            byte_offset: 0,
                                        },
                                        &bind_tree(values, &binding),
                                    );
                                }
                                edges.push(Edge {
                                    site: None,
                                    target: Some(target_node),
                                    span,
                                    kind: "assertion panic",
                                    detail: format!("assertion can fail: {msg:?}"),
                                    trusted: false,
                                });
                            } else {
                                edges.push(Edge {
                                    site: None,
                                    target: None,
                                    span,
                                    kind: "assertion panic",
                                    detail: format!("missing compiler panic entry: {lang_item:?}"),
                                    trusted,
                                });
                            }
                        }
                    }
                    TerminatorKind::InlineAsm {
                        operands, options, ..
                    } => {
                        edges.push(Edge {
                            site: None,
                            target: None,
                            span,
                            kind: "assembly",
                            detail: "assembly has no checked Rust call path".into(),
                            trusted,
                        });
                        let mut arguments = Vec::new();
                        for operand in operands {
                            match operand {
                                mir::InlineAsmOperand::In { value, .. }
                                | mir::InlineAsmOperand::InOut {
                                    in_value: value, ..
                                } => {
                                    arguments.push((
                                        analysis.ty(instance, value.ty(body, tcx)),
                                        analysis.operand(instance, body, value),
                                    ));
                                }
                                mir::InlineAsmOperand::SymFn { value } => {
                                    let operand = Operand::Constant(value.clone());
                                    arguments.push((
                                        analysis.ty(instance, operand.ty(body, tcx)),
                                        analysis.operand(instance, body, &operand),
                                    ));
                                }
                                mir::InlineAsmOperand::SymStatic { def_id } => {
                                    let value_ty = tcx.normalize_erasing_regions(
                                        TypingEnv::fully_monomorphized(),
                                        tcx.type_of(*def_id).instantiate_identity(),
                                    );
                                    arguments.push((
                                        value_ty,
                                        Tree::from([(
                                            Vec::new(),
                                            HashSet::from([Fact::Reference(Address {
                                                base: Base::Static(*def_id),
                                                fields: Vec::new(),
                                                pointee: Some(value_ty),
                                                byte_offset: 0,
                                            })]),
                                        )]),
                                    ));
                                }
                                _ => {}
                            }
                        }
                        let may_write = !options.intersects(
                            rustc_ast::InlineAsmOptions::NOMEM
                                | rustc_ast::InlineAsmOptions::READONLY,
                        );
                        edges.extend(analysis.opaque_effects(
                            &arguments,
                            span,
                            true,
                            may_write,
                            !options.contains(rustc_ast::InlineAsmOptions::NOMEM),
                        ));
                        for operand in operands {
                            if let mir::InlineAsmOperand::Out {
                                place: Some(place), ..
                            }
                            | mir::InlineAsmOperand::InOut {
                                out_place: Some(place),
                                ..
                            } = operand
                            {
                                analysis.write_place(
                                    instance,
                                    body,
                                    *place,
                                    &Tree::from([(Vec::new(), HashSet::from([Fact::Unknown]))]),
                                );
                            }
                        }
                    }
                    _ => {}
                }
                if edges.len() != first_edge {
                    analysis.record_site(
                    node,
                    body,
                    body.terminator_loc(block),
                    incoming.expect("a terminator call edge requires its input state"),
                    &mut edges[first_edge..],
                    );
                }
                let locals = analysis.flow_locals.take().unwrap_or_default();
                if let Some(states) = call_states {
                    match &terminator.kind {
                        TerminatorKind::Call { target, unwind, .. } => {
                            if let Some(target) = target {
                                pending.extend(states.into_iter().map(|state| {
                                    (
                                        mir::Location {
                                            block: *target,
                                            statement_index: 0,
                                        },
                                        state,
                                    )
                                }));
                            }
                            if let mir::UnwindAction::Cleanup(block) = unwind {
                                pending.extend(unwind_states.into_iter().map(|state| {
                                    (
                                        mir::Location {
                                            block: *block,
                                            statement_index: 0,
                                        },
                                        state,
                                    )
                                }));
                            }
                        }
                        TerminatorKind::TailCall { .. } => {
                            for state in states {
                                let value = state
                                    .get(&Base::Local(instance, node.context, 0))
                                    .cloned()
                                    .unwrap_or_default();
                                analysis.record_return(node, value, state);
                            }
                        }
                        _ => {}
                    }
                    continue;
                }
                if let TerminatorKind::SwitchInt { discr, targets } = &terminator.kind {
                    analysis.flow_locals = Some(locals.clone());
                    let values = analysis.operand(instance, body, discr);
                    analysis.flow_locals = None;
                    let facts = values.get(&Vec::new());
                    if !analysis.settling_unknowns && facts.is_none_or(HashSet::is_empty) {
                        continue 'states;
                    }
                    let unknown = facts.is_none_or(|facts| {
                        facts.is_empty()
                            || facts.iter().any(|fact| !matches!(fact, Fact::Scalar(_)))
                    });
                    let mut successors = HashSet::new();
                    if let Some(facts) = facts {
                        for fact in facts {
                            if let Fact::Scalar(value) = fact {
                                successors.insert(targets.target_for_value(*value));
                            }
                        }
                    }
                    if unknown {
                        successors.extend(targets.all_targets());
                    }
                    for successor in successors {
                        let mut refined = locals.clone();
                        if let Some(place) = discr.place() {
                            if place.projection.is_empty() {
                                let mut selected: HashSet<_> = targets
                                    .iter()
                                    .filter(|(_, target)| *target == successor)
                                    .map(|(value, _)| Fact::Scalar(value))
                                    .collect();
                                if successor == targets.otherwise() {
                                    if analysis.ty(instance, discr.ty(body, tcx)).is_bool() {
                                        selected.extend(
                                            (0..=1)
                                                .filter(|value| {
                                                    targets.target_for_value(*value) == successor
                                                })
                                                .map(Fact::Scalar),
                                        );
                                    } else if unknown {
                                        selected.insert(Fact::Unknown);
                                    } else if let Some(facts) = facts {
                                        selected.extend(facts.iter().filter(|fact| matches!(fact, Fact::Scalar(value) if targets.target_for_value(*value) == successor)).cloned());
                                    }
                                }
                                refined.insert(
                                    Base::Local(instance, node.context, place.local.as_usize()),
                                    Tree::from([(Vec::new(), selected)]),
                                );
                            }
                        }
                        pending.push_back((
                            mir::Location {
                                block: successor,
                                statement_index: 0,
                            },
                            refined,
                        ));
                    }
                } else if let TerminatorKind::Assert {
                    cond,
                    expected,
                    target,
                    unwind,
                    ..
                } = &terminator.kind
                {
                    analysis.flow_locals = Some(locals.clone());
                    let values = analysis.operand(instance, body, cond);
                    analysis.flow_locals = None;
                    let facts = values.get(&Vec::new());
                    let unknown = facts.is_none_or(|facts| {
                        facts.is_empty()
                            || facts.iter().any(|fact| !matches!(fact, Fact::Scalar(_)))
                    });
                    for result in [false, true] {
                        if !unknown
                            && !facts
                                .is_some_and(|facts| facts.contains(&Fact::Scalar(result as u128)))
                        {
                            continue;
                        }
                        let successor = if result == *expected {
                            Some(*target)
                        } else if let mir::UnwindAction::Cleanup(block) = unwind {
                            Some(*block)
                        } else {
                            None
                        };
                        if let Some(block) = successor {
                            let mut refined = locals.clone();
                            if let Some(place) = cond.place() {
                                if place.projection.is_empty() {
                                    refined.insert(
                                        Base::Local(instance, node.context, place.local.as_usize()),
                                        Tree::from([(
                                            Vec::new(),
                                            HashSet::from([Fact::Scalar(result as u128)]),
                                        )]),
                                    );
                                }
                            }
                            pending.push_back((
                                mir::Location {
                                    block,
                                    statement_index: 0,
                                },
                                refined,
                            ));
                        }
                    }
                } else {
                    pending.extend(terminator.successors().map(|block| {
                        (
                            mir::Location {
                                block,
                                statement_index: 0,
                            },
                            locals.clone(),
                        )
                    }));
                }
            }
            analysis.flow_locals = None;
        }
        for edge in &edges {
            if let Some(target) = edge.target {
                if !analysis.graph.contains_key(&target) {
                    analysis.schedule(target);
                }
            }
        }
        edges.sort_by_key(|edge| {
            (
                edge.span.lo(),
                edge.detail.clone(),
                edge.target
                    .map(|target| (target.instance.to_string(), target.context)),
            )
        });
        edges.dedup_by(|left, right| {
            left.target == right.target
                && left.span == right.span
                && left.detail == right.detail
                && left.site == right.site
        });
        analysis.graph.insert(node, edges);
    }
}
