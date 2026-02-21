use hapir_shared::schemas::StartedBy;

use super::session_base::SessionMode;

/// The exit reason from a local launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalLaunchExitReason {
    Switch,
    Exit,
}

/// Context for determining the local launch exit reason.
pub struct LocalLaunchContext {
    pub started_by: Option<StartedBy>,
    pub starting_mode: Option<SessionMode>,
}

/// Determine whether to switch to remote or exit after a local launch completes.
///
/// If started by the runner or starting in remote mode, we switch.
/// If started from terminal, also switch so that pending web messages
/// can be processed by the remote launcher.
pub fn get_local_launch_exit_reason(context: &LocalLaunchContext) -> LocalLaunchExitReason {
    if context.started_by == Some(StartedBy::Runner)
        || context.starting_mode == Some(SessionMode::Remote)
    {
        return LocalLaunchExitReason::Switch;
    }

    // Terminal-started sessions: switch to remote so the process stays alive
    // and can serve web UI messages after the local claude session ends.
    LocalLaunchExitReason::Switch
}
