use crate::trust::Trust;
use crate::{Edge, forbidden, known_intrinsic};
use rustc_abi::Size;
use rustc_hir::def_id::DefId;
use rustc_middle::mir::interpret::{AllocId, AllocRange, GlobalAlloc, Scalar};
use rustc_middle::mir::{
    self, AggregateKind, Body, CastKind, ConstValue, Operand, Place, ProjectionElem, Rvalue,
    StatementKind, TerminatorKind,
};
use rustc_middle::ty::adjustment::PointerCoercion;
use rustc_middle::ty::{self, EarlyBinder, Instance, InstanceKind, Ty, TyCtxt, TypingEnv};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum Base<'tcx> {
    Local(Instance<'tcx>, usize, usize),
    Heap(Instance<'tcx>, usize, usize),
    Static(DefId),
    Allocation(AllocId, u64),
    Unknown(Ty<'tcx>),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Address<'tcx> {
    base: Base<'tcx>,
    fields: Vec<u32>,
    pointee: Option<Ty<'tcx>>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum Fact<'tcx> {
    Function(Instance<'tcx>),
    Reference(Address<'tcx>),
    Concrete(Ty<'tcx>),
    Unknown,
}

type Tree<'tcx> = HashMap<Vec<u32>, HashSet<Fact<'tcx>>>;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Node<'tcx> {
    pub instance: Instance<'tcx>,
    pub context: usize,
}

pub struct Analysis<'tcx> {
    tcx: TyCtxt<'tcx>,
    memory: HashMap<Base<'tcx>, Tree<'tcx>>,
    constants: HashMap<mir::Const<'tcx>, Tree<'tcx>>,
    tracked: HashMap<Ty<'tcx>, bool>,
    loaded: HashSet<(AllocId, u64, Ty<'tcx>)>,
    statics: HashSet<DefId>,
    revision: usize,
    readers: HashMap<Base<'tcx>, HashSet<Node<'tcx>>>,
    pending: VecDeque<Node<'tcx>>,
    queued: HashSet<Node<'tcx>>,
    active: Option<Node<'tcx>>,
    local_context: usize,
    contextual: bool,
    contexts: HashMap<Instance<'tcx>, Vec<Vec<Tree<'tcx>>>>,
    parents: HashMap<Node<'tcx>, Node<'tcx>>,
    pub graph: HashMap<Node<'tcx>, Vec<Edge<'tcx>>>,
    pub roots: HashMap<Instance<'tcx>, Node<'tcx>>,
    pub incomplete: bool,
    pub limit_detail: String,
}

impl<'tcx> Analysis<'tcx> {
    pub fn build(
        tcx: TyCtxt<'tcx>,
        trust: &Trust,
        instances: &[Instance<'tcx>],
        roots: &[Instance<'tcx>],
    ) -> Self {
        let mut analysis = Self {
            tcx,
            memory: HashMap::new(),
            constants: HashMap::new(),
            tracked: HashMap::new(),
            loaded: HashSet::new(),
            statics: HashSet::new(),
            revision: 0,
            readers: HashMap::new(),
            pending: VecDeque::new(),
            queued: HashSet::new(),
            active: None,
            local_context: 0,
            contextual: false,
            contexts: HashMap::new(),
            parents: HashMap::new(),
            graph: HashMap::new(),
            roots: HashMap::new(),
            incomplete: false,
            limit_detail: String::new(),
        };
        let mut functions: HashSet<_> = instances
            .iter()
            .copied()
            .filter(|instance| trust.checks(instance.def_id()))
            .collect();
        functions.extend(roots.iter().copied());
        let exported: HashMap<_, _> = instances
            .iter()
            .filter(|instance| {
                matches!(
                    tcx.def_kind(instance.def_id()),
                    rustc_hir::def::DefKind::Fn | rustc_hir::def::DefKind::AssocFn
                )
            })
            .map(|instance| (tcx.symbol_name(*instance).name.to_owned(), *instance))
            .collect();
        let mut ordered: Vec<_> = functions.iter().copied().collect();
        ordered.sort_by_key(ToString::to_string);
        for instance in ordered {
            analysis.schedule(Node {
                instance,
                context: 0,
            });
        }
        let mut scans = 0;
        let mut seeded_revision = usize::MAX;
        loop {
            if analysis.pending.is_empty() && seeded_revision != analysis.revision {
                analysis.contextual = true;
                analysis.active = None;
                let mut entries: Vec<_> = analysis
                    .graph
                    .keys()
                    .map(|node| node.instance)
                    .filter(|instance| {
                        !matches!(instance.def, InstanceKind::Virtual(..))
                            && (roots.contains(instance)
                                || trust.checks(instance.def_id())
                                    && crate::context_method(tcx, instance.def_id()))
                    })
                    .collect();
                entries.sort_by_key(ToString::to_string);
                entries.dedup();
                for instance in entries {
                    let arguments: Vec<_> = if !tcx.is_foreign_item(instance.def_id())
                        && !matches!(
                            instance.def,
                            InstanceKind::Virtual(..) | InstanceKind::Intrinsic(..)
                        )
                        && (tcx.is_mir_available(instance.def_id())
                            || !matches!(instance.def, InstanceKind::Item(..)))
                    {
                        (1..=tcx.instance_mir(instance.def).arg_count)
                            .map(|local| {
                                analysis
                                    .memory
                                    .get(&Base::Local(instance, 0, local))
                                    .cloned()
                                    .unwrap_or_default()
                            })
                            .collect()
                    } else {
                        Vec::new()
                    };
                    let node = analysis.context(instance, arguments.clone());
                    analysis.roots.insert(instance, node);
                    for (index, values) in arguments.iter().enumerate() {
                        analysis.write(
                            &Address {
                                base: Base::Local(instance, node.context, index + 1),
                                fields: Vec::new(),
                                pointee: None,
                            },
                            values,
                        );
                    }
                }
                seeded_revision = analysis.revision;
            }
            let Some(node) = analysis.pending.pop_front() else {
                break;
            };
            let instance = node.instance;
            analysis.queued.remove(&node);
            analysis.active = Some(node);
            analysis.local_context = node.context;
            scans += 1;
            let mut edges = Vec::new();
            if forbidden(tcx, instance).is_some()
                || !trust.checks(instance.def_id())
                || known_intrinsic(tcx, instance)
            {
                analysis.graph.insert(node, edges);
                continue;
            }
            if matches!(
                instance.def,
                InstanceKind::Intrinsic(_) | InstanceKind::DropGlue(_, None)
            ) {
                analysis.graph.insert(node, edges);
                continue;
            }
            if tcx.is_foreign_item(instance.def_id()) {
                let symbol = tcx.symbol_name(instance).name;
                edges.push(Edge {
                    target: exported
                        .get(symbol)
                        .map(|instance| analysis.context(*instance, Vec::new())),
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
                    target: None,
                    span: tcx.def_span(instance.def_id()),
                    kind: "unresolved call",
                    detail: format!("Rust body is not available for {instance}"),
                    trusted: false,
                });
            } else {
                let body = tcx.instance_mir(instance.def);
                let mut pending = VecDeque::from([mir::START_BLOCK]);
                let mut seen = HashSet::new();
                while let Some(block) = pending.pop_front() {
                    if !seen.insert(block) {
                        continue;
                    }
                    let data = &body.basic_blocks[block];
                    for statement in &data.statements {
                        if trust.allows(tcx, instance, body, statement.source_info.scope, true) {
                            continue;
                        }
                        if let StatementKind::Assign(assignment) = &statement.kind {
                            let (destination, value) = &**assignment;
                            let mut tree = Tree::new();
                            match value {
                                Rvalue::Use(operand) | Rvalue::WrapUnsafeBinder(operand, _) => {
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
                                            projection: tcx
                                                .mk_place_elems(&place.projection[..index]),
                                        };
                                        for fact in analysis
                                            .place_value(instance, body, pointer)
                                            .values()
                                            .flatten()
                                        {
                                            if matches!(fact, Fact::Unknown)
                                                || matches!(referent.kind(), ty::Dynamic(..))
                                                    && index + 1 == place.projection.len()
                                                    && matches!(fact, Fact::Concrete(_))
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
                                            pointee: Some(analysis.ty(
                                                instance,
                                                tcx.type_of(*def).instantiate_identity(),
                                            )),
                                        })]),
                                    );
                                }
                                Rvalue::Cast(kind, operand, destination_ty) => {
                                    let source_ty = analysis.ty(instance, operand.ty(body, tcx));
                                    let destination_ty = analysis.ty(instance, *destination_ty);
                                    tree = analysis.operand(instance, body, operand);
                                    match kind {
                                        CastKind::PointerCoercion(
                                            PointerCoercion::ReifyFnPointer(_),
                                            _,
                                        ) => {
                                            if let ty::FnDef(def, args) = *source_ty.kind() {
                                                let target = Instance::resolve_for_fn_ptr(
                                                    tcx,
                                                    TypingEnv::fully_monomorphized(),
                                                    def,
                                                    args,
                                                );
                                                tree.insert(
                                                    Vec::new(),
                                                    HashSet::from([target
                                                        .map_or(Fact::Unknown, Fact::Function)]),
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
                                                            pointee: Some(source),
                                                        }));
                                                    }
                                                    values.insert(Fact::Concrete(source));
                                                }
                                            }
                                        }
                                        CastKind::PointerExposeProvenance
                                        | CastKind::PointerWithExposedProvenance => {
                                            tree.entry(Vec::new())
                                                .or_default()
                                                .insert(Fact::Unknown);
                                        }
                                        CastKind::Transmute if source_ty != destination_ty => {
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
                                                tree = HashMap::from([(destination_path, values)]);
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
                                                                    address.pointee =
                                                                        destination_ty
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
                                                    && matches!(
                                                        destination_ty.kind(),
                                                        ty::FnPtr(..)
                                                    );
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
                                                    edges.push(Edge {
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
                                    for (field, operand) in operands.iter_enumerated() {
                                        if matches!(&**kind, AggregateKind::RawPtr(..))
                                            && field.as_usize() != 0
                                        {
                                            continue;
                                        }
                                        let field = match &**kind {
                                            AggregateKind::Array(_) => u32::MAX,
                                            AggregateKind::Adt(_, _, _, _, Some(field)) => {
                                                field.as_u32()
                                            }
                                            _ => field.as_u32(),
                                        };
                                        for (path, values) in
                                            analysis.operand(instance, body, operand)
                                        {
                                            let mut output =
                                                if matches!(&**kind, AggregateKind::RawPtr(..)) {
                                                    Vec::new()
                                                } else {
                                                    vec![field]
                                                };
                                            output.extend(path);
                                            tree.entry(output).or_default().extend(values);
                                        }
                                    }
                                }
                                Rvalue::Repeat(operand, _) => {
                                    for (path, values) in analysis.operand(instance, body, operand)
                                    {
                                        let mut output = vec![u32::MAX];
                                        output.extend(path);
                                        tree.insert(output, values);
                                    }
                                }
                                Rvalue::BinaryOp(mir::BinOp::Offset, operands) => {
                                    tree = analysis.operand(instance, body, &operands.0);
                                    let element = analysis
                                        .ty(instance, operands.0.ty(body, tcx))
                                        .builtin_deref(true);
                                    if tree.values().flatten().any(|fact| matches!(fact, Fact::Reference(address) if address.fields.last() != Some(&u32::MAX) || address.pointee != element)) {
                                        tree.entry(Vec::new()).or_default().insert(Fact::Unknown);
                                    }
                                }
                                Rvalue::BinaryOp(_, _) => {
                                    tree.entry(Vec::new()).or_default().insert(Fact::Unknown);
                                }
                                Rvalue::UnaryOp(mir::UnOp::PtrMetadata, operand) => {
                                    for facts in analysis.operand(instance, body, operand).values()
                                    {
                                        tree.entry(Vec::new()).or_default().extend(
                                            facts
                                                .iter()
                                                .filter(|fact| matches!(fact, Fact::Concrete(_)))
                                                .cloned(),
                                        );
                                    }
                                }
                                Rvalue::UnaryOp(_, _) => {
                                    tree.entry(Vec::new()).or_default().insert(Fact::Unknown);
                                }
                                Rvalue::Discriminant(_) => {}
                            }
                            analysis.write_place(instance, body, *destination, &tree);
                        } else if let StatementKind::Intrinsic(intrinsic) = &statement.kind {
                            if let mir::NonDivergingIntrinsic::CopyNonOverlapping(copy) =
                                &**intrinsic
                            {
                                let sources = analysis.operand(instance, body, &copy.src);
                                let destinations = analysis.operand(instance, body, &copy.dst);
                                let element = analysis
                                    .ty(instance, copy.src.ty(body, tcx))
                                    .builtin_deref(true);
                                analysis.copy_memory(&sources, &destinations, element);
                            }
                        }
                    }
                    let terminator = data.terminator();
                    if trust.allows(tcx, instance, body, terminator.source_info.scope, true) {
                        continue;
                    }
                    pending.extend(terminator.successors());
                    let trusted =
                        trust.allows(tcx, instance, body, terminator.source_info.scope, false);
                    let span = terminator.source_info.span;
                    match &terminator.kind {
                        TerminatorKind::Call { func, args, .. }
                        | TerminatorKind::TailCall { func, args, .. } => {
                            let callee_ty = analysis.ty(instance, func.ty(body, tcx));
                            let mut targets = HashSet::new();
                            let mut unknown = false;
                            let mut virtual_call = false;
                            if let ty::FnDef(def, generic_args) = *callee_ty.kind() {
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
                                                        !matches!(
                                                            target.def,
                                                            InstanceKind::Virtual(..)
                                                        )
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
                                for fact in
                                    analysis.operand(instance, body, func).values().flatten()
                                {
                                    match fact {
                                        Fact::Function(target) => {
                                            targets.insert(*target);
                                        }
                                        Fact::Unknown => unknown = true,
                                        _ => {}
                                    }
                                }
                            }
                            if targets.is_empty() || unknown {
                                edges.push(Edge {
                                    target: None,
                                    span,
                                    kind: "unresolved call",
                                    detail: format!("call target is not fully known: {callee_ty}"),
                                    trusted,
                                });
                            }
                            let destination = match &terminator.kind {
                                TerminatorKind::Call { destination, .. } => Some(*destination),
                                TerminatorKind::TailCall { .. } => {
                                    Some(Place::from(mir::RETURN_PLACE))
                                }
                                _ => None,
                            };
                            for mut target in targets {
                                let intrinsic = known_intrinsic(tcx, target);
                                let mut linked = false;
                                if tcx.is_foreign_item(target.def_id())
                                    && trust.checks(target.def_id())
                                    && forbidden(tcx, target).is_none()
                                {
                                    if let Some(function) =
                                        exported.get(tcx.symbol_name(target).name)
                                    {
                                        target = *function;
                                        linked = true;
                                    }
                                }
                                let opaque = !trust.checks(target.def_id())
                                    || forbidden(tcx, target).is_some()
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
                                        let values =
                                            analysis.operand(instance, body, &argument.node);
                                        (ty, values)
                                    })
                                    .collect();
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
                                                let values =
                                                    arguments[0].1.entry(path).or_default();
                                                let unknown_data = values.remove(&Fact::Unknown);
                                                values.retain(|fact| match fact {
                                                    Fact::Concrete(ty) => *ty == concrete,
                                                    Fact::Reference(address) => {
                                                        address.pointee.is_none_or(|ty| {
                                                            ty == concrete
                                                                || matches!(
                                                                    ty.kind(),
                                                                    ty::Dynamic(..)
                                                                )
                                                        })
                                                    }
                                                    _ => true,
                                                });
                                                if unknown_data
                                                    || !values.iter().any(|fact| {
                                                        matches!(fact, Fact::Reference(_))
                                                    })
                                                {
                                                    values.insert(Fact::Reference(Address {
                                                        base: Base::Unknown(concrete),
                                                        fields: Vec::new(),
                                                        pointee: Some(concrete),
                                                    }));
                                                }
                                            }
                                        }
                                    }
                                }
                                let storage = destination.and_then(|destination| {
                                    let result_ty =
                                        analysis.ty(instance, destination.ty(body, tcx).ty);
                                    analysis.storage_call(
                                        instance, block, target, &arguments, result_ty,
                                    )
                                });
                                let mut inputs = Vec::new();
                                if !opaque && storage.is_none() && node.context != 0 {
                                    for (ty, values) in &arguments {
                                        let relevant = analysis.tracks(*ty, &mut HashSet::new())
                                            || values.values().flatten().any(|fact| match fact {
                                                Fact::Function(_) | Fact::Concrete(_) => true,
                                                Fact::Reference(address) => {
                                                    matches!(address.base, Base::Heap(..))
                                                        || address.pointee.is_some_and(|ty| {
                                                            analysis.tracks(ty, &mut HashSet::new())
                                                        })
                                                }
                                                Fact::Unknown => false,
                                            });
                                        inputs.push(if relevant {
                                            values.clone()
                                        } else {
                                            Tree::new()
                                        });
                                    }
                                }
                                let target_node = analysis.context(target, inputs);
                                if tcx.is_foreign_item(target.def_id())
                                    && trust.checks(target.def_id())
                                    && forbidden(tcx, target).is_none()
                                    && !intrinsic
                                {
                                    let symbol = tcx.symbol_name(target).name;
                                    edges.push(Edge {
                                        target: None,
                                        span,
                                        kind: "external call",
                                        detail: format!(
                                            "external symbol has no Rust body: {symbol}"
                                        ),
                                        trusted,
                                    });
                                } else if trust.checks(target.def_id())
                                    && !tcx.is_mir_available(target.def_id())
                                    && matches!(target.def, InstanceKind::Item(..))
                                    && forbidden(tcx, target).is_none()
                                    && !intrinsic
                                {
                                    edges.push(Edge {
                                        target: None,
                                        span,
                                        kind: "unresolved call",
                                        detail: format!("Rust body is not available for {target}"),
                                        trusted,
                                    });
                                } else {
                                    edges.push(Edge {
                                        target: Some(target_node),
                                        span,
                                        kind: if linked { "linked Rust call" } else { "call" },
                                        detail: String::new(),
                                        trusted: false,
                                    });
                                }
                                if let (Some(destination), Some(values)) = (destination, &storage) {
                                    analysis.write_place(instance, body, destination, values);
                                }
                                if storage.is_some() {
                                    continue;
                                }
                                if opaque {
                                    if let Some(destination) = destination {
                                        analysis.write_place(
                                            instance,
                                            body,
                                            destination,
                                            &HashMap::from([(
                                                Vec::new(),
                                                HashSet::from([Fact::Unknown]),
                                            )]),
                                        );
                                    }
                                    for argument in args {
                                        let argument_ty =
                                            analysis.ty(instance, argument.node.ty(body, tcx));
                                        if matches!(
                                            argument_ty.kind(),
                                            ty::RawPtr(..)
                                                | ty::Ref(_, _, rustc_hir::Mutability::Mut)
                                        ) {
                                            let values =
                                                analysis.operand(instance, body, &argument.node);
                                            for fact in values.values().flatten() {
                                                if let Fact::Reference(address) = fact {
                                                    analysis.write(
                                                        address,
                                                        &HashMap::from([(
                                                            Vec::new(),
                                                            HashSet::from([Fact::Unknown]),
                                                        )]),
                                                    );
                                                }
                                            }
                                        }
                                    }
                                    continue;
                                }
                                let target_body = tcx.instance_mir(target.def);
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
                                                    target_body.local_decls[mir::Local::from_usize(
                                                        args.len() + index,
                                                    )]
                                                    .ty,
                                                ) == ty
                                            }))
                                        .then_some(*fields)
                                    })
                                } else {
                                    None
                                };
                                for index in 0..args.len().min(target_body.arg_count) {
                                    let mut values = arguments[index].1.clone();
                                    let parameter_ty = analysis.ty(
                                        target,
                                        target_body.local_decls[mir::Local::from_usize(index + 1)]
                                            .ty,
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
                                            values = HashMap::from([(
                                                destination_path,
                                                analysis.cast_pointer(facts, source, destination),
                                            )]);
                                        } else {
                                            values
                                                .entry(Vec::new())
                                                .or_default()
                                                .insert(Fact::Unknown);
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
                                    analysis.local_context = node.context;
                                }
                                if let Some(destination) = destination.filter(|_| storage.is_none())
                                {
                                    let result = analysis.read(&Address {
                                        base: Base::Local(target, target_node.context, 0),
                                        fields: Vec::new(),
                                        pointee: None,
                                    });
                                    analysis.write_place(instance, body, destination, &result);
                                }
                            }
                            if unknown {
                                if let Some(destination) = destination {
                                    analysis.write_place(
                                        instance,
                                        body,
                                        destination,
                                        &HashMap::from([(
                                            Vec::new(),
                                            HashSet::from([Fact::Unknown]),
                                        )]),
                                    );
                                }
                            }
                        }
                        TerminatorKind::Drop { place, .. } => {
                            let ty = analysis.ty(instance, place.ty(body, tcx).ty);
                            let target = Instance::resolve_drop_in_place(tcx, ty);
                            let values = if !matches!(target.def, InstanceKind::DropGlue(_, None))
                                && analysis.tracks(ty, &mut HashSet::new())
                            {
                                HashMap::from([(
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
                            let target_node = analysis.context(target, vec![values.clone()]);
                            edges.push(Edge {
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
                                    pointee: None,
                                },
                                &values,
                            );
                        }
                        TerminatorKind::InlineAsm { operands, .. } => {
                            edges.push(Edge {
                                target: None,
                                span,
                                kind: "assembly",
                                detail: "assembly has no checked Rust call path".into(),
                                trusted,
                            });
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
                                        &HashMap::from([(
                                            Vec::new(),
                                            HashSet::from([Fact::Unknown]),
                                        )]),
                                    );
                                }
                            }
                        }
                        _ => {}
                    }
                }
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
            });
            analysis.graph.insert(node, edges);
            if scans > 1_000_000 || analysis.graph.len() > 200_000 || analysis.revision > 10_000_000
            {
                analysis.incomplete = true;
                analysis.limit_detail = format!(
                    "analyzed {scans} function states, retained {} calling states, and added {} memory facts",
                    analysis.graph.len(),
                    analysis.revision
                );
                break;
            }
        }
        analysis
    }

    fn ty(&self, instance: Instance<'tcx>, ty: Ty<'tcx>) -> Ty<'tcx> {
        instance.instantiate_mir_and_normalize_erasing_regions(
            self.tcx,
            TypingEnv::fully_monomorphized(),
            EarlyBinder::bind(ty),
        )
    }

    fn storage_call(
        &mut self,
        caller: Instance<'tcx>,
        block: mir::BasicBlock,
        target: Instance<'tcx>,
        arguments: &[(Ty<'tcx>, Tree<'tcx>)],
        result_ty: Ty<'tcx>,
    ) -> Option<Tree<'tcx>> {
        let crate_name = self.tcx.crate_name(target.def_id().krate);
        let name = self.tcx.opt_item_name(target.def_id())?;
        if crate_name.as_str() == "alloc"
            && matches!(
                name.as_str(),
                "reserve"
                    | "try_reserve"
                    | "reserve_exact"
                    | "try_reserve_exact"
                    | "grow_one"
                    | "grow_amortized"
                    | "grow_exact"
                    | "shrink_to_fit"
                    | "try_shrink_to_fit"
                    | "shrink"
                    | "shrink_unchecked"
                    | "deallocate"
            )
        {
            if let Some(ty::Adt(adt, _)) = arguments
                .first()
                .and_then(|(ty, _)| ty.builtin_deref(true))
                .map(|ty| ty.kind())
            {
                if self.tcx.crate_name(adt.did().krate).as_str() == "alloc"
                    && self.tcx.item_name(adt.did()).as_str() == "RawVecInner"
                {
                    return Some(Tree::new());
                }
            }
        }
        if crate_name.as_str() == "alloc"
            && matches!(name.as_str(), "from_raw_parts_in" | "from_nonnull_in")
            && arguments.len() == 3
        {
            if let ty::Adt(adt, _) = result_ty.kind() {
                if self.tcx.item_name(adt.did()).as_str() == "RawVecInner" {
                    let pointer_path = self.pointer_layout(arguments[0].0)?.0;
                    let mut result = Tree::new();
                    for (path, facts) in &arguments[0].1 {
                        if *path == pointer_path {
                            result
                                .entry(vec![0, 0, 0])
                                .or_default()
                                .extend(facts.iter().cloned());
                        } else if pointer_path.starts_with(path) && facts.contains(&Fact::Unknown) {
                            result
                                .entry(vec![0, 0, 0])
                                .or_default()
                                .insert(Fact::Unknown);
                        }
                    }
                    for (path, facts) in &arguments[2].1 {
                        let mut output = vec![2];
                        output.extend(path);
                        result
                            .entry(output)
                            .or_default()
                            .extend(facts.iter().cloned());
                    }
                    return Some(result);
                }
            }
        }
        if crate_name.as_str() == "core"
            && self.tcx.def_path_str(target.def_id()).contains("::ptr::")
            && matches!(
                name.as_str(),
                "new_unchecked"
                    | "as_ptr"
                    | "as_mut_ptr"
                    | "as_non_null_ptr"
                    | "cast"
                    | "cast_const"
                    | "cast_mut"
                    | "from_ref"
                    | "from_mut"
                    | "from"
            )
            && arguments.len() == 1
        {
            if let (Some((source_path, source)), Some((destination_path, destination))) = (
                self.pointer_layout(arguments[0].0),
                self.pointer_layout(result_ty),
            ) {
                let mut values = HashSet::new();
                for (path, facts) in &arguments[0].1 {
                    if *path == source_path {
                        values.extend(facts.iter().cloned());
                    } else if source_path.starts_with(path) && facts.contains(&Fact::Unknown) {
                        values.insert(Fact::Unknown);
                    }
                }
                return Some(HashMap::from([(
                    destination_path,
                    self.cast_pointer(values, source, destination),
                )]));
            }
        }
        if crate_name.as_str() == "alloc"
            && result_ty.is_box()
            && (matches!(name.as_str(), "new" | "box_new") && arguments.len() == 1
                || name.as_str() == "new_uninit" && arguments.is_empty())
        {
            if !self.tracks(result_ty, &mut HashSet::new()) {
                return Some(Tree::new());
            }
            let address = Address {
                base: Base::Heap(caller, self.local_context, block.as_usize()),
                fields: Vec::new(),
                pointee: result_ty.builtin_deref(true),
            };
            if let Some((_, values)) = arguments.first() {
                self.write(&address, values);
            }
            return Some(HashMap::from([(
                vec![0, 0, 0],
                HashSet::from([Fact::Reference(address)]),
            )]));
        }
        if crate_name.as_str() == "alloc"
            && result_ty.is_box()
            && name.as_str() == "from_raw"
            && arguments.len() == 1
        {
            return Some(
                arguments[0]
                    .1
                    .iter()
                    .map(|(path, facts)| {
                        let mut output = vec![0, 0, 0];
                        output.extend(path);
                        (output, facts.clone())
                    })
                    .collect(),
            );
        }
        if crate_name.as_str() == "alloc"
            && name.as_str() == "into_raw"
            && arguments.first().is_some_and(|(ty, _)| ty.is_box())
        {
            return Some(
                arguments[0]
                    .1
                    .iter()
                    .filter_map(|(path, facts)| {
                        if let Some(path) = path.strip_prefix(&[0, 0, 0]) {
                            Some((path.to_vec(), facts.clone()))
                        } else if [0, 0, 0].starts_with(path) && facts.contains(&Fact::Unknown) {
                            Some((Vec::new(), HashSet::from([Fact::Unknown])))
                        } else {
                            None
                        }
                    })
                    .fold(Tree::new(), |mut result, (path, facts)| {
                        result.entry(path).or_default().extend(facts);
                        result
                    }),
            );
        }
        if crate_name.as_str() == "alloc"
            && name.as_str() == "into_boxed_slice"
            && result_ty.is_box()
            && arguments.len() == 1
        {
            if let ty::Adt(adt, _) = arguments[0].0.kind() {
                if self.tcx.crate_name(adt.did().krate).as_str() == "alloc"
                    && self.tcx.item_name(adt.did()).as_str() == "Vec"
                {
                    let mut values = HashSet::new();
                    for (path, facts) in &arguments[0].1 {
                        if path.as_slice() == [0, 0, 0, 0, 0] {
                            for fact in facts {
                                if let Fact::Reference(address) = fact {
                                    let mut address = address.clone();
                                    if address.fields.last() == Some(&u32::MAX) {
                                        address.fields.pop();
                                        address.pointee = result_ty.builtin_deref(true);
                                        values.insert(Fact::Reference(address));
                                    } else {
                                        values.insert(Fact::Unknown);
                                    }
                                } else {
                                    values.insert(fact.clone());
                                }
                            }
                        } else if [0, 0, 0, 0, 0].starts_with(path)
                            && facts.contains(&Fact::Unknown)
                        {
                            values.insert(Fact::Unknown);
                        }
                    }
                    let mut result = HashMap::from([(vec![0, 0, 0], values)]);
                    for (path, facts) in &arguments[0].1 {
                        if let Some(path) = path.strip_prefix(&[0, 0, 2]) {
                            let mut output = vec![1];
                            output.extend(path);
                            result
                                .entry(output)
                                .or_default()
                                .extend(facts.iter().cloned());
                        }
                    }
                    return Some(result);
                }
            }
        }
        if crate_name.as_str() == "alloc" && self.tcx.is_foreign_item(target.def_id()) {
            if name.as_str() == "__rust_realloc" {
                return arguments.first().map(|(_, values)| values.clone());
            }
            if name.as_str() == "__rust_dealloc" {
                return Some(Tree::new());
            }
        }
        let pointer_operation = self.tcx.def_path_str(target.def_id()).contains("::ptr::");
        if crate_name.as_str() != "core"
            || !(matches!(target.def, InstanceKind::Intrinsic(_)) || pointer_operation)
        {
            return None;
        }
        match name.as_str() {
            "size_of_val"
            | "align_of_val"
            | "min_align_of_val"
            | "ptr_guaranteed_cmp"
            | "ptr_offset_from"
            | "ptr_offset_from_unsigned" => Some(Tree::new()),
            "read_via_copy"
            | "volatile_load"
            | "unaligned_volatile_load"
            | "read"
            | "read_volatile"
            | "read_unaligned"
                if arguments.len() == 1 =>
            {
                let mut result = Tree::new();
                for fact in arguments[0].1.values().flatten() {
                    match fact {
                        Fact::Reference(address) if address.pointee == Some(result_ty) => {
                            for (path, facts) in self.read(address) {
                                result.entry(path).or_default().extend(facts);
                            }
                        }
                        Fact::Reference(_) | Fact::Unknown => {
                            result.entry(Vec::new()).or_default().insert(Fact::Unknown);
                        }
                        _ => {}
                    }
                }
                Some(result)
            }
            "write_via_move"
            | "volatile_store"
            | "unaligned_volatile_store"
            | "write"
            | "write_volatile"
            | "write_unaligned"
                if arguments.len() == 2 =>
            {
                for fact in arguments[0].1.values().flatten() {
                    if let Fact::Reference(address) = fact {
                        if address.pointee == Some(arguments[1].0) {
                            self.write(address, &arguments[1].1);
                        } else {
                            self.write(
                                address,
                                &HashMap::from([(Vec::new(), HashSet::from([Fact::Unknown]))]),
                            );
                        }
                    }
                }
                Some(Tree::new())
            }
            "copy" | "copy_nonoverlapping" if arguments.len() == 3 => {
                self.copy_memory(
                    &arguments[0].1,
                    &arguments[1].1,
                    arguments[0].0.builtin_deref(true),
                );
                Some(Tree::new())
            }
            "offset" | "arith_offset" | "add" | "sub" | "wrapping_offset" | "wrapping_add"
            | "wrapping_sub"
                if arguments.len() == 2 =>
            {
                let mut result = arguments[0].1.clone();
                let element = self
                    .pointer_layout(arguments[0].0)
                    .map(|(_, element)| element);
                if result.values().flatten().any(|fact| matches!(fact, Fact::Reference(address) if address.fields.last() != Some(&u32::MAX) || address.pointee != element)) {
                    result.entry(Vec::new()).or_default().insert(Fact::Unknown);
                }
                Some(result)
            }
            _ => None,
        }
    }

    fn inner_path(
        &self,
        source: Ty<'tcx>,
        destination: Ty<'tcx>,
        depth: usize,
    ) -> Option<Vec<u32>> {
        if source == destination {
            return Some(Vec::new());
        }
        if depth >= 16 {
            return None;
        }
        let (inner, field) = match source.kind() {
            ty::Adt(adt, args) if self.tcx.crate_name(adt.did().krate).as_str() == "core" => {
                let field = match self.tcx.item_name(adt.did()).as_str() {
                    "UnsafeCell" | "SyncUnsafeCell" | "ManuallyDrop" | "MaybeDangling"
                    | "NonNull" | "Unique" => 0,
                    "MaybeUninit" => 1,
                    _ => return None,
                };
                (
                    adt.non_enum_variant().fields[rustc_abi::FieldIdx::from_usize(field)]
                        .ty(self.tcx, args),
                    field as u32,
                )
            }
            ty::Array(element, _) | ty::Slice(element) => (*element, u32::MAX),
            _ => return None,
        };
        let mut path = vec![field];
        path.extend(self.inner_path(inner, destination, depth + 1)?);
        Some(path)
    }

    fn pointer_layout(&self, ty: Ty<'tcx>) -> Option<(Vec<u32>, Ty<'tcx>)> {
        match ty.kind() {
            ty::RawPtr(pointee, _) | ty::Ref(_, pointee, _) => Some((Vec::new(), *pointee)),
            ty::Adt(adt, args)
                if self.tcx.crate_name(adt.did().krate).as_str() == "core"
                    && matches!(self.tcx.item_name(adt.did()).as_str(), "NonNull" | "Unique") =>
            {
                let field = adt.non_enum_variant().fields[rustc_abi::FieldIdx::from_usize(0)]
                    .ty(self.tcx, args);
                let (path, pointee) = self.pointer_layout(field)?;
                let mut output = vec![0];
                output.extend(path);
                Some((output, pointee))
            }
            _ => None,
        }
    }

    fn copy_memory(
        &mut self,
        sources: &Tree<'tcx>,
        destinations: &Tree<'tcx>,
        element: Option<Ty<'tcx>>,
    ) {
        let mut values = Tree::new();
        for fact in sources.values().flatten() {
            match fact {
                Fact::Reference(address) if address.pointee == element && element.is_some() => {
                    for (path, facts) in self.read(address) {
                        values.entry(path).or_default().extend(facts);
                    }
                }
                Fact::Reference(_) | Fact::Unknown => {
                    values.entry(Vec::new()).or_default().insert(Fact::Unknown);
                }
                _ => {}
            }
        }
        for fact in destinations.values().flatten() {
            if let Fact::Reference(address) = fact {
                if address.pointee != element {
                    self.write(
                        address,
                        &HashMap::from([(Vec::new(), HashSet::from([Fact::Unknown]))]),
                    );
                } else {
                    self.write(address, &values);
                }
            }
        }
    }

    fn cast_pointer(
        &self,
        values: HashSet<Fact<'tcx>>,
        source: Ty<'tcx>,
        destination: Ty<'tcx>,
    ) -> HashSet<Fact<'tcx>> {
        let mut output = HashSet::new();
        for fact in values {
            if let Fact::Reference(mut address) = fact {
                if source != destination
                    && destination != self.tcx.types.u8
                    && destination != self.tcx.types.unit
                    && address.pointee != Some(destination)
                {
                    if let Some(path) = self.inner_path(source, destination, 0).or_else(|| {
                        address
                            .pointee
                            .and_then(|source| self.inner_path(source, destination, 0))
                    }) {
                        address.fields.extend(path);
                        address.pointee = Some(destination);
                    } else {
                        output.insert(Fact::Unknown);
                        continue;
                    }
                }
                output.insert(Fact::Reference(address));
            } else {
                output.insert(fact);
            }
        }
        output
    }

    fn read(&mut self, address: &Address<'tcx>) -> Tree<'tcx> {
        if let Some(active) = self.active {
            self.readers
                .entry(address.base.clone())
                .or_default()
                .insert(active);
        }
        let mut tree = Tree::new();
        if matches!(address.base, Base::Unknown(_)) {
            tree.entry(Vec::new()).or_default().insert(Fact::Unknown);
        }
        let Some(memory) = self.memory.get(&address.base) else {
            return tree;
        };
        for (fields, facts) in memory {
            if let Some(path) = fields.strip_prefix(address.fields.as_slice()) {
                tree.entry(path.to_vec())
                    .or_default()
                    .extend(facts.iter().cloned());
            } else if address.fields.starts_with(fields) && facts.contains(&Fact::Unknown) {
                tree.entry(Vec::new()).or_default().insert(Fact::Unknown);
            }
        }
        tree
    }

    fn write(&mut self, address: &Address<'tcx>, tree: &Tree<'tcx>) {
        let revision = self.revision;
        for (path, facts) in tree {
            let mut address = address.clone();
            address.fields.extend(path);
            if address.fields.len() > 12 {
                address.fields.truncate(12);
                if self
                    .memory
                    .entry(address.base.clone())
                    .or_default()
                    .entry(address.fields.clone())
                    .or_default()
                    .insert(Fact::Unknown)
                {
                    self.revision += 1;
                }
            }
            let memory = self.memory.entry(address.base.clone()).or_default();
            if memory.len() >= 32 && !memory.contains_key(&address.fields) {
                if memory.entry(Vec::new()).or_default().insert(Fact::Unknown) {
                    self.revision += 1;
                }
                continue;
            }
            let output = self
                .memory
                .entry(address.base)
                .or_default()
                .entry(address.fields)
                .or_default();
            let count = output.len();
            let mut references = output
                .iter()
                .filter(|fact| matches!(fact, Fact::Reference(_)))
                .count();
            for fact in facts {
                if matches!(fact, Fact::Reference(_)) && !output.contains(fact) {
                    if references >= 16 {
                        output.insert(Fact::Unknown);
                        continue;
                    }
                    references += 1;
                }
                output.insert(fact.clone());
            }
            self.revision += output.len() - count;
        }
        if revision != self.revision {
            for reader in self.readers.get(&address.base).cloned().unwrap_or_default() {
                self.schedule(reader);
            }
        }
    }

    fn schedule(&mut self, node: Node<'tcx>) {
        if self.queued.insert(node) {
            self.pending.push_back(node);
        }
    }

    fn context(&mut self, instance: Instance<'tcx>, arguments: Vec<Tree<'tcx>>) -> Node<'tcx> {
        if !self.contextual || self.active.is_some_and(|node| node.context == 0) {
            let node = Node {
                instance,
                context: 0,
            };
            if !self.graph.contains_key(&node) {
                self.schedule(node);
            }
            return node;
        }
        let mut ancestor = self.active;
        while let Some(node) = ancestor {
            if node.instance == instance {
                return node;
            }
            ancestor = self.parents.get(&node).copied();
        }
        let contexts = self.contexts.entry(instance).or_default();
        let context = if let Some(index) = contexts.iter().position(|input| *input == arguments) {
            index + 1
        } else {
            contexts.push(arguments);
            if let Some(parent) = self.active {
                self.parents.insert(
                    Node {
                        instance,
                        context: contexts.len(),
                    },
                    parent,
                );
            }
            contexts.len()
        };
        let node = Node { instance, context };
        if !self.graph.contains_key(&node) {
            self.schedule(node);
        }
        node
    }

    fn tracks(&mut self, ty: Ty<'tcx>, visiting: &mut HashSet<Ty<'tcx>>) -> bool {
        if let Some(tracked) = self.tracked.get(&ty) {
            return *tracked;
        }
        if !visiting.insert(ty) {
            return true;
        }
        let tracked = match ty.kind() {
            ty::FnPtr(..)
            | ty::FnDef(..)
            | ty::Dynamic(..)
            | ty::Closure(..)
            | ty::Coroutine(..)
            | ty::CoroutineClosure(..)
            | ty::Param(..)
            | ty::Alias(..) => true,
            ty::Ref(_, ty, _) | ty::RawPtr(ty, _) | ty::Array(ty, _) | ty::Slice(ty) => {
                self.tracks(*ty, visiting)
            }
            ty::Tuple(fields) => fields.iter().any(|ty| self.tracks(ty, visiting)),
            ty::Adt(adt, args) => adt
                .all_fields()
                .any(|field| self.tracks(field.ty(self.tcx, args), visiting)),
            _ => false,
        };
        visiting.remove(&ty);
        self.tracked.insert(ty, tracked);
        tracked
    }

    fn write_place(
        &mut self,
        instance: Instance<'tcx>,
        body: &Body<'tcx>,
        place: Place<'tcx>,
        values: &Tree<'tcx>,
    ) {
        let ty = self.ty(instance, place.ty(body, self.tcx).ty);
        if place
            .projection
            .iter()
            .any(|projection| matches!(projection, ProjectionElem::Deref))
        {
            for address in self.places(instance, body, place) {
                if address.pointee.is_some_and(|pointee| pointee != ty) {
                    self.write(
                        &address,
                        &HashMap::from([(Vec::new(), HashSet::from([Fact::Unknown]))]),
                    );
                }
            }
        }
        if matches!(
            ty.kind(),
            ty::Bool | ty::Char | ty::Int(_) | ty::Uint(_) | ty::Float(_)
        ) && !values
            .values()
            .flatten()
            .any(|fact| matches!(fact, Fact::Function(_)))
        {
            return;
        }
        if !self.tracks(ty, &mut HashSet::new()) {
            let mut relevant = false;
            for fact in values.values().flatten() {
                relevant |= match fact {
                    Fact::Function(_) | Fact::Concrete(_) => true,
                    Fact::Reference(address) => {
                        matches!(address.base, Base::Heap(..))
                            || address
                                .pointee
                                .is_some_and(|ty| self.tracks(ty, &mut HashSet::new()))
                    }
                    _ => false,
                };
            }
            if !relevant {
                return;
            }
        }
        for address in self.places(instance, body, place) {
            self.write(&address, values);
        }
    }

    fn places(
        &mut self,
        instance: Instance<'tcx>,
        body: &Body<'tcx>,
        place: Place<'tcx>,
    ) -> Vec<Address<'tcx>> {
        let mut addresses = vec![Address {
            base: Base::Local(instance, self.local_context, place.local.as_usize()),
            fields: Vec::new(),
            pointee: None,
        }];
        for (index, projection) in place.projection.iter().enumerate() {
            match projection {
                ProjectionElem::Deref => {
                    addresses = addresses
                        .iter()
                        .flat_map(|address| {
                            self.read(address).remove(&Vec::new()).unwrap_or_default()
                        })
                        .filter_map(|fact| {
                            if let Fact::Reference(address) = fact {
                                Some(address)
                            } else {
                                None
                            }
                        })
                        .collect();
                }
                ProjectionElem::Field(field, _) => {
                    let field_ty = self.ty(
                        instance,
                        Place {
                            local: place.local,
                            projection: self.tcx.mk_place_elems(&place.projection[..=index]),
                        }
                        .ty(body, self.tcx)
                        .ty,
                    );
                    for address in &mut addresses {
                        address.fields.push(field.as_u32());
                        address.pointee = Some(field_ty);
                    }
                }
                ProjectionElem::Index(_) | ProjectionElem::ConstantIndex { .. } => {
                    let field_ty = self.ty(
                        instance,
                        Place {
                            local: place.local,
                            projection: self.tcx.mk_place_elems(&place.projection[..=index]),
                        }
                        .ty(body, self.tcx)
                        .ty,
                    );
                    for address in &mut addresses {
                        address.fields.push(u32::MAX);
                        address.pointee = Some(field_ty);
                    }
                }
                ProjectionElem::Subslice { .. } => {
                    let slice_ty = self.ty(
                        instance,
                        Place {
                            local: place.local,
                            projection: self.tcx.mk_place_elems(&place.projection[..=index]),
                        }
                        .ty(body, self.tcx)
                        .ty,
                    );
                    for address in &mut addresses {
                        address.pointee = Some(slice_ty);
                    }
                }
                ProjectionElem::Downcast(..)
                | ProjectionElem::OpaqueCast(..)
                | ProjectionElem::UnwrapUnsafeBinder(..) => {}
            }
        }
        let _ = body;
        addresses
    }

    fn place_value(
        &mut self,
        instance: Instance<'tcx>,
        body: &Body<'tcx>,
        place: Place<'tcx>,
    ) -> Tree<'tcx> {
        let mut result = Tree::new();
        for (index, projection) in place.projection.iter().enumerate() {
            if matches!(projection, ProjectionElem::Deref) {
                let pointer = Place {
                    local: place.local,
                    projection: self.tcx.mk_place_elems(&place.projection[..index]),
                };
                if self
                    .place_value(instance, body, pointer)
                    .values()
                    .flatten()
                    .any(|fact| matches!(fact, Fact::Unknown))
                {
                    result.entry(Vec::new()).or_default().insert(Fact::Unknown);
                }
            }
        }
        for address in self.places(instance, body, place) {
            let ty = self.ty(instance, place.ty(body, self.tcx).ty);
            if address.pointee.is_some_and(|pointee| pointee != ty) {
                result.entry(Vec::new()).or_default().insert(Fact::Unknown);
                continue;
            }
            for (path, values) in self.read(&address) {
                result.entry(path).or_default().extend(values);
            }
        }
        result
    }

    fn operand(
        &mut self,
        instance: Instance<'tcx>,
        body: &Body<'tcx>,
        operand: &Operand<'tcx>,
    ) -> Tree<'tcx> {
        let ty = self.ty(instance, operand.ty(body, self.tcx));
        if let ty::FnDef(def, args) = *ty.kind() {
            return HashMap::from([(
                Vec::new(),
                HashSet::from([Instance::try_resolve(
                    self.tcx,
                    TypingEnv::fully_monomorphized(),
                    def,
                    args,
                )
                .ok()
                .flatten()
                .map_or(Fact::Unknown, Fact::Function)]),
            )]);
        }
        match operand {
            Operand::Copy(place) | Operand::Move(place) => self.place_value(instance, body, *place),
            Operand::Constant(constant) => {
                let constant = instance.instantiate_mir_and_normalize_erasing_regions(
                    self.tcx,
                    TypingEnv::fully_monomorphized(),
                    EarlyBinder::bind(constant.const_),
                );
                if let Some(value) = self.constants.get(&constant) {
                    return value.clone();
                }
                let value =
                    match constant.eval(self.tcx, TypingEnv::fully_monomorphized(), body.span) {
                        Ok(ConstValue::Scalar(Scalar::Ptr(pointer, _))) => self.pointer(
                            pointer.provenance.alloc_id(),
                            pointer.prov_and_relative_offset().1.bytes(),
                            ty,
                        ),
                        Ok(ConstValue::Indirect { alloc_id, offset }) => {
                            self.allocation(alloc_id, offset.bytes(), ty)
                        }
                        Ok(ConstValue::Slice { alloc_id, .. }) => self.pointer(alloc_id, 0, ty),
                        Ok(_) => Tree::new(),
                        Err(_) => HashMap::from([(Vec::new(), HashSet::from([Fact::Unknown]))]),
                    };
                self.constants.insert(constant, value.clone());
                value
            }
            _ => Tree::new(),
        }
    }

    fn pointer(&mut self, allocation: AllocId, offset: u64, ty: Ty<'tcx>) -> Tree<'tcx> {
        let fact = match self.tcx.global_alloc(allocation) {
            GlobalAlloc::Function { instance } => Fact::Function(instance),
            GlobalAlloc::VTable(concrete, _) => Fact::Concrete(concrete),
            GlobalAlloc::Static(def) => {
                if self.statics.insert(def) && !self.tcx.is_foreign_item(def) {
                    if let Ok(value) = self.tcx.eval_static_initializer(def) {
                        let ty = self.tcx.type_of(def).instantiate_identity();
                        let allocation = self.tcx.reserve_and_set_memory_alloc(value);
                        let values = self.allocation(allocation, 0, ty);
                        self.write(
                            &Address {
                                base: Base::Static(def),
                                fields: Vec::new(),
                                pointee: Some(ty),
                            },
                            &values,
                        );
                    }
                }
                if offset != 0 {
                    Fact::Unknown
                } else {
                    Fact::Reference(Address {
                        base: Base::Static(def),
                        fields: Vec::new(),
                        pointee: ty.builtin_deref(true),
                    })
                }
            }
            GlobalAlloc::Memory(_) => {
                let address = Address {
                    base: Base::Allocation(allocation, offset),
                    fields: Vec::new(),
                    pointee: ty.builtin_deref(true),
                };
                if let Some(pointee) = ty.builtin_deref(true) {
                    if self.loaded.insert((allocation, offset, pointee)) {
                        let values = self.allocation(allocation, offset, pointee);
                        self.write(&address, &values);
                    }
                }
                Fact::Reference(address)
            }
            _ => Fact::Unknown,
        };
        HashMap::from([(Vec::new(), HashSet::from([fact]))])
    }

    fn allocation(&mut self, id: AllocId, offset: u64, ty: Ty<'tcx>) -> Tree<'tcx> {
        let mut result = Tree::new();
        let GlobalAlloc::Memory(memory) = self.tcx.global_alloc(id) else {
            return result;
        };
        let memory = memory.inner();
        let Ok(layout) = self
            .tcx
            .layout_of(TypingEnv::fully_monomorphized().as_query_input(ty))
        else {
            return HashMap::from([(Vec::new(), HashSet::from([Fact::Unknown]))]);
        };
        match ty.kind() {
            ty::FnPtr(..) | ty::Ref(..) | ty::RawPtr(..) => {
                let range = AllocRange {
                    start: Size::from_bytes(offset),
                    size: self.tcx.data_layout.pointer_size(),
                };
                if range.end().bytes() <= memory.len() as u64 {
                    if let Ok(Scalar::Ptr(pointer, _)) = memory.read_scalar(&self.tcx, range, true)
                    {
                        result = self.pointer(
                            pointer.provenance.alloc_id(),
                            pointer.prov_and_relative_offset().1.bytes(),
                            ty,
                        );
                    }
                    if layout.size > range.size {
                        let range = AllocRange {
                            start: range.end(),
                            size: range.size,
                        };
                        if range.end().bytes() <= memory.len() as u64 {
                            if let Ok(Scalar::Ptr(pointer, _)) =
                                memory.read_scalar(&self.tcx, range, true)
                            {
                                for (path, facts) in self.pointer(
                                    pointer.provenance.alloc_id(),
                                    pointer.prov_and_relative_offset().1.bytes(),
                                    ty,
                                ) {
                                    result.entry(path).or_default().extend(facts);
                                }
                            }
                        }
                    }
                }
            }
            ty::Tuple(fields) => {
                for (index, ty) in fields.iter().enumerate() {
                    for (path, facts) in
                        self.allocation(id, offset + layout.fields.offset(index).bytes(), ty)
                    {
                        let mut output = vec![index as u32];
                        output.extend(path);
                        result.entry(output).or_default().extend(facts);
                    }
                }
            }
            ty::Array(element, count) => {
                if let Some(count) = count.try_to_target_usize(self.tcx) {
                    for index in 0..count.min(4096) {
                        for (path, facts) in self.allocation(
                            id,
                            offset + layout.fields.offset(index as usize).bytes(),
                            *element,
                        ) {
                            let mut output = vec![u32::MAX];
                            output.extend(path);
                            result.entry(output).or_default().extend(facts);
                        }
                    }
                    if count > 4096 {
                        result.entry(Vec::new()).or_default().insert(Fact::Unknown);
                    }
                }
            }
            ty::Adt(adt, args) => {
                for (variant_index, variant) in adt.variants().iter_enumerated() {
                    let variant_layout = layout.for_variant(
                        &ty::layout::LayoutCx::new(self.tcx, TypingEnv::fully_monomorphized()),
                        variant_index,
                    );
                    for (index, field) in variant.fields.iter_enumerated() {
                        let ty = field.ty(self.tcx, args);
                        for (path, facts) in self.allocation(
                            id,
                            offset + variant_layout.fields.offset(index.as_usize()).bytes(),
                            ty,
                        ) {
                            let mut output = vec![index.as_u32()];
                            output.extend(path);
                            result.entry(output).or_default().extend(facts);
                        }
                    }
                }
            }
            _ => {}
        }
        result
    }
}
