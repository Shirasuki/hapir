use hapir_shared::modes::PermissionMode;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub mod run;
mod session_scanner;

#[derive(Debug, Clone, Default, Hash)]
pub struct CodexMode {
    pub permission_mode: Option<PermissionMode>,
    pub model: Option<String>,
    pub collaboration_mode: Option<String>,
}

fn compute_mode_hash(mode: &CodexMode) -> String {
    let mut hasher = DefaultHasher::new();
    mode.hash(&mut hasher);
    hasher.finish().to_string()
}
