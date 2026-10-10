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
    /// The log has shown this client inside a game at least once.
    pub joined: bool,
    /// A disconnect seen in the log, acted on after a short grace period unless the client rejoins
    /// (a teleport, or Roblox's own Reconnect button) in the meantime.
    pub pending_fail: Option<(Instant, String)>,
    /// Main window has been "not responding" since.
    pub hung_since: Option<Instant>,
    /// Launched with no game (Roblox home screen): being on the home screen is expected, not a failure.
    pub home_launch: bool,
    /// The last place this window was seen joining (its log), so a reconnect can go back there even
    /// when the account has no game set.
    pub last_place: Option<String>,
    /// Game the window is in (universe id from its log), for "Playing …".
    pub universe: Option<u64>,
    /// Account name for the window title ("Roblox — Main").
    pub title: String,
    /// Already told the user which profile settings Roblox refused for this window.
    pub denied_reported: bool,
    /// Last measured working set, bytes (watcher tick / memory saver).
    pub mem_ws: u64,
    pub mem_note: crate::runtime::trim::MemNote,
    /// The account's preset CPU priority tier, restored when its window comes to the foreground.
    pub base_priority: i32,
}
