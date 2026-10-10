mod worker;

use crate::trust::Trust;
use crate::{Edge, forbidden, known_intrinsic};
use rustc_abi::Size;
use rustc_hir::attrs::lang_items::LangItem;
use rustc_hir::def_id::DefId;
use rustc_middle::mir::interpret::{AllocId, AllocRange, GlobalAlloc, Scalar};
use rustc_middle::mir::visit::{PlaceContext, Visitor};
use rustc_middle::mir::{
    self, AggregateKind, Body, CastKind, ConstValue, Operand, Place, ProjectionElem, Rvalue,
    StatementKind, TerminatorKind,
};
use rustc_middle::ty::adjustment::PointerCoercion;
use rustc_middle::ty::{
    self, EarlyBinder, Instance, InstanceKind, ShimKind, Ty, TyCtxt, TypingEnv,
};
use rustc_mir_dataflow::Analysis as DataflowAnalysis;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use irq_check_rupta::pts_set::points_to::PointsToSet;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum Base<'tcx> {
    Local(Instance<'tcx>, usize, usize),
    Parameter(Instance<'tcx>, usize, usize, Ty<'tcx>),
    Heap(Instance<'tcx>, usize, usize, Ty<'tcx>),
    Static(DefId),
    Allocation(AllocId, u64),
    Unknown(Ty<'tcx>),
    Havoc,
}

impl<'tcx> Base<'tcx> {
    fn discovery_storage(&self) -> Self {
        match self {
            Self::Local(owner, _, local) => Self::Local(*owner, 0, *local),
            Self::Parameter(owner, _, slot, ty) => Self::Parameter(*owner, 0, *slot, *ty),
            Self::Heap(owner, _, site, ty) => Self::Heap(*owner, 0, *site, *ty),
            other => other.clone(),
        }
    }
}

struct BackendStorage<'tcx> {
    owner: Option<irq_check_rupta::mir::function::FuncId>,
    instance: Option<Instance<'tcx>>,
    mir_path: Option<std::rc::Rc<irq_check_rupta::mir::path::Path>>,
    object_path: std::rc::Rc<irq_check_rupta::mir::path::Path>,
    ty: Ty<'tcx>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Address<'tcx> {
    base: Base<'tcx>,
    fields: Vec<u32>,
    pointee: Option<Ty<'tcx>>,
    byte_offset: i128,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum PointerOrigin<'tcx> {
    Storage(Address<'tcx>),
    Function(Instance<'tcx>),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum Fact<'tcx> {
    Function(Instance<'tcx>),
    Reference(Address<'tcx>),
    Concrete(Ty<'tcx>),
    Scalar(u128),
    Length(u128),
    ExposedPointer(PointerOrigin<'tcx>, u64),
    IntegerPointer(u128),
    Unknown,
}

#[derive(Debug, Default)]
struct TreeStorage<'tcx> {
    values: HashMap<Vec<u32>, HashSet<Fact<'tcx>>>,
    references: std::sync::OnceLock<Vec<Base<'tcx>>>,
    fingerprint: std::sync::OnceLock<(usize, u64)>,
    normalized: std::sync::OnceLock<bool>,
}

impl Clone for TreeStorage<'_> {
    fn clone(&self) -> Self {
        Self {
            values: self.values.clone(),
            references: std::sync::OnceLock::new(),
            fingerprint: std::sync::OnceLock::new(),
            normalized: std::sync::OnceLock::new(),
        }
    }
}

#[derive(Clone, Default)]
struct Tree<'tcx>(std::sync::Arc<TreeStorage<'tcx>>);

impl std::fmt::Debug for Tree<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_tuple("Tree").field(&self.0.values).finish()
    }
}

impl PartialEq for Tree<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0) || self.0.values == other.0.values
    }
}

impl Eq for Tree<'_> {}

impl Hash for Tree<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0
            .fingerprint
            .get_or_init(|| {
                let mut sum = 0u64;
                for (path, facts) in self {
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    path.hash(&mut hasher);
                    facts.len().hash(&mut hasher);
                    let mut values = 0u64;
                    for fact in facts {
                        let mut value = std::collections::hash_map::DefaultHasher::new();
                        fact.hash(&mut value);
                        values = values.wrapping_add(value.finish());
                    }
                    values.hash(&mut hasher);
                    sum = sum.wrapping_add(hasher.finish());
                }
                (self.len(), sum)
            })
            .hash(state);
    }
}

impl<'tcx> Tree<'tcx> {
    fn new() -> Self {
        Self::default()
    }

    fn is_normalized(&self) -> bool {
        *self.0.normalized.get_or_init(|| self.values().all(|facts| {
            let numeric = facts.iter().filter(|fact|
                matches!(fact, Fact::Scalar(_) | Fact::Length(_) | Fact::IntegerPointer(_))).count();
            numeric <= 8 && (numeric == 0 || !facts.contains(&Fact::Unknown))
        }))
    }

    fn join(&mut self, other: &Self) -> bool {
        if self == other && self.is_normalized() {
            return false;
        }
        let normalized = self.is_normalized();
        let mut changed = false;
        for (path, facts) in other {
            if normalized && self.get(path) == Some(facts) {
                continue;
            }
            changed |= join_facts(self.entry(path.clone()).or_default(), facts.iter().cloned());
        }
        changed
    }

    fn references(&self) -> &[Base<'tcx>] {
        self.0.references.get_or_init(|| {
            self.values()
                .flatten()
                .filter_map(|fact| match fact {
                    Fact::Reference(address)
                    | Fact::ExposedPointer(PointerOrigin::Storage(address), _) => {
                        Some(address.base.clone())
                    }
                    _ => None,
                })
                .collect()
        })
    }

    fn project(&self, projection: &[u32]) -> Self {
        if projection.is_empty() {
            return self.clone();
        }
        let mut result = Self::new();
        for (fields, facts) in self {
            if !fields.iter().zip(projection)
                .all(|(a, b)| a == b || *a == u32::MAX || *b == u32::MAX)
            {
                continue;
            }
            if fields.len() >= projection.len() {
                result.entry(fields[projection.len()..].to_vec())
                    .or_default().extend(facts.iter().cloned());
            } else if facts.contains(&Fact::Unknown) {
                result.entry(Vec::new()).or_default().insert(Fact::Unknown);
            }
        }
        result
    }
}

impl<'tcx> std::ops::Deref for Tree<'tcx> {
    type Target = HashMap<Vec<u32>, HashSet<Fact<'tcx>>>;

    fn deref(&self) -> &Self::Target {
        &self.0.values
    }
}

impl std::ops::DerefMut for Tree<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        let storage = std::sync::Arc::make_mut(&mut self.0);
        storage.references.take();
        storage.fingerprint.take();
        storage.normalized.take();
        &mut storage.values
    }
}

impl<'tcx, const N: usize> From<[(Vec<u32>, HashSet<Fact<'tcx>>); N]> for Tree<'tcx> {
    fn from(value: [(Vec<u32>, HashSet<Fact<'tcx>>); N]) -> Self {
        value.into_iter().collect()
    }
}

impl<'tcx> FromIterator<(Vec<u32>, HashSet<Fact<'tcx>>)> for Tree<'tcx> {
    fn from_iter<T: IntoIterator<Item = (Vec<u32>, HashSet<Fact<'tcx>>)>>(iter: T) -> Self {
        Self(std::sync::Arc::new(TreeStorage {
            values: iter.into_iter().collect(),
            references: std::sync::OnceLock::new(),
            fingerprint: std::sync::OnceLock::new(),
            normalized: std::sync::OnceLock::new(),
        }))
    }
}

impl<'tcx> IntoIterator for Tree<'tcx> {
    type Item = (Vec<u32>, HashSet<Fact<'tcx>>);
    type IntoIter = std::collections::hash_map::IntoIter<Vec<u32>, HashSet<Fact<'tcx>>>;

    fn into_iter(self) -> Self::IntoIter {
        std::sync::Arc::try_unwrap(self.0)
            .unwrap_or_else(|shared| (*shared).clone())
            .values
            .into_iter()
    }
}

impl<'a, 'tcx> IntoIterator for &'a Tree<'tcx> {
    type Item = (&'a Vec<u32>, &'a HashSet<Fact<'tcx>>);
    type IntoIter = std::collections::hash_map::Iter<'a, Vec<u32>, HashSet<Fact<'tcx>>>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.values.iter()
    }
}

type FlowState<'tcx> = HashMap<Base<'tcx>, Tree<'tcx>>;

fn bind_tree<'tcx>(tree: &Tree<'tcx>, binding: &HashMap<Base<'tcx>, Base<'tcx>>) -> Tree<'tcx> {
    if tree.references().iter().all(|base| binding.get(base).is_none_or(|target| target == base))
        && tree.is_normalized()
    {
        return tree.clone();
    }
    tree.iter()
        .map(|(path, facts)| {
            let facts: HashSet<_> = facts
                .iter()
                .map(|fact| {
                    let mut fact = fact.clone();
                    match &mut fact {
                        Fact::Reference(address)
                        | Fact::ExposedPointer(PointerOrigin::Storage(address), _) => {
                            if let Some(base) = binding.get(&address.base) {
                                address.base = base.clone();
                            }
                        }
                        _ => {}
                    }
                    fact
                })
                .collect();
            let mut normalized = HashSet::new();
            join_facts(&mut normalized, facts);
            (path.clone(), normalized)
        })
        .collect()
}

fn bind_memory<'tcx>(
    memory: &FlowState<'tcx>,
    binding: &HashMap<Base<'tcx>, Base<'tcx>>,
) -> FlowState<'tcx> {
    let entries: Vec<_> = memory.iter().collect();
    let workers = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    let prepare = |chunk: &[(&Base<'tcx>, &Tree<'tcx>)]| {
        chunk
            .iter()
            .map(|(base, tree)| {
                (
                    binding.get(base).unwrap_or(base).clone(),
                    bind_tree(tree, binding),
                )
            })
            .collect::<Vec<_>>()
    };
    let prepared: Vec<Vec<_>> = if workers > 1 && entries.len() >= workers * 4 {
        rustc_data_structures::sync::par_map(
            entries
                .chunks(entries.len().div_ceil(workers))
                .collect::<Vec<_>>(),
            prepare,
        )
    } else {
        vec![prepare(&entries)]
    };
    let mut output = FlowState::new();
    for (base, tree) in prepared.into_iter().flatten() {
        match output.entry(base) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(tree);
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                for (path, facts) in tree {
                    join_facts(entry.get_mut().entry(path).or_default(), facts);
                }
            }
        }
    }
    output
}

fn join_facts<'tcx>(
    output: &mut HashSet<Fact<'tcx>>,
    facts: impl IntoIterator<Item = Fact<'tcx>>,
) -> bool {
    let previous_len = output.len();
    let mut inserted = false;
    let mut inserted_retained = false;
    for fact in facts {
        let retained = !matches!(
            fact,
            Fact::Scalar(_) | Fact::Length(_) | Fact::IntegerPointer(_)
        );
        if output.insert(fact) {
            inserted = true;
            inserted_retained |= retained;
        }
    }
    if output.contains(&Fact::Unknown)
        || output
            .iter()
            .filter(|fact| {
                matches!(
                    fact,
                    Fact::Scalar(_) | Fact::Length(_) | Fact::IntegerPointer(_)
                )
            })
            .count()
            > 8
    {
        output.retain(|fact| {
            !matches!(
                fact,
                Fact::Scalar(_) | Fact::Length(_) | Fact::IntegerPointer(_)
            )
        });
        inserted_retained |= output.insert(Fact::Unknown);
        return inserted_retained || output.len() != previous_len;
    }
    inserted
}
#[derive(Clone, PartialEq)]
struct ContextInput<'tcx> {
    arguments: Vec<Tree<'tcx>>,
    memory: FlowState<'tcx>,
    singletons: HashSet<Base<'tcx>>,
    key: ContextKey<'tcx>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct ContextKey<'tcx>(Vec<Tree<'tcx>>, FlowState<'tcx>);

impl Hash for ContextKey<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.len().hash(state);
        for argument in &self.0 {
            argument.hash(state);
        }
        self.1.len().hash(state);
        let mut memory = 0u64;
        for (base, tree) in &self.1 {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            base.hash(&mut hasher);
            tree.hash(&mut hasher);
            memory = memory.wrapping_add(hasher.finish());
        }
        memory.hash(state);
    }
}
#[derive(Clone, PartialEq)]
struct ReturnAlternative<'tcx> {
    value: Tree<'tcx>,
    memory: FlowState<'tcx>,
    key: ContextKey<'tcx>,
}
struct OpaqueRead<'tcx> {
    stored: Tree<'tcx>,
    havoc: Tree<'tcx>,
    local: Option<Option<Tree<'tcx>>>,
    local_havoc: Option<Tree<'tcx>>,
    facts: std::sync::Arc<Vec<Fact<'tcx>>>,
}

struct OpaqueTraversal<'tcx> {
    arguments: Vec<(Ty<'tcx>, Tree<'tcx>)>,
    locals: FlowState<'tcx>,
    dependencies: Option<HashSet<Base<'tcx>>>,
    has_locals: bool,
    local_callbacks: Option<HashSet<Instance<'tcx>>>,
    known_callbacks: usize,
    epoch: usize,
    escaped: usize,
    statics: usize,
    written_allocations: usize,
    visited: usize,
    writes: std::sync::Arc<Vec<Address<'tcx>>>,
    callbacks: std::sync::Arc<HashSet<Instance<'tcx>>>,
}

struct OpaqueScratch<'tcx> {
    batch: crate::pointer::WorkVec<Address<'tcx>>,
    hits: usize,
    misses: crate::pointer::WorkVec<Address<'tcx>>,
    next: crate::pointer::WorkSet<Address<'tcx>>,
    targets: crate::pointer::WorkSet<Instance<'tcx>>,
    rebuilt: crate::pointer::WorkVec<(Base<'tcx>, OpaqueRead<'tcx>)>,
    new_views: crate::pointer::WorkVec<(Base<'tcx>, Tree<'tcx>)>,
    new_escapes: crate::pointer::WorkVec<Base<'tcx>>,
}

fn storage_value<'tcx>(
    stored: &Tree<'tcx>,
    havoc: &Tree<'tcx>,
    local: &Option<Option<Tree<'tcx>>>,
    local_havoc: &Option<Tree<'tcx>>,
    exposed: bool,
    escaped: bool,
    unknown: bool,
) -> Tree<'tcx> {
    let mut global = stored.clone();
    if exposed {
        let mut root = global.get(&Vec::new()).cloned().unwrap_or_default();
        join_facts(&mut root, havoc.values().flatten().cloned());
        if global.get(&Vec::new()) != Some(&root) {
            global.insert(Vec::new(), root);
        }
    }
    let mut memory = match local {
        Some(values) => values.clone().unwrap_or_default(),
        None => global.clone(),
    };
    if local.is_some() && escaped {
        for (fields, facts) in &global {
            let mut merged = memory.get(fields).cloned().unwrap_or_default();
            join_facts(&mut merged, facts.iter().cloned());
            if memory.get(fields) != Some(&merged) {
                memory.insert(fields.clone(), merged);
            }
        }
    }
    if unknown {
        memory.entry(Vec::new()).or_default().insert(Fact::Unknown);
    }
    let extra = if exposed {
        if local.is_some() {
            local_havoc.as_ref()
        } else {
            Some(havoc)
        }
    } else {
        None
    };
    if let Some(extra) = extra.filter(|tree| !tree.is_empty()) {
        let mut root = memory.get(&Vec::new()).cloned().unwrap_or_default();
        join_facts(&mut root, extra.values().flatten().cloned());
        if memory.get(&Vec::new()) != Some(&root) {
            memory.insert(Vec::new(), root);
        }
    }

    memory
}

impl<'tcx> OpaqueRead<'tcx> {
    fn rebuild(
        stored: Tree<'tcx>,
        havoc: Tree<'tcx>,
        local: Option<Option<Tree<'tcx>>>,
        local_havoc: Option<Tree<'tcx>>,
        exposed: bool,
    ) -> Self {
        let memory = storage_value(&stored, &havoc, &local, &local_havoc, exposed, true, false);
        let facts = std::sync::Arc::new(
            memory
                .values()
                .flatten()
                .filter(|fact| {
                    matches!(
                        fact,
                        Fact::Reference(_)
                            | Fact::ExposedPointer(..)
                            | Fact::IntegerPointer(_)
                            | Fact::Function(_)
                    )
                })
                .cloned()
                .collect(),
        );
        Self {
            stored,
            havoc,
            local,
            local_havoc,
            facts,
        }
    }
}
const DISCRIMINANT: u32 = u32::MAX - 1;
const VARIANT: u32 = u32::MAX - 2;

#[derive(Default)]
struct IndexedPlaces<'tcx>(Vec<Place<'tcx>>);

impl<'tcx> Visitor<'tcx> for IndexedPlaces<'tcx> {
    fn visit_place(&mut self, place: &Place<'tcx>, context: PlaceContext, location: mir::Location) {
        if place
            .projection
            .iter()
            .any(|projection| matches!(projection, ProjectionElem::Index(_)))
        {
            self.0.push(*place);
        }
        self.super_place(place, context, location);
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Node<'tcx> {
    pub instance: Instance<'tcx>,
    pub context: usize,
}

pub struct CallSite<'tcx> {
    pub caller: Node<'tcx>,
    pub location: mir::Location,
    pub state: usize,
    pub scope: mir::SourceScope,
    pub normal: Vec<mir::Location>,
    pub unwind: Option<mir::BasicBlock>,
    bindings: Vec<(Node<'tcx>, HashMap<Base<'tcx>, Base<'tcx>>)>,
}

pub struct Analysis<'tcx> {
    tcx: TyCtxt<'tcx>,
    backend: irq_check_rupta::pta::context_sensitive::CallSiteSensitivePTA<'tcx, 'tcx>,
    backend_calls: HashMap<(Instance<'tcx>, mir::Location), HashSet<Instance<'tcx>>>,
    backend_feedback: HashSet<(Instance<'tcx>, mir::Location, Instance<'tcx>)>,
    backend_storage_feedback: HashSet<(Base<'tcx>, Vec<u32>, Instance<'tcx>)>,
    backend_reference_feedback: HashSet<(Base<'tcx>, Vec<u32>, Address<'tcx>)>,
    backend_observed: HashSet<(Instance<'tcx>, mir::Location, Instance<'tcx>)>,
    memory: crate::pointer::DiffPTData<Base<'tcx>, (Vec<u32>, Fact<'tcx>)>,
    memory_views: std::cell::RefCell<HashMap<Base<'tcx>, Tree<'tcx>>>,
    effective_views: std::cell::RefCell<HashMap<Base<'tcx>, (Tree<'tcx>, Tree<'tcx>, Tree<'tcx>)>>,
    effective_view_hits: std::cell::Cell<usize>,
    memory_view_hits: std::cell::Cell<usize>,
    memory_view_misses: std::cell::Cell<usize>,
    memory_epoch: usize,
    opaque_shared_dirty: HashSet<Base<'tcx>>,
    opaque_traversals:
        HashMap<(Node<'tcx>, mir::Location, bool, bool, bool, bool), OpaqueTraversal<'tcx>>,
    opaque_traversal_hits: usize,
    opaque_saved_visits: usize,
    redundant_memory_writes: usize,
    opaque_pending: VecDeque<Address<'tcx>>,
    opaque_visited: HashSet<Address<'tcx>>,
    opaque_scratch: Vec<OpaqueScratch<'tcx>>,
    opaque_calls: usize,
    opaque_addresses: usize,
    opaque_peak_addresses: usize,
    opaque_elapsed: std::time::Duration,
    opaque_reads: HashMap<Base<'tcx>, OpaqueRead<'tcx>>,
    opaque_read_hits: usize,
    opaque_read_misses: usize,
    opaque_parallel_batches: usize,
    opaque_parallel_addresses: usize,
    opaque_parallel_misses: usize,
    opaque_workers: HashSet<std::thread::ThreadId>,
    worker_limit: usize,
    constants: HashMap<mir::Const<'tcx>, Tree<'tcx>>,
    tracked: HashMap<Ty<'tcx>, bool>,
    loaded: HashSet<(AllocId, u64, Ty<'tcx>)>,
    initializing_allocation: bool,
    written_allocations: HashSet<AllocId>,
    statics: HashSet<DefId>,
    escaped: HashSet<Base<'tcx>>,
    escape_aliases: HashMap<Base<'tcx>, HashSet<Base<'tcx>>>,
    escape_readers: HashSet<Node<'tcx>>,
    revision: usize,
    readers: crate::pointer::ShardedMap<Base<'tcx>, HashMap<Vec<u32>, HashSet<Node<'tcx>>>>,
    read_subscriptions: HashMap<Base<'tcx>, HashSet<Vec<u32>>>,
    havoc_subscribed: bool,
    read_subscription_hits: usize,
    unknown_readers: HashSet<Node<'tcx>>,
    known_callbacks: HashSet<Instance<'tcx>>,
    pending: VecDeque<Node<'tcx>>,
    queued: HashSet<Node<'tcx>>,
    active: Option<Node<'tcx>>,
    active_location: mir::Location,
    havoc_origins: HashSet<(Node<'tcx>, mir::Location)>,
    havoc_order: Vec<(Node<'tcx>, mir::Location)>,
    local_context: usize,
    flow_locals: Option<FlowState<'tcx>>,
    singleton_storage: HashSet<Base<'tcx>>,
    pending_bindings: Vec<(Node<'tcx>, HashMap<Base<'tcx>, Base<'tcx>>)>,
    published_locals: HashSet<usize>,
    address_taken: HashMap<Instance<'tcx>, HashSet<usize>>,
    record_effects: bool,
    effects: HashMap<Node<'tcx>, HashMap<Address<'tcx>, Tree<'tcx>>>,
    returns: HashMap<Node<'tcx>, Vec<ReturnAlternative<'tcx>>>,
    effect_readers: HashMap<Node<'tcx>, HashSet<Node<'tcx>>>,
    strong_update: bool,
    settling_unknowns: bool,
    contexts: HashMap<Instance<'tcx>, Vec<ContextInput<'tcx>>>,
    value_only_bodies: HashMap<Instance<'tcx>, bool>,
    return_projections: HashMap<Instance<'tcx>, Option<(usize, Vec<u32>, Option<Ty<'tcx>>)>>,
    context_keys: HashMap<Instance<'tcx>, crate::pointer::ContextCache<ContextKey<'tcx>>>,
    parents: HashMap<Node<'tcx>, Node<'tcx>>,
    pub graph: HashMap<Node<'tcx>, Vec<Edge<'tcx>>>,
    pub roots: HashMap<Instance<'tcx>, Node<'tcx>>,
    pub sites: Vec<CallSite<'tcx>>,
    pub free_sites: HashSet<usize>,
    free_site_slots: Vec<usize>,
    site_index: HashMap<Node<'tcx>, HashMap<(mir::Location, usize), usize>>,
    states: HashMap<Node<'tcx>, crate::pointer::ContextCache<ContextKey<'tcx>>>,
    pub incomplete: bool,
    pub limit_detail: String,
    pub precision_loss: HashSet<String>,
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
            backend: irq_check_rupta::pta::context_sensitive::ContextSensitivePTA::new(
                irq_check_rupta::mir::analysis_context::AnalysisContext::new(
                    tcx.sess,
                    tcx,
                    irq_check_rupta::util::options::AnalysisOptions {
                        dump_stats: false,
                        ..Default::default()
                    },
                    Vec::new(),
                    trust.checked.clone(),
                ),
                irq_check_rupta::pta::strategies::context_strategy::KCallSiteSensitive::new(1),
            ),
            backend_calls: HashMap::new(),
            backend_feedback: HashSet::new(),
            backend_storage_feedback: HashSet::new(),
            backend_reference_feedback: HashSet::new(),
            backend_observed: HashSet::new(),
            memory: crate::pointer::DiffPTData::default(),
            memory_views: std::cell::RefCell::new(HashMap::new()),
            effective_views: std::cell::RefCell::new(HashMap::new()),
            effective_view_hits: std::cell::Cell::new(0),
            memory_view_hits: std::cell::Cell::new(0),
            memory_view_misses: std::cell::Cell::new(0),
            memory_epoch: 0,
            opaque_shared_dirty: HashSet::new(),
            opaque_traversals: HashMap::new(),
            opaque_traversal_hits: 0,
            opaque_saved_visits: 0,
            redundant_memory_writes: 0,
            opaque_pending: VecDeque::new(),
            opaque_visited: HashSet::new(),
            opaque_scratch: (0..std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1))
                .map(|_| OpaqueScratch {
                    batch: crate::pointer::WorkVec::new_in(crate::pointer::WorkerAllocator),
                    hits: 0,
                    misses: crate::pointer::WorkVec::new_in(crate::pointer::WorkerAllocator),
                    next: crate::pointer::WorkSet::default(),
                    targets: crate::pointer::WorkSet::default(),
                    rebuilt: crate::pointer::WorkVec::new_in(crate::pointer::WorkerAllocator),
                    new_views: crate::pointer::WorkVec::new_in(crate::pointer::WorkerAllocator),
                    new_escapes: crate::pointer::WorkVec::new_in(crate::pointer::WorkerAllocator),
                })
                .collect(),
            opaque_calls: 0,
            opaque_addresses: 0,
            opaque_peak_addresses: 0,
            opaque_elapsed: std::time::Duration::ZERO,
            opaque_reads: HashMap::new(),
            opaque_read_hits: 0,
            opaque_read_misses: 0,
            opaque_parallel_batches: 0,
            opaque_parallel_addresses: 0,
            opaque_parallel_misses: 0,
            opaque_workers: HashSet::new(),
            worker_limit: std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1),
            constants: HashMap::new(),
            tracked: HashMap::new(),
            loaded: HashSet::new(),
            initializing_allocation: false,
            written_allocations: HashSet::new(),
            statics: HashSet::new(),
            escaped: HashSet::new(),
            escape_aliases: HashMap::new(),
            escape_readers: HashSet::new(),
            revision: 0,
            readers: crate::pointer::ShardedMap {
                shards: (0..std::thread::available_parallelism()
                    .map(usize::from)
                    .unwrap_or(1))
                    .map(|_| HashMap::new())
                    .collect(),
            },
            unknown_readers: HashSet::new(),
            read_subscriptions: HashMap::new(),
            havoc_subscribed: false,
            read_subscription_hits: 0,
            known_callbacks: HashSet::new(),
            pending: VecDeque::new(),
            queued: HashSet::new(),
            active: None,
            active_location: mir::Location::START,
            havoc_origins: HashSet::new(),
            havoc_order: Vec::new(),
            local_context: 0,
            flow_locals: None,
            singleton_storage: HashSet::new(),
            pending_bindings: Vec::new(),
            published_locals: HashSet::new(),
            address_taken: HashMap::new(),
            record_effects: false,
            effects: HashMap::new(),
            returns: HashMap::new(),
            effect_readers: HashMap::new(),
            strong_update: false,
            settling_unknowns: false,
            contexts: HashMap::new(),
            value_only_bodies: HashMap::new(),
            return_projections: HashMap::new(),
            context_keys: HashMap::new(),
            parents: HashMap::new(),
            graph: HashMap::new(),
            roots: HashMap::new(),
            sites: Vec::new(),
            free_sites: HashSet::new(),
            free_site_slots: Vec::new(),
            site_index: HashMap::new(),
            states: HashMap::new(),
            incomplete: false,
            limit_detail: String::new(),
            precision_loss: HashSet::new(),
        };
        let entry = tcx.entry_fn(()).map(|(def, _)| def);
        let mut functions: HashSet<_> = instances
            .iter()
            .copied()
            .filter(|instance| {
                trust.checks(instance.def_id())
                    && (entry == Some(instance.def_id())
                        || crate::context_method(tcx, instance.def_id())
                        || matches!(
                            tcx.def_kind(instance.def_id()),
                            rustc_hir::def::DefKind::Fn | rustc_hir::def::DefKind::AssocFn
                        ) && tcx
                            .codegen_fn_attrs(instance.def_id())
                            .contains_extern_indicator())
            })
            .collect();
        functions.extend(roots.iter().copied());
        let mut exported: HashMap<_, _> = instances
            .iter()
            .filter(|instance| {
                !tcx.is_foreign_item(instance.def_id()) && matches!(
                    tcx.def_kind(instance.def_id()),
                    rustc_hir::def::DefKind::Fn | rustc_hir::def::DefKind::AssocFn
                )
            })
            .map(|instance| (tcx.symbol_name(*instance).name.to_owned(), *instance))
            .collect();
        {
            use rustc_ast::expand::allocator::{default_fn_name, global_fn_name};
            use rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags;
            use rustc_middle::middle::exported_symbols::ExportedSymbol;
            use rustc_symbol_mangling::mangle_internal_symbol;
            let local_definitions = exported.len();
            let dependencies = tcx.dependency_formats(());
            for &krate in tcx.crates(()) {
                if !dependencies.values().any(|list| list.get(krate).is_some_and(|linkage|
                    *linkage != rustc_middle::middle::dependency_format::Linkage::NotLinked))
                {
                    continue;
                }
                for &(symbol, _) in tcx.exported_non_generic_symbols(krate) {
                    let ExportedSymbol::NonGeneric(def) = symbol else { continue };
                    if !matches!(tcx.def_kind(def), rustc_hir::def::DefKind::Fn | rustc_hir::def::DefKind::AssocFn)
                        || tcx.is_foreign_item(def)
                    {
                        continue;
                    }
                    let instance = Instance::mono(tcx, def);
                    exported.entry(tcx.symbol_name(instance).name.to_owned()).or_insert(instance);
                }
            }
            if std::env::var_os("IRQ_CHECK_STATS").is_some() {
                eprintln!("irq-check: linked Rust definitions local={local_definitions} dependency={}",
                    exported.len() - local_definitions);
            }
            let methods = rustc_codegen_ssa::base::allocator_kind_for_codegen(tcx)
                .map(|kind| rustc_codegen_ssa::base::allocator_shim_contents(tcx, kind))
                .unwrap_or_default();
            for method in methods.into_iter().map(|method| method.name) {
                let default_name = default_fn_name(method);
                let destination = mangle_internal_symbol(tcx, &default_name);
                let mut target = exported.get(&destination).copied();
                if target.is_none() {
                    for &krate in tcx.crates(()) {
                        if !matches!(tcx.crate_name(krate).as_str(), "alloc" | "std") {
                            continue;
                        }
                        for &(symbol, _) in tcx.exported_non_generic_symbols(krate) {
                            let ExportedSymbol::NonGeneric(def) = symbol else { continue };
                            if tcx.def_kind(def) != rustc_hir::def::DefKind::Fn
                                || tcx.is_foreign_item(def)
                                || !tcx.opt_item_name(def).is_some_and(|name| name.as_str() == default_name)
                                || !tcx.codegen_fn_attrs(def).flags.contains(CodegenFnAttrFlags::RUSTC_STD_INTERNAL_SYMBOL)
                            {
                                continue;
                            }
                            let instance = Instance::mono(tcx, def);
                            if tcx.symbol_name(instance).name == destination {
                                target = Some(instance);
                                break;
                            }
                        }
                        if target.is_some() { break; }
                    }
                }
                if let Some(target) = target {
                    let source = mangle_internal_symbol(tcx, &global_fn_name(method));
                    if std::env::var_os("IRQ_CHECK_STATS").is_some() {
                        eprintln!("irq-check: compiler allocator shim {source} -> {target}");
                    }
                    exported.entry(source).or_insert(target);
                }
            }
        }
        let mut ordered: Vec<_> = functions.iter().copied().collect();
        ordered.sort_by_key(ToString::to_string);
        {
            use irq_check_rupta::pta::PointerAnalysis;
            analysis.backend.acx.discovery_roots = ordered.clone();
            analysis.backend.acx.linked_definitions = exported.iter()
                .filter(|(_, instance)| !tcx.is_foreign_item(instance.def_id()))
                .map(|(symbol, instance)| (symbol.clone(), *instance)).collect();
            analysis.backend.initialize();
            analysis.synchronize_backend();
        }
        for instance in ordered {
            if !tcx.is_foreign_item(instance.def_id())
                && tcx.is_mir_available(instance.def_id())
                && tcx
                    .codegen_fn_attrs(instance.def_id())
                    .contains_extern_indicator()
            {
                let body = tcx.instance_mir(instance.def);
                for local in 1..=body.arg_count {
                    let ty =
                        analysis.ty(instance, body.local_decls[mir::Local::from_usize(local)].ty);
                    let facts = if ty.is_bool() {
                        HashSet::from([Fact::Scalar(0), Fact::Scalar(1)])
                    } else {
                        HashSet::from([Fact::Unknown])
                    };
                    analysis.write(
                        &Address {
                            base: Base::Local(instance, 0, local),
                            fields: Vec::new(),
                            pointee: Some(ty),
                            byte_offset: 0,
                        },
                        &Tree::from([(Vec::new(), facts)]),
                    );
                }
            }
            analysis.schedule(Node {
                instance,
                context: 0,
            });
        }
        let mut scans = 0;
        let started = std::time::Instant::now();
        let mut last_progress = started;
        let progress = std::env::var_os("IRQ_CHECK_STATS").is_some();
        let progress_file = std::env::var_os("IRQ_CHECK_DUMP_DIR").map(|directory| {
            let directory = std::path::PathBuf::from(directory);
            let _ = std::fs::create_dir_all(&directory);
            directory.join(format!(
                "{}-progress.txt",
                tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE)
            ))
        });
        let mut block_visits = 0usize;
        let mut live_blocks =
            HashMap::<Instance<'tcx>, HashMap<mir::BasicBlock, HashSet<usize>>>::new();
        let mut loop_headers = HashMap::<Instance<'tcx>, HashSet<mir::BasicBlock>>::new();
        let mut seeded_revision = usize::MAX;
        loop {
            if analysis.incomplete {
                break;
            }
            let feedback_count = analysis.backend_feedback.len()
                + analysis.backend_storage_feedback.len()
                + analysis.backend_reference_feedback.len();
            if feedback_count != 0 && (analysis.pending.is_empty() || feedback_count >= 4096)
            {
                analysis.active = None;
                analysis.synchronize_backend();
            }
            if analysis.pending.is_empty() && seeded_revision != analysis.revision {
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
                    let external = tcx
                        .codegen_fn_attrs(instance.def_id())
                        .contains_extern_indicator();
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
                                let mut values =
                                    analysis.memory_tree(&Base::Local(instance, 0, local));
                                if external {
                                    values.entry(Vec::new()).or_default().insert(Fact::Unknown);
                                }
                                values
                            })
                            .collect()
                    } else {
                        Vec::new()
                    };
                    let (node, binding) = analysis.context(instance, arguments.clone());
                    analysis.roots.insert(instance, node);
                    for (index, values) in arguments.iter().enumerate() {
                        analysis.write(
                            &Address {
                                base: Base::Local(instance, node.context, index + 1),
                                fields: Vec::new(),
                                byte_offset: 0,
                                pointee: None,
                            },
                            &bind_tree(values, &binding),
                        );
                    }
                }
                seeded_revision = analysis.revision;
            }
            let Some(node) = analysis.pending.pop_front() else {
                if !analysis.settling_unknowns {
                    analysis.settling_unknowns = true;
                    for node in analysis.graph.keys().copied().collect::<Vec<_>>() {
                        analysis.schedule(node);
                    }
                    continue;
                }
                break;
            };
            let instance = node.instance;
            if !live_blocks.contains_key(&instance)
                && trust.checks(instance.def_id())
                && !tcx.is_foreign_item(instance.def_id())
                && !matches!(
                    instance.def,
                    InstanceKind::Virtual(..) | InstanceKind::Intrinsic(..)
                )
                && !known_intrinsic(tcx, instance)
                && (tcx.is_mir_available(instance.def_id())
                    || !matches!(instance.def, InstanceKind::Item(..)))
            {
                let mut bodies = HashSet::from([instance]);
                bodies.extend(analysis.pending.iter().map(|node| node.instance));
                let bodies: Vec<_> = bodies
                    .into_iter()
                    .filter(|instance| {
                        !live_blocks.contains_key(instance)
                            && trust.checks(instance.def_id())
                            && !tcx.is_foreign_item(instance.def_id())
                            && !matches!(
                                instance.def,
                                InstanceKind::Virtual(..) | InstanceKind::Intrinsic(..)
                            )
                            && (tcx.is_mir_available(instance.def_id())
                                || !matches!(instance.def, InstanceKind::Item(..)))
                    })
                    .collect();
                let prepared: Vec<_> = rustc_data_structures::sync::par_map(bodies, |instance| {
                    let body = tcx.instance_mir(instance.def);
                    let dominators = body.basic_blocks.dominators();
                    let loops: HashSet<_> = body
                        .basic_blocks
                        .iter_enumerated()
                        .filter(|(block, _)| dominators.is_reachable(*block))
                        .flat_map(|(block, data)| {
                            data.terminator()
                                .successors()
                                .filter(move |target| dominators.dominates(*target, block))
                        })
                        .collect();
                    let borrowed: HashSet<_> = rustc_mir_dataflow::impls::borrowed_locals(body)
                        .iter()
                        .map(|local| local.as_usize())
                        .collect();
                    let mut cursor = rustc_mir_dataflow::impls::MaybeLiveLocals
                        .iterate_to_fixpoint(tcx, body, None)
                        .into_results_cursor(body);
                    let live: HashMap<_, _> = body
                        .basic_blocks
                        .indices()
                        .map(|block| {
                            cursor.seek_to_block_start(block);
                            let mut live: HashSet<_> =
                                cursor.get().iter().map(|local| local.as_usize()).collect();
                            live.insert(0);
                            (block, live)
                        })
                        .collect();
                    (instance, loops, borrowed, live, std::thread::current().id())
                });
                if progress && !prepared.is_empty() {
                    let threads: HashSet<_> = prepared.iter().map(|entry| entry.4).collect();
                    eprintln!(
                        "irq-check: prepared-bodies={} worker-threads={}",
                        prepared.len(),
                        threads.len()
                    );
                }
                for (instance, loops, borrowed, live, _) in prepared {
                    loop_headers.insert(instance, loops);
                    analysis.address_taken.insert(instance, borrowed);
                    live_blocks.insert(instance, live);
                }
            }
            analysis.process_node(
                node,
                trust,
                roots,
                &exported,
                &mut live_blocks,
                &mut loop_headers,
                progress,
                &progress_file,
                started,
                &mut last_progress,
                &mut scans,
                &mut block_visits,
            );
        }
        if let (Some(directory), Ok(filter)) = (
            std::env::var_os("IRQ_CHECK_DUMP_DIR"),
            std::env::var("IRQ_CHECK_TRACE"),
        ) {
            let directory = std::path::PathBuf::from(directory);
            let result = std::fs::create_dir_all(&directory).and_then(|_| {
                let mut dump = String::new();
                for node in analysis.graph.keys().filter(|node| node.instance.to_string().contains(&filter)) {
                    use std::fmt::Write;
                    writeln!(&mut dump, "{} context {}", node.instance, node.context).unwrap();
                    if !tcx.is_foreign_item(node.instance.def_id()) && tcx.is_mir_available(node.instance.def_id()) {
                        writeln!(&mut dump, "{:#?}", tcx.instance_mir(node.instance.def)).unwrap();
                    }
                    for base in analysis.memory.objects.keys() {
                        if matches!(base, Base::Local(instance, context, _) if *instance == node.instance && *context == node.context) {
                            let values = analysis.memory_tree(base);
                            writeln!(&mut dump, "{base:?}: {values:#?}").unwrap();
                        }
                    }
                }
                std::fs::write(directory.join(format!("{}-trace.txt", tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE))), dump)
            });
            if let Err(error) = result {
                analysis.incomplete = true;
                analysis.limit_detail = format!("cannot write analysis trace: {error}");
            }
        }
        if progress {
            eprintln!(
                "irq-check: storage-view cache hits={} misses={} redundant-writes={} interned-storage={} interned-facts={}",
                analysis.memory_view_hits.get(),
                analysis.memory_view_misses.get(),
                analysis.redundant_memory_writes,
                analysis.memory.objects.len(),
                analysis.memory.values.len(),
            );
            eprintln!(
                "irq-check: opaque calls={} addresses={} peak-addresses={} elapsed={:.3}s read-hits={} read-misses={} parallel-batches={} parallel-addresses={} parallel-misses={} worker-threads={} traversal-hits={} saved-visits={} effective-view-hits={} subscription-hits={}",
                analysis.opaque_calls,
                analysis.opaque_addresses,
                analysis.opaque_peak_addresses,
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
        }
        analysis
    }

    fn ty(&self, instance: Instance<'tcx>, ty: Ty<'tcx>) -> Ty<'tcx> {
        instance.instantiate_mir_and_normalize_erasing_regions(
            self.tcx,
            TypingEnv::fully_monomorphized(),
            EarlyBinder::bind(self.tcx, ty),
        )
    }

    fn context_details(&self) -> String {
        let mut contexts: Vec<_> = self.contexts.iter().collect();
        contexts.sort_unstable_by_key(|(_, contexts)| std::cmp::Reverse(contexts.len()));
        let mut details = String::new();
        for (instance, inputs) in &contexts {
            if self.value_only_bodies.get(instance) == Some(&true) {
                details.push_str(&format!(
                    "{} value-only contexts: {instance}\n",
                    inputs.len()
                ));
            }
        }
        for (origin, location) in &self.havoc_order {
            details.push_str(&format!(
                "unknown write: {} context {} at {location:?}\n",
                origin.instance, origin.context
            ));
            if self.tcx.is_mir_available(origin.instance.def_id()) {
                let body = self.tcx.instance_mir(origin.instance.def);
                let block = &body.basic_blocks[location.block];
                if let Some(statement) = block.statements.get(location.statement_index) {
                    details.push_str(&format!("{:?}\n", statement.kind));
                } else {
                    details.push_str(&format!("{:?}\n", block.terminator().kind));
                }
            }
        }
        for (instance, contexts) in contexts.iter().take(10) {
            details.push_str(&format!("{} contexts: {instance}\n", contexts.len()));
        }
        let mut summaries: Vec<_> = self.returns.iter().collect();
        summaries.sort_unstable_by_key(|(_, alternatives)| std::cmp::Reverse(alternatives.len()));
        if let Some((node, alternatives)) = summaries.first() {
            if alternatives.len() >= 2 {
                let previous = &alternatives[alternatives.len() - 2].key;
                let current = &alternatives[alternatives.len() - 1].key;
                details.push_str(&format!(
                    "latest return-shape changes: {} context {}\n",
                    node.instance, node.context
                ));
                if previous.0 != current.0 {
                    details.push_str(&format!(
                        "return value: {:?} -> {:?}\n",
                        previous.0, current.0
                    ));
                }
                for base in previous
                    .1
                    .keys()
                    .chain(current.1.keys())
                    .collect::<HashSet<_>>()
                {
                    let before = previous.1.get(base);
                    let after = current.1.get(base);
                    let paths: HashSet<_> = before
                        .into_iter()
                        .flat_map(|tree| tree.keys())
                        .chain(after.into_iter().flat_map(|tree| tree.keys()))
                        .collect();
                    for path in paths {
                        let before = before.and_then(|tree| tree.get(path));
                        let after = after.and_then(|tree| tree.get(path));
                        if before != after {
                            details
                                .push_str(&format!("{base:?} {path:?}: {before:?} -> {after:?}\n"));
                        }
                    }
                }
            }
        }
        for (node, alternatives) in summaries.into_iter().take(10) {
            details.push_str(&format!(
                "{} return alternatives: {} context {}\n",
                alternatives.len(),
                node.instance,
                node.context
            ));
        }
        if let Some((instance, contexts)) = contexts.first() {
            for (index, context) in contexts.iter().enumerate().rev().take(2) {
                details.push_str(&format!(
                    "{instance} context {} key:\n{:#?}\n",
                    index + 1,
                    context.key
                ));
            }
        }
        if let Ok(filter) = std::env::var("IRQ_CHECK_TRACE") {
            for (instance, contexts) in &self.contexts {
                if instance.to_string().contains(&filter) {
                    for (index, context) in contexts.iter().enumerate().take(2) {
                        details.push_str(&format!(
                            "trace {instance} context {} input:\n{:#?}\n{:#?}\n",
                            index + 1,
                            context.arguments,
                            context.memory
                        ));
                    }
                }
            }
        }
        details
    }

    fn record_site(
        &mut self,
        node: Node<'tcx>,
        body: &Body<'tcx>,
        location: mir::Location,
        state: FlowState<'tcx>,
        edges: &mut [Edge<'tcx>],
    ) {
        if edges.is_empty() {
            return;
        }
        let states = self.states.entry(node).or_default();
        let state = states.get_context_id(std::borrow::Cow::Owned(ContextKey(Vec::new(), state)));
        let id = if let Some(index) = self
            .site_index
            .get(&node)
            .and_then(|sites| sites.get(&(location, state)))
            .copied()
        {
            index
        } else {
            let data = &body.basic_blocks[location.block];
            let (scope, normal, unwind) =
                if let Some(statement) = data.statements.get(location.statement_index) {
                    (
                        statement.source_info.scope,
                        vec![mir::Location {
                            block: location.block,
                            statement_index: location.statement_index + 1,
                        }],
                        None,
                    )
                } else {
                    let terminator = data.terminator();
                    let unwind = match &terminator.kind {
                        TerminatorKind::Call { unwind, .. }
                        | TerminatorKind::Drop { unwind, .. }
                        | TerminatorKind::Assert { unwind, .. }
                        | TerminatorKind::InlineAsm { unwind, .. } => {
                            if let mir::UnwindAction::Cleanup(block) = unwind {
                                Some(*block)
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };
                    (
                        terminator.source_info.scope,
                        terminator
                            .successors()
                            .filter(|block| Some(*block) != unwind)
                            .map(|block| mir::Location {
                                block,
                                statement_index: 0,
                            })
                            .collect(),
                        unwind,
                    )
                };
            let site = CallSite {
                caller: node,
                location,
                state,
                scope,
                normal,
                unwind,
                bindings: Vec::new(),
            };
            let index = if let Some(index) = self.free_site_slots.pop() {
                self.free_sites.remove(&index);
                self.sites[index] = site;
                index
            } else {
                self.sites.push(site);
                self.sites.len() - 1
            };
            self.site_index
                .entry(node)
                .or_default()
                .insert((location, state), index);
            index
        };
        for binding in self.pending_bindings.drain(..) {
            if !self.sites[id].bindings.contains(&binding) {
                self.sites[id].bindings.push(binding);
            }
        }
        for edge in edges {
            edge.site = Some(id);
        }
    }

    pub fn site_state(&self, id: usize) -> String {
        let site = &self.sites[id];
        format!(
            "{:#?}\ncall storage bindings:\n{:#?}",
            self.states[&site.caller].context_list[site.state].1, site.bindings
        )
    }

    fn index_states(
        &mut self,
        node: Node<'tcx>,
        body: &Body<'tcx>,
        places: Vec<Place<'tcx>>,
    ) -> Option<Vec<FlowState<'tcx>>> {
        let tcx = self.tcx;
        let instance = node.instance;
        for place in places {
            for (projection_index, projection) in place.projection.iter().enumerate() {
                let ProjectionElem::Index(index_local) = projection else {
                    continue;
                };
                let index_values = self.place_value(instance, body, Place::from(index_local));
                let facts = index_values.get(&Vec::new());
                if facts.is_some_and(|facts| {
                    facts.len() == 1 && facts.iter().all(|fact| matches!(fact, Fact::Scalar(_)))
                }) {
                    continue;
                }
                let prefix = Place {
                    local: place.local,
                    projection: tcx.mk_place_elems(&place.projection[..projection_index]),
                };
                let source_ty = self.ty(instance, prefix.ty(body, tcx).ty);
                let mut types = match source_ty.kind() {
                    ty::Array(element, _) | ty::Slice(element) => vec![*element],
                    _ => Vec::new(),
                };
                let mut visited_types = HashSet::new();
                let mut relational = false;
                while let Some(element) = types.pop() {
                    if !visited_types.insert(element) {
                        continue;
                    }
                    match element.kind() {
                        ty::Ref(..)
                        | ty::RawPtr(..)
                        | ty::FnPtr(..)
                        | ty::FnDef(..)
                        | ty::Dynamic(..)
                        | ty::Closure(..)
                        | ty::Coroutine(..)
                        | ty::CoroutineClosure(..)
                        | ty::Param(..)
                        | ty::Alias(..) => {
                            relational = true;
                            break;
                        }
                        ty::Array(element, _) | ty::Slice(element) | ty::Pat(element, _) => {
                            types.push(*element)
                        }
                        ty::Tuple(fields) => types.extend(fields.iter()),
                        ty::Adt(adt, args) => types.extend(adt.all_fields().map(|field| {
                            tcx.normalize_erasing_regions(
                                TypingEnv::fully_monomorphized(),
                                field.ty(tcx, args),
                            )
                        })),
                        _ => {}
                    }
                }
                if !relational {
                    continue;
                }
                let mut lengths = HashSet::new();
                let mut closed = true;
                match source_ty.kind() {
                    ty::Array(_, count) => {
                        if let Some(count) = count.try_to_target_usize(tcx) {
                            lengths.insert(count);
                        } else {
                            closed = false;
                        }
                    }
                    ty::Slice(_) => {
                        let addresses = self.places(instance, body, prefix);
                        if addresses.is_empty() {
                            closed = false;
                        }
                        for address in addresses {
                            if address.fields.is_empty() && address.byte_offset == 0 {
                                let storage_ty = match address.base {
                                    Base::Local(owner, _, local) => Some(
                                        self.ty(
                                            owner,
                                            tcx.instance_mir(owner.def).local_decls
                                                [mir::Local::from_usize(local)]
                                            .ty,
                                        ),
                                    ),
                                    Base::Parameter(_, _, _, ty) => Some(ty),
                                    _ => None,
                                };
                                if let Some(storage_ty) = storage_ty {
                                    if let ty::Array(_, count) = storage_ty.kind() {
                                        if let Some(count) = count.try_to_target_usize(tcx) {
                                            lengths.insert(count);
                                            continue;
                                        }
                                    }
                                }
                            }
                            closed = false;
                        }
                    }
                    _ => closed = false,
                }
                let count = lengths.into_iter().max().unwrap_or(0);
                if !closed || count == 0 || count >= VARIANT as u64 {
                    continue;
                }
                let snapshot = self.flow_locals.as_ref().cloned().unwrap_or_default();
                let mut alternatives = Vec::new();
                for index in 0..count {
                    if facts.is_some_and(|facts| {
                        !facts.is_empty()
                            && facts.iter().all(|fact| matches!(fact, Fact::Scalar(_)))
                            && !facts.contains(&Fact::Scalar(index as u128))
                    }) {
                        continue;
                    }
                    let mut alternative = snapshot.clone();
                    alternative.insert(
                        Base::Local(instance, node.context, index_local.as_usize()),
                        Tree::from([(Vec::new(), HashSet::from([Fact::Scalar(index as u128)]))]),
                    );
                    alternatives.push(alternative);
                }
                return Some(alternatives);
            }
        }
        None
    }

    fn numeric_cell(tcx: TyCtxt<'tcx>, base: &Base<'tcx>, path: &[u32]) -> bool {
        let mut cell = match base {
            Base::Parameter(_, _, _, ty) | Base::Unknown(ty) => Some(*ty),
            Base::Local(instance, _, local) => Some(
                instance.instantiate_mir_and_normalize_erasing_regions(
                    tcx,
                    TypingEnv::fully_monomorphized(),
                    EarlyBinder::bind(
                        tcx,
                        tcx.instance_mir(instance.def).local_decls[mir::Local::from_usize(*local)]
                            .ty,
                    ),
                ),
            ),
            Base::Static(def) => Some(tcx.normalize_erasing_regions(
                TypingEnv::fully_monomorphized(),
                tcx.type_of(*def).instantiate_identity(),
            )),
            _ => None,
        };
        for field in path {
            cell = cell.and_then(|cell| match cell.kind() {
                ty::Tuple(fields) => fields.get(*field as usize).copied(),
                ty::Adt(adt, args) if !adt.is_enum() && !adt.is_union() => adt
                    .non_enum_variant()
                    .fields
                    .get(rustc_abi::FieldIdx::from_u32(*field))
                    .map(|field| {
                        tcx.normalize_erasing_regions(
                            TypingEnv::fully_monomorphized(),
                            field.ty(tcx, args),
                        )
                    }),
                ty::Array(element, _) | ty::Slice(element)
                    if *field != DISCRIMINANT && *field != VARIANT =>
                {
                    Some(*element)
                }
                _ => None,
            });
        }
        cell.is_some_and(|cell| {
            matches!(
                cell.kind(),
                ty::Int(_) | ty::Uint(_) | ty::Float(_) | ty::Char
            )
        })
    }

    fn record_return(&mut self, node: Node<'tcx>, value: Tree<'tcx>, mut memory: FlowState<'tcx>) {
        let mut escaping = HashSet::new();
        let mut pending: VecDeque<_> = value.references().iter().cloned().collect();
        while let Some(base) = pending.pop_front() {
            if !escaping.insert(base.clone()) {
                continue;
            }
            pending.extend(
                memory
                    .get(&base)
                    .into_iter()
                    .flat_map(|tree| tree.references().iter().cloned()),
            );
        }
        let modified: HashSet<_> = self
            .effects
            .get(&node)
            .into_iter()
            .flat_map(|effects| effects.keys())
            .map(|address| address.base.clone())
            .collect();
        memory.retain(|base, _| escaping.contains(base) || modified.contains(base) && !matches!(base, Base::Local(instance, context, _) if *instance == node.instance && *context == node.context) || matches!(base, Base::Havoc));
        let shape = memory
            .iter()
            .filter_map(|(base, tree)| {
                let tree: Tree<'tcx> = tree
                    .iter()
                    .filter_map(|(path, facts)| {
                        let facts: HashSet<_> = facts
                            .iter()
                            .filter(|fact| {
                                !matches!(fact, Fact::Scalar(value) if *value > 1)
                                    && !(Self::numeric_cell(self.tcx, base, path)
                                        && matches!(fact, Fact::Scalar(_) | Fact::Unknown))
                            })
                            .cloned()
                            .collect();
                        (!facts.is_empty()).then(|| (path.clone(), facts))
                    })
                    .collect();
                (!tree.is_empty()).then(|| (base.clone(), tree))
            })
            .collect();
        let alternative = ReturnAlternative {
            key: ContextKey(vec![value.clone()], shape),
            value,
            memory,
        };
        let alternatives = self.returns.entry(node).or_default();
        let existing = if node.context == 0 && !alternatives.is_empty() {
            Some(0)
        } else {
            alternatives
                .iter()
                .position(|stored| stored.key == alternative.key)
        };
        let changed = if let Some(index) = existing {
            let output = &mut alternatives[index];
            let mut changed = false;
            changed |= output.value.join(&alternative.value);
            for (base, tree) in alternative.memory {
                let output = output.memory.entry(base).or_default();
                changed |= output.join(&tree);
            }
            changed
        } else {
            alternatives.push(alternative);
            true
        };
        if changed {
            if existing.is_some() {
                self.precision_loss.insert(format!(
                    "joined scalar return storage in {} context {}",
                    node.instance, node.context
                ));
            }
            for reader in self.effect_readers.get(&node).cloned().unwrap_or_default() {
                self.schedule(reader);
            }
        }
    }

    fn opaque_effects(
        &mut self,
        arguments: &[(Ty<'tcx>, Tree<'tcx>)],
        span: rustc_span::Span,
        follow_callbacks: bool,
        may_write: bool,
        access_shared: bool,
    ) -> Vec<Edge<'tcx>> {
        let started = std::time::Instant::now();
        let cache_key = self.active.map(|node| {
            (
                node,
                self.active_location,
                follow_callbacks,
                may_write,
                access_shared,
                self.settling_unknowns,
            )
        });
        let epoch = self.memory_epoch;
        let escaped = self.escaped.len();
        let statics = self.statics.len();
        let written_allocations = self.written_allocations.len();
        let known_callbacks = self.known_callbacks.len();
        let cached = cache_key
            .and_then(|key| self.opaque_traversals.get(&key))
            .filter(|cached| {
                cached.epoch == epoch
                    && cached.escaped == escaped
                    && cached.statics == statics
                    && cached.written_allocations == written_allocations
                    && cached.arguments == arguments
                    && cached.has_locals == self.flow_locals.is_some()
                    && cached.locals.iter().all(|(base, stored)| {
                        self.flow_locals
                            .as_ref()
                            .and_then(|memory| memory.get(base))
                            == Some(stored)
                    })
                    && self
                        .flow_locals
                        .iter()
                        .flat_map(|memory| memory.iter())
                        .filter(|(base, _)| {
                            matches!(base, Base::Havoc)
                                || cached.dependencies.as_ref().map_or_else(
                                    || self.escaped.contains(*base),
                                    |dependencies| dependencies.contains(*base),
                                )
                        })
                        .all(|(base, stored)| cached.locals.get(base) == Some(stored))
                    && cached.local_callbacks.as_ref().is_none_or(|callbacks| {
                        cached.known_callbacks == known_callbacks
                            && *callbacks
                                == self
                                    .flow_locals
                                    .iter()
                                    .flat_map(|memory| {
                                        memory.values().flat_map(|tree| tree.values().flatten())
                                    })
                                    .filter_map(|fact| match fact {
                                        Fact::Function(target)
                                        | Fact::ExposedPointer(
                                            PointerOrigin::Function(target),
                                            _,
                                        ) => Some(*target),
                                        _ => None,
                                    })
                                    .collect()
                    })
            })
            .map(|cached| {
                (
                    cached.writes.clone(),
                    cached.callbacks.clone(),
                    cached.visited,
                )
            });
        let (writes, callbacks) = if let Some((writes, callbacks, visited)) = cached {
            self.opaque_traversal_hits += 1;
            self.opaque_saved_visits += visited;
            (writes, callbacks)
        } else {
            let entry_locals = self.flow_locals.clone();
            let shared_unknown = access_shared && self.escaped.contains(&Base::Havoc);
            let mut pending = std::mem::take(&mut self.opaque_pending);
            let mut visited = std::mem::take(&mut self.opaque_visited);
            let mut callbacks = HashSet::new();
            for (ty, values) in arguments {
                if follow_callbacks {
                    if let ty::Closure(def, args) = ty.kind() {
                        callbacks.insert(Instance::resolve_closure(
                            self.tcx,
                            *def,
                            args,
                            ty::ClosureKind::FnOnce,
                        ));
                    }
                }
                for fact in values.values().flatten() {
                    match fact {
                        Fact::Reference(address)
                        | Fact::ExposedPointer(PointerOrigin::Storage(address), _) => {
                            pending.push_back(address.clone())
                        }
                        Fact::IntegerPointer(_) => pending.push_back(Address {
                            base: Base::Unknown(*ty),
                            fields: Vec::new(),
                            pointee: None,
                            byte_offset: 0,
                        }),
                        Fact::Unknown
                            if matches!(ty.kind(), ty::RawPtr(..) | ty::Ref(..))
                                || self.tracks(*ty, &mut HashSet::new()) =>
                        {
                            pending.push_back(Address {
                                base: Base::Havoc,
                                fields: Vec::new(),
                                pointee: None,
                                byte_offset: 0,
                            })
                        }
                        Fact::Function(target)
                        | Fact::ExposedPointer(PointerOrigin::Function(target), _)
                            if follow_callbacks =>
                        {
                            callbacks.insert(*target);
                        }
                        _ => {}
                    }
                }
            }
            if access_shared {
                if let Some(active) = self.active {
                    self.escape_readers.insert(active);
                }
                let mut shared: HashSet<_> = if shared_unknown {
                    self.opaque_shared_dirty.clone()
                } else {
                    self.escaped.clone()
                };
                if shared_unknown {
                    shared.insert(Base::Havoc);
                    shared.extend(entry_locals.iter().flat_map(|memory| memory.keys())
                        .filter(|base| self.escaped.contains(*base)).cloned());
                }
                shared.extend(self.statics.iter().map(|def| Base::Static(*def))
                    .filter(|base| !shared_unknown || !self.escaped.contains(base)));
                for base in shared {
                    pending.push_back(Address {
                        base,
                        fields: Vec::new(),
                        pointee: None,
                        byte_offset: 0,
                    });
                }
            }
            pending.retain(|address| visited.insert(address.clone()));
            let mut unknown_callbacks = false;
            if self.worker_limit > 1 && pending.len() >= self.worker_limit * 16 {
                let havoc = self.stored_tree(&Base::Havoc);
                let view_guard = self.memory_views.borrow();
                let views = &*view_guard;
                let reads = &self.opaque_reads;
                let memory = &self.memory;
                let tcx = self.tcx;
                let escaped = &self.escaped;
                let flow = self.flow_locals.as_ref();
                let active = self.active;
                let usize_ty = self.tcx.types.usize;
                let mut scratch = std::mem::take(&mut self.opaque_scratch);
                for address in pending.drain(..) {
                    scratch[self.readers.shard(&address.base)]
                        .batch
                        .push(address);
                }
                let batches: Vec<_> = scratch
                    .iter_mut()
                    .zip(self.readers.shards.iter_mut())
                    .map(rustc_data_structures::marker::IntoDynSyncSend)
                    .collect();
                let workers: Vec<_> = rustc_data_structures::sync::par_map(
                    batches,
                    |rustc_data_structures::marker::IntoDynSyncSend((scratch, readers))| {
                        let OpaqueScratch {
                            batch,
                            hits,
                            misses,
                            next,
                            targets,
                            rebuilt,
                            new_views,
                            new_escapes,
                        } = scratch;
                        *hits = 0;
                        for address in batch.iter() {
                            if !address.fields.is_empty()
                                || address.byte_offset != 0
                                || address.pointee.is_some()
                                || matches!(address.base, Base::Unknown(_) | Base::Havoc)
                            {
                                misses.push(address.clone());
                                continue;
                            }
                            if !escaped.contains(&address.base) {
                                new_escapes.push(address.base.clone());
                            }
                            let stored = views.get(&address.base).cloned().unwrap_or_else(|| {
                                let mut tree = Tree::new();
                                for (path, fact) in memory.get_pts(&address.base) {
                                    tree.entry(path.clone()).or_default().insert(fact.clone());
                                }
                                new_views.push((address.base.clone(), tree.clone()));
                                tree
                            });
                            let local = flow.and_then(|memory| {
                        if memory.contains_key(&address.base)
                            || matches!(&address.base, Base::Local(instance, context, _) if active.is_some_and(|node| node.instance == *instance && node.context == *context))
                        {
                            Some(memory.get(&address.base))
                        } else {
                            None
                        }
                    });
                            let local_havoc = local
                                .and_then(|_| flow.and_then(|memory| memory.get(&Base::Havoc)));
                            let cached = reads.get(&address.base).filter(|cached| {
                                cached.stored == stored
                                    && cached.havoc == havoc
                                    && cached.local.as_ref().map(|value| value.as_ref()) == local
                                    && cached.local_havoc.as_ref() == local_havoc
                            });
                            let facts = if let Some(cached) = cached {
                                *hits += 1;
                                cached.facts.clone()
                            } else {
                                let exposed = !matches!(address.base, Base::Allocation(id, _) if matches!(tcx.global_alloc(id), GlobalAlloc::Memory(value) if value.inner().mutability == rustc_hir::Mutability::Not));
                                let rebuilt_read = OpaqueRead::rebuild(
                                    stored,
                                    havoc.clone(),
                                    local.map(|value| value.cloned()),
                                    local_havoc.cloned(),
                                    exposed,
                                );
                                let facts = rebuilt_read.facts.clone();
                                rebuilt.push((address.base.clone(), rebuilt_read));
                                facts
                            };
                            if let Some(active) = active {
                                readers
                                    .entry(address.base.clone())
                                    .or_default()
                                    .entry(Vec::new())
                                    .or_default()
                                    .insert(active);
                            }
                            for fact in facts.iter() {
                                match fact {
                                    Fact::Reference(address)
                                    | Fact::ExposedPointer(PointerOrigin::Storage(address), _) => {
                                        next.insert(address.clone());
                                    }
                                    Fact::IntegerPointer(_) => {
                                        next.insert(Address {
                                            base: Base::Unknown(usize_ty),
                                            fields: Vec::new(),
                                            pointee: None,
                                            byte_offset: 0,
                                        });
                                    }
                                    Fact::Function(target)
                                    | Fact::ExposedPointer(PointerOrigin::Function(target), _)
                                        if follow_callbacks =>
                                    {
                                        targets.insert(*target);
                                    }
                                    _ => {}
                                }
                            }
                        }
                        batch.clear();
                        std::thread::current().id()
                    },
                );
                drop(view_guard);
                self.opaque_parallel_batches += 1;
                for (scratch, worker) in scratch.iter_mut().zip(workers) {
                    let OpaqueScratch {
                        hits,
                        misses,
                        next,
                        targets,
                        rebuilt,
                        new_views,
                        new_escapes,
                        ..
                    } = scratch;
                    for base in new_escapes.drain(..) {
                        self.mark_escaped(base);
                    }
                    self.opaque_workers.insert(worker);
                    self.opaque_read_hits += *hits;
                    self.opaque_parallel_addresses += *hits + rebuilt.len();
                    self.opaque_parallel_misses += rebuilt.len();
                    self.opaque_read_misses += rebuilt.len();
                    if let Some(active) = active {
                        if *hits != 0 || !rebuilt.is_empty() {
                            self.readers
                                .entry(Base::Havoc)
                                .or_default()
                                .entry(Vec::new())
                                .or_default()
                                .insert(active);
                        }
                    }
                    self.opaque_reads.extend(rebuilt.drain(..));
                    self.memory_view_misses
                        .set(self.memory_view_misses.get() + new_views.len());
                    self.memory_views.get_mut().extend(new_views.drain(..));
                    pending.extend(misses.drain(..));
                    for address in next.drain() {
                        if visited.insert(address.clone()) {
                            pending.push_back(address);
                        }
                    }
                    callbacks.extend(targets.drain());
                }
                self.opaque_scratch = scratch;
            }
            while let Some(address) = pending.pop_front() {
                if matches!(address.base, Base::Unknown(_) | Base::Havoc) {
                    self.mark_escaped(Base::Havoc);
                    if follow_callbacks {
                        unknown_callbacks = true;
                    }
                    continue;
                }
                self.mark_escaped(address.base.clone());
                let facts = if address.fields.is_empty()
                    && address.byte_offset == 0
                    && address.pointee.is_none()
                {
                    let stored = self.stored_tree(&address.base);
                    let havoc = self.stored_tree(&Base::Havoc);
                    let local = self.flow_locals.as_ref().and_then(|memory| {
                    if memory.contains_key(&address.base)
                        || matches!(&address.base, Base::Local(instance, context, _) if self.active.is_some_and(|node| node.instance == *instance && node.context == *context))
                    {
                        Some(memory.get(&address.base).cloned())
                    } else {
                        None
                    }
                });
                    let local_havoc = if local.is_some() {
                        self.flow_locals
                            .as_ref()
                            .and_then(|memory| memory.get(&Base::Havoc))
                            .cloned()
                    } else {
                        None
                    };
                    let cached = self
                        .opaque_reads
                        .get(&address.base)
                        .filter(|cached| {
                            cached.stored == stored
                                && cached.havoc == havoc
                                && cached.local == local
                                && cached.local_havoc == local_havoc
                        })
                        .map(|cached| cached.facts.clone());
                    if let Some(active) = self.active {
                        if self.exposed_to_havoc(&address.base) {
                            self.readers
                                .entry(Base::Havoc)
                                .or_default()
                                .entry(Vec::new())
                                .or_default()
                                .insert(active);
                        }
                        self.readers
                            .entry(address.base.clone())
                            .or_default()
                            .entry(Vec::new())
                            .or_default()
                            .insert(active);
                    }
                    if let Some(facts) = cached {
                        self.opaque_read_hits += 1;
                        facts
                    } else {
                        self.opaque_read_misses += 1;
                        let rebuilt = OpaqueRead::rebuild(
                            stored,
                            havoc,
                            local,
                            local_havoc,
                            self.exposed_to_havoc(&address.base),
                        );
                        let facts = rebuilt.facts.clone();
                        self.opaque_reads.insert(address.base.clone(), rebuilt);
                        facts
                    }
                } else {
                    std::sync::Arc::new(self.read(&address).values().flatten().cloned().collect())
                };
                for fact in facts.iter() {
                    match fact {
                        Fact::Reference(address)
                        | Fact::ExposedPointer(PointerOrigin::Storage(address), _) => {
                            if visited.insert(address.clone()) {
                                pending.push_back(address.clone());
                            }
                        }
                        Fact::IntegerPointer(_) => {
                            let address = Address {
                                base: Base::Unknown(self.tcx.types.usize),
                                fields: Vec::new(),
                                pointee: None,
                                byte_offset: 0,
                            };
                            if visited.insert(address.clone()) {
                                pending.push_back(address);
                            }
                        }
                        Fact::Function(target)
                        | Fact::ExposedPointer(PointerOrigin::Function(target), _)
                            if follow_callbacks =>
                        {
                            callbacks.insert(*target);
                        }
                        _ => {}
                    }
                }
            }
            if unknown_callbacks {
                if let Some(active) = self.active {
                    self.unknown_readers.insert(active);
                }
                callbacks.extend(self.known_callbacks.iter().copied());
                for fact in self
                    .flow_locals
                    .iter()
                    .flat_map(|memory| memory.values().flat_map(|tree| tree.values().flatten()))
                {
                    if let Fact::Function(target)
                    | Fact::ExposedPointer(PointerOrigin::Function(target), _) = fact
                    {
                        callbacks.insert(*target);
                    }
                }
            }
            let clobbers_all = visited
                .iter()
                .any(|address| matches!(address.base, Base::Unknown(_) | Base::Havoc));
            self.opaque_addresses += visited.len();
            self.opaque_peak_addresses = self.opaque_peak_addresses.max(visited.len());
            let visited_count = visited.len();
            let dependencies: Option<HashSet<_>> = (!access_shared)
                .then(|| visited.iter().map(|address| address.base.clone()).collect());
            let locals = entry_locals
                .iter()
                .flat_map(|memory| memory.iter())
                .filter(|(base, _)| {
                    matches!(base, Base::Havoc)
                        || dependencies.as_ref().map_or_else(
                            || self.escaped.contains(*base),
                            |dependencies| dependencies.contains(*base),
                        )
                })
                .map(|(base, stored)| (base.clone(), stored.clone()))
                .collect();
            let local_callbacks = unknown_callbacks.then(|| {
                entry_locals
                    .iter()
                    .flat_map(|memory| memory.values().flat_map(|tree| tree.values().flatten()))
                    .filter_map(|fact| match fact {
                        Fact::Function(target)
                        | Fact::ExposedPointer(PointerOrigin::Function(target), _) => Some(*target),
                        _ => None,
                    })
                    .collect()
            });
            let mut writes = Vec::new();
            for mut address in visited.drain() {
                if address.fields.is_empty() && address.byte_offset == 0
                    && address.pointee.is_none()
                    && !entry_locals.as_ref().is_some_and(|memory| {
                        memory.contains_key(&address.base)
                            || matches!(&address.base, Base::Local(instance, context, _)
                                if self.active.is_some_and(|node|
                                    node.instance == *instance && node.context == *context))
                    })
                {
                    self.opaque_shared_dirty.remove(&address.base);
                }
                if !may_write
                    || matches!(address.base, Base::Allocation(id, _) if matches!(self.tcx.global_alloc(id), GlobalAlloc::Memory(memory) if memory.inner().mutability == rustc_hir::Mutability::Not))
                    || clobbers_all && !matches!(address.base, Base::Unknown(_) | Base::Havoc)
                {
                    continue;
                }
                if address.byte_offset != 0 {
                    address.fields.clear();
                    address.byte_offset = 0;
                    address.pointee = None;
                }
                writes.push(address);
            }
            self.opaque_pending = pending;
            self.opaque_visited = visited;
            let writes = std::sync::Arc::new(writes);
            let callbacks = std::sync::Arc::new(callbacks);
            if let Some(key) = cache_key {
                self.opaque_traversals.insert(
                    key,
                    OpaqueTraversal {
                        arguments: arguments.to_vec(),
                        locals,
                        dependencies,
                        has_locals: entry_locals.is_some(),
                        local_callbacks,
                        known_callbacks,
                        epoch,
                        escaped,
                        statics,
                        written_allocations,
                        visited: visited_count,
                        writes: writes.clone(),
                        callbacks: callbacks.clone(),
                    },
                );
            }
            (writes, callbacks)
        };
        self.opaque_calls += 1;
        let previous = self.strong_update;
        let previous_effects = self.record_effects;
        self.record_effects = true;
        self.strong_update = false;
        for address in writes.iter() {
            self.write(
                &address,
                &Tree::from([(Vec::new(), HashSet::from([Fact::Unknown]))]),
            );
        }
        self.strong_update = previous;
        self.record_effects = previous_effects;
        let mut edges = Vec::new();
        for &target in callbacks.iter() {
            let arguments = if !self.tcx.is_foreign_item(target.def_id())
                && !matches!(
                    target.def,
                    InstanceKind::Virtual(..) | InstanceKind::Intrinsic(..)
                )
                && (self.tcx.is_mir_available(target.def_id())
                    || !matches!(target.def, InstanceKind::Item(..)))
            {
                vec![
                    Tree::from([(Vec::new(), HashSet::from([Fact::Unknown]))]);
                    self.tcx.instance_mir(target.def).arg_count
                ]
            } else {
                Vec::new()
            };
            let (node, binding) = self.context(target, arguments.clone());
            for (index, values) in arguments.iter().enumerate() {
                self.write(
                    &Address {
                        base: Base::Local(target, node.context, index + 1),
                        fields: Vec::new(),
                        pointee: None,
                        byte_offset: 0,
                    },
                    &bind_tree(values, &binding),
                );
            }
            edges.push(Edge { site: None, target: Some(node), span, kind: "opaque callback", detail: "an opaque call can invoke this callback from reachable argument storage; callback inputs are unknown".into(), trusted: false });
        }
        self.opaque_elapsed += started.elapsed();
        edges
    }

    fn apply_call_effects(
        &mut self,
        caller: Node<'tcx>,
        callee: Node<'tcx>,
        binding: &HashMap<Base<'tcx>, Base<'tcx>>,
    ) {
        self.effect_readers.entry(callee).or_default().insert(caller);
        let previous_effects = self.record_effects;
        let previous_strong = self.strong_update;
        self.record_effects = true;
        self.strong_update = false;
        for (mut address, values) in self.effects.get(&callee).cloned().unwrap_or_default() {
            if let Some(base) = binding.get(&address.base) {
                address.base = base.clone();
            }
            self.write(&address, &bind_tree(&values, binding));
        }
        self.strong_update = previous_strong;
        self.record_effects = previous_effects;
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
        if matches!(target.def, InstanceKind::Intrinsic(_)) {
            if let Some(requirement) = ty::layout::ValidityRequirement::from_intrinsic(name) {
                return self
                    .tcx
                    .check_validity_requirement((
                        requirement,
                        TypingEnv::fully_monomorphized().as_query_input(target.args.type_at(0)),
                    ))
                    .ok()
                    .filter(|valid| *valid)
                    .map(|_| Tree::new());
            }
            match name.as_str() {
                "fabs"
                    if arguments.len() == 1
                        && matches!(arguments[0].0.kind(), ty::Float(_))
                        && result_ty == arguments[0].0 =>
                {
                    let width = self
                        .tcx
                        .layout_of(TypingEnv::fully_monomorphized().as_query_input(result_ty))
                        .ok()?
                        .size
                        .bits();
                    if !matches!(width, 16 | 32 | 64 | 128) {
                        return None;
                    }
                    let mask = (1u128 << (width - 1)) - 1;
                    let mut values: HashSet<_> = arguments[0]
                        .1
                        .get(&Vec::new())
                        .into_iter()
                        .flatten()
                        .map(|fact| match fact {
                            Fact::Scalar(bits) => Fact::Scalar(bits & mask),
                            _ => Fact::Unknown,
                        })
                        .collect();
                    if values.is_empty() && self.settling_unknowns {
                        values.insert(Fact::Unknown);
                    }
                    return Some(Tree::from([(Vec::new(), values)]));
                }
                "black_box" | "likely" | "unlikely" => {
                    return arguments.first().map(|(_, values)| values.clone());
                }
                "size_of" | "min_align_of" | "pref_align_of" => {
                    let value = target
                        .args
                        .types()
                        .next()
                        .and_then(|ty| {
                            self.tcx
                                .layout_of(TypingEnv::fully_monomorphized().as_query_input(ty))
                                .ok()
                        })
                        .map(|layout| {
                            if name.as_str() == "size_of" {
                                layout.size.bytes()
                            } else {
                                layout.align.abi.bytes()
                            }
                        });
                    return Some(Tree::from([(
                        Vec::new(),
                        HashSet::from([
                            value.map_or(Fact::Unknown, |value| Fact::Scalar(value as u128))
                        ]),
                    )]));
                }
                "integer_min" | "integer_max" if arguments.len() == 2 => {
                    let width = self
                        .tcx
                        .layout_of(TypingEnv::fully_monomorphized().as_query_input(arguments[0].0))
                        .ok()
                        .map(|layout| layout.size.bits());
                    let mut values = HashSet::new();
                    for left in arguments[0].1.get(&Vec::new()).into_iter().flatten() {
                        for right in arguments[1].1.get(&Vec::new()).into_iter().flatten() {
                            let value = match (left, right, width) {
                                (Fact::Scalar(left), Fact::Scalar(right), Some(width))
                                    if width > 0 && width <= 128 =>
                                {
                                    let mask = u128::MAX >> (128 - width);
                                    let left = *left & mask;
                                    let right = *right & mask;
                                    let less = if matches!(arguments[0].0.kind(), ty::Int(_)) {
                                        (((left << (128 - width)) as i128) >> (128 - width))
                                            < (((right << (128 - width)) as i128) >> (128 - width))
                                    } else {
                                        left < right
                                    };
                                    Some(if less == (name.as_str() == "integer_min") {
                                        left
                                    } else {
                                        right
                                    })
                                }
                                _ => None,
                            };
                            values.insert(value.map_or(Fact::Unknown, Fact::Scalar));
                        }
                    }
                    if values.is_empty() && self.settling_unknowns {
                        values.insert(Fact::Unknown);
                    }
                    return Some(Tree::from([(Vec::new(), values)]));
                }
                "caller_location" | "compare_bytes" => {
                    return Some(Tree::from([(Vec::new(), HashSet::from([Fact::Unknown]))]));
                }
                "abort"
                | "cold_path"
                | "unreachable"
                | "assume"
                | "forget"
                | "breakpoint"
                | "atomic_fence"
                | "atomic_singlethreadfence"
                | "prefetch_read_data"
                | "prefetch_write_data"
                | "prefetch_read_instruction"
                | "prefetch_write_instruction" => return Some(Tree::new()),
                "bswap" | "bitreverse" | "ctlz" | "ctlz_nonzero" | "cttz" | "cttz_nonzero"
                | "ctpop"
                    if arguments.len() == 1 && arguments[0].0.is_integral() =>
                {
                    let width = self
                        .tcx
                        .layout_of(TypingEnv::fully_monomorphized().as_query_input(arguments[0].0))
                        .ok()
                        .map(|layout| layout.size.bits());
                    let mut values = HashSet::new();
                    for fact in arguments[0].1.get(&Vec::new()).into_iter().flatten() {
                        let value = match (fact, width) {
                            (Fact::Scalar(value), Some(width)) if width > 0 && width <= 128 => {
                                let value = *value & (u128::MAX >> (128 - width));
                                match name.as_str() {
                                    "bswap" => Some(value.swap_bytes() >> (128 - width)),
                                    "bitreverse" => Some(value.reverse_bits() >> (128 - width)),
                                    "ctlz_nonzero" | "cttz_nonzero" if value == 0 => None,
                                    "ctlz" | "ctlz_nonzero" => Some(
                                        u128::from(value.leading_zeros()) - u128::from(128 - width),
                                    ),
                                    "cttz" | "cttz_nonzero" => Some(
                                        u128::from(value.trailing_zeros()).min(u128::from(width)),
                                    ),
                                    "ctpop" => Some(u128::from(value.count_ones())),
                                    _ => None,
                                }
                            }
                            _ => None,
                        };
                        values.insert(value.map_or(Fact::Unknown, Fact::Scalar));
                    }
                    if values.is_empty() && self.settling_unknowns {
                        values.insert(Fact::Unknown);
                    }
                    return Some(Tree::from([(Vec::new(), values)]));
                }
                "type_id"
                | "needs_drop"
                | "variant_count"
                | "type_name"
                | "discriminant_value"
                | "bswap"
                | "bitreverse"
                | "ctlz"
                | "ctlz_nonzero"
                | "cttz"
                | "cttz_nonzero"
                | "ctpop"
                | "rotate_left"
                | "rotate_right"
                | "wrapping_add"
                | "wrapping_sub"
                | "wrapping_mul"
                | "saturating_add"
                | "saturating_sub"
                | "unchecked_add"
                | "unchecked_sub"
                | "unchecked_mul"
                | "unchecked_div"
                | "unchecked_rem"
                | "unchecked_shl"
                | "unchecked_shr"
                | "exact_div"
                | "add_with_overflow"
                | "sub_with_overflow"
                | "mul_with_overflow"
                | "carrying_mul_add"
                | "is_val_statically_known"
                | "ptr_mask" => {
                    return Some(Tree::from([(Vec::new(), HashSet::from([Fact::Unknown]))]));
                }
                "atomic_load" | "atomic_store" | "atomic_xchg" | "atomic_cxchg"
                | "atomic_cxchgweak" | "atomic_xadd" | "atomic_xsub" | "atomic_and"
                | "atomic_nand" | "atomic_or" | "atomic_xor" | "atomic_max" | "atomic_min"
                | "atomic_umax" | "atomic_umin" => {
                    let Some((_, pointers)) = arguments.first() else {
                        return None;
                    };
                    let mut result = Tree::new();
                    let compare = matches!(name.as_str(), "atomic_cxchg" | "atomic_cxchgweak");
                    let replacement = match name.as_str() {
                        "atomic_load" => None,
                        "atomic_store" | "atomic_xchg" => {
                            arguments.get(1).map(|(_, values)| values.clone())
                        }
                        "atomic_cxchg" | "atomic_cxchgweak" => {
                            arguments.get(2).map(|(_, values)| values.clone())
                        }
                        _ => Some(Tree::from([(Vec::new(), HashSet::from([Fact::Unknown]))])),
                    };
                    let previous = self.strong_update;
                    self.strong_update = false;
                    for fact in pointers.values().flatten() {
                        match fact {
                            Fact::Reference(address) => {
                                let saved_flow = self.flow_locals.take();
                                let loaded = self.read(address);
                                self.flow_locals = saved_flow;
                                for (path, facts) in loaded {
                                    let mut output = if compare { vec![0] } else { Vec::new() };
                                    output.extend(path);
                                    result.entry(output).or_default().extend(facts);
                                }
                                if let Some(values) = &replacement {
                                    self.write(address, values);
                                }
                            }
                            Fact::Unknown | Fact::Scalar(_) | Fact::IntegerPointer(_) => {
                                result
                                    .entry(if compare { vec![0] } else { Vec::new() })
                                    .or_default()
                                    .insert(Fact::Unknown);
                                if let Some(values) = &replacement {
                                    let element = arguments[0]
                                        .0
                                        .builtin_deref(true)
                                        .unwrap_or(self.tcx.types.unit);
                                    self.write(
                                        &Address {
                                            base: Base::Unknown(element),
                                            fields: Vec::new(),
                                            pointee: Some(element),
                                            byte_offset: 0,
                                        },
                                        values,
                                    );
                                }
                            }
                            _ => {}
                        }
                    }
                    self.strong_update = previous;
                    if compare {
                        result.insert(vec![1], HashSet::from([Fact::Scalar(0), Fact::Scalar(1)]));
                    }
                    if name.as_str() == "atomic_store" {
                        result.clear();
                    }
                    return Some(result);
                }
                "write_bytes" | "volatile_set_memory" if arguments.len() == 3 => {
                    let zero = arguments[2]
                        .1
                        .get(&Vec::new())
                        .is_some_and(|values| *values == HashSet::from([Fact::Scalar(0)]));
                    if !zero {
                        for fact in arguments[0].1.values().flatten() {
                            if let Fact::Reference(address) = fact {
                                let mut address = address.clone();
                                address.fields.clear();
                                address.byte_offset = 0;
                                self.write(
                                    &address,
                                    &Tree::from([(Vec::new(), HashSet::from([Fact::Unknown]))]),
                                );
                            } else if matches!(
                                fact,
                                Fact::Unknown | Fact::Scalar(_) | Fact::IntegerPointer(_)
                            ) {
                                let element = arguments[0]
                                    .0
                                    .builtin_deref(true)
                                    .unwrap_or(self.tcx.types.unit);
                                self.write(
                                    &Address {
                                        base: Base::Unknown(element),
                                        fields: Vec::new(),
                                        pointee: Some(element),
                                        byte_offset: 0,
                                    },
                                    &Tree::from([(Vec::new(), HashSet::from([Fact::Unknown]))]),
                                );
                            }
                        }
                    }
                    return Some(Tree::new());
                }
                "volatile_copy_memory" | "volatile_copy_nonoverlapping_memory"
                    if arguments.len() == 3 =>
                {
                    self.copy_memory(
                        &arguments[1].1,
                        &arguments[0].1,
                        arguments[1].0.builtin_deref(true),
                        &arguments[2].1,
                    );
                    return Some(Tree::new());
                }
                _ => {}
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
                return Some(Tree::from([(
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
            let pointee = result_ty.builtin_deref(true)?;
            let address = Address {
                base: Base::Heap(caller, self.local_context, block.as_usize(), pointee),
                fields: Vec::new(),
                byte_offset: 0,
                pointee: Some(pointee),
            };
            if let Some((_, values)) = arguments.first() {
                self.write(&address, values);
            }
            return Some(Tree::from([(
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
                    let mut result = Tree::from([(vec![0, 0, 0], values)]);
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
        let pointer_operation = self.tcx.def_path_str(target.def_id()).contains("::ptr::");
        if crate_name.as_str() != "core"
            || !(matches!(target.def, InstanceKind::Intrinsic(_)) || pointer_operation)
        {
            return None;
        }
        match name.as_str() {
            "size_of_val" | "align_of_val" | "min_align_of_val" | "ptr_guaranteed_cmp" => {
                Some(Tree::from([(Vec::new(), HashSet::from([Fact::Unknown]))]))
            }
            "ptr_offset_from" | "ptr_offset_from_unsigned"
                if matches!(target.def, InstanceKind::Intrinsic(_)) && arguments.len() == 2 =>
            {
                let size = self
                    .pointer_layout(arguments[0].0)
                    .and_then(|(_, element)| {
                        self.tcx
                            .layout_of(TypingEnv::fully_monomorphized().as_query_input(element))
                            .ok()
                    })
                    .map(|layout| i128::from(layout.size.bytes()));
                let width = self.tcx.data_layout.pointer_size().bits();
                let mask = u128::MAX >> (128 - width);
                let maximum = (mask >> 1) as i128;
                let mut result = HashSet::new();
                for left in arguments[0].1.get(&Vec::new()).into_iter().flatten() {
                    for right in arguments[1].1.get(&Vec::new()).into_iter().flatten() {
                        let distance = match (left, right, size) {
                            (Fact::Reference(left), Fact::Reference(right), Some(size))
                                if size > 0 =>
                            {
                                let extent = match (&left.base, &right.base) {
                                    (Base::Allocation(left, _), Base::Allocation(right, _))
                                        if left == right =>
                                    {
                                        match self.tcx.global_alloc(*left) {
                                            GlobalAlloc::Memory(memory) => {
                                                Some(memory.inner().len() as i128)
                                            }
                                            _ => None,
                                        }
                                    }
                                    (Base::Static(left), Base::Static(right)) if left == right => {
                                        let ty = self.tcx.normalize_erasing_regions(
                                            TypingEnv::fully_monomorphized(),
                                            self.tcx.type_of(*left).instantiate_identity(),
                                        );
                                        self.tcx
                                            .layout_of(
                                                TypingEnv::fully_monomorphized().as_query_input(ty),
                                            )
                                            .ok()
                                            .map(|layout| i128::from(layout.size.bytes()))
                                    }
                                    _ => None,
                                };
                                extent.and_then(|extent| {
                                    let (_, left) = self.storage_position(left)?;
                                    let (_, right) = self.storage_position(right)?;
                                    if !(0..=extent).contains(&left)
                                        || !(0..=extent).contains(&right)
                                    {
                                        return None;
                                    }
                                    let bytes = left.checked_sub(right)?;
                                    if bytes < -maximum - 1
                                        || bytes > maximum
                                        || bytes % size != 0
                                        || name.as_str() == "ptr_offset_from_unsigned" && bytes < 0
                                    {
                                        return None;
                                    }
                                    Some(((bytes / size) as u128) & mask)
                                })
                            }
                            _ => None,
                        };
                        result.insert(distance.map_or(Fact::Unknown, Fact::Scalar));
                    }
                }
                if result.is_empty() && self.settling_unknowns {
                    result.insert(Fact::Unknown);
                }
                Some(Tree::from([(Vec::new(), result)]))
            }
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
                            let shared_load = matches!(
                                name.as_str(),
                                "volatile_load" | "unaligned_volatile_load" | "read_volatile"
                            );
                            let flow = if shared_load {
                                self.flow_locals.take()
                            } else {
                                None
                            };
                            let loaded = self.read(address);
                            if shared_load {
                                self.flow_locals = flow;
                            }
                            for (path, facts) in loaded {
                                result.entry(path).or_default().extend(facts);
                            }
                        }
                        Fact::Reference(_) | Fact::Unknown | Fact::IntegerPointer(_) => {
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
                let previous_strong = self.strong_update;
                self.strong_update = !matches!(name.as_str(), "volatile_store" | "unaligned_volatile_store" | "write_volatile")
                    && arguments[0].1.values().map(HashSet::len).sum::<usize>() == 1
                    && arguments[0].1.values().flatten().next().is_some_and(|fact| {
                        matches!(fact, Fact::Reference(address) if self.can_replace(address, arguments[1].0))
                    });
                for fact in arguments[0].1.values().flatten() {
                    if let Fact::Reference(address) = fact {
                        if address.pointee == Some(arguments[1].0) {
                            self.write(address, &arguments[1].1);
                        } else {
                            self.write(
                                address,
                                &Tree::from([(Vec::new(), HashSet::from([Fact::Unknown]))]),
                            );
                        }
                    } else if matches!(
                        fact,
                        Fact::Unknown | Fact::Scalar(_) | Fact::IntegerPointer(_)
                    ) {
                        let element = arguments[1].0;
                        self.write(
                            &Address {
                                base: Base::Unknown(element),
                                fields: Vec::new(),
                                pointee: Some(element),
                                byte_offset: 0,
                            },
                            &arguments[1].1,
                        );
                    }
                }
                self.strong_update = previous_strong;
                Some(Tree::new())
            }
            "copy" | "copy_nonoverlapping" if arguments.len() == 3 => {
                self.copy_memory(
                    &arguments[0].1,
                    &arguments[1].1,
                    arguments[0].0.builtin_deref(true),
                    &arguments[2].1,
                );
                Some(Tree::new())
            }
            "offset" | "arith_offset" | "add" | "sub" | "wrapping_offset" | "wrapping_add"
            | "wrapping_sub"
                if arguments.len() == 2 =>
            {
                let element = self
                    .pointer_layout(arguments[0].0)
                    .map(|(_, element)| element);
                Some(self.offset_pointer(
                    arguments[0].1.clone(),
                    &arguments[1].1,
                    arguments[1].0,
                    element,
                    matches!(name.as_str(), "sub" | "wrapping_sub"),
                ))
            }
            _ => None,
        }
    }

    fn offset_pointer(
        &self,
        mut pointers: Tree<'tcx>,
        counts: &Tree<'tcx>,
        count_ty: Ty<'tcx>,
        element: Option<Ty<'tcx>>,
        subtract: bool,
    ) -> Tree<'tcx> {
        let size = element
            .and_then(|ty| {
                self.tcx
                    .layout_of(TypingEnv::fully_monomorphized().as_query_input(ty))
                    .ok()
            })
            .map(|layout| i128::from(layout.size.bytes()));
        let width = self
            .tcx
            .layout_of(TypingEnv::fully_monomorphized().as_query_input(count_ty))
            .ok()
            .map(|layout| layout.size.bits());
        let mut deltas = HashSet::new();
        if size == Some(0) {
            deltas.insert(Some(0));
        } else {
            for fact in counts.get(&Vec::new()).into_iter().flatten() {
                let delta = match (fact, size, width) {
                    (Fact::Scalar(value), Some(size), Some(width)) if width > 0 && width <= 128 => {
                        let value = if matches!(count_ty.kind(), ty::Int(_)) {
                            Some(((*value << (128 - width)) as i128) >> (128 - width))
                        } else {
                            i128::try_from(*value).ok()
                        };
                        value
                            .and_then(|value| value.checked_mul(size))
                            .and_then(|delta| {
                                if subtract {
                                    delta.checked_neg()
                                } else {
                                    Some(delta)
                                }
                            })
                    }
                    _ => None,
                };
                deltas.insert(delta);
            }
        }
        if deltas.is_empty() && self.settling_unknowns {
            deltas.insert(None);
        }
        let pointer_width = self.tcx.data_layout.pointer_size().bits();
        let mask = if pointer_width == 128 {
            u128::MAX
        } else {
            (1u128 << pointer_width) - 1
        };
        for facts in pointers.values_mut() {
            let mut output = HashSet::new();
            for fact in facts.drain() {
                if matches!(fact, Fact::Concrete(_) | Fact::Length(_)) {
                    output.insert(fact);
                    continue;
                }
                for delta in &deltas {
                    let value = match (&fact, delta) {
                        (Fact::Reference(address), Some(delta))
                            if !matches!(address.base, Base::Unknown(_) | Base::Havoc) =>
                        {
                            address
                                .byte_offset
                                .checked_add(*delta)
                                .map(|offset| {
                                    let mut address = address.clone();
                                    address.byte_offset = offset;
                                    if let Base::Allocation(id, base_offset) = address.base {
                                        if let GlobalAlloc::Memory(memory) =
                                            self.tcx.global_alloc(id)
                                        {
                                            if !i128::from(base_offset)
                                                .checked_add(offset)
                                                .is_some_and(|position| {
                                                    (0..=memory.inner().len() as i128)
                                                        .contains(&position)
                                                })
                                            {
                                                return Fact::Unknown;
                                            }
                                        }
                                    } else {
                                        let storage_ty = match address.base {
                                            Base::Local(owner, _, local) => Some(
                                                self.ty(
                                                    owner,
                                                    self.tcx.instance_mir(owner.def).local_decls
                                                        [mir::Local::from_usize(local)]
                                                    .ty,
                                                ),
                                            ),
                                            Base::Parameter(_, _, _, ty) => Some(ty),
                                            Base::Static(def) => {
                                                Some(self.tcx.normalize_erasing_regions(
                                                    TypingEnv::fully_monomorphized(),
                                                    self.tcx.type_of(def).instantiate_identity(),
                                                ))
                                            }
                                            _ => None,
                                        };
                                        if let Some(storage_ty) = storage_ty {
                                            let extent = self
                                                .tcx
                                                .layout_of(
                                                    TypingEnv::fully_monomorphized()
                                                        .as_query_input(storage_ty),
                                                )
                                                .ok()
                                                .map(|layout| i128::from(layout.size.bytes()));
                                            match (extent, self.storage_position(&address)) {
                                                (Some(extent), Some((_, position)))
                                                    if (0..=extent).contains(&position) => {}
                                                _ if *delta != 0 => return Fact::Unknown,
                                                _ => {}
                                            }
                                        }
                                    }
                                    Fact::Reference(address)
                                })
                                .unwrap_or(Fact::Unknown)
                        }
                        (Fact::IntegerPointer(value), Some(delta)) => {
                            Fact::IntegerPointer(value.wrapping_add(*delta as u128) & mask)
                        }
                        (Fact::Function(_), Some(0)) => fact.clone(),
                        _ => Fact::Unknown,
                    };
                    output.insert(value);
                }
            }
            *facts = output;
        }
        pointers
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
        if let ty::Pat(inner, _) = source.kind() {
            return self.inner_path(*inner, destination, depth + 1);
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
                    self.tcx.normalize_erasing_regions(
                        TypingEnv::fully_monomorphized(),
                        adt.non_enum_variant().fields[rustc_abi::FieldIdx::from_usize(field)]
                            .ty(self.tcx, args),
                    ),
                    field as u32,
                )
            }
            ty::Array(element, _) | ty::Slice(element) => (*element, 0),
            _ => return None,
        };
        let mut path = vec![field];
        path.extend(self.inner_path(inner, destination, depth + 1)?);
        Some(path)
    }

    fn pointer_layout(&self, ty: Ty<'tcx>) -> Option<(Vec<u32>, Ty<'tcx>)> {
        match ty.kind() {
            ty::Pat(inner, _) => self.pointer_layout(*inner),
            ty::RawPtr(pointee, _) | ty::Ref(_, pointee, _) => Some((Vec::new(), *pointee)),
            ty::Adt(adt, args)
                if self.tcx.crate_name(adt.did().krate).as_str() == "core"
                    && matches!(self.tcx.item_name(adt.did()).as_str(), "NonNull" | "Unique") =>
            {
                let field = adt.non_enum_variant().fields[rustc_abi::FieldIdx::from_usize(0)]
                    .ty(self.tcx, args);
                let field = self
                    .tcx
                    .normalize_erasing_regions(TypingEnv::fully_monomorphized(), field);
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
        count: &Tree<'tcx>,
    ) {
        let count = count
            .get(&Vec::new())
            .filter(|values| values.len() == 1)
            .and_then(|values| values.iter().next());
        if count == Some(&Fact::Scalar(0)) {
            return;
        }
        if let (Some(Fact::Scalar(count)), Some(element)) = (count, element) {
            if let Ok(layout) = self
                .tcx
                .layout_of(TypingEnv::fully_monomorphized().as_query_input(element))
            {
                if layout.size.bytes() == 0 {
                    return;
                }
                if *count > 1 {
                    let mut copies = Vec::new();
                    let mut complete = true;
                    for index in 0..*count {
                        let Some(offset) = i128::try_from(index)
                            .ok()
                            .and_then(|index| index.checked_mul(i128::from(layout.size.bytes())))
                        else {
                            complete = false;
                            break;
                        };
                        let mut values = Tree::new();
                        for fact in sources.values().flatten() {
                            if let Fact::Reference(address) = fact {
                                let mut source = address.clone();
                                let Some(offset) = source.byte_offset.checked_add(offset) else {
                                    complete = false;
                                    break;
                                };
                                source.byte_offset = offset;
                                let Some(source) = self.memory_view(&source, element) else {
                                    complete = false;
                                    break;
                                };
                                for (path, facts) in self.read(&source) {
                                    values.entry(path).or_default().extend(facts);
                                }
                            } else if matches!(
                                fact,
                                Fact::Unknown | Fact::Scalar(_) | Fact::IntegerPointer(_)
                            ) {
                                values.entry(Vec::new()).or_default().insert(Fact::Unknown);
                            }
                        }
                        if !complete {
                            break;
                        }
                        for fact in destinations.values().flatten() {
                            if let Fact::Reference(address) = fact {
                                let mut destination = address.clone();
                                let Some(offset) = destination.byte_offset.checked_add(offset)
                                else {
                                    complete = false;
                                    break;
                                };
                                destination.byte_offset = offset;
                                let Some(destination) = self.memory_view(&destination, element)
                                else {
                                    complete = false;
                                    break;
                                };
                                copies.push((destination, values.clone()));
                            } else if matches!(
                                fact,
                                Fact::Unknown | Fact::Scalar(_) | Fact::IntegerPointer(_)
                            ) {
                                complete = false;
                                break;
                            }
                        }
                        if !complete {
                            break;
                        }
                    }
                    if complete {
                        for (destination, values) in copies {
                            self.write(&destination, &values);
                        }
                        return;
                    }
                }
            }
        }
        let mut values = Tree::new();
        for fact in sources.values().flatten() {
            match fact {
                Fact::Reference(address) if address.pointee == element && element.is_some() => {
                    for (path, facts) in self.read(address) {
                        values.entry(path).or_default().extend(facts);
                    }
                }
                Fact::Reference(_) | Fact::Unknown | Fact::IntegerPointer(_) => {
                    values.entry(Vec::new()).or_default().insert(Fact::Unknown);
                }
                _ => {}
            }
        }
        if count != Some(&Fact::Scalar(1)) {
            values.entry(Vec::new()).or_default().insert(Fact::Unknown);
        }
        for fact in destinations.values().flatten() {
            if let Fact::Reference(address) = fact {
                if count != Some(&Fact::Scalar(1)) {
                    let mut storage = address.clone();
                    storage.fields.clear();
                    storage.byte_offset = 0;
                    storage.pointee = None;
                    self.write(
                        &storage,
                        &Tree::from([(Vec::new(), HashSet::from([Fact::Unknown]))]),
                    );
                }
                if address.pointee != element {
                    self.write(
                        address,
                        &Tree::from([(Vec::new(), HashSet::from([Fact::Unknown]))]),
                    );
                } else {
                    self.write(address, &values);
                }
            } else if matches!(
                fact,
                Fact::Unknown | Fact::Scalar(_) | Fact::IntegerPointer(_)
            ) {
                let element = element.unwrap_or(self.tcx.types.unit);
                self.write(
                    &Address {
                        base: Base::Unknown(element),
                        fields: Vec::new(),
                        pointee: Some(element),
                        byte_offset: 0,
                    },
                    &values,
                );
            }
        }
    }

    fn storage_position(&self, address: &Address<'tcx>) -> Option<(u64, i128)> {
        if let Base::Allocation(id, base_offset) = address.base {
            if !address.fields.is_empty() {
                return None;
            }
            let GlobalAlloc::Memory(memory) = self.tcx.global_alloc(id) else {
                return None;
            };
            return Some((
                memory.inner().align.bytes(),
                i128::from(base_offset).checked_add(address.byte_offset)?,
            ));
        }
        let source = match address.base {
            Base::Parameter(_, _, _, ty) | Base::Heap(_, _, _, ty) => ty,
            Base::Local(instance, _, local) => self.ty(
                instance,
                self.tcx.instance_mir(instance.def).local_decls[mir::Local::from_usize(local)].ty,
            ),
            Base::Static(def) => self.tcx.normalize_erasing_regions(
                TypingEnv::fully_monomorphized(),
                self.tcx.type_of(def).instantiate_identity(),
            ),
            _ => return None,
        };
        let alignment = self
            .tcx
            .layout_of(TypingEnv::fully_monomorphized().as_query_input(source))
            .ok()?
            .align
            .abi
            .bytes();
        let mut ty = source;
        let mut offset = address.byte_offset;
        for field in &address.fields {
            if *field == u32::MAX {
                return None;
            }
            let layout = self
                .tcx
                .layout_of(TypingEnv::fully_monomorphized().as_query_input(ty))
                .ok()?;
            if *field as usize >= layout.fields.count() {
                return None;
            }
            offset =
                offset.checked_add(i128::from(layout.fields.offset(*field as usize).bytes()))?;
            ty = match ty.kind() {
                ty::Tuple(fields) => *fields.get(*field as usize)?,
                ty::Adt(adt, args) if !adt.is_enum() => self.tcx.normalize_erasing_regions(
                    TypingEnv::fully_monomorphized(),
                    adt.non_enum_variant()
                        .fields
                        .get(rustc_abi::FieldIdx::from_u32(*field))?
                        .ty(self.tcx, args),
                ),
                ty::Array(element, _) => *element,
                _ => return None,
            };
        }
        Some((alignment, offset))
    }

    fn memory_view(&self, address: &Address<'tcx>, destination: Ty<'tcx>) -> Option<Address<'tcx>> {
        let source = match address.base {
            Base::Parameter(_, _, _, ty) | Base::Heap(_, _, _, ty) => ty,
            Base::Local(instance, _, local) => self.ty(
                instance,
                self.tcx.instance_mir(instance.def).local_decls[mir::Local::from_usize(local)].ty,
            ),
            Base::Static(def) => self.tcx.normalize_erasing_regions(
                TypingEnv::fully_monomorphized(),
                self.tcx.type_of(def).instantiate_identity(),
            ),
            _ => return None,
        };
        let (_, offset) = self.storage_position(address)?;
        let fields = self.view_path(source, destination, u64::try_from(offset).ok()?, 0)?;
        Some(Address {
            base: address.base.clone(),
            fields,
            pointee: Some(destination),
            byte_offset: 0,
        })
    }

    fn view_path(
        &self,
        source: Ty<'tcx>,
        destination: Ty<'tcx>,
        offset: u64,
        depth: usize,
    ) -> Option<Vec<u32>> {
        if source == destination && offset == 0 {
            return Some(Vec::new());
        }
        if depth >= 32 {
            return None;
        }
        let layout = self
            .tcx
            .layout_of(TypingEnv::fully_monomorphized().as_query_input(source))
            .ok()?;
        let fields: Vec<_> = match source.kind() {
            ty::Tuple(fields) => fields.iter().collect(),
            ty::Adt(adt, args)
                if !adt.is_enum()
                    && (!adt.is_union()
                        || self.tcx.lang_items().get(LangItem::MaybeUninit) == Some(adt.did())) =>
            {
                adt.non_enum_variant()
                    .fields
                    .iter()
                    .map(|field| {
                        self.tcx.normalize_erasing_regions(
                            TypingEnv::fully_monomorphized(),
                            field.ty(self.tcx, args),
                        )
                    })
                    .collect()
            }
            ty::Array(element, _) => {
                let element_size = self
                    .tcx
                    .layout_of(TypingEnv::fully_monomorphized().as_query_input(*element))
                    .ok()?
                    .size
                    .bytes();
                if element_size == 0 || offset >= layout.size.bytes() {
                    return None;
                }
                let mut path = vec![u32::try_from(offset / element_size).ok()?];
                path.extend(self.view_path(
                    *element,
                    destination,
                    offset % element_size,
                    depth + 1,
                )?);
                return Some(path);
            }
            ty::Pat(inner, _) => return self.view_path(*inner, destination, offset, depth + 1),
            _ => return None,
        };
        for (index, ty) in fields.into_iter().enumerate() {
            let start = layout.fields.offset(index).bytes();
            if let Some(relative) = offset.checked_sub(start) {
                if let Some(tail) = self.view_path(ty, destination, relative, depth + 1) {
                    let mut path = vec![index as u32];
                    path.extend(tail);
                    return Some(path);
                }
            }
        }
        None
    }

    fn provenance_cast(
        &self,
        tree: &Tree<'tcx>,
        source: Ty<'tcx>,
        destination: Ty<'tcx>,
        transmute: bool,
    ) -> Option<Tree<'tcx>> {
        let source_pointer = self.pointer_layout(source).or_else(|| {
            matches!(source.kind(), ty::FnPtr(..)).then_some((Vec::new(), self.tcx.types.unit))
        });
        let destination_pointer = self.pointer_layout(destination).or_else(|| {
            matches!(destination.kind(), ty::FnPtr(..)).then_some((Vec::new(), self.tcx.types.unit))
        });
        let pointer_bits = self.tcx.data_layout.pointer_size().bits();
        if destination.is_integral() {
            let (path, _) = source_pointer?;
            let destination_layout = self
                .tcx
                .layout_of(TypingEnv::fully_monomorphized().as_query_input(destination))
                .ok()?;
            let source_layout = self
                .tcx
                .layout_of(TypingEnv::fully_monomorphized().as_query_input(source))
                .ok()?;
            if transmute
                && (source_layout.size.bits() != pointer_bits
                    || source_layout.size != destination_layout.size)
            {
                return None;
            }
            let mut values = HashSet::new();
            for (stored, facts) in tree {
                if *stored == path {
                    for fact in facts {
                        let value = if destination_layout.size.bits() < pointer_bits {
                            Fact::Unknown
                        } else {
                            match fact {
                                Fact::Reference(address) => Fact::ExposedPointer(
                                    PointerOrigin::Storage(address.clone()),
                                    destination_layout.size.bits(),
                                ),
                                Fact::Function(instance) => Fact::ExposedPointer(
                                    PointerOrigin::Function(*instance),
                                    destination_layout.size.bits(),
                                ),
                                Fact::IntegerPointer(value) => Fact::Scalar(*value),
                                Fact::Concrete(_) | Fact::Length(_) => continue,
                                _ => Fact::Unknown,
                            }
                        };
                        values.insert(value);
                    }
                } else if path.starts_with(stored) && facts.contains(&Fact::Unknown) {
                    values.insert(Fact::Unknown);
                }
            }
            return Some(Tree::from([(Vec::new(), values)]));
        }
        if source.is_integral() {
            let (path, pointee) = destination_pointer?;
            let source_layout = self
                .tcx
                .layout_of(TypingEnv::fully_monomorphized().as_query_input(source))
                .ok()?;
            let destination_layout = self
                .tcx
                .layout_of(TypingEnv::fully_monomorphized().as_query_input(destination))
                .ok()?;
            if destination_layout.size.bits() != pointer_bits
                || transmute && source_layout.size != destination_layout.size
            {
                return None;
            }
            let mut values = HashSet::new();
            for fact in tree.get(&Vec::new()).into_iter().flatten() {
                match fact {
                    Fact::ExposedPointer(PointerOrigin::Storage(address), width)
                        if *width >= pointer_bits =>
                    {
                        values.extend(self.cast_pointer(
                            HashSet::from([Fact::Reference(address.clone())]),
                            address.pointee.unwrap_or(self.tcx.types.unit),
                            pointee,
                        ));
                    }
                    Fact::ExposedPointer(PointerOrigin::Function(instance), width)
                        if *width >= pointer_bits =>
                    {
                        values.insert(Fact::Function(*instance));
                    }
                    Fact::Scalar(value) => {
                        values.insert(Fact::IntegerPointer(if pointer_bits == 128 {
                            *value
                        } else {
                            *value & ((1u128 << pointer_bits) - 1)
                        }));
                    }
                    _ => {
                        values.insert(Fact::Unknown);
                    }
                }
            }
            return Some(Tree::from([(path, values)]));
        }
        None
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
                if matches!(address.base, Base::Allocation(..)) && address.fields.is_empty() {
                    address.pointee = Some(destination);
                    output.insert(Fact::Reference(address));
                    continue;
                }
                if (address.byte_offset != 0 || source != destination)
                    && destination != self.tcx.types.u8
                    && destination != self.tcx.types.unit
                {
                    if let Some(view) = self.memory_view(&address, destination) {
                        output.insert(Fact::Reference(view));
                        continue;
                    }
                    if address.byte_offset != 0 {
                        output.insert(Fact::Unknown);
                        continue;
                    }
                }
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
            } else if matches!(fact, Fact::Concrete(_))
                && !matches!(destination.kind(), ty::Dynamic(..))
            {
                continue;
            } else if matches!(fact, Fact::Length(_))
                && !matches!(destination.kind(), ty::Slice(_) | ty::Str)
            {
                continue;
            } else {
                output.insert(fact);
            }
        }
        output
    }

    fn exposed_to_havoc(&self, base: &Base<'tcx>) -> bool {
        match base {
            Base::Havoc => false,
            Base::Allocation(id, _) => {
                !matches!(self.tcx.global_alloc(*id), GlobalAlloc::Memory(memory) if memory.inner().mutability == rustc_hir::Mutability::Not)
            }
            Base::Local(instance, _, local) => {
                self.escaped.contains(base)
                    || self.address_taken.get(instance).map_or_else(
                        || {
                            rustc_mir_dataflow::impls::borrowed_locals(
                                self.tcx.instance_mir(instance.def),
                            )
                            .contains(mir::Local::from_usize(*local))
                        },
                        |locals| locals.contains(local),
                    )
            }
            _ => true,
        }
    }

    fn stored_tree(&self, base: &Base<'tcx>) -> Tree<'tcx> {
        let cached = self.memory_views.borrow().get(base).cloned();
        if let Some(tree) = cached {
            self.memory_view_hits.set(self.memory_view_hits.get() + 1);
            return tree;
        }
        self.memory_view_misses
            .set(self.memory_view_misses.get() + 1);
        let mut tree = Tree::new();
        for (path, fact) in self.memory.get_pts(base) {
            tree.entry(path.clone()).or_default().insert(fact.clone());
        }
        self.memory_views
            .borrow_mut()
            .insert(base.clone(), tree.clone());
        tree
    }

    fn memory_tree(&self, base: &Base<'tcx>) -> Tree<'tcx> {
        let mut tree = self.stored_tree(base);
        if self.exposed_to_havoc(base)
            && !(matches!(base, Base::Local(..)) && !self.escaped.contains(base))
        {
            let havoc = self.stored_tree(&Base::Havoc);
            if let Some((_, _, cached)) = self
                .effective_views
                .borrow()
                .get(base)
                .filter(|(stored, previous_havoc, _)| *stored == tree && *previous_havoc == havoc)
            {
                self.effective_view_hits
                    .set(self.effective_view_hits.get() + 1);
                return cached.clone();
            }
            let stored = tree.clone();
            let mut root = tree.get(&Vec::new()).cloned().unwrap_or_default();
            join_facts(&mut root, havoc.values().flatten().cloned());
            if tree.get(&Vec::new()) != Some(&root) {
                tree.insert(Vec::new(), root);
            }
            self.effective_views
                .borrow_mut()
                .insert(base.clone(), (stored, havoc, tree.clone()));
        }
        tree
    }

    fn subscribe_read(&mut self, base: &Base<'tcx>, path: &[u32]) {
        let Some(active) = self.active else {
            return;
        };
        if matches!(base, Base::Havoc) && path.is_empty() {
            if self.havoc_subscribed {
                self.read_subscription_hits += 1;
                return;
            }
            self.havoc_subscribed = true;
        } else if !self
            .read_subscriptions
            .entry(base.clone())
            .or_default()
            .insert(path.to_vec())
        {
            self.read_subscription_hits += 1;
            return;
        }
        self.readers
            .entry(base.clone())
            .or_default()
            .entry(path.to_vec())
            .or_default()
            .insert(active);
    }

    fn read(&mut self, address: &Address<'tcx>) -> Tree<'tcx> {
        if let (Base::Allocation(id, base_offset), Some(pointee)) = (&address.base, address.pointee)
        {
            if address.fields.is_empty()
                && matches!(self.tcx.global_alloc(*id), GlobalAlloc::Memory(memory) if memory.inner().mutability == rustc_hir::Mutability::Not)
            {
                self.subscribe_read(&address.base, &[]);
                let mut result = i128::from(*base_offset)
                    .checked_add(address.byte_offset)
                    .and_then(|offset| u64::try_from(offset).ok())
                    .map(|offset| self.allocation(*id, offset, pointee))
                    .unwrap_or_else(|| Tree::from([(Vec::new(), HashSet::from([Fact::Unknown]))]));
                if self.written_allocations.contains(id) {
                    let facts = result.entry(Vec::new()).or_default();
                    facts.insert(Fact::Unknown);
                    for base in self.memory.objects.keys() {
                        if matches!(base, Base::Allocation(other, _) if other == id) {
                            facts.extend(self.memory.get_pts(base).filter_map(|(_, fact)| {
                                matches!(
                                    fact,
                                    Fact::Function(_)
                                        | Fact::Reference(_)
                                        | Fact::ExposedPointer(..)
                                )
                                .then_some(fact.clone())
                            }));
                        }
                    }
                }
                return result;
            }
        }
        if address.byte_offset != 0 {
            if let Some(view) = address.pointee.and_then(|ty| self.memory_view(address, ty)) {
                return self.read(&view);
            }
            return Tree::from([(Vec::new(), HashSet::from([Fact::Unknown]))]);
        }
        if self.exposed_to_havoc(&address.base) {
            self.subscribe_read(&Base::Havoc, &[]);
        }
        self.subscribe_read(&address.base, &address.fields);
        let mut tree = Tree::new();
        if matches!(address.base, Base::Unknown(_)) {
            tree.entry(Vec::new()).or_default().insert(Fact::Unknown);
        }
        let local = self.flow_locals.as_ref().and_then(|memory| {
            if memory.contains_key(&address.base) || matches!(&address.base, Base::Local(instance, context, _) if self.active.is_some_and(|node| node.instance == *instance && node.context == *context)) {
                Some(memory.get(&address.base))
            } else { None }
        });
        let havoc = if !self.exposed_to_havoc(&address.base)
            || matches!(address.base, Base::Local(..)) && !self.escaped.contains(&address.base)
        {
            Tree::new()
        } else {
            match local {
                Some(_) => self
                    .flow_locals
                    .as_ref()
                    .and_then(|memory| memory.get(&Base::Havoc))
                    .cloned()
                    .unwrap_or_default(),
                None => self.memory_tree(&Base::Havoc),
            }
        };
        let mut memory = match local {
            Some(values) => values.cloned().unwrap_or_default(),
            None => self.memory_tree(&address.base),
        };
        if local.is_some() && self.escaped.contains(&address.base) {
            for (fields, facts) in &self.memory_tree(&address.base) {
                let mut merged = memory.get(fields).cloned().unwrap_or_default();
                join_facts(&mut merged, facts.iter().cloned());
                if memory.get(fields) != Some(&merged) {
                    memory.insert(fields.clone(), merged);
                }
            }
        }
        if address.fields.is_empty() {
            if matches!(address.base, Base::Unknown(_))
                && !memory
                    .get(&Vec::new())
                    .is_some_and(|facts| facts.contains(&Fact::Unknown))
            {
                memory.entry(Vec::new()).or_default().insert(Fact::Unknown);
            }
            tree = memory;
        } else {
            for (fields, facts) in memory.project(&address.fields) {
                tree.entry(fields).or_default().extend(facts);
            }
        }
        if !havoc.is_empty() {
            let mut root = tree.get(&Vec::new()).cloned().unwrap_or_default();
            join_facts(&mut root, havoc.values().flatten().cloned());
            if tree.get(&Vec::new()) != Some(&root) {
                tree.insert(Vec::new(), root);
            }
        }
        tree
    }

    fn write(&mut self, address: &Address<'tcx>, tree: &Tree<'tcx>) {
        if !self.initializing_allocation
            && (!matches!(address.base, Base::Local(_, _, local) if local != 0)
                && !self.singleton_storage.contains(&address.base)
                || self.escaped.contains(&address.base))
        {
            let mut pending: VecDeque<_> = tree
                .values()
                .flatten()
                .filter_map(|fact| match fact {
                    Fact::Reference(pointer)
                    | Fact::ExposedPointer(PointerOrigin::Storage(pointer), _) => {
                        Some(pointer.base.clone())
                    }
                    _ => None,
                })
                .collect();
            let mut visited = HashSet::new();
            while let Some(base) = pending.pop_front() {
                if !visited.insert(base.clone()) {
                    continue;
                }
                if matches!(address.base, Base::Local(_, _, 0))
                    && matches!(base, Base::Parameter(..))
                {
                    continue;
                }
                if matches!(base, Base::Local(..) | Base::Parameter(..)) {
                    self.mark_escaped(base.clone());
                }
                let values = self.read(&Address {
                    base,
                    fields: Vec::new(),
                    pointee: None,
                    byte_offset: 0,
                });
                pending.extend(values.values().flatten().filter_map(|fact| match fact {
                    Fact::Reference(pointer)
                    | Fact::ExposedPointer(PointerOrigin::Storage(pointer), _) => {
                        Some(pointer.base.clone())
                    }
                    _ => None,
                }));
            }
        }
        if matches!(address.base, Base::Havoc) {
            let affected: Vec<_> = self
                .flow_locals
                .as_ref()
                .into_iter()
                .flat_map(|memory| memory.keys())
                .filter(|base| matches!(base, Base::Local(..)) && self.exposed_to_havoc(base))
                .cloned()
                .collect();
            let previous_strong = self.strong_update;
            let previous_effects = self.record_effects;
            self.strong_update = false;
            self.record_effects = true;
            for base in affected {
                self.write(
                    &Address {
                        base,
                        fields: Vec::new(),
                        pointee: None,
                        byte_offset: 0,
                    },
                    tree,
                );
            }
            self.strong_update = previous_strong;
            self.record_effects = previous_effects;
            if let Some(active) = self.active {
                if self.havoc_origins.insert((active, self.active_location)) {
                    self.havoc_order.push((active, self.active_location));
                }
            }
        }
        if !self.initializing_allocation {
            if let Base::Allocation(id, _) = address.base {
                if self.written_allocations.insert(id) {
                    if std::env::var_os("IRQ_CHECK_STATS").is_some()
                        && matches!(self.tcx.global_alloc(id), GlobalAlloc::Memory(memory) if memory.inner().mutability == rustc_hir::Mutability::Not)
                    {
                        eprintln!(
                            "irq-check: immutable storage write: {address:?}, caller {:?}, location {:?}",
                            self.active, self.active_location
                        );
                    }
                    let readers: HashSet<_> = self
                        .readers
                        .iter()
                        .filter(
                            |(base, _)| matches!(base, Base::Allocation(other, _) if *other == id),
                        )
                        .flat_map(|(_, paths)| {
                            paths.values().flat_map(|readers| readers.iter().copied())
                        })
                        .collect();
                    for reader in readers {
                        self.schedule(reader);
                    }
                }
            }
        }
        if matches!(address.base, Base::Unknown(_)) {
            let mut facts: HashSet<_> = tree
                .values()
                .flatten()
                .filter(|fact| {
                    !matches!(
                        fact,
                        Fact::Scalar(_) | Fact::Length(_) | Fact::IntegerPointer(_)
                    )
                })
                .cloned()
                .collect();
            facts.insert(Fact::Unknown);
            self.write(
                &Address {
                    base: Base::Havoc,
                    fields: Vec::new(),
                    pointee: None,
                    byte_offset: 0,
                },
                &Tree::from([(Vec::new(), facts)]),
            );
            return;
        }
        if self.record_effects && !self.initializing_allocation {
            if let Some(active) = self.active {
                if !matches!(address.base, Base::Local(instance, context, _) if instance == active.instance && context == active.context)
                {
                    let output = self
                        .effects
                        .entry(active)
                        .or_default()
                        .entry(address.clone())
                        .or_default();
                    let changed = output.join(tree);
                    if changed {
                        for reader in self
                            .effect_readers
                            .get(&active)
                            .cloned()
                            .unwrap_or_default()
                        {
                            self.schedule(reader);
                        }
                    }
                }
            }
        }
        if address.byte_offset != 0 {
            if let Some(view) = address.pointee.and_then(|ty| self.memory_view(address, ty)) {
                self.write(&view, tree);
            } else {
                let mut storage = address.clone();
                storage.fields.clear();
                storage.byte_offset = 0;
                self.write(
                    &storage,
                    &Tree::from([(Vec::new(), HashSet::from([Fact::Unknown]))]),
                );
            }
            return;
        }
        let own_local = matches!(&address.base, Base::Local(instance, context, _) if self.active.is_some_and(|node| node.instance == *instance && node.context == *context));
        if !self.initializing_allocation
            && (own_local || self.record_effects || matches!(address.base, Base::Havoc))
        {
            if let Some(locals) = self.flow_locals.as_mut() {
                let memory = locals.entry(address.base.clone()).or_default();
                if self.strong_update
                    && !matches!(address.base, Base::Havoc)
                    && !self.escaped.contains(&address.base)
                {
                    memory.retain(|fields, _| !fields.starts_with(&address.fields));
                }
                for (fields, facts) in tree {
                    let mut path = address.fields.clone();
                    path.extend(fields);
                    join_facts(memory.entry(path).or_default(), facts.iter().cloned());
                }
            }
            if own_local
                && self.flow_locals.is_some()
                && matches!(address.base, Base::Local(_, _, local) if !self.published_locals.contains(&local))
            {
                return;
            }
        }
        let mut memory = self.stored_tree(&address.base);
        if tree.iter().all(|(fields, facts)| {
            let mut path = address.fields.clone();
            path.extend(fields);
            facts.is_empty()
                || memory
                    .get(&path)
                    .is_some_and(|stored| facts.is_subset(stored))
        }) {
            self.redundant_memory_writes += 1;
            return;
        }
        let mut added = HashSet::new();
        for (path, facts) in tree {
            let mut address = address.clone();
            address.fields.extend(path);
            if address.fields.len() > 12 {
                address.fields.truncate(12);
                added.insert((address.fields.clone(), Fact::Unknown));
            }
            if memory.len() >= 32 && !memory.contains_key(&address.fields) {
                added.insert((Vec::new(), Fact::Unknown));
                continue;
            }
            let output = memory.entry(address.fields.clone()).or_default();
            let mut references = output
                .iter()
                .filter(|fact| matches!(fact, Fact::Reference(_)))
                .count();
            for fact in facts {
                if matches!(
                    fact,
                    Fact::Scalar(_) | Fact::Length(_) | Fact::IntegerPointer(_)
                ) && (output.contains(&Fact::Unknown)
                    || !output.contains(fact)
                        && output
                            .iter()
                            .filter(|fact| {
                                matches!(
                                    fact,
                                    Fact::Scalar(_) | Fact::Length(_) | Fact::IntegerPointer(_)
                                )
                            })
                            .count()
                            >= 8)
                {
                    output.retain(|fact| {
                        !matches!(
                            fact,
                            Fact::Scalar(_) | Fact::Length(_) | Fact::IntegerPointer(_)
                        )
                    });
                    output.insert(Fact::Unknown);
                    added.insert((address.fields.clone(), Fact::Unknown));
                    continue;
                }
                if matches!(fact, Fact::Reference(_)) && !output.contains(fact) {
                    if references >= 16 {
                        added.insert((address.fields.clone(), Fact::Unknown));
                        continue;
                    }
                    references += 1;
                }
                output.insert(fact.clone());
                added.insert((address.fields.clone(), fact.clone()));
            }
        }
        let widened: HashSet<_> = added
            .iter()
            .filter_map(|(path, fact)| matches!(fact, Fact::Unknown).then_some(path.clone()))
            .collect();
        if !widened.is_empty() {
            added.retain(|(path, fact)| {
                !widened.contains(path)
                    || !matches!(
                        fact,
                        Fact::Scalar(_) | Fact::Length(_) | Fact::IntegerPointer(_)
                    )
            });
            if let Some(object) = self.memory.objects.get(&address.base).copied() {
                let removed: Vec<_> = self.memory.data.get_propa_pts(object).into_iter()
                    .flat_map(PointsToSet::iter).filter(|id| {
                        let (path, fact) = &self.memory.values[id.0];
                        widened.contains(path) && matches!(fact,
                            Fact::Scalar(_) | Fact::Length(_) | Fact::IntegerPointer(_))
                    }).collect();
                for id in &removed {
                    self.memory.data.remove_pts_elem(object, *id);
                }
                if !removed.is_empty() {
                    if self.escaped.contains(&address.base)
                        || matches!(
                            address.base,
                            Base::Havoc | Base::Static(_) | Base::Allocation(..)
                        )
                    {
                        self.memory_epoch += 1;
                    }
                    self.memory_views.get_mut().remove(&address.base);
                }
            }
        }
        if self.memory.union_pts_to(address.base.clone(), &added) {
            if self.escaped.contains(&address.base)
                || matches!(
                    address.base,
                    Base::Havoc | Base::Static(_) | Base::Allocation(..)
                )
            {
                self.memory_epoch += 1;
            }
            self.memory_views.get_mut().remove(&address.base);
            let delta = self.memory.flush(address.base.clone());
            if matches!(address.base, Base::Havoc) {
                self.opaque_shared_dirty.extend(self.escaped.iter().cloned());
            } else if let Base::Allocation(id, _) = address.base {
                self.opaque_shared_dirty.extend(self.escaped.iter().filter(|base|
                    matches!(base, Base::Allocation(other, _) if *other == id)).cloned());
            } else if self.escaped.contains(&address.base) {
                self.opaque_shared_dirty.insert(address.base.clone());
            }
            if matches!(address.base, Base::Havoc | Base::Allocation(..))
                || self.escaped.contains(&address.base)
            {
                for reader in self.escape_readers.clone() {
                    self.schedule(reader);
                }
            }
            self.revision += delta.len();
            let mut callbacks_changed = false;
            for (path, fact) in &delta {
                if matches!(address.base, Base::Local(..) | Base::Parameter(..) | Base::Heap(..) | Base::Static(_)) {
                    let storage = address.base.discovery_storage();
                    match fact {
                        Fact::Function(target) => {
                            self.backend_storage_feedback.insert((storage, path.clone(), *target));
                        }
                        Fact::Reference(source)
                            if matches!(source.base, Base::Local(..) | Base::Parameter(..) | Base::Heap(..) | Base::Static(_)) =>
                        {
                            let mut source = source.clone();
                            source.base = source.base.discovery_storage();
                            self.backend_reference_feedback.insert((storage, path.clone(), source));
                        }
                        _ => {}
                    }
                }
                if let Fact::Function(target)
                | Fact::ExposedPointer(PointerOrigin::Function(target), _) = fact
                {
                    callbacks_changed |= self.known_callbacks.insert(*target);
                }
            }
            let readers: HashSet<_> = self
                .readers
                .get(&address.base)
                .into_iter()
                .flat_map(|paths| paths.iter())
                .filter(|(path, _)| {
                    delta.iter().any(|(changed, _)| {
                        changed
                            .iter()
                            .zip(path.iter())
                            .all(|(a, b)| a == b || *a == u32::MAX || *b == u32::MAX)
                    })
                })
                .flat_map(|(_, readers)| readers.iter().copied())
                .collect();
            for reader in readers {
                self.schedule(reader);
            }
            if callbacks_changed {
                for reader in self.unknown_readers.clone() {
                    self.schedule(reader);
                }
            }
            if !self.initializing_allocation {
                if let Base::Allocation(id, _) = address.base {
                    let readers: HashSet<_> = self
                        .readers
                        .iter()
                        .filter(
                            |(base, _)| matches!(base, Base::Allocation(other, _) if *other == id),
                        )
                        .flat_map(|(_, paths)| {
                            paths.values().flat_map(|readers| readers.iter().copied())
                        })
                        .collect();
                    for reader in readers {
                        self.schedule(reader);
                    }
                }
            }
        }
    }

    fn mark_escaped(&mut self, base: Base<'tcx>) {
        if self.escaped.contains(&base) {
            return;
        }
        let mut pending = VecDeque::from([base]);
        let mut changed = false;
        while let Some(base) = pending.pop_front() {
            if !self.escaped.insert(base.clone()) {
                continue;
            }
            changed = true;
            self.opaque_shared_dirty.insert(base.clone());
            self.singleton_storage.remove(&base);
            pending.extend(
                self.escape_aliases
                    .get(&base)
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
            let readers: HashSet<_> = self
                .readers
                .get(&base)
                .into_iter()
                .flat_map(|paths| paths.values().flatten().copied())
                .collect();
            for reader in readers {
                self.schedule(reader);
            }
            if let Base::Local(instance, context, _) | Base::Parameter(instance, context, ..) = base
            {
                let owner = Node { instance, context };
                self.schedule(owner);
                for reader in self.effect_readers.get(&owner).cloned().unwrap_or_default() {
                    self.schedule(reader);
                }
            }
        }
        if changed {
            for reader in self.escape_readers.clone() {
                self.schedule(reader);
            }
        }
    }

    fn backend_storage_path(&mut self, base: &Base<'tcx>, fields: &[u32]) -> Option<BackendStorage<'tcx>> {
        use irq_check_rupta::mir::path::{Path as BackendPath, PathEnum, PathSelector};
        let tcx = self.tcx;
        let base = base.discovery_storage();
        let (owner, instance, mir_root, mut storage_ty) = match base {
            Base::Local(instance, _, ordinal) => {
                let function = self.backend.acx.get_instance_id(instance);
                let body = tcx.instance_mir(instance.def);
                let local = body.local_decls.get(mir::Local::from_usize(ordinal))?;
                (Some(function), Some(instance),
                    Some(BackendPath::new_local_parameter_or_result(function, ordinal, body.arg_count)),
                    self.ty(instance, local.ty))
            }
            Base::Parameter(instance, _, _, ty) | Base::Heap(instance, _, _, ty) => (
                Some(self.backend.acx.get_instance_id(instance)), None, None, ty),
            Base::Static(def) => (None, None, Some(BackendPath::new_static_variable(def)),
                tcx.normalize_erasing_regions(TypingEnv::fully_monomorphized(),
                    tcx.type_of(def).instantiate_identity())),
            _ => return None,
        };
        let object = self.memory.intern_object(base);
        let path = std::rc::Rc::new(BackendPath { value: PathEnum::SharedStorage(object.0) });
        if let Some(mir_root) = &mir_root {
            self.backend.acx.set_path_rustc_type(mir_root.clone(), storage_ty);
        }
        let mut projections = Vec::new();
        let mut fields = fields.iter().copied();
        let mut variant = None;
        let mut supported = true;
        self.backend.acx.set_path_rustc_type(path.clone(), storage_ty);
        while let Some(field) = fields.next() {
            while let ty::Pat(inner, _) = storage_ty.kind() {
                storage_ty = *inner;
            }
            if field == VARIANT {
                let Some(index) = fields.next() else {
                    supported = false;
                    break;
                };
                let ty::Adt(adt, _) = storage_ty.kind() else {
                    supported = false;
                    break;
                };
                if !adt.is_enum() || index as usize >= adt.variants().len() {
                    supported = false;
                    break;
                }
                variant = Some(rustc_abi::VariantIdx::from_u32(index));
                projections.push(PathSelector::Downcast(index as usize));
                continue;
            }
            let next = match storage_ty.kind() {
                ty::Tuple(types) => types.get(field as usize).copied().map(|ty| {
                    projections.push(PathSelector::Field(field as usize));
                    ty
                }),
                ty::Array(element, _) | ty::Slice(element) => {
                    projections.push(PathSelector::Index);
                    Some(*element)
                }
                ty::Adt(adt, args) => {
                    let selected = if adt.is_enum() {
                        variant.take().map(|variant| adt.variant(variant))
                    } else {
                        Some(adt.non_enum_variant())
                    };
                    selected.and_then(|variant| variant.fields.get(rustc_abi::FieldIdx::from_u32(field)))
                        .map(|definition| {
                            projections.push(if adt.is_union() {
                                PathSelector::UnionField(field as usize)
                            } else {
                                PathSelector::Field(field as usize)
                            });
                            tcx.normalize_erasing_regions(TypingEnv::fully_monomorphized(), definition.ty(tcx, args))
                        })
                }
                ty::Closure(_, args) => args.as_closure().upvar_tys().get(field as usize).copied().map(|ty| {
                    projections.push(PathSelector::Field(field as usize));
                    ty
                }),
                _ => None,
            };
            let Some(next) = next else {
                supported = false;
                break;
            };
            storage_ty = next;
        }
        while let ty::Pat(inner, _) = storage_ty.kind() {
            storage_ty = *inner;
        }
        if !supported {
            if let Some(owner) = owner {
                self.backend.acx.coverage_gaps.insert((owner, None,
                    "shared storage projection requires flow refinement".into()));
            }
            return None;
        }
        let object_path = BackendPath::append_projection(&path, &projections);
        self.backend.acx.set_path_rustc_type(object_path.clone(), storage_ty);
        let mir_path = mir_root.map(|root| {
            let path = BackendPath::append_projection(&root, &projections);
            self.backend.acx.set_path_rustc_type(path.clone(), storage_ty);
            path
        });
        Some(BackendStorage { owner, instance, mir_path, object_path, ty: storage_ty })
    }

    fn synchronize_backend(&mut self) {
        use irq_check_rupta::mir::path::Path as BackendPath;
        use irq_check_rupta::mir::call_site::{BaseCallSite, CallType};
        use irq_check_rupta::mir::function::GenericArgE;
        use irq_check_rupta::pta::PointerAnalysis;
        let started = std::time::Instant::now();
        let tcx = self.tcx;
        let feedback = std::mem::take(&mut self.backend_feedback);
        let feedback_count = feedback.len();
        let storage_feedback = std::mem::take(&mut self.backend_storage_feedback);
        let storage_feedback_count = storage_feedback.len();
        let reference_feedback = std::mem::take(&mut self.backend_reference_feedback);
        let reference_feedback_count = reference_feedback.len();
        let mut updates = Vec::new();
        let mut storage_updates = Vec::new();
        let mut roots: HashSet<_> = self.backend.acx.discovery_roots.iter().copied().collect();
        let mut discovered: HashSet<_> = self.backend.call_graph.func_nodes.keys()
            .map(|function| function.func_id).collect();
        let mut storage_update_count = 0usize;
        let mut reference_update_count = 0usize;
        for (base, fields, target) in storage_feedback {
            let target_id = self.backend.acx.get_instance_id(target);
            let Some(cell) = self.backend_storage_path(&base, &fields) else {
                self.backend.acx.coverage_gaps.insert((target_id, None,
                    "stored callable requires a shared storage view".into()));
                continue;
            };
            if !matches!(cell.ty.kind(), ty::FnPtr(..) | ty::RawPtr(..))
                || !matches!(tcx.def_kind(target.def_id()), rustc_hir::def::DefKind::Fn | rustc_hir::def::DefKind::AssocFn)
            {
                self.backend.acx.coverage_gaps.insert((cell.owner.unwrap_or(target_id), None,
                    "stored callable requires flow refinement".into()));
                continue;
            }
            let source = BackendPath::new_function(target_id);
            self.backend.acx.set_path_rustc_type(source.clone(),
                Ty::new_fn_def(tcx, target.def_id(), ty::Binder::dummy(target.args)));
            storage_update_count += usize::from(self.backend.add_refined_address(&source, &cell.object_path));
            storage_updates.push(cell);
        }
        for (base, fields, source) in reference_feedback {
            let Some(cell) = self.backend_storage_path(&base, &fields) else { continue };
            let source = if source.byte_offset == 0 {
                Some(source)
            } else {
                source.pointee.and_then(|ty| self.memory_view(&source, ty))
            };
            let referent = source.and_then(|source| self.backend_storage_path(&source.base, &source.fields));
            let Some(referent) = referent.filter(|_| matches!(cell.ty.kind(), ty::Ref(..) | ty::RawPtr(..))) else {
                if let Some(owner) = cell.owner {
                    self.backend.acx.coverage_gaps.insert((owner, None,
                        "stored reference requires flow refinement".into()));
                }
                continue;
            };
            reference_update_count += usize::from(self.backend.add_refined_address(
                &referent.object_path, &cell.object_path));
            storage_updates.extend([cell, referent]);
        }
        let mut view_updates = Vec::new();
        for cell in storage_updates {
            let Some(path) = cell.mir_path else { continue };
            if self.backend.refined_storage_views.entry(cell.owner).or_default()
                .insert((path.clone(), cell.object_path.clone()))
            {
                if let Some(instance) = cell.instance {
                    if discovered.insert(cell.owner.unwrap()) && roots.insert(instance) {
                        self.backend.acx.discovery_roots.push(instance);
                    }
                }
                view_updates.push((cell.owner, path, cell.object_path));
            }
        }
        for (caller, location, target) in feedback {
            let caller_id = self.backend.acx.get_instance_id(caller);
            let site = BaseCallSite::new(caller_id, location);
            let target_id = self.backend.acx.get_instance_id(target);
            if !self.backend.call_sites.contains_key(&site) && roots.insert(caller) {
                self.backend.acx.discovery_roots.push(caller);
            }
            updates.push((site, target_id, target));
        }
        if !updates.is_empty() || !view_updates.is_empty() {
            self.backend.initialize();
        }
        for (owner, path, object) in view_updates {
            let contexts: Vec<_> = if let Some(owner) = owner {
                self.backend.call_graph.func_nodes.keys()
                    .filter(|function| function.func_id == owner)
                    .map(|function| function.cid).collect()
            } else {
                vec![self.backend.get_empty_context_id()]
            };
            for context in contexts {
                self.backend.add_refined_storage_view(&path, context, &object);
            }
        }
        let mut new_calls = Vec::new();
        for (site, target_id, target) in updates {
            let supported = matches!(self.backend.call_graph.get_callsite_type(&site),
                Some(CallType::FnPtr | CallType::DynamicDispatch));
            if !supported {
                self.backend.acx.coverage_gaps.insert((site.func, Some(site.location),
                    "refined target has no supported backend argument binding".into()));
                continue;
            }
            let expected = if !tcx.is_foreign_item(target.def_id())
                && !matches!(target.def, InstanceKind::Intrinsic(_) | InstanceKind::LlvmIntrinsic(_) | InstanceKind::Virtual(..))
                && (tcx.is_mir_available(target.def_id()) || !matches!(target.def, InstanceKind::Item(_)))
            {
                Some(tcx.instance_mir(target.def).arg_count)
            } else if matches!(tcx.def_kind(target.def_id()),
                rustc_hir::def::DefKind::Fn | rustc_hir::def::DefKind::AssocFn)
            {
                Some(tcx.fn_sig(target.def_id()).skip_binder().skip_binder().inputs().len())
            } else {
                None
            };
            let mut bound = false;
            if let Some(calls) = self.backend.call_sites.get(&site) {
                for call in calls {
                    if expected == Some(call.args.len()) {
                        new_calls.push((call.clone(), target_id));
                        bound = true;
                    } else {
                        self.backend.acx.coverage_gaps.insert((site.func, Some(site.location),
                            "refined target requires a different argument shape".into()));
                    }
                }
            }
            if bound {
                self.backend.refined_targets.entry(site).or_default().insert(target_id);
            }
        }
        self.backend.process_new_calls(&new_calls);
        self.backend.propagate();
        let mut changed = HashSet::new();
        for site in self.backend.call_graph.callsite_to_edges.keys() {
            let caller = self.backend.acx.get_function_reference(site.func.func_id);
            let caller = Instance {
                def: caller.kind,
                args: tcx.mk_args_from_iter(caller.generic_args.iter().map(|arg| match arg {
                    GenericArgE::Region => ty::GenericArg::from(tcx.lifetimes.re_erased),
                    GenericArgE::Const(value) => ty::GenericArg::from(*value),
                    GenericArgE::Type(value) => ty::GenericArg::from(*value),
                })),
            };
            let candidates = self.backend_calls.entry((caller, site.location)).or_default();
            for target in self.backend.call_graph.get_callees(site) {
                let target = self.backend.acx.get_function_reference(target.func_id);
                let target = Instance {
                    def: target.kind,
                    args: tcx.mk_args_from_iter(target.generic_args.iter().map(|arg| match arg {
                        GenericArgE::Region => ty::GenericArg::from(tcx.lifetimes.re_erased),
                        GenericArgE::Const(value) => ty::GenericArg::from(*value),
                        GenericArgE::Type(value) => ty::GenericArg::from(*value),
                    })),
                };
                if candidates.insert(target) {
                    changed.insert(caller);
                }
            }
        }
        let affected: Vec<_> = self.graph.keys().filter(|node| changed.contains(&node.instance)).copied().collect();
        for node in affected {
            self.schedule(node);
        }
        if let Some(directory) = std::env::var_os("IRQ_CHECK_DUMP_DIR") {
            let directory = std::path::PathBuf::from(directory);
            let mut gaps: Vec<_> = self.backend.acx.coverage_gaps.iter()
                .map(|(function, location, reason)| {
                    let function = self.backend.acx.get_function_reference(*function);
                    format!("{} {:?} {:?} at {location:?}: {reason}",
                        tcx.def_path_str(function.def_id), function.kind, function.generic_args)
                }).collect();
            gaps.sort_unstable();
            let result = std::fs::create_dir_all(&directory).and_then(|_| {
                std::fs::write(directory.join(format!("{}-backend-coverage.txt",
                    tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE))), gaps.join("\n"))
            });
            if let Err(error) = result {
                self.incomplete = true;
                self.limit_detail = format!("cannot write backend coverage evidence: {error}");
            }
        }
        if std::env::var_os("IRQ_CHECK_STATS").is_some() {
            eprintln!("irq-check: Rupta propagation seconds={:.3} contexts={} call_sites={} call_bindings={} coverage_gaps={} refined_targets={feedback_count} stored_callables={storage_update_count}/{storage_feedback_count} stored_references={reference_update_count}/{reference_feedback_count} changed_callers={}",
                started.elapsed().as_secs_f64(), self.backend.call_graph.func_nodes.len(),
                self.backend_calls.len(), self.backend.call_bindings.len(), self.backend.acx.coverage_gaps.len(), changed.len());
        }
    }

    fn schedule(&mut self, node: Node<'tcx>) {
        if self.queued.insert(node) {
            self.pending.push_back(node);
        }
    }

    fn context(
        &mut self,
        instance: Instance<'tcx>,
        arguments: Vec<Tree<'tcx>>,
    ) -> (Node<'tcx>, HashMap<Base<'tcx>, Base<'tcx>>) {
        let value_only = *self.value_only_bodies.entry(instance).or_insert_with(|| {
            if !matches!(instance.def, InstanceKind::Item(_))
                || !self.tcx.is_mir_available(instance.def_id())
                || self.tcx.is_foreign_item(instance.def_id())
            {
                return false;
            }
            let local_value = |place: Place<'tcx>| {
                place
                    .projection
                    .iter()
                    .all(|projection| matches!(projection, ProjectionElem::Field(..)))
            };
            self.tcx
                .instance_mir(instance.def)
                .basic_blocks
                .iter()
                .all(|block| {
                    matches!(
                        block.terminator().kind,
                        TerminatorKind::Return | TerminatorKind::Goto { .. }
                    ) && block
                        .statements
                        .iter()
                        .all(|statement| match &statement.kind {
                            StatementKind::Assign(assignment) => {
                                let (destination, value) = &**assignment;
                                let operand = match value {
                                    Rvalue::Use(operand, _)
                                    | Rvalue::Cast(
                                        CastKind::PtrToPtr | CastKind::Transmute,
                                        operand,
                                        _,
                                    ) => operand,
                                    _ => return false,
                                };
                                local_value(*destination)
                                    && match operand {
                                        Operand::Copy(place) | Operand::Move(place) => {
                                            local_value(*place)
                                        }
                                        _ => false,
                                    }
                            }
                            StatementKind::StorageLive(_)
                            | StatementKind::StorageDead(_)
                            | StatementKind::Nop => true,
                            _ => false,
                        })
                })
        });
        let mut ancestor = self.active;
        let mut recursive = None;
        let mut ancestors = Vec::new();
        while let Some(node) = ancestor {
            if node.instance == instance && node.context != 0 {
                ancestors.push(node);
            }
            ancestor = self.parents.get(&node).copied();
        }
        let mut memory = FlowState::new();
        if let Some(havoc) = self
            .flow_locals
            .as_ref()
            .and_then(|memory| memory.get(&Base::Havoc))
            .filter(|_| !value_only)
        {
            memory.insert(Base::Havoc, havoc.clone());
        }
        let mut pending = VecDeque::new();
        let mut visited = HashSet::new();
        for tree in &arguments {
            let mut paths: Vec<_> = tree.iter().collect();
            paths.sort_unstable_by_key(|(path, _)| *path);
            for (_, facts) in paths {
                let mut bases: Vec<_> = facts
                    .iter()
                    .filter_map(|fact| match fact {
                        Fact::Reference(address)
                        | Fact::ExposedPointer(PointerOrigin::Storage(address), _) => {
                            Some(address.base.clone())
                        }
                        _ => None,
                    })
                    .collect();
                bases.sort_unstable_by_key(|base| {
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    base.hash(&mut hasher);
                    hasher.finish()
                });
                pending.extend(bases.into_iter().filter(|base| visited.insert(base.clone())));
            }
        }
        let mut binding = HashMap::new();
        let mut prepared_reads = HashMap::new();
        while let Some(base) = pending.pop_front() {
            if !value_only
                && self.worker_limit > 1
                && pending.len() >= self.worker_limit * 4
                && !prepared_reads.contains_key(&base)
            {
                let mut requested = HashSet::new();
                let requests: Vec<_> = std::iter::once(&base)
                    .chain(pending.iter())
                    .filter(|base| {
                        !memory.contains_key(*base)
                            && !prepared_reads.contains_key(*base)
                            && requested.insert((*base).clone())
                    })
                    .cloned()
                    .collect();
                let havoc = self.stored_tree(&Base::Havoc);
                let view_guard = self.memory_views.borrow();
                let views = &*view_guard;
                let stored_memory = &self.memory;
                let flow = self.flow_locals.as_ref();
                let active = self.active;
                let escaped = &self.escaped;
                let borrowed = &self.address_taken;
                let tcx = self.tcx;
                let chunk_size = requests.len().div_ceil(self.worker_limit).max(1);
                let chunks: Vec<_> = requests.chunks(chunk_size).collect();
                let prepared: Vec<Vec<_>> = rustc_data_structures::sync::par_map(chunks, |chunk| {
                    chunk.iter().map(|base| {
                        let stored = views.get(base).cloned().unwrap_or_else(|| {
                            let mut tree = Tree::new();
                            for (path, fact) in stored_memory.get_pts(base) {
                                tree.entry(path.clone()).or_default().insert(fact.clone());
                            }
                            tree
                        });
                        let is_escaped = escaped.contains(base);
                        let exposed = match base {
                            Base::Havoc => false,
                            Base::Allocation(id, _) => !matches!(tcx.global_alloc(*id), GlobalAlloc::Memory(value) if value.inner().mutability == rustc_hir::Mutability::Not),
                            Base::Local(owner, _, local) => is_escaped || borrowed.get(owner).map_or_else(
                                || rustc_mir_dataflow::impls::borrowed_locals(tcx.instance_mir(owner.def)).contains(mir::Local::from_usize(*local)),
                                |locals| locals.contains(local)),
                            _ => true,
                        };
                        let local = flow.and_then(|memory| {
                            if memory.contains_key(base) || matches!(base, Base::Local(owner, context, _) if active.is_some_and(|node| node.instance == *owner && node.context == *context)) {
                                Some(memory.get(base).cloned())
                            } else { None }
                        });
                        let local_havoc = if local.is_some() { flow.and_then(|memory| memory.get(&Base::Havoc)).cloned() } else { None };
                        let values = storage_value(&stored, &havoc, &local, &local_havoc,
                            exposed && !(matches!(base, Base::Local(..)) && !is_escaped), is_escaped, matches!(base, Base::Unknown(_)));
                        (base.clone(), (values, stored, exposed))
                    }).collect()
                });
                drop(view_guard);
                prepared_reads.extend(prepared.into_iter().flatten());
            }
            let storage_ty = match &base {
                Base::Local(owner, _, local) => Some(self.ty(
                    *owner,
                    self.tcx.instance_mir(owner.def).local_decls[mir::Local::from_usize(*local)].ty,
                )),
                Base::Parameter(_, _, _, ty) => Some(*ty),
                _ => None,
            };
            if let Some(ty) = storage_ty {
                let slot = binding.len();
                binding.insert(base.clone(), Base::Parameter(instance, 0, slot, ty));
            }
            if value_only {
                continue;
            }
            let values = if let Some((values, stored, exposed)) = prepared_reads.remove(&base) {
                self.memory_views
                    .get_mut()
                    .entry(base.clone())
                    .or_insert(stored);
                if let Some(active) = self.active {
                    self.readers
                        .entry(base.clone())
                        .or_default()
                        .entry(Vec::new())
                        .or_default()
                        .insert(active);
                    if exposed {
                        self.readers
                            .entry(Base::Havoc)
                            .or_default()
                            .entry(Vec::new())
                            .or_default()
                            .insert(active);
                    }
                }
                values
            } else {
                self.read(&Address {
                    base: base.clone(),
                    fields: Vec::new(),
                    pointee: None,
                    byte_offset: 0,
                })
            };
            let mut paths: Vec<_> = values.iter().collect();
            paths.sort_unstable_by_key(|(path, _)| *path);
            for (_, facts) in paths {
                let mut bases: Vec<_> = facts
                    .iter()
                    .filter_map(|fact| match fact {
                        Fact::Reference(address)
                        | Fact::ExposedPointer(PointerOrigin::Storage(address), _) => {
                            Some(address.base.clone())
                        }
                        _ => None,
                    })
                    .collect();
                bases.sort_unstable_by_key(|base| {
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    base.hash(&mut hasher);
                    hasher.finish()
                });
                pending.extend(bases.into_iter().filter(|base| visited.insert(base.clone())));
            }
            memory.insert(base, values);
        }
        let arguments: Vec<_> = arguments
            .iter()
            .map(|tree| bind_tree(tree, &binding))
            .collect();
        let memory = bind_memory(&memory, &binding);
        let tcx = self.tcx;
        let entries: Vec<_> = memory.iter().collect();
        let chunk_size = entries.len().div_ceil(self.worker_limit).max(1);
        let prepare = |chunk: &[(&Base<'tcx>, &Tree<'tcx>)]| -> FlowState<'tcx> {
            chunk
                .iter()
                .filter_map(|(base, tree)| {
                    let tree: Tree<'tcx> = tree
                        .iter()
                        .filter_map(|(path, facts)| {
                            let facts: HashSet<_> = facts
                                .iter()
                                .filter(|fact| {
                                    !matches!(fact, Fact::Scalar(value) if *value > 1)
                                        && !(Self::numeric_cell(tcx, base, path)
                                            && matches!(fact, Fact::Scalar(_) | Fact::Unknown))
                                })
                                .cloned()
                                .collect();
                            (!facts.is_empty()).then(|| (path.clone(), facts))
                        })
                        .collect();
                    (!tree.is_empty()).then(|| ((*base).clone(), tree))
                })
                .collect()
        };
        let memory_key: FlowState<'tcx> = if entries.len() >= self.worker_limit * 4 {
            let chunks: Vec<_> = entries.chunks(chunk_size).collect();
            let prepared: Vec<_> = rustc_data_structures::sync::par_map(chunks, prepare);
            prepared.into_iter().flatten().collect()
        } else {
            prepare(&entries)
        };
        let mut input = ContextInput {
            key: ContextKey(arguments.clone(), memory_key),
            arguments,
            memory,
            singletons: binding.iter().filter_map(|(source, parameter)| {
                (recursive.is_none()
                    && !self.escaped.contains(source)
                    && (self.singleton_storage.contains(source)
                        || matches!(source, Base::Local(owner, context, _) if self.active.is_some_and(|active| active.instance == *owner && active.context == *context))))
                    .then_some(parameter.clone())
            }).collect(),
        };
        let scalar_shape = |arguments: &[Tree<'tcx>]| -> Vec<Tree<'tcx>> {
            arguments
                .iter()
                .map(|tree| {
                    tree.iter()
                        .map(|(path, facts)| {
                            (
                                path.clone(),
                                facts
                                    .iter()
                                    .map(|fact| {
                                        if matches!(fact, Fact::Scalar(_)) {
                                            Fact::Unknown
                                        } else {
                                            fact.clone()
                                        }
                                    })
                                    .collect(),
                            )
                        })
                        .collect()
                })
                .collect()
        };
        let input_shape = scalar_shape(&input.key.0);
        for ancestor in ancestors {
            let stored = &self.contexts[&instance][ancestor.context - 1];
            if stored.key.1 == input.key.1 && scalar_shape(&stored.key.0) == input_shape {
                recursive = Some(ancestor);
                input.singletons.clear();
                break;
            }
        }
        let contexts = self.contexts.entry(instance).or_default();
        let context = recursive.map(|node| node.context).unwrap_or_else(|| {
            self.context_keys
                .entry(instance)
                .or_default()
                .get_context_id(std::borrow::Cow::Borrowed(&input.key))
                + 1
        });
        let stamp: HashMap<_, _> = binding
            .values()
            .map(|base| {
                let Base::Parameter(owner, _, slot, ty) = base else {
                    unreachable!()
                };
                (base.clone(), Base::Parameter(*owner, context, *slot, *ty))
            })
            .collect();
        input.arguments = input
            .arguments
            .iter()
            .map(|tree| bind_tree(tree, &stamp))
            .collect();
        input.memory = bind_memory(&input.memory, &stamp);
        input.singletons = input
            .singletons
            .iter()
            .map(|base| stamp[base].clone())
            .collect();
        for base in binding.values_mut() {
            *base = stamp[base].clone();
        }
        let mut changed = false;
        if context <= contexts.len() {
            let stored = &mut contexts[context - 1];
            let previous_singletons = stored.singletons.len();
            stored
                .singletons
                .retain(|base| input.singletons.contains(base));
            changed |= stored.singletons.len() != previous_singletons;
            stored
                .arguments
                .resize_with(input.arguments.len().max(stored.arguments.len()), Tree::new);
            for (output, tree) in stored.arguments.iter_mut().zip(input.arguments) {
                changed |= output.join(&tree);
            }
            for (base, tree) in input.memory {
                let output = stored.memory.entry(base).or_default();
                changed |= output.join(&tree);
            }
        } else {
            contexts.push(input);
            if let Some(parent) = self.active {
                self.parents.insert(
                    Node {
                        instance,
                        context: contexts.len(),
                    },
                    parent,
                );
            }
        }
        let node = Node { instance, context };
        for (source, parameter) in &binding {
            self.escape_aliases
                .entry(parameter.clone())
                .or_default()
                .insert(source.clone());
            self.escape_aliases
                .entry(source.clone())
                .or_default()
                .insert(parameter.clone());
            if self.escaped.contains(parameter) {
                self.mark_escaped(source.clone());
            }
            if self.escaped.contains(source) {
                self.mark_escaped(parameter.clone());
            }
        }
        if changed {
            self.precision_loss.insert(format!(
                "joined scalar storage versions in {instance}, context {context}"
            ));
        }
        if changed || !self.graph.contains_key(&node) {
            self.schedule(node);
        }
        if self.active.is_some() {
            self.pending_bindings.push((node, binding.clone()));
        }
        (node, binding)
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
            ty::Ref(_, ty, _)
            | ty::RawPtr(ty, _)
            | ty::Array(ty, _)
            | ty::Slice(ty)
            | ty::Pat(ty, _) => self.tracks(*ty, visiting),
            ty::Tuple(fields) => fields.iter().any(|ty| self.tracks(ty, visiting)),
            ty::Adt(adt, args) => adt.all_fields().any(|field| {
                self.tracks(
                    self.tcx.normalize_erasing_regions(
                        TypingEnv::fully_monomorphized(),
                        field.ty(self.tcx, args),
                    ),
                    visiting,
                )
            }),
            _ => false,
        };
        visiting.remove(&ty);
        self.tracked.insert(ty, tracked);
        tracked
    }

    fn can_replace(&self, address: &Address<'tcx>, ty: Ty<'tcx>) -> bool {
        (self.singleton_storage.contains(&address.base)
            || matches!(address.base, Base::Local(owner, context, _) if self.active.is_some_and(|active| active.instance == owner && active.context == context)))
            && !self.escaped.contains(&address.base)
            && address.byte_offset == 0
            && !address.fields.contains(&u32::MAX)
            && address.pointee == Some(ty)
            && !self
                .flow_locals
                .as_ref()
                .is_some_and(|memory| memory.contains_key(&Base::Havoc))
            && self.memory.get_pts(&Base::Havoc).next().is_none()
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
                        &Tree::from([(Vec::new(), HashSet::from([Fact::Unknown]))]),
                    );
                }
            }
        }
        let previous_strong_update = self.strong_update;
        let previous_effects = self.record_effects;
        self.record_effects |= place
            .projection
            .iter()
            .any(|projection| matches!(projection, ProjectionElem::Deref));
        let addresses = self.places(instance, body, place);
        let direct = place.projection.iter().all(|projection| {
            !matches!(
                projection,
                ProjectionElem::Deref | ProjectionElem::Index(_) | ProjectionElem::Subslice { .. }
            )
        });
        let unique = addresses.len() == 1
            && addresses.first().is_some_and(|address| {
                self.can_replace(address, ty)
                    && !place
                        .projection
                        .iter()
                        .any(|projection| matches!(projection, ProjectionElem::Subslice { .. }))
            });
        self.strong_update = direct || unique;
        for address in addresses {
            self.write(&address, values);
        }
        self.strong_update = previous_strong_update;
        self.record_effects = previous_effects;
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
            byte_offset: 0,
            pointee: None,
        }];
        for (index, projection) in place.projection.iter().enumerate() {
            match projection {
                ProjectionElem::PhantomDeref => {
                    self.incomplete = true;
                    self.limit_detail = "unsupported phantom dereference".into();
                    addresses = vec![Address {
                        base: Base::Unknown(self.ty(instance, place.ty(body, self.tcx).ty)),
                        fields: Vec::new(),
                        byte_offset: 0,
                        pointee: None,
                    }];
                }
                ProjectionElem::Deref => {
                    let pointer_ty = self.ty(
                        instance,
                        Place {
                            local: place.local,
                            projection: self.tcx.mk_place_elems(&place.projection[..index]),
                        }
                        .ty(body, self.tcx)
                        .ty,
                    );
                    let pointee = pointer_ty
                        .builtin_deref(true)
                        .unwrap_or(self.tcx.types.unit);
                    addresses = addresses
                        .iter()
                        .flat_map(|address| {
                            self.read(address).remove(&Vec::new()).unwrap_or_default()
                        })
                        .filter_map(|fact| {
                            if let Fact::Reference(address) = fact {
                                Some(address)
                            } else if matches!(
                                fact,
                                Fact::Unknown | Fact::Scalar(_) | Fact::IntegerPointer(_)
                            ) {
                                Some(Address {
                                    base: Base::Unknown(pointee),
                                    fields: Vec::new(),
                                    pointee: Some(pointee),
                                    byte_offset: 0,
                                })
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
                    let indices: HashSet<_> = match projection {
                        ProjectionElem::Index(local) => {
                            let values = self.place_value(instance, body, Place::from(local));
                            let mut indices: HashSet<_> = values
                                .values()
                                .flatten()
                                .map(|fact| match fact {
                                    Fact::Scalar(value) => {
                                        u32::try_from(*value).unwrap_or(u32::MAX)
                                    }
                                    _ => u32::MAX,
                                })
                                .collect();
                            if indices.is_empty() {
                                indices.insert(u32::MAX);
                            }
                            indices
                        }
                        ProjectionElem::ConstantIndex {
                            offset,
                            from_end: false,
                            ..
                        } => HashSet::from([u32::try_from(offset).unwrap_or(u32::MAX)]),
                        ProjectionElem::ConstantIndex {
                            offset,
                            from_end: true,
                            ..
                        } => {
                            let source = self.ty(
                                instance,
                                Place {
                                    local: place.local,
                                    projection: self.tcx.mk_place_elems(&place.projection[..index]),
                                }
                                .ty(body, self.tcx)
                                .ty,
                            );
                            let element = if let ty::Array(_, count) = source.kind() {
                                count
                                    .try_to_target_usize(self.tcx)
                                    .and_then(|count| count.checked_sub(offset))
                                    .and_then(|index| u32::try_from(index).ok())
                                    .unwrap_or(u32::MAX)
                            } else {
                                u32::MAX
                            };
                            HashSet::from([element])
                        }
                        _ => unreachable!(),
                    };
                    let field_ty = self.ty(
                        instance,
                        Place {
                            local: place.local,
                            projection: self.tcx.mk_place_elems(&place.projection[..=index]),
                        }
                        .ty(body, self.tcx)
                        .ty,
                    );
                    addresses = addresses
                        .into_iter()
                        .flat_map(|address| {
                            indices.iter().map(move |index| {
                                let mut address = address.clone();
                                address.fields.push(*index);
                                address.pointee = Some(field_ty);
                                address
                            })
                        })
                        .collect();
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
                ProjectionElem::Downcast(_, variant) => {
                    for address in &mut addresses {
                        address.fields.extend([VARIANT, variant.as_u32()]);
                    }
                }
                ProjectionElem::OpaqueCast(..) | ProjectionElem::UnwrapUnsafeBinder(..) => {}
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
        if self.ty(instance, place.ty(body, self.tcx).ty).is_bool() {
            if let Some(facts) = result.get_mut(&Vec::new()) {
                if facts.remove(&Fact::Unknown) {
                    facts.extend([Fact::Scalar(0), Fact::Scalar(1)]);
                }
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
            let args = self.tcx.instantiate_bound_regions_with_erased(args);
            return Tree::from([(
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
                    EarlyBinder::bind(self.tcx, constant.const_),
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
                        Ok(ConstValue::Slice { alloc_id, meta }) => {
                            let mut value = self.pointer(alloc_id, 0, ty);
                            value
                                .entry(Vec::new())
                                .or_default()
                                .insert(Fact::Length(meta as u128));
                            value
                        }
                        Ok(ConstValue::Scalar(Scalar::Int(value))) => Tree::from([(
                            Vec::new(),
                            HashSet::from([Fact::Scalar(value.to_bits(value.size()))]),
                        )]),
                        Ok(_) => Tree::new(),
                        Err(_) => Tree::from([(Vec::new(), HashSet::from([Fact::Unknown]))]),
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
                        let ty = self.tcx.normalize_erasing_regions(
                            TypingEnv::fully_monomorphized(),
                            self.tcx.type_of(def).instantiate_identity(),
                        );
                        let allocation = self.tcx.reserve_and_set_memory_alloc(value);
                        let values = self.allocation(allocation, 0, ty);
                        let previous = self.initializing_allocation;
                        self.initializing_allocation = true;
                        self.write(
                            &Address {
                                base: Base::Static(def),
                                fields: Vec::new(),
                                byte_offset: 0,
                                pointee: Some(ty),
                            },
                            &values,
                        );
                        self.initializing_allocation = previous;
                    }
                }
                if offset != 0 {
                    Fact::Unknown
                } else {
                    Fact::Reference(Address {
                        base: Base::Static(def),
                        fields: Vec::new(),
                        byte_offset: 0,
                        pointee: ty.builtin_deref(true),
                    })
                }
            }
            GlobalAlloc::Memory(_) => {
                let address = Address {
                    base: Base::Allocation(allocation, offset),
                    fields: Vec::new(),
                    byte_offset: 0,
                    pointee: ty.builtin_deref(true),
                };
                if let Some(pointee) = ty.builtin_deref(true) {
                    if self.loaded.insert((allocation, offset, pointee)) {
                        let values = self.allocation(allocation, offset, pointee);
                        let previous = self.initializing_allocation;
                        self.initializing_allocation = true;
                        self.write(&address, &values);
                        self.initializing_allocation = previous;
                    }
                }
                Fact::Reference(address)
            }
            _ => Fact::Unknown,
        };
        Tree::from([(Vec::new(), HashSet::from([fact]))])
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
            return Tree::from([(Vec::new(), HashSet::from([Fact::Unknown]))]);
        };
        if offset
            .checked_add(layout.size.bytes())
            .is_none_or(|end| end > memory.len() as u64)
        {
            return Tree::from([(Vec::new(), HashSet::from([Fact::Unknown]))]);
        }
        match ty.kind() {
            ty::FnPtr(..) | ty::Ref(..) | ty::RawPtr(..) => {
                let range = AllocRange {
                    start: Size::from_bytes(offset),
                    size: self.tcx.data_layout.pointer_size(),
                };
                if range.end().bytes() <= memory.len() as u64 {
                    result = match memory.read_scalar(&self.tcx, range, true) {
                        Ok(Scalar::Ptr(pointer, _)) => self.pointer(
                            pointer.provenance.alloc_id(),
                            pointer.prov_and_relative_offset().1.bytes(),
                            ty,
                        ),
                        Ok(Scalar::Int(value)) => Tree::from([(
                            Vec::new(),
                            HashSet::from([Fact::IntegerPointer(value.to_bits(value.size()))]),
                        )]),
                        Err(_) => Tree::from([(Vec::new(), HashSet::from([Fact::Unknown]))]),
                    };
                    if layout.size > range.size {
                        let range = AllocRange {
                            start: range.end(),
                            size: range.size,
                        };
                        if range.end().bytes() <= memory.len() as u64 {
                            match memory.read_scalar(&self.tcx, range, true) {
                                Ok(Scalar::Ptr(pointer, _)) => {
                                    for (path, facts) in self.pointer(
                                        pointer.provenance.alloc_id(),
                                        pointer.prov_and_relative_offset().1.bytes(),
                                        ty,
                                    ) {
                                        result.entry(path).or_default().extend(facts);
                                    }
                                }
                                Ok(Scalar::Int(value))
                                    if ty.builtin_deref(true).is_some_and(|ty| {
                                        matches!(ty.kind(), ty::Slice(_) | ty::Str)
                                    }) =>
                                {
                                    result
                                        .entry(Vec::new())
                                        .or_default()
                                        .insert(Fact::Length(value.to_bits(value.size())));
                                }
                                _ => {
                                    result.entry(Vec::new()).or_default().insert(Fact::Unknown);
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
                            let mut output = vec![index as u32];
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
                        let ty = self.tcx.normalize_erasing_regions(
                            TypingEnv::fully_monomorphized(),
                            field.ty(self.tcx, args),
                        );
                        for (path, facts) in self.allocation(
                            id,
                            offset + variant_layout.fields.offset(index.as_usize()).bytes(),
                            ty,
                        ) {
                            let mut output = if adt.is_enum() {
                                vec![VARIANT, variant_index.as_u32(), index.as_u32()]
                            } else {
                                vec![index.as_u32()]
                            };
                            output.extend(path);
                            result.entry(output).or_default().extend(facts);
                        }
                    }
                }
                if adt.is_enum() {
                    result
                        .entry(vec![DISCRIMINANT])
                        .or_default()
                        .insert(Fact::Unknown);
                }
            }
            _ if ty.is_integral() || ty.is_bool() || ty.is_char() || ty.is_floating_point() => {
                let range = AllocRange {
                    start: Size::from_bytes(offset),
                    size: layout.size,
                };
                let fact = if layout.size.bytes() > 0
                    && layout.size.bytes() <= 16
                    && range.end().bytes() <= memory.len() as u64
                {
                    match memory.read_scalar(&self.tcx, range, false) {
                        Ok(Scalar::Int(value)) => Fact::Scalar(value.to_bits(value.size())),
                        _ => Fact::Unknown,
                    }
                } else {
                    Fact::Unknown
                };
                result.entry(Vec::new()).or_default().insert(fact);
            }
            _ => {}
        }
        result
    }
}
