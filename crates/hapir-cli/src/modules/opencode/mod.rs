use hapir_shared::modes::PermissionMode;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub mod run;

#[derive(Debug, Clone, Default, Hash)]
pub struct OpencodeMode {
    pub permission_mode: Option<PermissionMode>,
}

pub(crate) fn compute_mode_hash(mode: &OpencodeMode) -> String {
    let mut hasher = DefaultHasher::new();
    mode.hash(&mut hasher);
    hasher.finish().to_string()
}
