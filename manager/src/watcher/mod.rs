pub mod crash_detect;
pub mod detect;
pub mod process;
pub mod reconnect;
pub mod winscan;

use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum State {
    Starting,
    Live,
    Reconnecting,
    #[default]
    Dead,
}

#[derive(Default)]
pub struct Tracked {
    /// Launch generation. Every do_launch takes a fresh, globally unique value; a launch task that
    /// finds a different value (or no entry) after an await knows it was cancelled/superseded.
    pub gen: u64,
    pub pid: Option<u32>,
    pub start_time: u64,
    pub launch_ts: u64,
    pub state: State,
    pub attempts: u32,
    pub live_since: Option<Instant>,
    /// Wall-clock unix seconds when the client went live (for display).
    pub live_since_ts: i64,
    pub user_killed: bool,
    pub log: Option<crash_detect::LogTail>,
    pub job_captured: bool,
    /// Last measured working set, bytes (watcher tick / memory saver).
    pub mem_ws: u64,
    pub mem_note: crate::runtime::trim::MemNote,
    /// The account's preset CPU priority tier, restored when its window comes to the foreground.
    pub base_priority: i32,
}
