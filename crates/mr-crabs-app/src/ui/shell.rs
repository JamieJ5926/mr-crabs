//! App shell: owns the shared model and exact native window handles.
//!
//! Startup and `NewWindow` open native windows for every model `WindowId`;
//! model/native additions/removals are synchronized. Native close removes
//! the exact originating `WindowId`, never `active_window`. Actions are
//! registered through the shell so `NewWindow` materializes immediately,
//! other actions refresh/sync, and quit follows model policy only.

use std::collections::{BTreeMap, BTreeSet};

use gpui::{
    App, AppContext as _, Context, Entity, SharedString, Subscription, TitlebarOptions, WeakEntity,
    WindowHandle, WindowOptions,
};

use crate::action::AppAction;
use crate::model::app_model::AppModel;
use crate::model::window::WindowId;

use super::workspace::{WINDOW_TITLE_PREFIX, WindowView};

/// Pure, unit-testable plan for synchronizing model vs native window sets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowSyncPlan {
    pub to_open: Vec<WindowId>,
    pub to_close: Vec<WindowId>,
}

/// Pure helper: diff model vs native `WindowId`s.
pub fn window_sync_plan(
    model_ids: impl IntoIterator<Item = WindowId>,
    native_ids: impl IntoIterator<Item = WindowId>,
) -> WindowSyncPlan {
    let model: BTreeSet<WindowId> = model_ids.into_iter().collect();
    let native: BTreeSet<WindowId> = native_ids.into_iter().collect();
    WindowSyncPlan {
        to_open: model.difference(&native).copied().collect(),
        to_close: native.difference(&model).copied().collect(),
    }
}

/// Pure helper: find the originating model `WindowId` for a closed GPUI
/// window, given the current shell bindings. Returns `None` if the closed
/// window is unknown (e.g. already removed).
pub fn originating_model_window(
    bindings: impl IntoIterator<Item = (WindowId, gpui::WindowId)>,
    closed: gpui::WindowId,
) -> Option<WindowId> {
    bindings
        .into_iter()
        .find_map(|(mid, gid)| (gid == closed).then_some(mid))
}

/// Shared shell owning the model and exact native window map.
pub struct AppShell {
    pub model: Entity<AppModel>,
    windows: BTreeMap<WindowId, WindowHandle<WindowView>>,
    _window_closed: Option<Subscription>,
}

impl AppShell {
    pub fn new(model: Entity<AppModel>) -> Self {
        Self {
            model,
            windows: BTreeMap::new(),
            _window_closed: None,
        }
    }

    pub fn install_window_closed_handler(&mut self, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        self._window_closed = Some(cx.on_window_closed(move |cx, closed_gpui_id| {
            let should_quit = weak
                .update(cx, |this, cx| this.handle_native_close(closed_gpui_id, cx))
                .unwrap_or(false);
            // macOS GPUI Default quit mode keeps the process alive with no
            // windows. A terminal must exit. `cx.quit()` posts `terminate:`
            // asynchronously and has been observed not to reap this binary.
            if should_quit || cx.windows().is_empty() {
                if !should_quit {
                    let _ = weak.update(cx, |this, cx| {
                        this.model.update(cx, |model, _| model.shutdown_all());
                    });
                }
                cx.quit();
                std::process::exit(0);
            }
        }));
    }

    /// Native window closed (platform close button). Remove/close the exact
    /// originating model window, never `active_window`.
    fn handle_native_close(&mut self, closed: gpui::WindowId, cx: &mut Context<Self>) -> bool {
        let origin = self
            .windows
            .iter()
            .find_map(|(mid, handle)| (handle.window_id() == closed).then_some(*mid));
        if let Some(mid) = origin {
            self.windows.remove(&mid);
            self.model.update(cx, |model, _| {
                model.close_window(mid);
            });
        }
        self.model.read(cx).should_quit()
    }

    /// Synchronize native windows to the model: close extras, open missing.
    /// Called on the main thread only.
    pub fn sync_windows(&mut self, cx: &mut Context<Self>) {
        let desired: BTreeSet<WindowId> = self.model.read(cx).windows.keys().copied().collect();
        let native: BTreeSet<WindowId> = self.windows.keys().copied().collect();
        let plan = window_sync_plan(desired, native);

        for mid in plan.to_close {
            if let Some(handle) = self.windows.remove(&mid) {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            }
        }
        // Clone weak for child views.
        let weak_shell = cx.weak_entity();
        for mid in plan.to_open {
            self.open_native_window(mid, weak_shell.clone(), cx);
        }
    }

    fn open_native_window(
        &mut self,
        window_id: WindowId,
        weak_shell: WeakEntity<Self>,
        cx: &mut Context<Self>,
    ) {
        let model = self.model.clone();
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from(WINDOW_TITLE_PREFIX)),
                ..Default::default()
            }),
            focus: true,
            show: true,
            ..Default::default()
        };
        let handle = cx
            .open_window(options, |window, cx| {
                cx.new(|cx| {
                    WindowView::new(model.clone(), window_id, weak_shell.clone(), window, cx)
                })
            })
            .expect("open the shell window");
        self.windows.insert(window_id, handle);
    }

    /// Register every shell action through this shell entity.
    pub fn register_actions(shell: &Entity<Self>, cx: &mut App) {
        use crate::ui::actions::*;

        macro_rules! register {
            ($($gpui:ident => $shell:ident),* $(,)?) => {
                $(
                    {
                        let weak = shell.downgrade();
                        cx.on_action::<$gpui>(move |_, cx| {
                            let should_quit = weak
                                .update(cx, |shell, cx| {
                                    shell.model.update(cx, |model, _| {
                                        model.dispatch(AppAction::$shell);
                                    });
                                    shell.sync_windows(cx);
                                    cx.refresh_windows();
                                    shell.model.read(cx).should_quit()
                                })
                                .unwrap_or(false);
                            if should_quit {
                                cx.quit();
                                std::process::exit(0);
                            }
                        });
                    }
                )*
            };
        }

        register!(
            NewWindow => NewWindow,
            CloseWindow => CloseWindow,
            NewTab => NewTab,
            CloseTab => CloseTab,
            NextTab => NextTab,
            PreviousTab => PreviousTab,
            NewSplitRight => NewSplitRight,
            NewSplitDown => NewSplitDown,
            ClosePane => ClosePane,
            NextPane => NextPane,
            PreviousPane => PreviousPane,
            GotoSplitUp => GotoSplitUp,
            GotoSplitDown => GotoSplitDown,
            GotoSplitLeft => GotoSplitLeft,
            GotoSplitRight => GotoSplitRight,
            TogglePalette => TogglePalette,
            ToggleQuickTerminal => ToggleQuickTerminal,
            ToggleSecureInput => ToggleSecureInput,
            ReloadConfig => ReloadConfig,
            CheckForUpdates => CheckForUpdates,
            SearchNext => SearchNext,
            SearchPrevious => SearchPrevious,
            Quit => Quit,
        );
    }

    /// Expose windows map for pure testing (read-only snapshot).
    #[cfg(test)]
    pub fn windows_snapshot(&self) -> BTreeMap<WindowId, gpui::WindowId> {
        self.windows
            .iter()
            .map(|(mid, handle)| (*mid, handle.window_id()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::window::WindowId;
    use gpui::TestAppContext;

    #[test]
    fn window_sync_plan_opens_missing_and_closes_extra() {
        let a = WindowId::new(1);
        let b = WindowId::new(2);
        let c = WindowId::new(3);
        // Model has A,B ; native has B,C
        let plan = window_sync_plan(vec![a, b], vec![b, c]);
        assert_eq!(plan.to_open, vec![a]);
        assert_eq!(plan.to_close, vec![c]);
    }

    #[test]
    fn window_sync_plan_empty_sets() {
        let plan = window_sync_plan(vec![], vec![]);
        assert!(plan.to_open.is_empty());
        assert!(plan.to_close.is_empty());
    }

    #[test]
    fn window_sync_plan_no_diff() {
        let a = WindowId::new(7);
        let plan = window_sync_plan(vec![a], vec![a]);
        assert!(plan.to_open.is_empty());
        assert!(plan.to_close.is_empty());
    }

    #[test]
    fn window_sync_plan_is_sorted_and_deduplicated() {
        let one = WindowId::new(1);
        let two = WindowId::new(2);
        let three = WindowId::new(3);
        let plan = window_sync_plan(vec![three, one, two, one], vec![two, three, two]);
        assert_eq!(plan.to_open, vec![one]);
        assert!(plan.to_close.is_empty());
    }

    #[test]
    fn originating_model_window_finds_exact_match() {
        let mid1 = WindowId::new(10);
        let mid2 = WindowId::new(20);
        let gid1 = gpui::WindowId::from(1u64);
        let gid2 = gpui::WindowId::from(2u64);
        let found = originating_model_window(vec![(mid1, gid1), (mid2, gid2)], gid2);
        assert_eq!(found, Some(mid2));
    }

    #[test]
    fn originating_model_window_none_on_unknown() {
        let mid = WindowId::new(5);
        let gid = gpui::WindowId::from(99u64);
        let other = gpui::WindowId::from(100u64);
        assert_eq!(originating_model_window(vec![(mid, gid)], other), None);
    }

    #[gpui::test]
    fn new_window_has_matching_native_handle(cx: &mut TestAppContext) {
        let (model, shell) = cx.update(|cx| {
            let model = cx.new(|_| AppModel::headless());
            let shell = cx.new(|_| AppShell::new(model.clone()));
            shell.update(cx, |shell, cx| shell.sync_windows(cx));
            (model, shell)
        });

        cx.update(|cx| {
            let model_ids: BTreeSet<_> = model.read(cx).windows.keys().copied().collect();
            let bindings = shell.read(cx).windows_snapshot();
            assert_eq!(bindings.keys().copied().collect::<BTreeSet<_>>(), model_ids);

            model.update(cx, |model, _| {
                model.dispatch(AppAction::NewWindow);
            });
            shell.update(cx, |shell, cx| shell.sync_windows(cx));

            let model_ids: BTreeSet<_> = model.read(cx).windows.keys().copied().collect();
            let bindings = shell.read(cx).windows_snapshot();
            assert_eq!(bindings.len(), 2);
            assert_eq!(bindings.keys().copied().collect::<BTreeSet<_>>(), model_ids);
        });
    }

    #[gpui::test]
    fn closing_native_targets_its_window_id(cx: &mut TestAppContext) {
        let (model, shell) = cx.update(|cx| {
            let model = cx.new(|_| AppModel::headless());
            model.update(cx, |model, _| {
                model.dispatch(AppAction::NewWindow);
            });
            let shell = cx.new(|_| AppShell::new(model.clone()));
            shell.update(cx, |shell, cx| shell.sync_windows(cx));
            (model, shell)
        });

        cx.update(|cx| {
            let bindings = shell.read(cx).windows_snapshot();
            let mut model_ids = bindings.keys().copied();
            let closing = model_ids.next().expect("first model window");
            let survivor = model_ids.next().expect("second model window");
            let closed_native = bindings[&closing];

            let should_quit =
                shell.update(cx, |shell, cx| shell.handle_native_close(closed_native, cx));
            assert!(!should_quit);
            assert!(!model.read(cx).windows.contains_key(&closing));
            assert!(model.read(cx).windows.contains_key(&survivor));
            let remaining = shell.read(cx).windows_snapshot();
            assert!(!remaining.contains_key(&closing));
            assert!(remaining.contains_key(&survivor));
        });
    }

    #[gpui::test]
    fn last_native_close_requests_quit(cx: &mut TestAppContext) {
        let (model, shell) = cx.update(|cx| {
            let model = cx.new(|_| AppModel::headless());
            let shell = cx.new(|_| AppShell::new(model.clone()));
            shell.update(cx, |shell, cx| shell.sync_windows(cx));
            (model, shell)
        });

        cx.update(|cx| {
            let bindings = shell.read(cx).windows_snapshot();
            assert_eq!(bindings.len(), 1);
            let closing = *bindings.keys().next().expect("seeded window");
            let closed_native = bindings[&closing];
            let should_quit =
                shell.update(cx, |shell, cx| shell.handle_native_close(closed_native, cx));
            assert!(should_quit);
            assert!(model.read(cx).windows.is_empty());
            assert!(shell.read(cx).windows_snapshot().is_empty());
        });
    }
}
