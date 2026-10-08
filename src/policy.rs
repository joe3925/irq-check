use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;

pub const INTERFACE: &str = "strict-blocks-2";

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct ScopeMarks {
    pub trusted: Vec<usize>,
    pub unreachable: Vec<usize>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Policy {
    pub root: String,
    pub packages: Vec<Package>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Package {
    pub id: String,
    pub manifest: PathBuf,
}

#[derive(Default, Deserialize, Serialize)]
pub struct CrateData {
    pub interface: String,
    pub compiler: String,
    pub policy: String,
    pub crate_id: u64,
    pub checked: bool,
    pub metadata: PathBuf,
    pub metadata_hash: u64,
    pub scopes: HashMap<String, ScopeMarks>,
    pub contexts: Vec<u32>,
}

pub fn hash(value: impl Hash) -> u64 {
    let mut state = DefaultHasher::new();
    value.hash(&mut state);
    state.finish()
}
