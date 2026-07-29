//! Stands in for herdr's `crate::platform`.
//!
//! herdr inspects the foreground process group to work out which agent is
//! running inside a pane, because a pane starts as a shell and the agent turns
//! up later as a child. gaze never has that problem: `gaze claude …` spawns the
//! agent itself and knows what it launched. So the process-scanning entry
//! points exist only to satisfy the vendored code that references them, and
//! report honestly that they found nothing rather than reimplementing herdr's
//! per-platform process tables.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForegroundProcess {
    pub pid: u32,
    pub name: String,
    pub argv0: Option<String>,
    pub argv: Option<Vec<String>>,
    pub cmdline: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForegroundJob {
    pub process_group_id: u32,
    pub processes: Vec<ForegroundProcess>,
}

/// Always `None` in gaze — see the module comment.
pub fn foreground_job(_child_pid: u32) -> Option<ForegroundJob> {
    None
}

/// Always `None` in gaze — see the module comment.
pub fn foreground_group_leader_job(_process_group_id: u32) -> Option<ForegroundJob> {
    None
}

/// Always `None` in gaze — see the module comment.
pub fn foreground_process_group_id(_child_pid: u32) -> Option<u32> {
    None
}
