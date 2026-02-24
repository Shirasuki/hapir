use hapir_shared::modes::PermissionMode;
use sha2::{Digest, Sha256};

pub mod run;

/// The mode type for OpenCode sessions.
#[derive(Debug, Clone, Default)]
pub struct OpencodeMode {
    pub permission_mode: Option<PermissionMode>,
}

/// Compute a deterministic hash of the opencode mode for queue batching.
pub(crate) fn compute_mode_hash(mode: &OpencodeMode) -> String {
    let mut hasher = Sha256::new();
    hasher.update(mode.permission_mode.map_or("", |p| p.as_str()));
    hex::encode(hasher.finalize())
}
