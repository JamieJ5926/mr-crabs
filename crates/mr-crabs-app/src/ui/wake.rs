//! Event-driven foreground PTY wake.
//!
//! Reader/exit threads set a coalesced dirty flag and post to the macOS
//! main queue. The trampoline body calls `AppModel::pump` on the live
//! `AsyncApp` stored at install time. No parked `cx.spawn` future.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::shell::AppShell;
use gpui::{App, AsyncApp, Entity, WeakEntity};

use super::workspace::PUMP_CAP_PER_PANE;

struct WakeState {
    cx: AsyncApp,
    model: WeakEntity<crate::model::app_model::AppModel>,
    shell: WeakEntity<AppShell>,
    dirty: Arc<AtomicBool>,
}

thread_local! {
    static WAKE: RefCell<Option<WakeState>> = const { RefCell::new(None) };
    static PUMPING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Create a capacity-1 coalesced `OutputWake`.
///
/// The first dirty transition schedules a main-thread pump; later wakes
/// while dirty are already coalesced.
pub fn new_output_wake() -> (mr_crabs_pty::OutputWake, Arc<AtomicBool>) {
    let dirty = Arc::new(AtomicBool::new(false));
    let scheduled = Arc::clone(&dirty);
    let output_wake: mr_crabs_pty::OutputWake = Arc::new(move || {
        if !scheduled.swap(true, Ordering::AcqRel) {
            schedule_main_pump();
        }
    });
    (output_wake, dirty)
}

/// Install the main-thread pump target. Must run on the GPUI thread
/// **before** `sync_windows` so the first PTY wake can land.
pub fn install_wake(
    cx: &mut App,
    model: Entity<crate::model::app_model::AppModel>,
    shell: Entity<AppShell>,
    dirty: Arc<AtomicBool>,
) {
    let async_cx = cx.to_async();
    WAKE.with(|slot| {
        *slot.borrow_mut() = Some(WakeState {
            cx: async_cx,
            model: model.downgrade(),
            shell: shell.downgrade(),
            dirty,
        });
    });
}

/// Convenience used by tests: install TLS and return the wake closure.
pub fn spawn_wake_task(
    cx: &mut App,
    model: Entity<crate::model::app_model::AppModel>,
    shell: Entity<AppShell>,
) -> mr_crabs_pty::OutputWake {
    let (wake, dirty) = new_output_wake();
    install_wake(cx, model, shell, dirty);
    wake
}

/// Pump queued PTY bytes on the live `&mut App`.
///
/// Called from window render and terminal paint. Returns whether a new
/// frame was published.
pub fn pump_output(cx: &mut App) -> bool {
    pump_now(cx)
}

/// Drain a pending wake using a live `&mut App`.
pub fn drain_scheduled(cx: &mut App) {
    let _ = pump_output(cx);
}

/// Pure helper: re-arm exactly once for pending work unless pumping failed.
#[inline]
pub fn should_rearm(pending: bool, failed: bool) -> bool {
    pending && !failed
}

fn schedule_main_pump() {
    post_to_main_queue();
}

fn pump_now(cx: &mut App) -> bool {
    if PUMPING.with(std::cell::Cell::get) {
        return false;
    }
    PUMPING.with(|flag| flag.set(true));
    let changed = pump_now_inner(cx);
    PUMPING.with(|flag| flag.set(false));
    let dirty = WAKE.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|state| state.dirty.load(Ordering::Acquire))
            .unwrap_or(false)
    });
    if dirty {
        schedule_main_pump();
    }
    changed
}

fn pump_now_inner(cx: &mut App) -> bool {
    let state = WAKE.with(|slot| {
        slot.borrow().as_ref().map(|state| {
            (
                state.model.clone(),
                state.shell.clone(),
                Arc::clone(&state.dirty),
            )
        })
    });
    let Some((model, shell, dirty)) = state else {
        return false;
    };
    dirty.store(false, Ordering::Release);
    let Some(model) = model.upgrade() else {
        return false;
    };
    let (changed, pending, failed, should_quit) = model.update(cx, |model, _| {
        let stats = model.pump(PUMP_CAP_PER_PANE);
        (
            stats.changed(),
            stats.pending,
            stats.error.is_some(),
            model.should_quit(),
        )
    });
    if changed && let Some(shell) = shell.upgrade() {
        shell.update(cx, |shell, cx| shell.refresh_windows(cx));
    }
    if should_quit {
        cx.quit();
        return changed;
    }
    if should_rearm(pending, failed) {
        dirty.store(true, Ordering::Release);
        cx.defer(|cx| {
            let _ = pump_now(cx);
        });
    }
    changed
}

fn pump_from_tls() {
    let async_cx = WAKE.with(|slot| slot.borrow().as_ref().map(|state| state.cx.clone()));
    let Some(async_cx) = async_cx else {
        return;
    };
    async_cx.update(pump_now);
}

#[cfg(target_os = "macos")]
fn post_to_main_queue() {
    use std::ffi::c_void;

    unsafe extern "C" {
        static mut _dispatch_main_q: u8;
        fn dispatch_async_f(
            queue: *mut u8,
            context: *mut c_void,
            work: unsafe extern "C" fn(*mut c_void),
        );
    }

    unsafe extern "C" fn trampoline(_context: *mut c_void) {
        pump_from_tls();
    }

    unsafe {
        dispatch_async_f(&raw mut _dispatch_main_q, std::ptr::null_mut(), trampoline);
    }
}

#[cfg(not(target_os = "macos"))]
fn post_to_main_queue() {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;

    use gpui::{AppContext as _, TestAppContext};

    use crate::model::app_model::AppModel;
    use crate::model::pane::{PaneSession, PtyLifecycle};
    use crate::ui::shell::AppShell;

    #[test]
    fn rearm_only_for_pending_success() {
        assert!(should_rearm(true, false));
        assert!(!should_rearm(false, false));
        assert!(!should_rearm(true, true));
    }

    #[test]
    fn pump_cap_is_bounded() {
        assert_eq!(PUMP_CAP_PER_PANE, 64);
    }

    #[test]
    fn wake_coalesces_and_rearms() {
        let dirty = Arc::new(AtomicBool::new(false));
        assert!(!dirty.swap(true, Ordering::AcqRel));
        assert!(dirty.swap(true, Ordering::AcqRel));
        assert!(should_rearm(true, false));
        dirty.store(false, Ordering::Release);
        assert!(!dirty.swap(true, Ordering::AcqRel));
        assert!(!should_rearm(false, false));
    }

    #[gpui::test]
    fn idle_output_publishes_without_input(cx: &mut TestAppContext) {
        let (wake, model, _shell, window_id, reader_tx) = cx.update(move |cx| {
            let model = cx.new(|_| AppModel::headless());
            let window_id = model.read(cx).active_window.expect("active window");
            let pane_id = model.read(cx).focused_pane_id().expect("focused pane");
            let size = model.read(cx).windows[&window_id]
                .active_tab()
                .expect("active tab")
                .panes[&pane_id]
                .last_size;
            let (reader_tx, reader_rx) = sync_channel(1);
            model.update(cx, |model, _| {
                let pane = model
                    .windows
                    .get_mut(&window_id)
                    .and_then(|window| window.active_tab_mut())
                    .and_then(|tab| tab.panes.get_mut(&pane_id))
                    .expect("focused pane");
                pane.session = PaneSession::from_receivers(size, Some(reader_rx), None);
                pane.lifecycle = PtyLifecycle::Live;
            });
            let shell = cx.new(|_| AppShell::new(model.clone()));
            let wake = spawn_wake_task(cx, model.clone(), shell.clone());
            (wake, model, shell, window_id, reader_tx)
        });

        reader_tx.send(b"LATE".to_vec()).expect("queue output");
        wake();
        cx.update(drain_scheduled);
        cx.run_until_parked();
        cx.update(|cx| {
            let frame = model
                .read(cx)
                .focused_frame(window_id)
                .expect("wake publishes frame");
            assert_eq!(frame.cursor.col, 4);
        });
    }
}
