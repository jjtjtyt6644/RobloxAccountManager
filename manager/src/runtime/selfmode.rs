//! Keeps the manager itself light while you're not looking at it (Settings → "Keep the manager light
//! in the background", on by default).
//!
//! * Not focused  → Windows Efficiency mode + below-normal priority. The UI only redraws on events
//!                  anyway; this just makes the little work it does (watcher ticks, launches) cheaper.
//! * Minimized    → no redraws at all (UiTx skips repaint requests; eframe would otherwise render
//!                  frames into the minimized window), and ~1.5 s after minimizing, a one-time trim of
//!                  the manager's working set. While minimized nothing touches the UI's pages, so they
//!                  stay out of RAM until you open the window again — unlike the periodic self-trim
//!                  removed in 0.2.0, which was undone by the next repaint.
//!
//! Most of the manager's RAM while visible is the graphics driver's OpenGL state, which is why the
//! visible-window figure can't go much lower; minimized, it drops to a few MB.
use super::trim::Proc;
use crate::app::UiTx;
use std::sync::atomic::Ordering;
use std::time::Duration;
use windows::Win32::System::Threading::{
    GetCurrentProcess, SetPriorityClass, BELOW_NORMAL_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
};

#[derive(Default)]
pub struct SelfMode {
    eco: bool,
    minimized: bool,
}

impl SelfMode {
    pub fn update(&mut self, ui: &UiTx, enabled: bool, focused: bool, minimized: bool) {
        let want_eco = enabled && !focused;
        if want_eco != self.eco {
            self.eco = want_eco;
            Proc::current().set_efficiency(want_eco);
            unsafe {
                let _ = SetPriorityClass(
                    GetCurrentProcess(),
                    if want_eco { BELOW_NORMAL_PRIORITY_CLASS } else { NORMAL_PRIORITY_CLASS },
                );
            }
        }
        let hide = enabled && minimized;
        ui.hidden.store(hide, Ordering::SeqCst);
        if hide && !self.minimized {
            // After the minimize frame has been drawn; skipped if the window came back meanwhile.
            let flag = ui.hidden.clone();
            std::thread::Builder::new()
                .name("self-trim".into())
                .stack_size(64 * 1024)
                .spawn(move || {
                    std::thread::sleep(Duration::from_millis(1500));
                    if flag.load(Ordering::SeqCst) {
                        Proc::current().trim();
                    }
                })
                .ok();
        }
        self.minimized = minimized;
    }
}
