use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use irq_check_rupta::pts_set::points_to::{HybridPointsToSet, PointsToSet};

#[derive(Clone, Copy, Debug, Default)]
pub struct WorkerAllocator;

unsafe impl allocator_api2::alloc::Allocator for WorkerAllocator {
    fn allocate(
        &self,
        layout: std::alloc::Layout,
    ) -> Result<std::ptr::NonNull<[u8]>, allocator_api2::alloc::AllocError> {
        use std::alloc::GlobalAlloc;
        let allocation = std::alloc::Layout::from_size_align(layout.size().max(1), layout.align())
            .map_err(|_| allocator_api2::alloc::AllocError)?;
        let pointer = std::ptr::NonNull::new(unsafe { mimalloc::MiMalloc.alloc(allocation) })
            .ok_or(allocator_api2::alloc::AllocError)?;
        Ok(std::ptr::NonNull::slice_from_raw_parts(
            pointer,
            layout.size(),
        ))
    }

    unsafe fn deallocate(&self, pointer: std::ptr::NonNull<u8>, layout: std::alloc::Layout) {
        use std::alloc::GlobalAlloc;
        let allocation =
            std::alloc::Layout::from_size_align(layout.size().max(1), layout.align()).unwrap();
        unsafe { mimalloc::MiMalloc.dealloc(pointer.as_ptr(), allocation) };
    }
}

pub type WorkVec<T> = allocator_api2::vec::Vec<T, WorkerAllocator>;
pub type WorkSet<T> =
    hashbrown::HashSet<T, std::collections::hash_map::RandomState, WorkerAllocator>;

pub struct ShardedMap<K, V> {
    pub shards: Vec<HashMap<K, V>>,
}

impl<K: Hash + Eq, V> ShardedMap<K, V> {
    pub fn shard(&self, key: &K) -> usize {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        hasher.finish() as usize % self.shards.len()
    }

    pub fn entry(&mut self, key: K) -> std::collections::hash_map::Entry<'_, K, V> {
        let shard = self.shard(&key);
        self.shards[shard].entry(key)
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.shards[self.shard(key)].get(key)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.shards.iter().flat_map(|shard| shard.iter())
    }
}

pub struct ContextCache<K> {
    pub context_list: Vec<Rc<K>>,
    context_hashes: Vec<u64>,
    context_to_index_map: hashbrown::HashTable<usize>,
}

impl<K> Default for ContextCache<K> {
    fn default() -> Self {
        Self {
            context_list: Vec::new(),
            context_hashes: Vec::new(),
            context_to_index_map: hashbrown::HashTable::new(),
        }
    }
}

impl<K: Eq + Hash + Clone> ContextCache<K> {
    pub fn get_context_id(&mut self, context: std::borrow::Cow<'_, K>) -> usize {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        context.hash(&mut hasher);
        let hash = hasher.finish();
        if let Some(id) = self
            .context_to_index_map
            .find(hash, |id| self.context_list[*id].as_ref() == context.as_ref())
        {
            *id
        } else {
            let id = self.context_list.len();
            self.context_list.push(Rc::new(context.into_owned()));
            self.context_hashes.push(hash);
            self.context_to_index_map
                .insert_unique(hash, id, |id| self.context_hashes[*id]);
            id
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct StorageId(pub usize);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FactId(pub usize);

impl irq_check_rupta::util::bit_vec::Idx for FactId {
    fn new(index: usize) -> Self {
        Self(index)
    }

    fn index(self) -> usize {
        self.0
    }
}

#[derive(Clone)]
pub struct DiffPTData<K, D> {
    pub objects: HashMap<K, StorageId>,
    pub values: Vec<D>,
    value_ids: HashMap<D, FactId>,
    pub data: irq_check_rupta::pts_set::pt_data::DiffPTData<StorageId, FactId, HybridPointsToSet<FactId>>,
}

impl<K, D> Default for DiffPTData<K, D> {
    fn default() -> Self {
        Self {
            objects: HashMap::new(),
            values: Vec::new(),
            value_ids: HashMap::new(),
            data: irq_check_rupta::pts_set::pt_data::DiffPTData::new(),
        }
    }
}

impl<K: Hash + Eq + Clone, D: Hash + Eq + Clone> DiffPTData<K, D> {
    pub fn intern_object(&mut self, var: K) -> StorageId {
        let next = StorageId(self.objects.len());
        *self.objects.entry(var).or_insert(next)
    }

    pub fn union_pts_to(&mut self, var: K, src: &HashSet<D>) -> bool {
        let object = self.intern_object(var);
        let mut changed = false;
        for value in src {
            let next = FactId(self.values.len());
            let id = *self.value_ids.entry(value.clone()).or_insert_with(|| {
                self.values.push(value.clone());
                next
            });
            changed |= self.data.add_pts(object, id);
        }
        changed
    }

    pub fn get_pts(&self, var: &K) -> impl Iterator<Item = &D> {
        self.objects.get(var).into_iter().flat_map(|object| {
            self.data.get_propa_pts(*object).into_iter().flat_map(PointsToSet::iter)
                .chain(self.data.get_diff_pts(*object).into_iter().flat_map(PointsToSet::iter))
                .map(|id| &self.values[id.0])
        })
    }

    pub fn flush(&mut self, var: K) -> HashSet<D> {
        let Some(object) = self.objects.get(&var).copied() else {
            return HashSet::new();
        };
        let diff = self.data.get_diff_pts(object).into_iter()
            .flat_map(PointsToSet::iter)
            .map(|id| self.values[id.0].clone()).collect();
        self.data.flush(object);
        diff
    }
}
