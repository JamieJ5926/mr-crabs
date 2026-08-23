//! WindowView siblings for the semantic input dock.
//!
//! Chrome is existing `div()` siblings (palette pattern). Glyphs reuse
//! `TerminalElement::with_shared` on a synthetic one-row frame. This module
//! never calls `request_animation_frame`.

use gpui::{
    Div, FocusHandle, Font, InteractiveElement as _, ParentElement as _, Pixels, Styled as _, div,
    px, rgb,
};

use mr_crabs_element::{CellMetrics, PixelExtent, TerminalElement, TerminalPalette};

use crate::model::geometry::SurfaceGeometry;
use crate::model::input_dock::{
    CARET_W, CHEVRON_GAP, CHEVRON_X_INSET, DOCK_H, DockHit, InputDockLayout, InputDockSnapshot,
    InputDockState, PAD_Y, PointF, layout_input_dock,
};
use crate::model::split::PaneId;

/// Light/dark tokens. Do not reuse TerminalPalette::light canvas.
#[derive(Clone, Copy, Debug)]
pub struct InputDockTokens {
    pub canvas: u32,
    pub dock_bg: u32,
    pub separator: u32,
    pub footer_bg: u32,
    pub prompt: u32,
    pub input_fg: u32,
    pub footer_fg: u32,
    pub caret: u32,
    pub error: u32,
}

impl InputDockTokens {
    pub const LIGHT: Self = Self {
        canvas: 0xF7F4EE,
        dock_bg: 0xEFECE6,
        separator: 0xD9D4CC,
        footer_bg: 0xE6E2DB,
        prompt: 0x85631C,
        input_fg: 0x202020,
        footer_fg: 0x5E5A52,
        caret: 0x202020,
        error: 0xB54A32,
    };

    pub const DARK: Self = Self {
        canvas: 0x1C1A17,
        dock_bg: 0x161412,
        separator: 0x3A372F,
        footer_bg: 0x12110F,
        prompt: 0xD4A24A,
        input_fg: 0xE8E2D6,
        footer_fg: 0x8A8478,
        caret: 0xE8E2D6,
        error: 0xB54A32,
    };

    pub fn for_palette(palette: TerminalPalette) -> Self {
        if palette.background[0] > 0x80 {
            Self::LIGHT
        } else {
            Self::DARK
        }
    }
}

pub fn compose_input_dock_layout(
    window_viewport: PixelExtent,
    pane_origin: PointF,
    pane_geometry: SurfaceGeometry,
    snap: &InputDockSnapshot,
    focused: bool,
) -> Option<InputDockLayout> {
    layout_input_dock(
        (window_viewport.width, window_viewport.height),
        pane_origin,
        (pane_geometry.content.width, pane_geometry.content.height),
        pane_geometry.metrics,
        snap,
        focused,
    )
}

pub fn input_dock_mask(layout: InputDockLayout, tokens: InputDockTokens) -> Div {
    bounds_div(layout.mask).bg(rgb(tokens.canvas)).occlude()
}

pub fn input_dock_separator(layout: InputDockLayout, tokens: InputDockTokens) -> Div {
    bounds_div(layout.separator)
        .bg(rgb(tokens.separator))
        .occlude()
}

/// Paint inputs for the dock chrome + synthetic one-row element.
pub struct InputDockOverlayView<'a> {
    pub snap: &'a InputDockSnapshot,
    pub layout: InputDockLayout,
    pub tokens: InputDockTokens,
    pub font: Font,
    pub font_size: Pixels,
    pub metrics: CellMetrics,
    pub terminal_palette: TerminalPalette,
    pub focused: bool,
    pub focus: Option<FocusHandle>,
    pub ime_tx: Option<std::sync::mpsc::Sender<(PaneId, String)>>,
    pub pane_id: PaneId,
}

pub fn input_dock_overlay(view: InputDockOverlayView<'_>) -> Div {
    let InputDockOverlayView {
        snap,
        layout,
        tokens,
        font,
        font_size,
        metrics,
        terminal_palette,
        focused,
        focus,
        ime_tx,
        pane_id,
    } = view;
    let frame = crate::model::input_dock::synthetic_dock_frame(snap);
    let mut element = TerminalElement::new(frame, metrics)
        .with_font(font)
        .with_font_size(font_size)
        .with_palette(terminal_palette);
    if let Some(focus) = focus {
        element = element.with_focus(focus);
    }
    if let Some(ime_tx) = ime_tx {
        element = element.with_input_sink(move |text| {
            let _ = ime_tx.send((pane_id, text.to_owned()));
        });
    }
    let caret_col = snap.cursor.source_col.saturating_sub(snap.source.start_col);
    let caret_x = CHEVRON_X_INSET + CHEVRON_GAP + f32::from(caret_col) * metrics.width;
    let caret_y = (DOCK_H - 21.0).max(0.0) * 0.5;
    let prompt_color = if focused {
        tokens.prompt
    } else {
        mix_alpha(tokens.prompt, 0.5)
    };
    let show_caret = focused && snap.cursor.visible;

    bounds_div(layout.dock)
        .bg(rgb(tokens.dock_bg))
        .occlude()
        .child(
            div()
                .absolute()
                .left(px(CHEVRON_X_INSET))
                .top(px((DOCK_H - metrics.height).max(0.0) * 0.5))
                .text_color(rgb(prompt_color))
                .child("\u{276F}"),
        )
        .child(
            div()
                .absolute()
                .left(px(CHEVRON_X_INSET + CHEVRON_GAP))
                .top(px(PAD_Y))
                .w(px(
                    (layout.dock.width - CHEVRON_X_INSET - CHEVRON_GAP).max(metrics.width)
                ))
                .h(px(metrics.height))
                .child(element),
        )
        .child(
            div()
                .absolute()
                .left(px(caret_x))
                .top(px(caret_y))
                .w(px(CARET_W))
                .h(px(21.0))
                .bg(rgb(if show_caret {
                    tokens.caret
                } else {
                    tokens.dock_bg
                })),
        )
}

pub fn input_dock_footer(
    snap: &InputDockSnapshot,
    layout: InputDockLayout,
    tokens: InputDockTokens,
) -> Div {
    let _ = snap;
    bounds_div(layout.footer)
        .bg(rgb(tokens.footer_bg))
        .occlude()
}

pub fn dock_is_active(snap: &InputDockSnapshot) -> bool {
    snap.state == InputDockState::ShellInputActive
}

pub fn dock_hit_consumes(hit: DockHit) -> bool {
    matches!(hit, DockHit::Separator | DockHit::Footer | DockHit::Chevron)
}

fn bounds_div(bounds: crate::model::input_dock::DockBounds) -> Div {
    div()
        .absolute()
        .left(px(bounds.x))
        .top(px(bounds.y))
        .w(px(bounds.width))
        .h(px(bounds.height))
}

fn mix_alpha(color: u32, alpha: f32) -> u32 {
    let r = ((color >> 16) & 0xff) as f32 * alpha;
    let g = ((color >> 8) & 0xff) as f32 * alpha;
    let b = (color & 0xff) as f32 * alpha;
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}
