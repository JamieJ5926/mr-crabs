//! The window view: renders the focused pane's `TerminalElement`, routes keys
//! to the shell (keymap → palette → terminal encoders), and exposes
//! accessibility roles.
//!
//! Rendering is a pure function of the model plus the measured drawable
//! viewport: the view measures the configured font into per-cell metrics,
//! commits one `SurfaceGeometry` through the model (window → tab → pane →
//! terminal/PTY), then clones the focused pane's `Arc<FrameDelta>` and
//! builds a `TerminalElement` with the committed pane derivative — the
//! engine is never locked during paint. A separate bounded foreground task
//! is woken by PTY readers, pumps queued bytes into the model, publishes the
//! frame, and refreshes GPUI before this view renders.
//!
//! Each pane's `TerminalElement` carries a stable `ElementId::NamedInteger`
//! derived from its `PaneId` (see [`terminal_element_id`]), so the element
//! is reconstructed every render against the same global identity:
//! `Window::with_element_state` retained paint state (render cache, shaped
//! rows, resize dedupe, font identity) survives reconstruction, and
//! post-resize SIGWINCH redraws — incremental `Partial` `FrameDelta`s —
//! compose on top of the retained full frame instead of repainting
//! everything.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use gpui::{
    App, ClipboardItem, Context, ElementId, Entity, ExternalPaths, FocusHandle, Font,
    FontFallbacks, InteractiveElement as _, IntoElement, KeyDownEvent, KeyUpEvent, Keystroke,
    Modifiers as GpuiModifiers, MouseButton as GpuiMouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement as _, Render, Role, ScrollDelta, ScrollWheelEvent, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity, Window,
    WindowAppearance, div, font, px,
};
use parking_lot::Mutex;

use mr_crabs_element::{
    CellMetrics, EffectsConfig, GraphicsOverlay, PixelExtent, TerminalElement, TerminalPalette,
};
use mr_crabs_history::SelectionGesture;
#[cfg(test)]
use mr_crabs_input::Key;
use mr_crabs_input::{
    ClipboardBackend, ClipboardController, ClipboardKind, ClipboardPermission,
    KeyAction as InputKeyAction, KeyEvent as InputKeyEvent, Modifiers as InputModifiers,
    MouseAction as InputMouseAction, MouseButton as InputMouseButton, encode_drop_paths,
    sanitize_drop_paths,
};
use mr_crabs_terminal::FrameDelta;

use crate::model::app_model::AppModel;
use crate::model::geometry::{PaddingPx, SurfaceGeometry};
use crate::model::input_dock::{
    CHROME_TOTAL, InputDockLayout, InputDockSnapshot, InputDockState, PointF, hit_test_dock,
    remap_pointer,
};
use crate::model::presentation::{ConversationEvent, SurfaceMode};
use crate::model::split::{GridRect, PaneId};
use crate::model::window::WindowId;
use crate::palette::PaletteState;
use crate::ui::input_dock::{
    InputDockOverlayView, InputDockTokens, compose_input_dock_layout, dock_hit_consumes,
    input_dock_footer, input_dock_mask, input_dock_overlay, input_dock_separator,
};
use crate::ui::input_surface::{
    encode_ime, encode_live_focus, encode_live_key, encode_live_mouse, encode_live_paste,
};
use crate::ui::shell::AppShell;

/// A cached measured cell metric: the per-cell size derived from the
/// configured font, keyed to the settings generation that produced it.
/// `None` until the first successful measurement, so there is never a
/// guessed startup metric.
const SYMBOLS_FONT_FAMILY: &str = "Symbols Nerd Font Mono";

struct Osc52Backend {
    current: String,
    write: Arc<Mutex<Option<String>>>,
}

impl ClipboardBackend for Osc52Backend {
    fn write(&self, _: ClipboardKind, text: &str) -> Result<(), String> {
        *self.write.lock() = Some(text.to_owned());
        Ok(())
    }

    fn read(&self, _: ClipboardKind) -> Result<String, String> {
        Ok(self.current.clone())
    }
}

#[derive(Clone)]
struct MeasuredCellMetrics {
    settings_generation: u64,
    metrics: CellMetrics,
    font: Font,
}

/// Permanent native title prefix identifying the pure-Rust application.
pub const WINDOW_TITLE_PREFIX: &str = "Mr Crabs";

fn prefixed_window_title(shell_title: &str) -> String {
    format!("{WINDOW_TITLE_PREFIX} — {shell_title}")
}

/// Per-wake pump cap: at most this many chunks per pane before GPUI refreshes,
/// so a burst cannot monopolize the main thread; bounded reader queues apply
/// backpressure.
pub const PUMP_CAP_PER_PANE: usize = 64;

/// The root view of a shell window.
pub struct WindowView {
    pub model: Entity<AppModel>,
    pub window_id: WindowId,
    shell: WeakEntity<AppShell>,
    /// Cached measured cell metrics for the current settings generation;
    /// `None` until the first successful measurement.
    measured: Option<MeasuredCellMetrics>,
    /// Shared with each pane's text-input sink. Drained on the main thread
    /// during render; never parked on a `cx.spawn` future.
    pub focus: FocusHandle,
    ime_tx: Sender<(PaneId, String)>,
    ime_rx: Receiver<(PaneId, String)>,
    /// Focus subscriptions must remain alive for the lifetime of the view.
    _focus_subscriptions: Vec<Subscription>,
}

impl WindowView {
    pub fn new(
        model: Entity<AppModel>,
        window_id: WindowId,
        shell: WeakEntity<AppShell>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let initial_focus = focus.clone();
        window.defer(cx, move |window, cx| window.focus(&initial_focus, cx));

        let (ime_tx, ime_rx) = mpsc::channel();

        let focused = cx.on_focus_in(&focus, window, |view, _, cx| {
            view.write_focus_report(true, cx);
        });
        let blurred = cx.on_focus_out(&focus, window, |view, _, _, cx| {
            view.write_focus_report(false, cx);
        });

        Self {
            model,
            window_id,
            shell,
            measured: None,
            focus,
            ime_tx,
            ime_rx,
            _focus_subscriptions: vec![focused, blurred],
        }
    }
}

impl WindowView {
    fn write_focus_report(&mut self, focused: bool, cx: &mut Context<Self>) {
        let window_id = self.window_id;
        self.model.update(cx, |model, _| {
            let Some(window) = model.window(window_id) else {
                return;
            };
            let Some(tab) = window.active_tab() else {
                return;
            };
            let Some(pane_id) = tab.focused_pane_id() else {
                return;
            };
            let Some(pane) = tab.pane(pane_id) else {
                return;
            };
            let bytes = encode_live_focus(&pane.core, focused);
            if !bytes.is_empty() {
                model.write_to_pane(pane_id, &bytes);
            }
        });
    }

    fn drain_ime_commits(&mut self, cx: &mut Context<Self>) {
        while let Ok((pane_id, text)) = self.ime_rx.try_recv() {
            if self.model.read(cx).palette.is_open() {
                continue;
            }
            let bytes = encode_ime(&text, false);
            if !bytes.is_empty() {
                self.model
                    .update(cx, |model, _| model.write_to_pane(pane_id, &bytes));
            }
        }
    }

    fn process_clipboard_requests(&mut self, cx: &mut Context<Self>) {
        let (requests, permission) = self.model.update(cx, |model, _| {
            let settings = model.settings.current();
            (
                model.drain_clipboard_requests(),
                ClipboardPermission {
                    allow_osc52_write: settings.allow_osc52_write,
                    allow_osc52_read: settings.allow_osc52_read,
                    ..ClipboardPermission::default()
                },
            )
        });
        if requests.is_empty() {
            return;
        }

        for (pane_id, event) in requests {
            let staged_write = Arc::new(Mutex::new(None));
            let current = if event.data.as_slice() == b"?" && permission.allow_osc52_read {
                cx.read_from_clipboard()
                    .and_then(|item| item.text())
                    .unwrap_or_default()
            } else {
                String::new()
            };
            let backend = Osc52Backend {
                current,
                write: Arc::clone(&staged_write),
            };
            let controller = ClipboardController::new(permission.clone(), Some(Arc::new(backend)));
            if event.data.as_slice() == b"?" {
                if let Ok(encoded) = controller.osc52_read_request() {
                    let reply = format!("\x1b]52;{};{}\x1b\\", char::from(event.kind), encoded);
                    self.model.update(cx, |model, _| {
                        model.write_to_pane(pane_id, reply.as_bytes());
                    });
                }
            } else if let Ok(payload) = std::str::from_utf8(&event.data)
                && controller.osc52_write(payload).is_ok()
                && let Some(text) = staged_write.lock().take()
            {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        }
    }
}

/// Immutable render data for one pane in this window's active tab. Every pane
/// gets its own split derivative, frame, graphics overlay, and stable GPUI
/// element identity; no pane borrows the focused pane's geometry or state.
struct PaneRender {
    pane_id: PaneId,
    geometry: SurfaceGeometry,
    rect: GridRect,
    pane_geometry: SurfaceGeometry,
    frame: Arc<FrameDelta>,
    graphics: Arc<Mutex<GraphicsOverlay>>,
    dock: Option<Arc<InputDockSnapshot>>,
    focused: bool,
}

struct FocusedDockRender {
    pane_id: PaneId,
    geometry: SurfaceGeometry,
    rect: GridRect,
    pane_geometry: SurfaceGeometry,
    snap: Arc<InputDockSnapshot>,
}

struct DockMouseRoute {
    pane_id: PaneId,
    geometry: SurfaceGeometry,
    layout: InputDockLayout,
    window_x: f32,
    window_y: f32,
    button: Option<InputMouseButton>,
    action: InputMouseAction,
    modifiers: GpuiModifiers,
    click_count: usize,
}

/// Static namespace for pane terminal element IDs (see
/// [`terminal_element_id`]); kept inside the ID so a pane's identity is
/// distinguishable from any other `NamedInteger` element.
const TERMINAL_ELEMENT_NAME: &str = "mr-crabs-terminal";

/// The stable GPUI element ID for one pane's `TerminalElement`: the static
/// namespace string plus the pane's unique ID, as `ElementId::NamedInteger`
/// (`SharedString::new_static` is zero-allocation — no formatted string is
/// built per frame). The element is reconstructed on every render, so this
/// pure function of the pane ID is what keeps `Window::with_element_state`
/// retained paint state (render cache, shaped rows, resize dedupe, font
/// identity) keyed to the same pane across frames; distinct panes always
/// produce distinct IDs.
fn terminal_element_id(pane_id: PaneId) -> ElementId {
    ElementId::NamedInteger(
        SharedString::new_static(TERMINAL_ELEMENT_NAME),
        pane_id.as_u64(),
    )
}

impl Render for WindowView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focus.is_focused(window) {
            window.focus(&self.focus, cx);
        }
        self.process_clipboard_requests(cx);
        self.drain_ime_commits(cx);

        // 1. Measure the settings-driven cell metrics (cached until the
        //    settings generation changes; only successful measurements are
        //    cached) and derive the surface geometry from the drawable
        //    viewport, committing it through the single model authority.
        //    Nothing here guesses from window bounds or a hardcoded grid.
        let (settings, settings_generation) = {
            let shell = self.model.read(cx);
            (shell.settings.current(), shell.settings.generation)
        };
        let terminal_palette = match settings.theme.as_str() {
            "light" => TerminalPalette::light(settings.background_opacity),
            "dark" => TerminalPalette::dark(settings.background_opacity),
            _ => match window.appearance() {
                WindowAppearance::Light | WindowAppearance::VibrantLight => {
                    TerminalPalette::light(settings.background_opacity)
                }
                WindowAppearance::Dark | WindowAppearance::VibrantDark => {
                    TerminalPalette::dark(settings.background_opacity)
                }
            },
        };
        let measurement_is_stale = self
            .measured
            .as_ref()
            .is_none_or(|cached| cached.settings_generation != settings_generation);
        if measurement_is_stale {
            self.measured = measure_cell_metrics(
                window,
                &settings.font_family,
                settings.font_size,
                settings.line_height_adjust_percent,
            )
            .map(|(metrics, font)| MeasuredCellMetrics {
                settings_generation,
                metrics,
                font,
            });
        }
        let metrics = self.measured.as_ref().map(|measured| measured.metrics);
        let terminal_font = self.measured.as_ref().map(|measured| measured.font.clone());
        let viewport = window.viewport_size();
        let geometry = metrics.and_then(|metrics| {
            settings_padding(settings.padding_x, settings.padding_y).and_then(|padding| {
                SurfaceGeometry::from_viewport(
                    PixelExtent {
                        width: f32::from(viewport.width),
                        height: f32::from(viewport.height),
                    },
                    metrics,
                    padding,
                )
            })
        });
        if let Some(geometry) = geometry {
            self.model.update(cx, |model, _| {
                model.commit_geometry(self.window_id, geometry);
            });
        }

        // 3. Keep the native title in sync with the shell model.
        let title = self
            .model
            .read(cx)
            .window(self.window_id)
            .map(|window| window.window_title())
            .unwrap_or_default();
        if !title.is_empty() {
            window.set_window_title(&prefixed_window_title(&title));
        }

        // 4. Compose every pane in the active tab from immutable model state.
        //    Invalid surface geometry paints shell layers only; no guessed grid
        //    or focused-pane surrogate is used.
        let bundles = if geometry.is_some() {
            self.model
                .read(cx)
                .window(self.window_id)
                .and_then(|window_model| {
                    let geometry = window_model.geometry?;
                    let tab = window_model.active_tab()?;
                    Some(
                        tab.rects(geometry.grid)
                            .into_iter()
                            .filter_map(|(pane_id, rect)| {
                                let pane = tab.pane(pane_id)?;
                                Some(PaneRender {
                                    pane_id,
                                    geometry,
                                    rect,
                                    pane_geometry: geometry.for_rect(rect),
                                    frame: pane.frame()?,
                                    graphics: Arc::clone(&pane.graphics),
                                    dock: pane.input_dock(),
                                    focused: tab.focused_pane_id() == Some(pane_id),
                                })
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let palette = self.model.read(cx).palette.clone();
        let secure_input = self.model.read(cx).secure_input.is_enabled();
        let trace_for_paint = self.model.read(cx).diagnostic_trace();
        let focused_dock = bundles.iter().find_map(|bundle| {
            if !bundle.focused {
                return None;
            }
            let snap = bundle.dock.clone()?;
            if snap.state != InputDockState::ShellInputActive {
                return None;
            }
            Some(FocusedDockRender {
                pane_id: bundle.pane_id,
                geometry: bundle.geometry,
                rect: bundle.rect,
                pane_geometry: bundle.pane_geometry,
                snap,
            })
        });
        let focused_pane_bounds = bundles.iter().find(|bundle| bundle.focused).map(|bundle| {
            (
                f32::from(bundle.geometry.padding.left)
                    + f32::from(bundle.rect.x) * bundle.pane_geometry.metrics.width,
                f32::from(bundle.geometry.padding.top)
                    + f32::from(bundle.rect.y) * bundle.pane_geometry.metrics.height,
                bundle.pane_geometry.content.width,
                bundle.pane_geometry.content.height,
            )
        });
        let dock_chrome_active = focused_dock.is_some();

        let key_model = self.model.clone();
        let key_shell = self.shell.clone();
        let key_up_model = self.model.clone();
        let mut root = div()
            .relative()
            .size_full()
            .bg(terminal_palette.background_color())
            .id(ElementId::Name(SharedString::from("mr-crabs-window")))
            .role(Role::Window)
            .track_focus(&self.focus)
            .on_key_down(move |event, window, cx| {
                handle_key_event(&key_model, &key_shell, event, window, cx);
            })
            .on_key_up(move |event, _, cx| {
                handle_key_release(&key_up_model, event, cx);
            });
        for bundle in bundles {
            let left = f32::from(bundle.geometry.padding.left)
                + f32::from(bundle.rect.x) * bundle.pane_geometry.metrics.width;
            let top = f32::from(bundle.geometry.padding.top)
                + f32::from(bundle.rect.y) * bundle.pane_geometry.metrics.height;
            let ime_tx = self.ime_tx.clone();
            let ime_pane_id = bundle.pane_id;
            let mut element =
                TerminalElement::with_shared(bundle.frame, bundle.pane_geometry.metrics)
                    .with_element_id(terminal_element_id(bundle.pane_id))
                    .with_focus(self.focus.clone())
                    .with_font(
                        terminal_font
                            .as_ref()
                            .expect("measured font accompanies geometry")
                            .clone(),
                    )
                    .with_font_size(px(settings.font_size))
                    .with_palette(terminal_palette)
                    .with_effects(EffectsConfig::from(settings.animation_defaults()))
                    .with_graphics(bundle.graphics)
                    .with_input_sink(move |text| {
                        let _ = ime_tx.send((ime_pane_id, text.to_owned()));
                    });
            if let Some(trace) = trace_for_paint.clone() {
                let pane_id = bundle.pane_id;
                element = element.with_paint_diagnostics(move |ev| {
                    trace.push(crate::diagnostics::DiagnosticEvent::Paint(
                        crate::diagnostics::DiagnosticPaintEvent {
                            pane_id,
                            sequence: ev.sequence,
                            cursor_blink_requested: ev.cursor_blink_requested,
                            cursor_visible_phase: ev.cursor_visible_phase,
                            effects_busy: ev.effects_busy,
                            burst_bypass: ev.burst_bypass,
                            revealing: ev.revealing,
                            pending: ev.pending,
                            effects_needs_frame: ev.effects_needs_frame,
                            trail_active: ev.trail_active,
                            trail_alpha: ev.trail_alpha,
                            raf_reason: match ev.raf_reason {
                                mr_crabs_element::PaintRafReason::None => {
                                    crate::diagnostics::DiagnosticRafReason::None
                                }
                                mr_crabs_element::PaintRafReason::CursorBlink => {
                                    crate::diagnostics::DiagnosticRafReason::CursorBlink
                                }
                                mr_crabs_element::PaintRafReason::Effects => {
                                    crate::diagnostics::DiagnosticRafReason::Effects
                                }
                                mr_crabs_element::PaintRafReason::Both => {
                                    crate::diagnostics::DiagnosticRafReason::Both
                                }
                            },
                        },
                    ));
                });
            }

            let mouse_geometry = bundle.pane_geometry;
            let mouse_pane_id = bundle.pane_id;
            let mouse_model = self.model.clone();
            let surface = div()
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(bundle.pane_geometry.content.width))
                .h(px(bundle.pane_geometry.content.height))
                .on_any_mouse_down(move |event, _, cx| {
                    route_mouse_down(
                        &mouse_model,
                        mouse_pane_id,
                        mouse_geometry,
                        left,
                        top,
                        event,
                        cx,
                    );
                });
            let mouse_model = self.model.clone();
            let surface = surface.on_mouse_up(GpuiMouseButton::Left, move |event, _, cx| {
                route_mouse_up(
                    &mouse_model,
                    mouse_pane_id,
                    mouse_geometry,
                    left,
                    top,
                    event,
                    cx,
                );
            });
            let mouse_model = self.model.clone();
            let surface = surface.on_mouse_up(GpuiMouseButton::Right, move |event, _, cx| {
                route_mouse_up(
                    &mouse_model,
                    mouse_pane_id,
                    mouse_geometry,
                    left,
                    top,
                    event,
                    cx,
                );
            });
            let mouse_model = self.model.clone();
            let surface = surface.on_mouse_up(GpuiMouseButton::Middle, move |event, _, cx| {
                route_mouse_up(
                    &mouse_model,
                    mouse_pane_id,
                    mouse_geometry,
                    left,
                    top,
                    event,
                    cx,
                );
            });
            let mouse_model = self.model.clone();
            let surface = surface.on_mouse_move(move |event, _, cx| {
                route_mouse_move(
                    &mouse_model,
                    mouse_pane_id,
                    mouse_geometry,
                    left,
                    top,
                    event,
                    cx,
                );
            });
            let scroll_model = self.model.clone();
            let scroll_shell = self.shell.clone();
            let surface = surface.on_scroll_wheel(move |event, _, cx| {
                route_scroll(
                    &scroll_model,
                    &scroll_shell,
                    mouse_pane_id,
                    mouse_geometry,
                    PointF { x: left, y: top },
                    event,
                    cx,
                );
            });
            let drop_model = self.model.clone();
            let surface = surface.on_drop(move |paths: &ExternalPaths, _, cx| {
                route_drop_paths(&drop_model, mouse_pane_id, paths, cx);
            });
            root = root.child(surface.child(element));
        }

        // Focused-pane semantic dock: mask + 1px separator + 55px dock + 31px
        // footer. Palette stays last so it paints above the dock. Hidden when
        // the palette is open so keys stay on the existing path.
        if !palette.is_open() {
            if let Some(focused) = focused_dock {
                let FocusedDockRender {
                    pane_id,
                    geometry,
                    rect,
                    pane_geometry,
                    snap,
                } = focused;
                let left = f32::from(geometry.padding.left)
                    + f32::from(rect.x) * pane_geometry.metrics.width;
                let top = f32::from(geometry.padding.top)
                    + f32::from(rect.y) * pane_geometry.metrics.height;
                let tokens = InputDockTokens::for_palette(terminal_palette);
                if let Some(layout) = compose_input_dock_layout(
                    PixelExtent {
                        width: f32::from(viewport.width),
                        height: f32::from(viewport.height),
                    },
                    PointF { x: left, y: top },
                    pane_geometry,
                    &snap,
                    true,
                ) {
                    let dock_model = self.model.clone();
                    let dock_pane = pane_id;
                    let dock_geometry = pane_geometry;
                    let dock_layout = layout;
                    let mask_move_model = self.model.clone();
                    let mask_up_model = self.model.clone();
                    let mask_scroll_model = self.model.clone();
                    let mask_scroll_shell = self.shell.clone();
                    root = root.child(
                        input_dock_mask(layout, tokens)
                            .on_any_mouse_down(move |event, _, cx| {
                                route_dock_mouse(
                                    &dock_model,
                                    DockMouseRoute {
                                        pane_id: dock_pane,
                                        geometry: dock_geometry,
                                        layout: dock_layout,
                                        window_x: f32::from(event.position.x),
                                        window_y: f32::from(event.position.y),
                                        button: Some(input_mouse_button(event.button)),
                                        action: InputMouseAction::Press,
                                        modifiers: event.modifiers,
                                        click_count: event.click_count,
                                    },
                                    cx,
                                );
                            })
                            .on_mouse_move(move |event, _, cx| {
                                route_dock_mouse(
                                    &mask_move_model,
                                    DockMouseRoute {
                                        pane_id: dock_pane,
                                        geometry: dock_geometry,
                                        layout: dock_layout,
                                        window_x: f32::from(event.position.x),
                                        window_y: f32::from(event.position.y),
                                        button: event.pressed_button.map(input_mouse_button),
                                        action: InputMouseAction::Motion,
                                        modifiers: event.modifiers,
                                        click_count: 0,
                                    },
                                    cx,
                                );
                            })
                            .on_mouse_up(gpui::MouseButton::Left, move |event, _, cx| {
                                route_dock_mouse(
                                    &mask_up_model,
                                    DockMouseRoute {
                                        pane_id: dock_pane,
                                        geometry: dock_geometry,
                                        layout: dock_layout,
                                        window_x: f32::from(event.position.x),
                                        window_y: f32::from(event.position.y),
                                        button: Some(input_mouse_button(event.button)),
                                        action: InputMouseAction::Release,
                                        modifiers: event.modifiers,
                                        click_count: 0,
                                    },
                                    cx,
                                );
                            })
                            .on_scroll_wheel(move |event, _, cx| {
                                route_scroll(
                                    &mask_scroll_model,
                                    &mask_scroll_shell,
                                    dock_pane,
                                    dock_geometry,
                                    PointF { x: left, y: top },
                                    event,
                                    cx,
                                );
                            }),
                    );
                    let sep_scroll_model = self.model.clone();
                    let sep_scroll_shell = self.shell.clone();
                    root = root.child(input_dock_separator(layout, tokens).on_scroll_wheel(
                        move |event, _, cx| {
                            route_scroll(
                                &sep_scroll_model,
                                &sep_scroll_shell,
                                dock_pane,
                                dock_geometry,
                                PointF { x: left, y: top },
                                event,
                                cx,
                            );
                        },
                    ));
                    let overlay_move_model = self.model.clone();
                    let overlay_move_pane = pane_id;
                    let overlay_move_geometry = pane_geometry;
                    let overlay_move_layout = layout;
                    let overlay_up_model = self.model.clone();
                    let overlay_up_pane = pane_id;
                    let overlay_up_geometry = pane_geometry;
                    let overlay_up_layout = layout;
                    let overlay_scroll_model = self.model.clone();
                    let overlay_scroll_shell = self.shell.clone();
                    let overlay_scroll_pane = pane_id;
                    let overlay_scroll_geometry = pane_geometry;
                    let overlay_scroll_left = left;
                    let overlay_scroll_top = top;
                    let overlay_model = self.model.clone();
                    let overlay_pane = pane_id;
                    let overlay_geometry = pane_geometry;
                    let overlay_layout = layout;
                    let ime_tx = self.ime_tx.clone();
                    root = root.child(
                        input_dock_overlay(InputDockOverlayView {
                            snap: &snap,
                            layout,
                            tokens,
                            font: terminal_font
                                .as_ref()
                                .expect("measured font accompanies geometry")
                                .clone(),
                            font_size: px(settings.font_size),
                            metrics: pane_geometry.metrics,
                            terminal_palette,
                            focused: true,
                            focus: Some(self.focus.clone()),
                            ime_tx: Some(ime_tx),
                            pane_id,
                        })
                        .on_any_mouse_down(move |event, _, cx| {
                            route_dock_mouse(
                                &overlay_model,
                                DockMouseRoute {
                                    pane_id: overlay_pane,
                                    geometry: overlay_geometry,
                                    layout: overlay_layout,
                                    window_x: f32::from(event.position.x),
                                    window_y: f32::from(event.position.y),
                                    button: Some(input_mouse_button(event.button)),
                                    action: InputMouseAction::Press,
                                    modifiers: event.modifiers,
                                    click_count: event.click_count,
                                },
                                cx,
                            );
                        })
                        .on_mouse_move(move |event, _, cx| {
                            route_dock_mouse(
                                &overlay_move_model,
                                DockMouseRoute {
                                    pane_id: overlay_move_pane,
                                    geometry: overlay_move_geometry,
                                    layout: overlay_move_layout,
                                    window_x: f32::from(event.position.x),
                                    window_y: f32::from(event.position.y),
                                    button: event.pressed_button.map(input_mouse_button),
                                    action: InputMouseAction::Motion,
                                    modifiers: event.modifiers,
                                    click_count: 0,
                                },
                                cx,
                            );
                        })
                        .on_mouse_up(gpui::MouseButton::Left, move |event, _, cx| {
                            route_dock_mouse(
                                &overlay_up_model,
                                DockMouseRoute {
                                    pane_id: overlay_up_pane,
                                    geometry: overlay_up_geometry,
                                    layout: overlay_up_layout,
                                    window_x: f32::from(event.position.x),
                                    window_y: f32::from(event.position.y),
                                    button: Some(input_mouse_button(event.button)),
                                    action: InputMouseAction::Release,
                                    modifiers: event.modifiers,
                                    click_count: 0,
                                },
                                cx,
                            );
                        })
                        .on_scroll_wheel(move |event, _, cx| {
                            route_scroll(
                                &overlay_scroll_model,
                                &overlay_scroll_shell,
                                overlay_scroll_pane,
                                overlay_scroll_geometry,
                                PointF {
                                    x: overlay_scroll_left,
                                    y: overlay_scroll_top,
                                },
                                event,
                                cx,
                            );
                        }),
                    );
                    let footer_scroll_model = self.model.clone();
                    let footer_scroll_shell = self.shell.clone();
                    root = root.child(input_dock_footer(&snap, layout, tokens).on_scroll_wheel(
                        move |event, _, cx| {
                            route_scroll(
                                &footer_scroll_model,
                                &footer_scroll_shell,
                                dock_pane,
                                dock_geometry,
                                PointF { x: left, y: top },
                                event,
                                cx,
                            );
                        },
                    ));
                }
            }
        }

        // Chat presentation: per-pane preference, effective mode fails closed.
        // TerminalElement remains mounted underneath; chat is a read-only overlay
        // clipped to the pane geometry, not a replacement.
        let chat_info = {
            let model = self.model.read(cx);
            let focused_pane_id = model.focused_pane_id();
            focused_pane_id.and_then(|pid| {
                model
                    .active_tab()
                    .and_then(|tab| tab.panes.get(&pid))
                    .map(|pane| {
                        let effective = pane.effective_mode(palette.is_open(), false);
                        let events = pane.conversation_events(palette.is_open(), false);
                        (effective, events, pid)
                    })
            })
        };
        let chat_active = chat_info
            .as_ref()
            .is_some_and(|(mode, _, _)| *mode == SurfaceMode::Chat);
        if chat_active {
            if let (Some((_, events, _)), Some((left, top, width, height))) =
                (chat_info, focused_pane_bounds)
            {
                root = root.child(chat_overlay(
                    &events,
                    terminal_palette,
                    left,
                    top,
                    width,
                    chat_overlay_height(height, dock_chrome_active),
                ));
            }
        }

        // Chat is toggled only by cmd+shift+j (ToggleChatPresentation).

        if palette.is_open() {
            root = root.child(palette_overlay(&palette, terminal_palette));
        }
        if secure_input {
            root = root.child(
                div()
                    .absolute()
                    .top(px(8.0))
                    .right(px(8.0))
                    .id(ElementId::Name(SharedString::from("secure-input-badge")))
                    .child("Secure Input"),
            );
        }
        root
    }
}

/// Measure Ghostty/G-Spot cell metrics from the configured font and size.
///
/// Printable-ASCII advances determine cell width. Face height uses ascent
/// plus the absolute CoreText descent. Both dimensions round to device
/// pixels; `adjust-cell-height` then applies to the rounded height and rounds
/// again, matching G-Spot's Ghostty projection. The returned font carries the
/// bundled Symbols Nerd Font fallback used by the paint path.
fn measure_cell_metrics(
    window: &mut Window,
    font_family: &str,
    font_size: f32,
    line_height_adjust_percent: f32,
) -> Option<(CellMetrics, Font)> {
    if !font_size.is_finite() || font_size <= 0.0 || !line_height_adjust_percent.is_finite() {
        return None;
    }
    let mut terminal_font = font(font_family);
    terminal_font.fallbacks = Some(FontFallbacks::from_fonts(vec![
        SYMBOLS_FONT_FAMILY.to_owned(),
    ]));
    let text_system = window.text_system();
    let font_id = text_system.resolve_font(&terminal_font);
    let size = px(font_size);
    let mut face_width: Option<f32> = None;
    for ch in ' '..='~' {
        if let Ok(advance) = text_system.advance(font_id, size, ch) {
            let width = f32::from(advance.width);
            face_width = Some(face_width.map_or(width, |current| current.max(width)));
        }
    }
    let face_width = face_width.unwrap_or_else(|| {
        text_system
            .ch_advance(font_id, size)
            .or_else(|_| text_system.em_advance(font_id, size))
            .map_or(0.6 * font_size, f32::from)
    });
    let face_height = f32::from(text_system.ascent(font_id, size))
        + f32::from(text_system.descent(font_id, size)).abs();
    let metrics = rounded_cell_metrics(
        face_width,
        face_height,
        window.scale_factor(),
        line_height_adjust_percent,
    )?;
    Some((metrics, terminal_font))
}

fn rounded_cell_metrics(
    face_width: f32,
    face_height: f32,
    scale_factor: f32,
    line_height_adjust_percent: f32,
) -> Option<CellMetrics> {
    if !face_width.is_finite()
        || !face_height.is_finite()
        || !scale_factor.is_finite()
        || scale_factor <= 0.0
        || !line_height_adjust_percent.is_finite()
    {
        return None;
    }
    let width_device = (face_width * scale_factor).round().max(1.0);
    let base_height_device = (face_height * scale_factor).round();
    let adjusted_percent = line_height_adjust_percent.clamp(-90.0, 500.0);
    let height_device = (base_height_device * (1.0 + adjusted_percent / 100.0))
        .round()
        .max(1.0);
    CellMetrics::new(width_device / scale_factor, height_device / scale_factor)
}
/// Convert Ghostty/G-Spot logical-pixel padding into integral geometry.
/// Each side is rounded once; non-finite or negative input is invalid and
/// values above `u16::MAX` saturate.
fn settings_padding(padding_x: f32, padding_y: f32) -> Option<PaddingPx> {
    if !padding_x.is_finite() || !padding_y.is_finite() || padding_x < 0.0 || padding_y < 0.0 {
        return None;
    }
    let x = padding_x.round().min(f32::from(u16::MAX)) as u16;
    let y = padding_y.round().min(f32::from(u16::MAX)) as u16;
    Some(PaddingPx::new(x, x, y, y))
}

fn input_modifiers(modifiers: GpuiModifiers) -> InputModifiers {
    InputModifiers {
        shift: modifiers.shift,
        alt: modifiers.alt,
        ctrl: modifiers.control,
        super_: modifiers.platform,
    }
}

fn input_mouse_button(button: GpuiMouseButton) -> InputMouseButton {
    match button {
        GpuiMouseButton::Left => InputMouseButton::Left,
        GpuiMouseButton::Right => InputMouseButton::Right,
        GpuiMouseButton::Middle => InputMouseButton::Middle,
        GpuiMouseButton::Navigate(gpui::NavigationDirection::Back) => InputMouseButton::Four,
        GpuiMouseButton::Navigate(gpui::NavigationDirection::Forward) => InputMouseButton::Five,
    }
}

struct MouseRoute {
    pane_id: PaneId,
    geometry: SurfaceGeometry,
    local_x: f32,
    local_y: f32,
    button: Option<InputMouseButton>,
    action: InputMouseAction,
    modifiers: GpuiModifiers,
    click_count: usize,
}

fn route_mouse(model: &Entity<AppModel>, route: MouseRoute, cx: &mut App) {
    model.update(cx, |model, _| {
        if route.action == InputMouseAction::Press {
            model.focus_pane(route.pane_id);
        }
        let Some((window_id, tab_id)) = model.locate_pane(route.pane_id) else {
            return;
        };
        let mut bytes = model
            .window(window_id)
            .and_then(|window| window.tabs.get(&tab_id))
            .and_then(|tab| tab.pane(route.pane_id))
            .map(|pane| {
                encode_live_mouse(
                    &pane.core,
                    &route.geometry,
                    route.local_x,
                    route.local_y,
                    route.button,
                    route.action,
                    input_modifiers(route.modifiers),
                )
            })
            .unwrap_or_default();
        let selecting = (bytes.is_empty() || route.modifiers.shift)
            && route.button == Some(InputMouseButton::Left);
        if selecting {
            bytes.clear();
            let (col, row) = crate::ui::input_surface::surface_cell(
                &route.geometry,
                route.local_x,
                route.local_y,
            );
            if let Some(pane) = model
                .window_mut(window_id)
                .and_then(|window| window.tabs.get_mut(&tab_id))
                .and_then(|tab| tab.pane_mut(route.pane_id))
            {
                match route.action {
                    InputMouseAction::Press => {
                        let gesture = if route.modifiers.alt {
                            SelectionGesture::Block
                        } else if route.click_count >= 3 {
                            SelectionGesture::Line
                        } else if route.click_count == 2 {
                            SelectionGesture::Word
                        } else {
                            SelectionGesture::Cell
                        };
                        pane.begin_selection(row, col, gesture);
                    }
                    InputMouseAction::Motion => {
                        if route.local_y < 0.0 {
                            pane.scroll_viewport_up(1);
                        } else if route.local_y >= route.geometry.content.height {
                            pane.scroll_viewport_down(1);
                        }
                        pane.update_selection(row, col);
                    }
                    InputMouseAction::Release => {}
                }
            }
        }
        if !bytes.is_empty() {
            model.write_to_pane(route.pane_id, &bytes);
        }
    });
}

fn route_dock_mouse(model: &Entity<AppModel>, route: DockMouseRoute, cx: &mut App) {
    let hit = hit_test_dock(
        &route.layout,
        PointF {
            x: route.window_x,
            y: route.window_y,
        },
    );
    if dock_hit_consumes(hit) {
        return;
    }
    let Some((local_x, local_y)) = remap_pointer(&route.layout.map, hit) else {
        return;
    };
    route_mouse(
        model,
        MouseRoute {
            pane_id: route.pane_id,
            geometry: route.geometry,
            local_x,
            local_y,
            button: route.button,
            action: route.action,
            modifiers: route.modifiers,
            click_count: route.click_count,
        },
        cx,
    );
}
fn route_mouse_down(
    model: &Entity<AppModel>,
    pane_id: PaneId,
    geometry: SurfaceGeometry,
    left: f32,
    top: f32,
    event: &MouseDownEvent,
    cx: &mut App,
) {
    route_mouse(
        model,
        MouseRoute {
            pane_id,
            geometry,
            local_x: f32::from(event.position.x) - left,
            local_y: f32::from(event.position.y) - top,
            button: Some(input_mouse_button(event.button)),
            action: InputMouseAction::Press,
            modifiers: event.modifiers,
            click_count: event.click_count,
        },
        cx,
    );
}

fn route_mouse_up(
    model: &Entity<AppModel>,
    pane_id: PaneId,
    geometry: SurfaceGeometry,
    left: f32,
    top: f32,
    event: &MouseUpEvent,
    cx: &mut App,
) {
    route_mouse(
        model,
        MouseRoute {
            pane_id,
            geometry,
            local_x: f32::from(event.position.x) - left,
            local_y: f32::from(event.position.y) - top,
            button: Some(input_mouse_button(event.button)),
            action: InputMouseAction::Release,
            modifiers: event.modifiers,
            click_count: 0,
        },
        cx,
    );
}

fn route_mouse_move(
    model: &Entity<AppModel>,
    pane_id: PaneId,
    geometry: SurfaceGeometry,
    left: f32,
    top: f32,
    event: &MouseMoveEvent,
    cx: &mut App,
) {
    route_mouse(
        model,
        MouseRoute {
            pane_id,
            geometry,
            local_x: f32::from(event.position.x) - left,
            local_y: f32::from(event.position.y) - top,
            button: event.pressed_button.map(input_mouse_button),
            action: InputMouseAction::Motion,
            modifiers: event.modifiers,
            click_count: 0,
        },
        cx,
    );
}

fn route_scroll(
    model: &Entity<AppModel>,
    shell: &WeakEntity<AppShell>,
    pane_id: PaneId,
    geometry: SurfaceGeometry,
    origin: PointF,
    event: &ScrollWheelEvent,
    cx: &mut App,
) {
    let delta = match event.delta {
        ScrollDelta::Pixels(delta) => f32::from(delta.y) / geometry.metrics.height.max(1.0),
        ScrollDelta::Lines(delta) => delta.y,
    };
    if delta == 0.0 {
        return;
    }
    let button = if delta > 0.0 {
        InputMouseButton::Four
    } else {
        InputMouseButton::Five
    };
    let lines = delta.abs().ceil().clamp(1.0, 16.0) as usize;
    let mouse = MouseRoute {
        pane_id,
        geometry,
        local_x: f32::from(event.position.x) - origin.x,
        local_y: f32::from(event.position.y) - origin.y,
        button: Some(button),
        action: InputMouseAction::Press,
        modifiers: event.modifiers,
        click_count: 0,
    };
    let offset_changed = model.update(cx, |model, _| {
        let Some((window_id, tab_id)) = model.locate_pane(pane_id) else {
            return false;
        };
        let bytes = model
            .window(window_id)
            .and_then(|window| window.tabs.get(&tab_id))
            .and_then(|tab| tab.pane(pane_id))
            .map(|pane| {
                let mut bytes = Vec::new();
                for _ in 0..lines {
                    bytes.extend_from_slice(&encode_live_mouse(
                        &pane.core,
                        &mouse.geometry,
                        mouse.local_x,
                        mouse.local_y,
                        mouse.button,
                        mouse.action,
                        input_modifiers(mouse.modifiers),
                    ));
                }
                bytes
            })
            .unwrap_or_default();
        if !bytes.is_empty() {
            model.write_to_pane(pane_id, &bytes);
            return false;
        }
        let Some(pane) = model
            .window_mut(window_id)
            .and_then(|window| window.tabs.get_mut(&tab_id))
            .and_then(|tab| tab.pane_mut(pane_id))
        else {
            return false;
        };
        let before = pane.viewport_offset();
        if delta > 0.0 {
            pane.scroll_viewport_up(lines);
        } else {
            pane.scroll_viewport_down(lines);
        }
        pane.viewport_offset() != before
    });
    if offset_changed {
        let _ = shell.update(cx, |shell, cx| shell.refresh_windows(cx));
    }
}

fn route_drop_paths(
    model: &Entity<AppModel>,
    pane_id: PaneId,
    paths: &ExternalPaths,
    cx: &mut App,
) {
    let paths = sanitize_drop_paths(paths.paths());
    if paths.is_empty() {
        return;
    }
    let mut bytes = Vec::new();
    encode_drop_paths(&paths, &mut bytes);
    if bytes.is_empty() {
        return;
    }
    model.update(cx, |model, _| {
        model.focus_pane(pane_id);
        model.write_to_pane(pane_id, &bytes);
    });
}

fn chat_overlay_height(pane_height: f32, dock_chrome_active: bool) -> f32 {
    let reserved = if dock_chrome_active {
        CHROME_TOTAL
    } else {
        0.0
    };
    (pane_height - reserved).max(0.0)
}

fn chat_overlay(
    events: &[ConversationEvent],
    terminal_palette: TerminalPalette,
    pane_left: f32,
    pane_top: f32,
    pane_width: f32,
    pane_height: f32,
) -> impl gpui::IntoElement {
    let is_light = terminal_palette.background[0] > 0x80;
    let panel: gpui::Hsla = if is_light {
        gpui::rgba(0xfafa_faff).into()
    } else {
        gpui::rgba(0x2424_24ff).into()
    };
    let foreground: gpui::Hsla = if is_light {
        gpui::rgb(0x202020).into()
    } else {
        gpui::rgb(0xe5e5e5).into()
    };
    let mut list = gpui::div()
        .absolute()
        .left(gpui::px(pane_left))
        .top(gpui::px(pane_top))
        .w(gpui::px(pane_width.max(0.0)))
        .h(gpui::px(pane_height.max(0.0)))
        .flex()
        .flex_col()
        .gap(gpui::px(6.0))
        .p(gpui::px(18.0))
        .bg(panel)
        .text_color(foreground)
        .id(gpui::ElementId::Name(gpui::SharedString::from(
            "chat-overlay",
        )))
        .role(gpui::Role::Region)
        .occlude();
    for ev in events {
        list = list.child(
            gpui::div()
                .id(gpui::ElementId::Name(gpui::SharedString::from(format!(
                    "chat-event-{}",
                    ev.id
                ))))
                .w_full()
                .p(gpui::px(4.0))
                .rounded(gpui::px(4.0))
                .role(gpui::Role::ListItem)
                .child(ev.text.clone()),
        );
    }
    list
}

/// The command-palette overlay: a popover listing the current search
/// results. Navigation is keyboard-driven (`palette_key`), matching the
/// keyboard-only-operation contract.
fn palette_overlay(palette: &PaletteState, terminal_palette: TerminalPalette) -> impl IntoElement {
    let is_light = terminal_palette.background[0] > 0x80;
    let panel: gpui::Hsla = if is_light {
        gpui::rgba(0xfafa_faff).into()
    } else {
        gpui::rgba(0x2424_24ff).into()
    };
    let foreground: gpui::Hsla = if is_light {
        gpui::rgb(0x202020).into()
    } else {
        gpui::rgb(0xe5e5e5).into()
    };
    let border: gpui::Hsla = if is_light {
        gpui::rgba(0x2020_2033).into()
    } else {
        gpui::rgba(0xe5e5_e533).into()
    };
    let selected = terminal_palette.selection_color();
    let mut list = div()
        .absolute()
        .top(px(8.0))
        .left(px(8.0))
        .right(px(8.0))
        .flex()
        .flex_col()
        .gap(px(4.0))
        .p(px(8.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(border)
        .bg(panel)
        .text_color(foreground)
        .id(ElementId::Name(SharedString::from("command-palette")))
        .role(Role::Menu);
    for (index, result) in palette.results.iter().enumerate() {
        let marker = if index == palette.selection {
            "> "
        } else {
            "  "
        };
        let label = format!("{marker}{}", result.title);
        let mut item = div()
            .id(ElementId::Name(SharedString::from(format!(
                "palette-item-{}",
                result.id
            ))))
            .w_full()
            .p(px(4.0))
            .rounded(px(4.0))
            .role(Role::ListItem)
            .child(label);
        if index == palette.selection {
            item = item.bg(selected);
        }
        list = list.child(item);
    }
    list
}

/// Route one key event: keymap action, palette, or terminal bytes.
fn handle_key_event(
    model: &Entity<AppModel>,
    shell: &WeakEntity<AppShell>,
    event: &KeyDownEvent,
    window: &mut Window,
    cx: &mut App,
) {
    let mut refresh_immediately = false;
    let is_copy = event.keystroke.modifiers.platform
        && !event.keystroke.modifiers.control
        && !event.keystroke.modifiers.alt
        && event.keystroke.key.eq_ignore_ascii_case("c");
    let mut copied_text = None;
    let page_scroll = (event.keystroke.modifiers.shift
        && !event.keystroke.modifiers.control
        && !event.keystroke.modifiers.alt
        && !event.keystroke.modifiers.platform)
        .then_some(match event.keystroke.key.as_str() {
            "pageup" | "up" => Some(true),
            "pagedown" | "down" => Some(false),
            _ => None,
        })
        .flatten();
    let is_paste = event.keystroke.modifiers.platform
        && !event.keystroke.modifiers.control
        && !event.keystroke.modifiers.alt
        && event.keystroke.key.eq_ignore_ascii_case("v");
    let paste_text = is_paste
        .then(|| cx.read_from_clipboard().and_then(|item| item.text()))
        .flatten();
    model.update(cx, |shell_model, model_cx| {
        if shell_model.palette.is_open() {
            if event.keystroke.modifiers.modified() {
                let keystroke = shell_keystroke(&event.keystroke);
                if let Some(action) = shell_model.keymap_resolver().resolve(&keystroke, "") {
                    shell_model.dispatch(action);
                    refresh_immediately = true;
                    model_cx.stop_propagation();
                    return;
                }
            }
            shell_model.palette_key(
                &event.keystroke.key,
                printable_text(&event.keystroke).as_deref(),
            );
            refresh_immediately = true;
            model_cx.stop_propagation();
            return;
        }
        if is_copy {
            copied_text = shell_model
                .focused_pane_mut()
                .and_then(crate::model::pane::PaneModel::selected_text);
            model_cx.stop_propagation();
            return;
        }
        if let Some(up) = page_scroll {
            if let Some(pane) = shell_model.focused_pane_mut() {
                let lines = usize::from(pane.last_size.rows).saturating_sub(1).max(1);
                if up {
                    pane.scroll_viewport_up(lines);
                } else {
                    pane.scroll_viewport_down(lines);
                }
            }
            refresh_immediately = true;
            model_cx.stop_propagation();
            return;
        }
        if is_paste {
            if let Some(text) = paste_text.as_deref()
                && let Some(pane_id) = shell_model.focused_pane_id()
            {
                let bytes = shell_model
                    .focused_pane()
                    .map(|pane| encode_live_paste(&pane.core, text))
                    .unwrap_or_default();
                if !bytes.is_empty() {
                    if let Some(pane) = shell_model.focused_pane_mut()
                        && pane.viewport_offset() > 0
                    {
                        pane.scroll_viewport_down(usize::MAX);
                    }
                    shell_model.write_to_pane(pane_id, &bytes);
                }
            }
            model_cx.stop_propagation();
            return;
        }
        let keystroke = shell_keystroke(&event.keystroke);
        if let Some(action) = shell_model.keymap_resolver().resolve(&keystroke, "") {
            shell_model.dispatch(action);
            refresh_immediately = true;
            model_cx.stop_propagation();
            return;
        }
        let action = if event.is_held {
            InputKeyAction::Repeat
        } else {
            InputKeyAction::Press
        };
        if let Some(pane_id) = shell_model.focused_pane_id() {
            if let Some(bytes) = command_backspace_bytes(&event.keystroke, action) {
                write_terminal_bytes(shell_model, pane_id, bytes);
                model_cx.stop_propagation();
                return;
            }
            if let Some(input) = to_input_key_event(&event.keystroke, action) {
                let bytes = shell_model
                    .focused_pane()
                    .map(|pane| encode_live_key(&pane.core, &input))
                    .unwrap_or_default();
                write_terminal_bytes(shell_model, pane_id, &bytes);
                if !bytes.is_empty() {
                    model_cx.stop_propagation();
                }
            }
        }
    });
    if let Some(text) = copied_text {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }
    if refresh_immediately {
        let should_quit = shell
            .update(cx, |shell, cx| {
                shell.sync_windows(cx);
                cx.refresh_windows();
                shell.model.read(cx).should_quit()
            })
            .unwrap_or_else(|_| model.read(cx).should_quit());
        window.refresh();
        if should_quit {
            cx.quit();
        }
    }
}

fn handle_key_release(model: &Entity<AppModel>, event: &KeyUpEvent, cx: &mut App) {
    model.update(cx, |shell_model, cx| {
        if shell_model.palette.is_open() {
            cx.stop_propagation();
            return;
        }
        let Some(pane_id) = shell_model.focused_pane_id() else {
            return;
        };
        let Some(input) = to_input_key_event(&event.keystroke, InputKeyAction::Release) else {
            return;
        };
        let bytes = shell_model
            .focused_pane()
            .map(|pane| encode_live_key(&pane.core, &input))
            .unwrap_or_default();
        if !bytes.is_empty() {
            shell_model.write_to_pane(pane_id, &bytes);
            cx.stop_propagation();
        }
    });
}

fn write_terminal_bytes(shell_model: &mut AppModel, pane_id: PaneId, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    if let Some(pane) = shell_model.focused_pane_mut()
        && pane.viewport_offset() > 0
    {
        pane.scroll_viewport_down(usize::MAX);
    }
    shell_model.write_to_pane(pane_id, bytes);
}

/// Convert a GPUI keystroke to the shell keystroke string used by the
/// keymap resolver (`ctrl+cmd+up`).
fn shell_keystroke(keystroke: &Keystroke) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if keystroke.modifiers.control {
        parts.push("ctrl");
    }
    if keystroke.modifiers.alt {
        parts.push("alt");
    }
    if keystroke.modifiers.shift {
        parts.push("shift");
    }
    if keystroke.modifiers.platform {
        parts.push("cmd");
    }
    parts.push(&keystroke.key);
    parts.join("+")
}

/// The printable text a keystroke produces, if any.
fn printable_text(keystroke: &Keystroke) -> Option<String> {
    if let Some(text) = &keystroke.key_char {
        return Some(text.clone());
    }
    if keystroke.key.chars().count() == 1 {
        return Some(keystroke.key.clone());
    }
    None
}

/// Printable text is committed by GPUI's text-input client. Keeping it off
/// the physical key path prevents one keystroke from reaching the PTY twice.
fn text_input_owns_keystroke(keystroke: &Keystroke) -> bool {
    // GPUI tags Enter/Tab with key_char "\n"/"\t". Those are not printable
    // text: Kitty/Alacritty/xterm send CR for Return, HT for Tab.
    !keystroke.modifiers.control
        && !keystroke.modifiers.alt
        && !keystroke.modifiers.platform
        && !keystroke.modifiers.function
        && !matches!(
            keystroke.key.as_str(),
            "enter" | "return" | "tab" | "backspace"
        )
        && printable_text(keystroke).is_some()
}

/// macOS Command-Backspace means delete to the beginning of the line. Shells
/// expose that readline operation as Ctrl-U.
fn command_backspace_bytes(keystroke: &Keystroke, action: InputKeyAction) -> Option<&'static [u8]> {
    (action != InputKeyAction::Release
        && keystroke.modifiers.platform
        && !keystroke.modifiers.control
        && !keystroke.modifiers.alt
        && !keystroke.modifiers.shift
        && !keystroke.modifiers.function
        && keystroke.key.eq_ignore_ascii_case("backspace"))
    .then_some(b"\x15")
}

/// Map a GPUI keystroke onto the S5 input encoder's key event.
fn to_input_key_event(keystroke: &Keystroke, action: InputKeyAction) -> Option<InputKeyEvent> {
    if action != InputKeyAction::Release && text_input_owns_keystroke(keystroke) {
        return None;
    }
    let key = crate::ui::input_surface::map_key(&keystroke.key);
    let mods = InputModifiers {
        shift: keystroke.modifiers.shift,
        alt: keystroke.modifiers.alt,
        ctrl: keystroke.modifiers.control,
        super_: keystroke.modifiers.platform,
    };
    let text = printable_text(keystroke).unwrap_or_default();
    Some(InputKeyEvent {
        key,
        mods,
        consumed_mods: InputModifiers::NONE,
        composing: false,
        utf8: text,
        unshifted_codepoint: keystroke
            .key
            .chars()
            .next()
            .map(|ch| ch as u32)
            .unwrap_or(0),
        action,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext as _, Modifiers};
    use mr_crabs_terminal::GridSize;

    fn keystroke(key: &str, modifiers: gpui::Modifiers) -> Keystroke {
        Keystroke {
            key: key.to_string(),
            key_char: None,
            modifiers,
        }
    }

    #[test]
    fn native_window_title_keeps_rust_identifier() {
        assert_eq!(WINDOW_TITLE_PREFIX, "Mr Crabs");
        assert_eq!(prefixed_window_title("shell"), "Mr Crabs — shell");
    }

    #[test]
    fn gspot_metrics_round_in_device_pixels_then_adjust_height() {
        let metrics = rounded_cell_metrics(10.2, 23.760_132, 2.0, 5.0).expect("valid metrics");
        assert_eq!(
            metrics,
            CellMetrics::new(10.0, 25.0).expect("expected metrics")
        );
    }

    #[test]
    fn settings_padding_uses_logical_pixels_once() {
        let metrics = CellMetrics::new(10.0, 20.0).expect("valid metrics");
        let padding = settings_padding(10.0, 10.0).expect("valid padding");
        assert_eq!(padding, PaddingPx::new(10, 10, 10, 10));

        // Ten logical pixels on each side converts an 800x480 viewport to a
        // 780x460 content area and a 78x23 grid.
        let geometry = SurfaceGeometry::from_viewport(
            PixelExtent {
                width: 800.0,
                height: 480.0,
            },
            metrics,
            padding,
        )
        .expect("derivable geometry");
        assert_eq!(
            geometry.content,
            PixelExtent {
                width: 780.0,
                height: 460.0,
            }
        );
        assert_eq!(geometry.grid, GridSize::new(78, 23));

        // Non-finite or negative pixel padding is rejected; oversized
        // values saturate at u16::MAX.
        assert_eq!(settings_padding(f32::NAN, 1.0), None);
        assert_eq!(settings_padding(1.0, f32::NEG_INFINITY), None);
        assert_eq!(settings_padding(-1.0, 1.0), None);
        assert_eq!(
            settings_padding(70_000.0, 1.0),
            Some(PaddingPx::new(u16::MAX, u16::MAX, 1, 1))
        );
    }

    #[test]
    fn terminal_element_ids_are_stable_and_distinct_per_pane() {
        // Same pane: identical ID on every render, so `with_element_state`
        // retained paint state survives element reconstruction.
        let first = terminal_element_id(PaneId::new(7));
        assert_eq!(terminal_element_id(PaneId::new(7)), first);
        // Different panes never share an element identity.
        assert_ne!(first, terminal_element_id(PaneId::new(8)));
        assert_ne!(
            terminal_element_id(PaneId::new(8)),
            terminal_element_id(PaneId::new(9))
        );
        // The documented stable shape: the static namespace plus the pane
        // ID — no formatted string built per frame.
        match first {
            ElementId::NamedInteger(name, id) => {
                assert_eq!(name, SharedString::from(TERMINAL_ELEMENT_NAME));
                assert_eq!(id, 7);
            }
            _ => panic!("terminal element ids must be NamedInteger(name, pane)"),
        }
    }

    #[test]
    fn shell_keystroke_renders_modifier_order() {
        let modifiers = Modifiers {
            control: true,
            platform: true,
            ..Modifiers::default()
        };
        let ks = keystroke("right", modifiers);
        assert_eq!(shell_keystroke(&ks), "ctrl+cmd+right");
        let ks = keystroke("t", Modifiers::default());
        assert_eq!(shell_keystroke(&ks), "t");
    }

    #[test]
    fn printable_text_uses_key_char_then_single_char_key() {
        let mut ks = keystroke("s", Modifiers::default());
        ks.key_char = Some("ß".to_string());
        assert_eq!(printable_text(&ks).as_deref(), Some("ß"));
        let ks = keystroke("t", Modifiers::default());
        assert_eq!(printable_text(&ks).as_deref(), Some("t"));
        let ks = keystroke("enter", Modifiers::default());
        assert_eq!(printable_text(&ks), None);
    }

    #[test]
    fn input_key_event_maps_named_keys_and_modifiers() {
        let modifiers = Modifiers {
            control: true,
            shift: true,
            ..Modifiers::default()
        };
        let event =
            to_input_key_event(&keystroke("up", modifiers), InputKeyAction::Press).expect("event");
        assert_eq!(event.key, Key::ArrowUp);
        assert!(event.mods.ctrl && event.mods.shift);
        assert_eq!(event.action, InputKeyAction::Press);

        let event = to_input_key_event(
            &keystroke(
                "t",
                Modifiers {
                    control: true,
                    ..Modifiers::default()
                },
            ),
            InputKeyAction::Repeat,
        )
        .expect("event");
        assert_eq!(event.key, Key::Character('t'));
        assert!(event.mods.ctrl);
        assert_eq!(event.action, InputKeyAction::Repeat);

        let event = to_input_key_event(
            &keystroke("f13", Modifiers::default()),
            InputKeyAction::Release,
        )
        .expect("event");
        assert_eq!(event.key, Key::F(13));
        assert_eq!(event.action, InputKeyAction::Release);

        let event = to_input_key_event(
            &keystroke("unknown-key", Modifiers::default()),
            InputKeyAction::Press,
        )
        .expect("event");
        assert_eq!(event.key, Key::Unidentified);
    }

    #[test]
    fn printable_press_and_repeat_use_only_the_text_input_path() {
        for action in [InputKeyAction::Press, InputKeyAction::Repeat] {
            let plain = keystroke("a", Modifiers::default());
            assert!(to_input_key_event(&plain, action).is_none());

            let mut shifted = keystroke(
                "a",
                Modifiers {
                    shift: true,
                    ..Modifiers::default()
                },
            );
            shifted.key_char = Some("A".to_string());
            assert!(to_input_key_event(&shifted, action).is_none());
        }

        let control = keystroke(
            "w",
            Modifiers {
                control: true,
                ..Modifiers::default()
            },
        );
        assert!(to_input_key_event(&control, InputKeyAction::Press).is_some());

        let option_backspace = keystroke(
            "backspace",
            Modifiers {
                alt: true,
                ..Modifiers::default()
            },
        );
        assert!(to_input_key_event(&option_backspace, InputKeyAction::Press).is_some());
    }

    #[test]
    fn gpui_enter_with_newline_key_char_uses_the_key_path() {
        let mut enter = keystroke("enter", Modifiers::default());
        enter.key_char = Some("\n".to_string());
        let event = to_input_key_event(&enter, InputKeyAction::Press).expect("enter");
        assert_eq!(event.key, Key::Enter);

        let mut tab = keystroke("tab", Modifiers::default());
        tab.key_char = Some("\t".to_string());
        let event = to_input_key_event(&tab, InputKeyAction::Press).expect("tab");
        assert_eq!(event.key, Key::Tab);
    }

    #[test]
    fn command_backspace_is_one_ctrl_u_on_press_or_repeat() {
        let command_backspace = keystroke(
            "backspace",
            Modifiers {
                platform: true,
                ..Modifiers::default()
            },
        );
        assert_eq!(
            command_backspace_bytes(&command_backspace, InputKeyAction::Press),
            Some(&b"\x15"[..])
        );
        assert_eq!(
            command_backspace_bytes(&command_backspace, InputKeyAction::Repeat),
            Some(&b"\x15"[..])
        );
        assert_eq!(
            command_backspace_bytes(&command_backspace, InputKeyAction::Release),
            None
        );
        assert_eq!(
            command_backspace_bytes(
                &keystroke("backspace", Modifiers::default()),
                InputKeyAction::Press,
            ),
            None
        );
    }

    #[test]
    fn chat_overlay_reserves_dock_chrome_below_transcript() {
        let pane_height = 480.0;
        let reserved = chat_overlay_height(pane_height, true);
        let unreserved = chat_overlay_height(pane_height, false);
        assert_eq!(unreserved, pane_height);
        assert_eq!(reserved, pane_height - CHROME_TOTAL);
        assert!(reserved + CHROME_TOTAL <= pane_height);
        assert_eq!(chat_overlay_height(50.0, true), 0.0);
    }
    #[gpui::test]
    fn palette_printable_keys_do_not_leak_to_pty_writer(cx: &mut gpui::TestAppContext) {
        use crate::model::app_model::AppModel;
        use crate::model::pane::PaneSession;
        use crate::ui::shell::AppShell;
        use gpui::Keystroke;
        use std::sync::mpsc::sync_channel;

        let (model, shell, writer_rx, _reader_tx) = cx.update(|cx| {
            let model = cx.new(|_| AppModel::headless());
            let window_id = model.read(cx).active_window.expect("active window");
            let pane_id = model.read(cx).focused_pane_id().expect("focused pane");
            let size = model.read(cx).windows[&window_id]
                .active_tab()
                .expect("active tab")
                .panes[&pane_id]
                .last_size;
            let (reader_tx, reader_rx) = sync_channel::<Vec<u8>>(8);
            let (writer_tx, writer_rx) = sync_channel::<Vec<u8>>(8);
            model.update(cx, |model, _| {
                let pane = model
                    .windows
                    .get_mut(&window_id)
                    .unwrap()
                    .active_tab_mut()
                    .unwrap()
                    .panes
                    .get_mut(&pane_id)
                    .unwrap();
                pane.session = PaneSession::from_receivers_with_writer(
                    size,
                    Some(reader_rx),
                    None,
                    Some(writer_tx),
                );
            });
            let shell = cx.new(|_| AppShell::new(model.clone()));
            shell.update(cx, |shell, cx| shell.sync_windows(cx));
            (model, shell, writer_rx, reader_tx)
        });
        // Keep shell alive for window lifecycle.
        let _keep_shell = shell;

        let handle = cx.windows().into_iter().next().expect("one window");
        // Force draw so WindowView key listeners and terminal input handler are installed.
        cx.update_window(handle, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();

        let assert_writer_empty = |rx: &std::sync::mpsc::Receiver<Vec<u8>>| {
            assert!(
                rx.try_recv().is_err(),
                "PTY writer must stay empty while palette is open"
            );
        };

        // Cmd+Shift+P opens the palette.
        cx.dispatch_keystroke(handle, Keystroke::parse("cmd-shift-p").unwrap());
        assert!(
            cx.update(|cx| model.read(cx).palette.is_open()),
            "palette should be open after cmd-shift-p"
        );
        cx.update_window(handle, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();
        assert_writer_empty(&writer_rx);

        // Printable `a` updates palette query but must not leak to PTY writer.
        cx.dispatch_keystroke(handle, Keystroke::parse("a").unwrap());
        cx.update_window(handle, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();
        assert_eq!(
            cx.update(|cx| model.read(cx).palette.query.clone()),
            "a",
            "palette query should be 'a'"
        );
        assert_writer_empty(&writer_rx);

        // Down mutates palette selection but must not leak.
        let sel_before = cx.update(|cx| model.read(cx).palette.selection);
        cx.dispatch_keystroke(handle, Keystroke::parse("down").unwrap());
        cx.update_window(handle, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();
        assert!(
            cx.update(|cx| model.read(cx).palette.is_open()),
            "palette should stay open after Down"
        );
        assert_eq!(
            cx.update(|cx| model.read(cx).palette.query.clone()),
            "a",
            "Down must not change query"
        );
        if cx.update(|cx| model.read(cx).palette.results.len() > 1) {
            let sel_after = cx.update(|cx| model.read(cx).palette.selection);
            assert_ne!(sel_before, sel_after, "Down should move selection");
        }
        assert_writer_empty(&writer_rx);

        // Backspace mutates palette state but must not leak.
        cx.dispatch_keystroke(handle, Keystroke::parse("backspace").unwrap());
        cx.update_window(handle, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();
        assert_eq!(
            cx.update(|cx| model.read(cx).palette.query.clone()),
            "",
            "Backspace should clear query"
        );
        assert_writer_empty(&writer_rx);

        // Escape closes the palette.
        cx.dispatch_keystroke(handle, Keystroke::parse("escape").unwrap());
        cx.update_window(handle, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();
        assert!(
            !cx.update(|cx| model.read(cx).palette.is_open()),
            "Escape should close palette"
        );
        assert_writer_empty(&writer_rx);

        // After close, printable `b` must reach the PTY writer (IME drain needs a draw).
        cx.dispatch_keystroke(handle, Keystroke::parse("b").unwrap());
        cx.update_window(handle, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();
        cx.update_window(handle, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();
        let bytes = writer_rx
            .try_recv()
            .expect("b must reach PTY writer after palette closed");
        assert!(!bytes.is_empty(), "writer bytes must be non-empty");
        assert!(
            bytes.contains(&b'b'),
            "writer should contain b, got {bytes:?}"
        );

        cx.update(|cx| {
            model.update(cx, |model, _| {
                if let Some(pane) = model.focused_pane_mut() {
                    pane.core
                        .feed_terminal_output(b"\x1b[=2u")
                        .expect("workspace fixture feed should succeed");
                }
            });
            model.update(cx, |model, _| {
                model.dispatch(crate::action::AppAction::TogglePalette);
            });
        });
        assert!(
            cx.update(|cx| model.read(cx).palette.is_open()),
            "palette should be open after TogglePalette with ReportEventTypes enabled"
        );
        cx.update(|cx| {
            let event = gpui::KeyUpEvent {
                keystroke: gpui::Keystroke::parse("a").unwrap(),
            };
            handle_key_release(&model, &event, cx);
        });
        assert_writer_empty(&writer_rx);

        cx.update(|cx| {
            model.update(cx, |model, _| {
                model.dispatch(crate::action::AppAction::TogglePalette);
            });
        });
        assert!(
            !cx.update(|cx| model.read(cx).palette.is_open()),
            "palette should be closed after second TogglePalette"
        );
        cx.update(|cx| {
            let event = gpui::KeyUpEvent {
                keystroke: gpui::Keystroke::parse("a").unwrap(),
            };
            handle_key_release(&model, &event, cx);
        });
        let release_bytes = writer_rx.try_recv().expect(
            "KeyUp release must reach PTY writer after palette closed with ReportEventTypes",
        );
        assert!(
            !release_bytes.is_empty(),
            "release bytes must be non-empty, got {release_bytes:?}"
        );
    }

    #[gpui::test]
    fn cmd_shift_j_toggles_chat_once_without_pty_leak(cx: &mut gpui::TestAppContext) {
        use crate::model::app_model::AppModel;
        use crate::model::pane::PaneSession;
        use crate::model::presentation::SurfaceMode;
        use crate::ui::shell::AppShell;
        use gpui::Keystroke;
        use std::sync::mpsc::sync_channel;

        let (model, shell, writer_rx, _reader_tx) = cx.update(|cx| {
            let model = cx.new(|_| AppModel::headless());
            let window_id = model.read(cx).active_window.expect("active window");
            let pane_id = model.read(cx).focused_pane_id().expect("focused pane");
            let size = model.read(cx).windows[&window_id]
                .active_tab()
                .expect("active tab")
                .panes[&pane_id]
                .last_size;
            let (reader_tx, reader_rx) = sync_channel::<Vec<u8>>(8);
            let (writer_tx, writer_rx) = sync_channel::<Vec<u8>>(8);
            model.update(cx, |model, _| {
                let pane = model
                    .windows
                    .get_mut(&window_id)
                    .unwrap()
                    .active_tab_mut()
                    .unwrap()
                    .panes
                    .get_mut(&pane_id)
                    .unwrap();
                pane.session = PaneSession::from_receivers_with_writer(
                    size,
                    Some(reader_rx),
                    None,
                    Some(writer_tx),
                );
            });
            let shell = cx.new(|_| AppShell::new(model.clone()));
            cx.bind_keys(crate::ui::actions::key_bindings(
                &crate::keymap::default_keybindings(),
            ));
            AppShell::register_actions(&shell, cx);
            shell.update(cx, |shell, cx| shell.sync_windows(cx));
            (model, shell, writer_rx, reader_tx)
        });
        let _keep_shell = shell;

        let handle = cx.windows().into_iter().next().expect("one window");
        cx.update_window(handle, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();

        cx.update(|cx| {
            model.update(cx, |model, _| {
                model
                    .focused_pane_mut()
                    .expect("pane")
                    .feed_test_output(b"\x1b]133;A\x07hello")
                    .expect("feed OSC133");
            });
        });

        let (generation_before, preferred_before) = cx.update(|cx| {
            let model = model.read(cx);
            let pane = model.focused_pane().expect("pane");
            (model.generation, pane.preferred_mode)
        });
        assert_eq!(
            preferred_before,
            SurfaceMode::Terminal,
            "idle eligible pane stays terminal until shortcut"
        );

        cx.dispatch_keystroke(handle, Keystroke::parse("cmd-shift-j").unwrap());
        cx.update_window(handle, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();
        assert!(
            writer_rx.try_recv().is_err(),
            "cmd-shift-j must not write PTY bytes"
        );
        let (generation_after_open, preferred_after_open, effective_after_open) = cx.update(|cx| {
            let model = model.read(cx);
            let pane = model.focused_pane().expect("pane");
            (
                model.generation,
                pane.preferred_mode,
                pane.effective_mode(false, false),
            )
        });
        assert_eq!(
            preferred_after_open,
            SurfaceMode::Chat,
            "first cmd-shift-j must prefer chat"
        );
        assert_eq!(
            effective_after_open,
            SurfaceMode::Chat,
            "first cmd-shift-j must toggle chat on"
        );
        assert_eq!(
            generation_after_open,
            generation_before + 1,
            "double dispatch would restore Terminal and bump generation by 2"
        );

        cx.dispatch_keystroke(handle, Keystroke::parse("cmd-shift-j").unwrap());
        cx.update_window(handle, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();
        assert!(
            writer_rx.try_recv().is_err(),
            "second cmd-shift-j must not write PTY bytes"
        );
        let (generation_after_close, preferred_after_close, effective_after_close) =
            cx.update(|cx| {
                let model = model.read(cx);
                let pane = model.focused_pane().expect("pane");
                (
                    model.generation,
                    pane.preferred_mode,
                    pane.effective_mode(false, false),
                )
            });
        assert_eq!(
            preferred_after_close,
            SurfaceMode::Terminal,
            "second cmd-shift-j must restore terminal preference"
        );
        assert_eq!(
            effective_after_close,
            SurfaceMode::Terminal,
            "second cmd-shift-j must restore terminal"
        );
        assert_eq!(
            generation_after_close,
            generation_before + 2,
            "each press must increment generation exactly once"
        );
    }

    type FakeWriterFixture = (
        gpui::Entity<AppModel>,
        gpui::Entity<AppShell>,
        std::sync::mpsc::Receiver<Vec<u8>>,
        std::sync::mpsc::SyncSender<Vec<u8>>,
    );

    fn attach_fake_writer(cx: &mut gpui::App) -> FakeWriterFixture {
        use crate::model::app_model::AppModel;
        use crate::model::pane::PaneSession;
        use crate::ui::shell::AppShell;
        use std::sync::mpsc::sync_channel;

        let model = cx.new(|_| AppModel::headless());
        let window_id = model.read(cx).active_window.expect("active window");
        let pane_id = model.read(cx).focused_pane_id().expect("focused pane");
        let size = model.read(cx).windows[&window_id]
            .active_tab()
            .expect("active tab")
            .panes[&pane_id]
            .last_size;
        let (reader_tx, reader_rx) = sync_channel::<Vec<u8>>(8);
        let (writer_tx, writer_rx) = sync_channel::<Vec<u8>>(8);
        model.update(cx, |model, _| {
            let pane = model
                .windows
                .get_mut(&window_id)
                .unwrap()
                .active_tab_mut()
                .unwrap()
                .panes
                .get_mut(&pane_id)
                .unwrap();
            pane.session = PaneSession::from_receivers_with_writer(
                size,
                Some(reader_rx),
                None,
                Some(writer_tx),
            );
        });
        let shell = cx.new(|_| AppShell::new(model.clone()));
        cx.bind_keys(crate::ui::actions::key_bindings(
            &crate::keymap::default_keybindings(),
        ));
        AppShell::register_actions(&shell, cx);
        shell.update(cx, |shell, cx| shell.sync_windows(cx));
        (model, shell, writer_rx, reader_tx)
    }

    fn drain_writer(rx: &std::sync::mpsc::Receiver<Vec<u8>>) -> Vec<u8> {
        let mut bytes = Vec::new();
        while let Ok(chunk) = rx.try_recv() {
            bytes.extend_from_slice(&chunk);
        }
        bytes
    }

    fn draw_window(cx: &mut gpui::TestAppContext, handle: gpui::AnyWindowHandle) {
        cx.update_window(handle, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();
    }

    #[gpui::test]
    fn ctrl_c_writes_etx_and_never_printable_c_after_ime_drain(cx: &mut gpui::TestAppContext) {
        let (model, shell, writer_rx, _reader_tx) = cx.update(attach_fake_writer);
        let _keep_shell = shell;
        let handle = cx.windows().into_iter().next().expect("one window");
        cx.dispatch_keystroke(handle, Keystroke::parse("ctrl-c->c").unwrap());

        draw_window(cx, handle);
        draw_window(cx, handle);

        let bytes = drain_writer(&writer_rx);
        assert_eq!(
            bytes,
            vec![0x03],
            "Ctrl-C must write exactly ETX after InputHandler commit/drain, got {bytes:?}"
        );
        assert!(
            !bytes.contains(&b'c') && !bytes.contains(&b'C'),
            "Ctrl-C must never leak printable c, got {bytes:?}"
        );
        let _ = model;
    }

    #[gpui::test]
    fn cmd_backspace_writes_exactly_ctrl_u_through_window_view(cx: &mut gpui::TestAppContext) {
        let (_model, shell, writer_rx, _reader_tx) = cx.update(attach_fake_writer);
        let _keep_shell = shell;
        let handle = cx.windows().into_iter().next().expect("one window");
        draw_window(cx, handle);

        cx.dispatch_keystroke(handle, Keystroke::parse("cmd-backspace").unwrap());
        draw_window(cx, handle);
        draw_window(cx, handle);

        let bytes = drain_writer(&writer_rx);
        assert_eq!(
            bytes,
            vec![0x15],
            "Cmd+Backspace must write exactly Ctrl-U, got {bytes:?}"
        );
    }

    #[gpui::test]
    fn cmd_v_writes_one_bracketed_paste_and_no_second_ime_payload(cx: &mut gpui::TestAppContext) {
        let (model, shell, writer_rx, _reader_tx) = cx.update(attach_fake_writer);
        let _keep_shell = shell;
        let handle = cx.windows().into_iter().next().expect("one window");
        draw_window(cx, handle);

        cx.update(|cx| {
            model.update(cx, |model, _| {
                model
                    .focused_pane_mut()
                    .expect("pane")
                    .feed_test_output(b"\x1b[?2004h")
                    .expect("enable bracketed paste");
            });
        });
        let paste = "echo hi\nsecond line";
        cx.write_to_clipboard(ClipboardItem::new_string(paste.to_string()));

        cx.dispatch_keystroke(handle, Keystroke::parse("cmd-v->v").unwrap());

        draw_window(cx, handle);
        draw_window(cx, handle);

        let bytes = drain_writer(&writer_rx);
        let expected = cx.update(|cx| {
            encode_live_paste(&model.read(cx).focused_pane().expect("pane").core, paste)
        });

        assert_eq!(
            bytes, expected,
            "Cmd+V must write exactly one bracketed payload and no second IME copy, got {bytes:?}"
        );
        assert_eq!(
            bytes
                .windows(b"\x1b[200~".len())
                .filter(|w| *w == b"\x1b[200~")
                .count(),
            1,
            "only one bracketed paste start is allowed"
        );
    }

    #[gpui::test]
    fn terminal_and_dock_wheel_move_viewport_hide_dock_and_write_nothing(
        cx: &mut gpui::TestAppContext,
    ) {
        let (model, shell, writer_rx, _reader_tx) = cx.update(attach_fake_writer);
        let _keep_shell = shell;
        let handle = cx.windows().into_iter().next().expect("one window");
        draw_window(cx, handle);

        cx.update(|cx| {
            model.update(cx, |model, _| {
                let pane = model.focused_pane_mut().expect("pane");
                pane.feed_test_output(b"\x1b]133;A\x07$ \x1b]133;B\x07")
                    .expect("prompt");
                let line_count = usize::from(pane.last_size.rows) + 32;
                for i in 0..line_count {
                    pane.feed_test_output(format!("line-{i:03}\r\n").as_bytes())
                        .expect("history");
                }
                pane.feed_test_output(b"\x1b]133;A\x07$ \x1b]133;B\x07")
                    .expect("live prompt");
            });
        });
        draw_window(cx, handle);

        let (offset_before, dock_before) = cx.update(|cx| {
            let pane = model.read(cx).focused_pane().expect("pane");
            (
                pane.viewport_offset(),
                pane.input_dock()
                    .map(|snap| snap.state)
                    .unwrap_or(InputDockState::Hidden),
            )
        });
        assert_eq!(offset_before, 0);
        assert_eq!(dock_before, InputDockState::ShellInputActive);

        let mut visual = gpui::VisualTestContext::from_window(handle, cx);
        visual.simulate_event(ScrollWheelEvent {
            position: gpui::point(gpui::px(40.0), gpui::px(40.0)),
            delta: ScrollDelta::Lines(gpui::point(0.0, 3.0)),
            modifiers: GpuiModifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        drop(visual);
        draw_window(cx, handle);

        let (offset_after, dock_after, painted_offset) = cx.update(|cx| {
            let pane = model.read(cx).focused_pane().expect("pane");
            let frame = pane.frame().expect("painted frame");
            (
                pane.viewport_offset(),
                pane.input_dock()
                    .map(|snap| snap.state)
                    .unwrap_or(InputDockState::Hidden),
                frame.viewport.scroll_offset,
            )
        });
        assert!(
            offset_after > offset_before,
            "terminal-body wheel must move viewport offset, before={offset_before} after={offset_after}"
        );
        assert_eq!(
            painted_offset,
            u32::try_from(offset_after).expect("offset"),
            "painted frame must move with the viewport"
        );
        assert_eq!(
            dock_after,
            InputDockState::Hidden,
            "scrolled dock must hide"
        );
        assert!(
            writer_rx.try_recv().is_err(),
            "wheel must not write PTY bytes"
        );

        cx.update(|cx| {
            model.update(cx, |model, _| {
                let pane = model.focused_pane_mut().expect("pane");
                pane.scroll_viewport_down(usize::MAX);
                pane.feed_test_output(b"\x1b]133;A\x07$ \x1b]133;B\x07live")
                    .expect("restore prompt");
            });
        });
        draw_window(cx, handle);

        let dock_y = cx
            .update_window(handle, |_, window, _| {
                window.viewport_size().height - gpui::px(40.0)
            })
            .unwrap();
        let mut visual = gpui::VisualTestContext::from_window(handle, cx);
        visual.simulate_event(ScrollWheelEvent {
            position: gpui::point(gpui::px(40.0), dock_y),
            delta: ScrollDelta::Lines(gpui::point(0.0, 3.0)),
            modifiers: GpuiModifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        drop(visual);
        draw_window(cx, handle);

        let (dock_wheel_offset, dock_wheel_state) = cx.update(|cx| {
            let pane = model.read(cx).focused_pane().expect("pane");
            (
                pane.viewport_offset(),
                pane.input_dock()
                    .map(|snap| snap.state)
                    .unwrap_or(InputDockState::Hidden),
            )
        });
        assert!(
            dock_wheel_offset > 0,
            "dock wheel must move viewport offset"
        );
        assert_eq!(
            dock_wheel_state,
            InputDockState::Hidden,
            "dock wheel must hide the dock"
        );
        assert!(
            writer_rx.try_recv().is_err(),
            "dock wheel must not write PTY bytes"
        );
    }

    #[gpui::test]
    fn shifted_pageup_and_shift_up_fn_alias_scroll_without_pty_csi(cx: &mut gpui::TestAppContext) {
        let (model, shell, writer_rx, _reader_tx) = cx.update(attach_fake_writer);
        let _keep_shell = shell;
        let handle = cx.windows().into_iter().next().expect("one window");
        draw_window(cx, handle);

        cx.update(|cx| {
            model.update(cx, |model, _| {
                let pane = model.focused_pane_mut().expect("pane");
                let line_count = usize::from(pane.last_size.rows) + 32;
                for i in 0..line_count {
                    pane.feed_test_output(format!("hist-{i:03}\r\n").as_bytes())
                        .expect("history");
                }
            });
        });
        draw_window(cx, handle);

        cx.dispatch_keystroke(handle, Keystroke::parse("shift-pageup").unwrap());
        draw_window(cx, handle);
        let page_offset = cx.update(|cx| {
            model
                .read(cx)
                .focused_pane()
                .expect("pane")
                .viewport_offset()
        });
        assert!(
            page_offset > 0,
            "Shift+PageUp must scroll the viewport, offset={page_offset}"
        );
        let page_bytes = drain_writer(&writer_rx);
        assert!(
            page_bytes.is_empty(),
            "Shift+PageUp must not write PTY bytes, got {page_bytes:?}"
        );

        cx.update(|cx| {
            model.update(cx, |model, _| {
                model
                    .focused_pane_mut()
                    .expect("pane")
                    .scroll_viewport_down(usize::MAX);
            });
        });

        cx.dispatch_keystroke(handle, Keystroke::parse("shift-up").unwrap());
        draw_window(cx, handle);
        let up_offset = cx.update(|cx| {
            model
                .read(cx)
                .focused_pane()
                .expect("pane")
                .viewport_offset()
        });
        let up_bytes = drain_writer(&writer_rx);
        assert!(
            up_offset > 0,
            "Shift+Up must scroll the viewport instead of inserting CSI, offset={up_offset}"
        );
        assert!(
            !up_bytes
                .windows(b"\x1b[1;2A".len())
                .any(|w| w == b"\x1b[1;2A")
                && !up_bytes
                    .windows(b"\x1b[1;2B".len())
                    .any(|w| w == b"\x1b[1;2B"),
            "Shift+Up must not write ;2A/;2B PTY bytes, got {up_bytes:?}"
        );
        assert!(
            up_bytes.is_empty(),
            "Shift+Up must write no PTY bytes, got {up_bytes:?}"
        );

        cx.update(|cx| {
            model.update(cx, |model, _| {
                model
                    .focused_pane_mut()
                    .expect("pane")
                    .scroll_viewport_down(usize::MAX);
            });
        });
        cx.dispatch_keystroke(handle, Keystroke::parse("shift-fn-up").unwrap());
        draw_window(cx, handle);
        let fn_offset = cx.update(|cx| {
            model
                .read(cx)
                .focused_pane()
                .expect("pane")
                .viewport_offset()
        });
        let fn_bytes = drain_writer(&writer_rx);
        assert!(
            fn_offset > 0,
            "Shift+Fn+Up alias must scroll the viewport, offset={fn_offset}"
        );
        assert!(
            fn_bytes.is_empty(),
            "Shift+Fn+Up must not write PTY bytes, got {fn_bytes:?}"
        );
    }

    #[gpui::test]
    fn alternate_screen_wheel_is_fail_closed(cx: &mut gpui::TestAppContext) {
        let (model, shell, writer_rx, _reader_tx) = cx.update(attach_fake_writer);
        let _keep_shell = shell;
        let handle = cx.windows().into_iter().next().expect("one window");
        draw_window(cx, handle);

        cx.update(|cx| {
            model.update(cx, |model, _| {
                let pane = model.focused_pane_mut().expect("pane");
                let line_count = usize::from(pane.last_size.rows) + 32;
                for i in 0..line_count {
                    pane.feed_test_output(format!("pri-{i:03}\r\n").as_bytes())
                        .expect("primary");
                }
            });
        });
        draw_window(cx, handle);

        let mut visual = gpui::VisualTestContext::from_window(handle, cx);
        visual.simulate_event(ScrollWheelEvent {
            position: gpui::point(gpui::px(40.0), gpui::px(40.0)),
            delta: ScrollDelta::Lines(gpui::point(0.0, 5.0)),
            modifiers: GpuiModifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        drop(visual);
        draw_window(cx, handle);
        assert!(
            cx.update(|cx| model
                .read(cx)
                .focused_pane()
                .expect("pane")
                .viewport_offset())
                > 0,
            "the primary-screen wheel must reach route_scroll before testing fallback"
        );

        cx.update(|cx| {
            model.update(cx, |model, _| {
                let pane = model.focused_pane_mut().expect("pane");
                pane.scroll_viewport_down(usize::MAX);
                pane.feed_test_output(b"\x1b[?1049hALT").expect("alternate");
            });
        });
        draw_window(cx, handle);

        let mut visual = gpui::VisualTestContext::from_window(handle, cx);
        visual.simulate_event(ScrollWheelEvent {
            position: gpui::point(gpui::px(40.0), gpui::px(40.0)),
            delta: ScrollDelta::Lines(gpui::point(0.0, 5.0)),
            modifiers: GpuiModifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        drop(visual);
        draw_window(cx, handle);

        let (offset, alternate, dock) = cx.update(|cx| {
            let pane = model.read(cx).focused_pane().expect("pane");
            let frame = pane.frame().expect("frame");
            (
                pane.viewport_offset(),
                frame.viewport.alternate_screen,
                pane.input_dock()
                    .map(|snap| snap.state)
                    .unwrap_or(InputDockState::Hidden),
            )
        });
        assert!(alternate, "fixture must be on the alternate screen");
        assert_eq!(
            offset, 0,
            "alternate-screen wheel must fail closed and keep offset 0"
        );
        assert_eq!(dock, InputDockState::Hidden);
        assert!(
            writer_rx.try_recv().is_err(),
            "alternate-screen wheel must not write PTY bytes"
        );
    }

    #[gpui::test]
    fn seq_countdown_chat_has_no_synthetic_prefix_and_second_toggle_hides_later_echo(
        cx: &mut gpui::TestAppContext,
    ) {
        let (model, shell, writer_rx, _reader_tx) = cx.update(attach_fake_writer);
        let _keep_shell = shell;
        let handle = cx.windows().into_iter().next().expect("one window");
        draw_window(cx, handle);

        let mut seq = Vec::from(&b"\x1b]133;A\x07"[..]);
        for n in 1..=200 {
            seq.extend_from_slice(format!("{n}\r\n").as_bytes());
        }
        cx.update(|cx| {
            model.update(cx, |model, _| {
                model
                    .focused_pane_mut()
                    .expect("pane")
                    .feed_test_output(&seq)
                    .expect("seq fixture");
            });
        });

        cx.dispatch_keystroke(handle, Keystroke::parse("cmd-shift-j").unwrap());
        draw_window(cx, handle);
        assert!(writer_rx.try_recv().is_err(), "toggle must not write PTY");

        let (effective, events, chat_a11y) = cx.update(|cx| {
            let model = model.read(cx);
            let pane = model.focused_pane().expect("pane");
            let events = pane.conversation_events(false, false);
            let chat_a11y = model
                .accessibility_snapshot()
                .root
                .children
                .iter()
                .any(|node| node.label == "Chat");
            (pane.effective_mode(false, false), events, chat_a11y)
        });
        assert_eq!(effective, SurfaceMode::Chat);
        assert!(chat_a11y, "Chat overlay must be present after first toggle");
        assert!(
            !events.is_empty(),
            "seq 1 200 fixture must project countdown rows"
        );
        let joined = events
            .iter()
            .map(|event| event.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let countdown: Vec<&str> = joined.lines().filter(|line| !line.is_empty()).collect();
        assert_eq!(countdown.last(), Some(&"200"), "got {countdown:?}");
        assert!(
            countdown
                .iter()
                .all(|line| line.bytes().all(|byte| byte.is_ascii_digit())),
            "countdown rows must contain only the PTY digits, got {countdown:?}"
        );

        cx.dispatch_keystroke(handle, Keystroke::parse("cmd-shift-j").unwrap());
        draw_window(cx, handle);
        let effective_off = cx.update(|cx| {
            model
                .read(cx)
                .focused_pane()
                .expect("pane")
                .effective_mode(false, false)
        });
        assert_eq!(effective_off, SurfaceMode::Terminal);

        cx.update(|cx| {
            model.update(cx, |model, _| {
                model
                    .focused_pane_mut()
                    .expect("pane")
                    .feed_test_output(b"\r\necho later-shell\r\n")
                    .expect("later echo");
            });
        });
        draw_window(cx, handle);

        let (effective_later, events_later, chat_a11y_later) = cx.update(|cx| {
            let model = model.read(cx);
            let pane = model.focused_pane().expect("pane");
            let events = pane.conversation_events(false, false);
            let chat_a11y = model
                .accessibility_snapshot()
                .root
                .children
                .iter()
                .any(|node| node.label == "Chat");
            (pane.effective_mode(false, false), events, chat_a11y)
        });
        assert_eq!(effective_later, SurfaceMode::Terminal);
        assert!(
            events_later.is_empty(),
            "later shell echo must not appear in Chat after overlay is removed, got {events_later:?}"
        );
        assert!(
            !chat_a11y_later,
            "second Cmd+Shift+J must remove the Chat overlay exactly once"
        );
    }
}
