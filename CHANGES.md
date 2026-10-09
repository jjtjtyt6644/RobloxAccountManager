# 1.3.0 — finds every Roblox window, clearer settings

## New
- Finds Roblox windows the manager didn't open: ones started from the website, by another tool, or before the manager was opened. Each is matched to its account through Roblox's own log (the user id it writes when it joins a game) and is then watched, reconnected and saved like any other. Windows of accounts that aren't in the manager are still covered by the memory and CPU savers.
- Savers can now apply to every Roblox window, including the one you're playing (Settings > Performance > Which windows the savers apply to). The default is still background windows only.

## Settings, reorganised
- Tabs: Games, Performance, Updates, This app, Security.
- "Presets" are now Performance profiles: each is a card showing what it does and which accounts use it. Edit opens plain controls (frame rate with quick picks, texture quality Full to Lowest, shadows, visual effects, priority) and lets you switch accounts onto it with one click. The exact texture level and frame buffer cap are under Advanced. "+ New profile" and Duplicate are there too.
- Slider tracks and checkboxes are easier to see.

# 1.2.0 — keeps your games running, one-click updates

## New
- Running games are picked up automatically. When the manager starts, including right after an update, every Roblox window it launched that is still open is tracked again (watched, reconnected, saved). There's no need to close and relaunch Roblox.
- Updater window: when a new version is out, a window pops up as soon as you open the manager, with what's new and one "Update now" button. It shows download progress, then the manager closes and reopens on the new version by itself. "Later" hides it until next time; the green Update button in the top bar brings it back.
- With "Install automatically" on, the update downloads and the manager reopens without asking.

## Fixed
- Arrows in help text showed as boxes.

# 1.1.0 — CPU saver, lighter manager

## New
- CPU saver (Settings): Roblox windows you aren't playing in run in Windows Efficiency mode at lower priority. "Strong" also uses the lowest priority and limits each background window to 2 CPU cores. The window you click into gets full speed back within a second.
- Memory and CPU savers now react to window switches within a second (memory trimming still runs on its own 15 / 5 / 3 s schedule).

## The manager itself
- "Keep the manager light in the background" (on by default): Efficiency mode while it isn't the active window; while minimized it doesn't draw at all (0% GPU) and its memory drops from about 80 MB to about 3 MB.
- Process scans read only what they need (names and start times, plus memory for running clients) instead of CPU, disk and file info for every process on the PC.
- Smaller database cache.

# 1.0.0 — first stable release

## New
- About screen: version, links, update status and this changelog.
- Updates: Settings > Updates checks this project's GitHub releases for a newer manager. One click downloads and installs it; restart to finish. Optional: check on start, and install automatically.
- Roblox updater: Settings > Updates shows your installed Roblox version and the latest one, and "Update Roblox" runs Roblox's official installer for the latest version.
- An "Update" button appears in the top bar when a new manager version is out.
- One download: the sign-in helper (ram-login.exe) is now built into the manager, so roblox_account_manager.exe is the only file you need.

## Fixed
- The account page no longer runs off the right edge of the window (its width was forced to 780 px even in smaller windows).
- Refined look: cleaner tab switcher, card-style account rows with left-aligned names, square checkboxes. Double-click an account to play.
- Small windows: Play / Stop buttons on account rows and on the account page no longer get cut off. Rows shrink to icon buttons, the account page stacks its Play button under the name, and the top bar drops the title and counts before the buttons.
- The window can now be made smaller (720 × 480).

# 0.3.0 — memory saver, multi-client fix, clearer settings

## Several accounts at once (fix)
The old lock created an *event* named `ROBLOX_singletonEvent` — the same object Roblox uses — so it never
stopped the one-window check. Worse, it was manual-reset and kept alive, so once signalled every new client
closed the previous one. Now the manager holds a *mutex* with that name (plus `ROBLOX_singletonMutex`), so
Roblox's CreateEvent fails and it skips the check. The handles live on a dedicated thread.
If a Roblox window was already open when the lock was taken, launches are held back with an explanation
instead of closing that window, and Settings shows "Needs attention" with a "Close all Roblox windows and fix" button.

## Memory saver (replaces "trim every N seconds")
Off / Balanced (15 s) / Strong (5 s) / Max (hard working-set ceiling), with a per-window target (default 100 MB).
The foreground window is never trimmed or limited. Background windows also get low memory priority.
Each running account shows its memory on its row and in the Session card; the status bar shows the total.
Old `trim_enabled: true` settings migrate to Balanced.

## UI
Plain-language settings with status pills, choice tiles for the memory level, segmented top tabs, refined colours.

# 0.2.0 — follow-ups

Build: `cargo build --release --workspace` produces two exes in `target\release\`. Ship them together:
`roblox_account_manager.exe` and `ram-login.exe`.
Then run `scripts\verify.ps1`, which checks the dependency split and measures idle RSS.

## 1. tao/wry are only in the login binary
Before, this was one binary that branched on `--login`, so tao, wry and WebView2 were linked into the manager.
It is now a Cargo workspace:
- `manager/`: `roblox_account_manager.exe`. Its Cargo.toml has no tao or wry.
- `login/`: `ram-login.exe`. tao and wry are its only real dependencies.

To check: `cargo tree -p roblox_account_manager -e normal | findstr /i "tao wry webview2"` should print nothing.

## 2. Login pipe
Before, it used `Stdio::piped()`. On Windows, Rust std implements that as a **named** pipe
(`\\.\pipe\__rust_anonymous_pipe1__.<pid>.<n>`) with the default DACL.
Now:
- It uses `CreatePipe`, which is a real anonymous pipe with no name.
- The pipe has an explicit protected DACL, `D:P(A;;GA;;;<current user SID>)`, with a single ACE.
- Neither end is created inheritable. std makes the write end inheritable only inside its CreateProcess lock.
- `ram-login.exe` refuses to run (exit 3) unless its stdout is a pipe (`GetFileType == FILE_TYPE_PIPE`).
  So the cookie can't end up on a console or in a redirected file.
- The manager reads up to a newline, with a 16 KB cap and pre-reserved capacity, then zeroizes the buffer.
- Each sign-in gets a fresh WebView2 profile folder with a random name. It is deleted afterwards.

## 3. Error-window detection
`watcher/winscan.rs`. For each tracked client it builds a family: the client plus its direct child processes.
Each tick it then walks:
- `EnumThreadWindows` for every thread of the family,
- `EnumWindows`, filtered to the family, as a backstop,
- `EnumChildWindows` under each of those windows.

Text is read with `InternalGetWindowText`, which never sends a message, so a hung client can't stall the watcher.
It closes the dialog's root window, not the child control.

This also fixes a multi-client bug. A window from *any* process titled "Roblox …error…" used to be
counted as a failure for *every* client.

Limit: Roblox's in-game disconnect screen (Error 277 etc.) is drawn by the engine and isn't a window.
Only the log-tail option detects it, and Settings now recommends turning that on.

## 4. Same-server rejoin is gated on evidence
- Reconnects are now a plain relaunch by default.
- `launcher/uri_probe.rs` scans `%LOCALAPPDATA%\Roblox\logs` for a `roblox-player:1+…` URI that Roblox launched itself.
- It only accepts a `RequestGameJob` link that has `gameId`.
- It takes the `launchtime` unit from the digit count (13 = ms, 10 = s).
- It rejects any URI within 15 s of this manager's own launches, so it can't verify against its own guesses.
- Only then does it store the template: key order and non-dynamic values. The ticket is never stored.
- The UI shows the feature as **Locked** until verified, with a "Check Roblox logs" button.
- Once verified, it is still opt-in, labelled **best-effort**, and only used on reconnect attempts 1–2.

The plain-launch URI (`launchtime` in ms) is unchanged. It's what you were already using.

## 5. Manager self-trim removed
The 10-second `EmptyWorkingSet(GetCurrentProcess())` is gone. Client trimming stays optional.
Measure with `scripts\verify.ps1`, or read the "Diagnostic" entry the app writes to Activity 5 minutes after start.

## Threading fixes
- `ShellExecuteW` used to run on a tokio worker without COM. It now runs on `spawn_blocking`, with an STA `CoInitializeEx` around it.
- Full process scans, window scans and flag-file writes inside a launch now run on `spawn_blocking` instead of tokio workers.
- Stop during queue/start now works. Before, Stop removed the tracker entry and the launch task re-created it and bound the client anyway.
  Each launch now has a unique generation number. A stale task aborts, and if the client already appeared, the task closes it.
- FastFlags race: the launch gate was released 1.5 s after the PID appeared. A second account with a different preset
  could overwrite `ClientAppSettings.json` before the first client read it. The gate is now held until the client's main window is up (max 20 s).
- Multiple clients: the manager holds `ROBLOX_singletonMutex` and `ROBLOX_singletonEvent` (Settings, on by default).
  This is not verified against your Roblox version. If a client closes right after another starts, Activity now says so.
- `on_failure` no longer resurrects an entry for an account the user just stopped.
- Settings gained `#[serde(default)]`. Before, adding a field made old saved settings fail to parse, and they silently reset to defaults.

## UI
- Top bar: Accounts, Activity and Settings tabs, a running count, Stop all, and Add account.
- Account list: search, group filter, a status dot and text on each row, one-click ▶ Play / ⏹ Stop,
  and checkboxes with a bulk Launch/Stop bar.
- Account page: big Play/Stop button, a status pill, and cards for Game, Setup, Session and Remove.
  - The Game box accepts a full roblox.com link.
  - Nickname and Group save as you type.
  - A relogin banner appears when the sign-in has expired.
- New: Remove account, with confirmation. It didn't exist before.
- Activity: plain-language event names, local time, filter by account, "Problems only".
- Settings: grouped by task, with a one-line explanation each, friendly slider labels and a Diagnostics block.
- A first-run empty state with a 4-step guide.
