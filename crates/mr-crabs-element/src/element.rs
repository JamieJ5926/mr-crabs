//! The [`TerminalElement`]: a custom GPUI element that renders a
//! [`FrameDelta`] through the retained [`RenderCache`].
//!
//! The element is a pure consumer of frames: it owns (or shares, via `Arc`)
//! the latest frame and never locks the terminal engine during layout or
//! paint. Font resolution and shaping go through GPUI's text system
//! (CoreText on macOS); no wgpu, no libghostty-vt, no Zig.

use gpui::{
    App, BorderStyle, Bounds, BoxShadow, Corners, Element, ElementId, FocusHandle, Font,
    FontFeatures, FontStyle, FontWeight, GlobalElementId, Hsla, InspectorElementId, IntoElement,
    LayoutId, PathBuilder, Pixels, Point, RenderImage, ShapedLine, SharedString,
    StrikethroughStyle, Style as GpuiStyle, TextRun, UnderlineStyle, Window, fill, font, outline,
    point, px, size, white,
};
use mr_crabs_effects::{
    CellPx, EffectsConfig, EffectsModel, LinePx, RectPx, RevealMath, TextAnimation,
};
use mr_crabs_graphics::{
    image::{Image, ImageData, ImageFormat},
    iterm::{self, ItermUploads},
    kitty,
    placement::{Location as PlacementLocation, Placement, PlacementId, TerminalContext},
    store::{ImageStore, StoreConfig},
    texture::{TextureCache, TextureKey},
};
use mr_crabs_terminal::{CursorShape, FrameDelta, GridSize};
use parking_lot::{Mutex, MutexGuard};
use std::collections::HashMap;
use std::ops::Range;
use std::panic::Location;
use std::sync::Arc;
use std::time::Instant;

use crate::{
    CacheAction, CellMetrics, RenderCache, ResizeDeduper, RunBatch, cursor,
    geometry::{pixel_bounds_to_grid, run_bounds},
    paint_diagnostics::{
        PaintDiagnosticsEvent, PaintDiagnosticsSink, PaintEffectsOutcome, diagnostic_event,
    },
    palette,
    selection::selection_rects,
};

type PaintCallback = Arc<dyn Fn(&mut App)>;
/// A GPUI element that renders a terminal [`FrameDelta`].
///
/// Construct with [`TerminalElement::new`] (owned frame) or
/// [`TerminalElement::with_shared`] (shared `Arc` frame). The app
/// reconstructs the element on every render, so all paint state that must
/// survive frames — the [`RenderCache`], the GPUI-shaped lines, the
/// [`ResizeDeduper`], and the last painted font identity — is retained in
/// GPUI element storage (`Window::with_element_state`) keyed by the
/// [`GlobalElementId`] that [`Element::id`] establishes. A fresh instance
/// with the same element id therefore merges a `Partial` frame into the
/// batches retained from the previous instance's `Full` frame instead of
/// starting from an empty cache. Unchanged same-capacity frames repaint with
/// zero allocation, and animation frames are requested only while the cursor
/// blinks.
///
/// The element fills its parent's content viewport: the app owns the
/// padding (padding precedes grid calculation), so the laid-out box is the
/// padded content box and its origin is the content origin. Every paint
/// primitive — glyph, cursor, selection, background, IME — is positioned
/// from that single origin plus `CellMetrics`; glyphs are anchored to their
/// terminal cells, never to the shaper's natural advance.
pub struct TerminalElement {
    /// The frame to render; paint borrows this and never touches the engine.
    frame: Option<Arc<FrameDelta>>,
    /// Cell dimensions in pixels.
    metrics: CellMetrics,
    palette: palette::TerminalPalette,
    /// Explicit content origin in window pixels (the padded content box's
    /// top-left). When unset, the element uses its layout bounds origin —
    /// the laid-out position of the filled parent content box.
    content_origin: Option<Point<Pixels>>,
    /// The per-surface graphics overlay (S7): one bounded `ImageStore` plus
    /// texture cache per pane, handed here via `with_graphics`. Paint draws
    /// the overlay's visible placements through GPUI's image pipeline
    /// (`window.paint_image`) between the background pass and the text pass.
    /// When `None`, the graphics pass is skipped entirely.
    graphics: Option<Arc<Mutex<GraphicsOverlay>>>,
    /// The base shaping font: family plus OpenType features (weight/style
    /// are defaulted here; per-run bold/italic are applied at shape time).
    font: Font,
    /// Font size for glyph shaping, in pixels.
    font_size: Pixels,
    /// Optional real IME/text-input sink. No handler is registered until the
    /// application supplies one.
    input: Option<TerminalInputHandler>,
    /// Optional callback for deduplicated terminal grid resizes.
    resize_sink: Option<Arc<dyn Fn(mr_crabs_terminal::GridSize) + Send + Sync>>,
    /// Focus handle for focus/IME registration, created at layout time.
    focus: Option<FocusHandle>,
    /// Stable element identity, pinned via [`TerminalElement::with_element_id`].
    /// GPUI keys the retained paint state in element storage by the
    /// `GlobalElementId` derived from this id, so a rebuilt instance with
    /// the same id inherits the previous instance's cache, shaped lines,
    /// resize deduper, and font identity. `None` renders standalone with
    /// scratch state that is not retained across frames.
    element_id: Option<ElementId>,
    /// Optional main-thread hook invoked at the start of paint so the app
    /// can drain PTY output on frames that already run (cursor blink).
    on_paint: Option<PaintCallback>,
    borrowed_focus: bool,
    /// Live text/trail effects. `None` keeps the disabled fast path.
    effects: Option<EffectsConfig>,
    paint_diagnostics: Option<PaintDiagnosticsSink>,
}

/// The shaping identity that determines whether retained glyph batches must
/// be rebuilt: the resolved base font (family + OpenType features), the glyph
/// size, and the cell metrics. Per-run weight/style and decorations are
/// applied at shape time whenever a row is damaged, so they are not part of
/// the identity. The pinned GPUI exposes no atlas generation counter — atlas
/// entries are keyed by font id, glyph id, and size, and GPUI drops them on
/// device recovery — so this identity is the complete observable shaping
/// state.
#[derive(Clone, Debug, PartialEq)]
struct FontIdentity {
    font: Font,
    font_size: Pixels,
    metrics: CellMetrics,
}

/// Retained paint state persisted in GPUI element storage
/// (`Window::with_element_state`, keyed by the element's
/// [`GlobalElementId`]) so it survives [`TerminalElement`] reconstruction
/// between frames.
///
/// The app builds a fresh [`TerminalElement`] on every render; without
/// element-scoped retention, a `Partial` frame after a `Full` frame would
/// start from an empty cache and paint only the damaged rows. Everything
/// that must survive reconstruction lives here instead of on the element:
///
/// - the [`RenderCache`] (row batches + retained capacities, allocation-free
///   on unchanged frames),
/// - the GPUI-shaped lines parallel to the cache batches,
/// - the [`ResizeDeduper`] (each distinct grid size emits at most once, so a
///   rebuilt element does not spam resize events),
/// - the last painted [`FontIdentity`], driving change-based invalidation.
/// - the retained cursor blink epoch and last activity identity.
#[derive(Default)]
struct PaintState {
    cache: RenderCache,
    shaped_lines: Vec<Vec<Option<ShapedLine>>>,
    deduper: ResizeDeduper,
    painted_font: Option<FontIdentity>,
    cursor_blink: cursor::BlinkState,
    effects: Option<EffectsModel>,
    effects_origin: Option<Instant>,
}

impl PaintState {
    /// Apply `frame` to the retained cache and invalidate shaped glyphs when
    /// the shaping identity changes. Row batches are font-independent, so
    /// they stay retained: clearing them would make an incremental `Partial`
    /// frame erase every undamaged row after a font or metric change.
    fn apply_frame(&mut self, frame: &FrameDelta, identity: &FontIdentity) -> CacheAction {
        let font_changed = self.painted_font.as_ref() != Some(identity);
        if font_changed {
            self.painted_font = Some(identity.clone());
            self.shaped_lines.clear();
        }
        let mut action = self.cache.apply_frame(frame);
        action.needs_redraw |= font_changed;
        action
    }

    /// Keep `shaped_lines` parallel to the retained cache batches: grow for
    /// newly damaged rows and drop stale rows after a Full frame shrink, so
    /// the paint pass always finds one slot per batch.
    fn sync_shaped_rows(&mut self) {
        let rows = self.cache.batches().len();
        if self.shaped_lines.len() > rows {
            self.shaped_lines.truncate(rows);
        }
        while self.shaped_lines.len() < rows {
            self.shaped_lines.push(Vec::new());
        }
    }

    fn effects_model(
        &mut self,
        config: EffectsConfig,
        size: GridSize,
        cell: CellPx,
    ) -> &mut EffectsModel {
        let model = self
            .effects
            .get_or_insert_with(|| EffectsModel::new(config, size, cell));
        if model.cell() != cell {
            *model = EffectsModel::new(config, size, cell);
        } else if model.config() != &config {
            model.set_config(config);
        }
        model
    }
}

pub(crate) fn trail_glow_bounds(
    glow_rect: RectPx,
    origin: Point<Pixels>,
) -> Option<Bounds<Pixels>> {
    if glow_rect.degenerate() {
        return None;
    }
    Some(Bounds {
        origin: point(
            origin.x + px(glow_rect.x as f32),
            origin.y + px(glow_rect.y as f32),
        ),
        size: size(px(glow_rect.w as f32), px(glow_rect.h as f32)),
    })
}

pub(crate) fn trail_segment_points(
    segment: LinePx,
    origin: Point<Pixels>,
) -> (Point<Pixels>, Point<Pixels>) {
    let from = point(
        origin.x + px(segment.from.x as f32),
        origin.y + px(segment.from.y as f32),
    );
    let to = point(
        origin.x + px(segment.to.x as f32),
        origin.y + px(segment.to.y as f32),
    );
    (from, to)
}

pub(crate) fn trail_stroke_width(radius_px: f64) -> Pixels {
    px((radius_px * 0.5).max(1.0) as f32)
}

impl TerminalElement {
    /// Create an element owning `frame`.
    pub fn new(frame: FrameDelta, metrics: CellMetrics) -> Self {
        Self::with_shared(Arc::new(frame), metrics)
    }

    /// Create an element sharing `frame`. Paint borrows the shared frame by
    /// reference, so multiple elements (or an app-side copy) can render the
    /// same frame without any terminal lock.
    pub fn with_shared(frame: Arc<FrameDelta>, metrics: CellMetrics) -> Self {
        let font_size = px(metrics.height);
        Self {
            font: font("Menlo"),
            frame: Some(frame),
            metrics,
            content_origin: None,
            palette: palette::TerminalPalette::default(),
            graphics: None,
            font_size,
            resize_sink: None,
            input: None,
            focus: None,
            element_id: None,
            on_paint: None,
            borrowed_focus: false,
            effects: None,
            paint_diagnostics: None,
        }
    }

    /// An empty element that paints nothing (no frame attached).
    pub fn empty(metrics: CellMetrics) -> Self {
        Self {
            frame: None,
            metrics,
            palette: palette::TerminalPalette::default(),
            content_origin: None,
            graphics: None,
            font_size: px(metrics.height),
            font: font("Menlo"),
            input: None,
            resize_sink: None,
            focus: None,
            element_id: None,
            on_paint: None,
            borrowed_focus: false,
            effects: None,
            paint_diagnostics: None,
        }
    }

    /// Builder: pin the explicit content origin in window pixels.
    ///
    /// The content origin is the top-left of the padded content box — the
    /// surface's viewport origin plus the padding. Every paint primitive
    /// positions itself from this origin plus the cell metrics. When unset,
    /// the element derives the origin from its layout bounds (the filled
    /// parent content box), so callers that lay the element out inside a
    /// padded container need not set it.
    pub fn with_content_origin(mut self, origin: Point<Pixels>) -> Self {
        self.content_origin = Some(origin);
        self
    }

    /// The explicit content origin, if one was pinned.
    pub const fn content_origin(&self) -> Option<Point<Pixels>> {
        self.content_origin
    }

    /// Builder: attach the per-surface graphics overlay (S7). The window
    /// view owns one bounded overlay per pane and hands it here; paint draws
    /// the overlay's visible placements through GPUI's image pipeline after
    /// the background pass and before the text pass (kitty z=0 semantics:
    /// images sit above backgrounds and below glyphs). The element never
    /// locks the terminal engine for graphics: the overlay owns the
    /// `ImageStore`, the bounded texture cache, and the decoded
    /// `RenderImage`s, and paint is a pure function of the overlay plus the
    /// current placement/context state.
    pub fn with_graphics(mut self, overlay: Arc<Mutex<GraphicsOverlay>>) -> Self {
        self.graphics = Some(overlay);
        self
    }

    /// The attached graphics overlay handle, if any. The window view may
    /// clone this shared handle across renders to keep one overlay per
    /// surface.
    pub fn graphics(&self) -> Option<&Arc<Mutex<GraphicsOverlay>>> {
        self.graphics.as_ref()
    }

    /// Builder: override the glyph font size (defaults to the cell height).
    pub fn with_font_size(mut self, font_size: Pixels) -> Self {
        self.font_size = font_size;
        self
    }

    /// Configure the glyph font family. The shaping identity (family +
    /// features + size + cell metrics) is compared at paint time against the
    /// identity retained in element storage, so a family change invalidates
    /// the retained glyph batches exactly once.
    pub fn with_font_family(mut self, family: impl Into<SharedString>) -> Self {
        self.font.family = family.into();
        self
    }
    /// Configure the complete shaping font, including fallback families.
    pub fn with_font(mut self, font: Font) -> Self {
        self.font = font;
        self
    }

    /// Configure OpenType font features (e.g. `calt`, `liga`, `ss01`). Like
    /// the family, features are part of the paint-time shaping identity and
    /// invalidate the retained glyph batches when they change.
    pub fn with_font_features(mut self, features: FontFeatures) -> Self {
        self.font.features = features;
        self
    }

    pub fn with_palette(mut self, palette: palette::TerminalPalette) -> Self {
        self.palette = palette;
        self
    }

    /// Attach the application text sink used for committed IME input.
    pub fn with_input_sink(mut self, sink: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.input = Some(TerminalInputHandler::new(sink));
        self
    }

    /// Attach the application sink for deduplicated cell-grid resize events.
    pub fn with_resize_sink(
        mut self,
        sink: impl Fn(mr_crabs_terminal::GridSize) + Send + Sync + 'static,
    ) -> Self {
        self.resize_sink = Some(Arc::new(sink));
        self
    }

    /// Replace the frame to render. The cache is kept: unchanged rows (by
    /// sequence/damage) still reuse their retained batches.
    pub fn set_frame(&mut self, frame: FrameDelta) {
        self.frame = Some(Arc::new(frame));
    }

    /// Replace the frame to render, sharing an `Arc`.
    pub fn set_shared_frame(&mut self, frame: Arc<FrameDelta>) {
        self.frame = Some(frame);
    }

    /// Builder: pin the stable [`ElementId`] that keys the retained paint
    /// state in GPUI element storage.
    ///
    /// The app reconstructs a [`TerminalElement`] on every render; GPUI
    /// keeps element state alive across frames for elements whose
    /// [`Element::id`] is stable, so a `Partial` frame merges into the
    /// batches retained from the previous instance's `Full` frame instead of
    /// starting from an empty cache. Elements without an id paint standalone
    /// with scratch state that is not retained.
    pub fn with_element_id(mut self, element_id: ElementId) -> Self {
        self.element_id = Some(element_id);
        self
    }

    /// Invoke `hook` at the start of each paint with the live `&mut App`.
    pub fn with_on_paint(mut self, hook: impl Fn(&mut App) + 'static) -> Self {
        self.on_paint = Some(Arc::new(hook));
        self
    }

    /// Use the window's focus handle for IME/text-input registration.
    pub fn with_focus(mut self, focus: FocusHandle) -> Self {
        self.focus = Some(focus);
        self.borrowed_focus = true;
        self
    }

    /// Builder: attach the live effects config. Disabled configs allocate
    /// nothing in paint state.
    pub fn with_effects(mut self, config: EffectsConfig) -> Self {
        self.effects = Some(config);
        self
    }

    pub fn with_paint_diagnostics<F>(mut self, sink: F) -> Self
    where
        F: Fn(PaintDiagnosticsEvent) + Send + Sync + 'static,
    {
        self.paint_diagnostics = Some(std::sync::Arc::new(sink));
        self
    }

    /// The frame being rendered, if any.
    pub fn frame(&self) -> Option<&Arc<FrameDelta>> {
        self.frame.as_ref()
    }

    pub const fn metrics(&self) -> CellMetrics {
        self.metrics
    }

    fn paint_frame(
        &mut self,
        state: &mut PaintState,
        frame: &FrameDelta,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Font/atlas invalidation compares the actual shaping identity — the
        // resolved base font (family + features), the glyph size, and the
        // cell metrics — against the identity retained from the last paint,
        // never a per-instance counter: elements are reconstructed every
        // frame, and only the retained identity can tell a changed font from
        // an unchanged one across instances. Same font never resets; any
        // identity change drops the retained batches so the next frame
        // re-shapes every glyph with the new font.
        let identity = FontIdentity {
            font: self.font.clone(),
            font_size: self.font_size,
            metrics: self.metrics,
        };

        // Resize dedup: derive the grid from the available pixel bounds (the
        // laid-out parent content box — padding precedes grid calculation)
        // and emit only when the cell dimensions actually change. The
        // deduper is retained with the rest of the paint state, so an
        // element rebuilt every frame does not re-emit the last grid.
        if let Some(grid) = pixel_bounds_to_grid(bounds, self.metrics)
            && state.deduper.offer(grid)
            && let Some(sink) = &self.resize_sink
        {
            sink(grid);
        }

        let action = state.apply_frame(frame, &identity);
        // One explicit content origin for every primitive: the pinned origin
        // when the app supplies one, otherwise the laid-out content box
        // origin (the element fills the padded content viewport).
        let origin = self.content_origin.unwrap_or(bounds.origin);

        window.paint_quad(fill(bounds, self.palette.background_color()));

        if action.needs_redraw {
            state.sync_shaped_rows();
            for (row_index, row_batch) in state.cache.batches().iter().enumerate() {
                let shaped_row = &mut state.shaped_lines[row_index];
                shaped_row.clear();
                shaped_row.reserve(row_batch.runs.len().saturating_sub(shaped_row.capacity()));
                for run in &row_batch.runs {
                    if run.text.is_empty() || run.flags & 0x0100 != 0 {
                        shaped_row.push(None);
                        continue;
                    }
                    let (color, _, _) = run_paint_style(frame, run, white(), self.palette);
                    let mut run_font = self.font.clone();
                    if run.flags & 0x0002 != 0 {
                        run_font.weight = FontWeight::BOLD;
                    }
                    if run.flags & 0x0004 != 0 {
                        run_font.style = FontStyle::Italic;
                    }
                    // Decorations are painted cell-aligned by the paint pass;
                    // shaping carries color only.
                    let text_run = TextRun {
                        len: run.text.len(),
                        font: run_font,
                        color,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    shaped_row.push(Some(window.text_system().shape_line(
                        run.text.clone(),
                        self.font_size,
                        std::slice::from_ref(&text_run),
                        None,
                    )));
                }
            }
        }

        // GPUI paint commands must be emitted on every window redraw, even
        // when terminal damage is clean. The retained batches and shaped
        // lines avoid rebuilding or allocating on cursor-only redraws.
        for row_batch in state.cache.batches() {
            for rect in &row_batch.backgrounds {
                let style = frame.styles.get(usize::from(rect.style));
                let color = style
                    .map(|style| {
                        if rect.flags & 0x0001 != 0 {
                            palette::style_foreground_with_palette(style, self.palette)
                        } else {
                            palette::style_background_with_palette(style, self.palette)
                        }
                    })
                    .unwrap_or_else(|| self.palette.background_color());
                let rect_bounds =
                    run_bounds(origin, rect.col, rect.len, row_batch.row, self.metrics);
                window.paint_quad(fill(rect_bounds, color));
            }
        }

        // S7 graphics: paint the per-surface overlay's visible placements
        // above backgrounds and below glyphs. The overlay owns the bounded
        // ImageStore + texture cache; paint resolves placement rects against
        // the overlay's current terminal context (viewport top row, cell
        // size) and uploads/caches the decoded images through GPUI's sprite
        // atlas. Pending or evicted textures are skipped; the store's own
        // budgets keep the overlay bounded.
        if let Some(overlay) = &self.graphics {
            overlay.lock().paint(window, origin);
        }

        // Text pass: every glyph is anchored to its terminal cell — glyph
        // origin is `content_origin + col * cell_width`, from the run's
        // per-character cell widths — so natural shaping advances can never
        // accumulate drift against the grid. Ink is clipped to the content
        // rect (padding precedes the grid), so first-column overhang stays
        // inside the padded content.
        let content_bounds = Bounds::new(
            origin,
            size(
                px(self.metrics.width * f32::from(frame.size.cols)),
                px(self.metrics.height * f32::from(frame.size.rows)),
            ),
        );
        window.paint_layer(content_bounds, |window| {
            for (row_index, row_batch) in state.cache.batches().iter().enumerate() {
                let Some(shaped_row) = state.shaped_lines.get(row_index) else {
                    continue;
                };
                for (run, line) in row_batch.runs.iter().zip(shaped_row) {
                    let Some(line) = line else {
                        continue;
                    };
                    let paint_style = run_paint_style(frame, run, white(), self.palette);
                    self.paint_run_cell_aligned(
                        line,
                        run,
                        row_batch.row,
                        origin,
                        paint_style,
                        window,
                    );
                }
            }
        });

        // Selection overlay above backgrounds, below the cursor.
        for rect in selection_rects(&frame.selection, frame.size, self.metrics) {
            window.paint_quad(fill(rect + origin, self.palette.selection_color()));
        }

        // Cursor: terminal activity and cursor movement start a fresh visible
        // blink phase; animation redraws of the same frame advance it.
        let cursor = &frame.cursor;
        let cursor_visible_phase = if cursor.visible {
            if cursor.blinking {
                state.cursor_blink.phase_at(frame, Instant::now())
            } else {
                true
            }
        } else {
            false
        };
        if cursor.visible && cursor_visible_phase {
            let geometry = cursor::cursor_geometry(cursor, self.metrics);
            let rect = geometry.bounds + origin;
            match geometry.shape {
                CursorShape::Block | CursorShape::Bar | CursorShape::Underline => {
                    window.paint_quad(fill(rect, self.palette.cursor_color()));
                }
                CursorShape::HollowBlock => {
                    window.paint_quad(outline(
                        rect,
                        self.palette.cursor_color(),
                        BorderStyle::default(),
                    ));
                }
            }
        }

        if let (Some(focus), Some(input)) = (&self.focus, &self.input) {
            input.set_bounds(cursor::cursor_geometry(cursor, self.metrics).bounds + origin);
            window.handle_input(focus, input.clone(), cx);
        }
        // Animation scheduling: cursor blink plus live text/trail effects.
        let outcome = self.paint_effects(state, frame, origin, window);
        let effects_busy = outcome.busy;
        let cursor_requested = cursor::should_request_animation(frame);
        if let Some(sink) = self.paint_diagnostics.clone() {
            sink(diagnostic_event(
                frame.sequence,
                cursor_requested,
                cursor_visible_phase,
                outcome,
            ));
        }
        if cursor_requested || effects_busy {
            window.request_animation_frame();
        }
    }

    fn paint_effects(
        &self,
        state: &mut PaintState,
        frame: &FrameDelta,
        origin: Point<Pixels>,
        window: &mut Window,
    ) -> PaintEffectsOutcome {
        let Some(config) = self.effects else {
            return PaintEffectsOutcome::default();
        };
        if config.text_animation == TextAnimation::Disabled && !config.cursor_trail {
            state.effects = None;
            return PaintEffectsOutcome::default();
        }
        let cell = CellPx::new(
            f64::from(self.metrics.width),
            f64::from(self.metrics.height),
        );
        let origin_clock = state.effects_origin.get_or_insert_with(Instant::now);
        let now_ms = origin_clock.elapsed().as_millis() as u64;
        let model = state.effects_model(config, frame.size, cell);
        let focused = self
            .focus
            .as_ref()
            .is_some_and(|focus| focus.is_focused(window));
        let fx = model.apply_frame(frame, now_ms, focused);
        let burst = !fx.text_reveal_allowed;
        if !burst {
            let math = RevealMath::new(
                config.text_animation,
                config.text_animation_duration_ms,
                config.text_animation_intensity,
                cell.width,
            );
            let bg = self.palette.background_color();
            let cw = px(self.metrics.width);
            let ch = px(self.metrics.height);
            for pending in &fx.pending {
                let rect = gpui::Bounds {
                    origin: point(
                        origin.x + px(f32::from(pending.col) * self.metrics.width),
                        origin.y + px(f32::from(pending.row) * self.metrics.height),
                    ),
                    size: size(cw, ch),
                };
                window.paint_quad(fill(rect, bg));
            }
            for reveal in &fx.revealing {
                let hidden = reveal.hidden_fraction_at(&math, cell.width);
                if hidden <= 0.0 {
                    continue;
                }
                let frac = reveal.boundary_fraction(&math) as f32;
                let shown = cw * frac;
                let remain = cw - shown;
                if remain <= px(0.0) {
                    continue;
                }
                let mut color = bg;
                color.a = hidden as f32;
                let rect = gpui::Bounds {
                    origin: point(
                        origin.x + px(f32::from(reveal.pos.col) * self.metrics.width) + shown,
                        origin.y + px(f32::from(reveal.pos.row) * self.metrics.height),
                    ),
                    size: size(remain, ch),
                };
                window.paint_quad(fill(rect, color));
            }
        }
        if fx.trail.active && fx.trail.alpha > 0.0 {
            if let Some(rect) = trail_glow_bounds(fx.trail.glow_rect, origin) {
                let mut glow = self.palette.cursor_color();
                glow.a = fx.trail.alpha as f32;
                if glow.a > 0.0 {
                    window.paint_drop_shadows(
                        rect,
                        Corners::all(px(0.0)),
                        &[BoxShadow::new(px(0.0), px(0.0), glow)
                            .blur_radius(px(fx.trail.radius_px as f32))],
                    );
                    if let Some(segment) = fx.trail.segment {
                        let (from, to) = trail_segment_points(segment, origin);
                        let width = trail_stroke_width(fx.trail.radius_px);
                        let mut builder = PathBuilder::stroke(width);
                        builder.move_to(from);
                        builder.line_to(to);
                        if let Ok(path) = builder.build() {
                            window.paint_path(path, glow);
                        }
                    }
                }
            }
        }
        let needs_frame = fx.needs_frame;
        let trail_active = fx.trail.active;
        let trail_alpha = fx.trail.alpha;
        let revealing = fx.revealing.len();
        let pending = fx.pending.len();
        let busy = (!burst && needs_frame) || trail_active;
        PaintEffectsOutcome {
            busy,
            burst_bypass: burst,
            revealing,
            pending,
            needs_frame,
            trail_active,
            trail_alpha,
        }
    }

    /// Paint one shaped run with every glyph anchored to its terminal cell.
    ///
    /// The shaper's natural horizontal advances are discarded: glyph `i`
    /// paints at the content origin plus its terminal column multiplied by
    /// `cell_width`, so glyph positions can never drift from the cell grid.
    ///
    /// Vertical placement keeps GPUI's shaped baseline geometry
    /// (ascent/descent centering plus per-glyph vertical offset).
    /// Underline/strikethrough span the run's cell rectangle.
    fn paint_run_cell_aligned(
        &self,
        line: &ShapedLine,
        run: &RunBatch,
        row: u16,
        origin: Point<Pixels>,
        paint_style: (Hsla, Option<UnderlineStyle>, Option<StrikethroughStyle>),
        window: &mut Window,
    ) {
        let (color, underline, strikethrough) = paint_style;
        let cell_height = px(self.metrics.height);
        // Same baseline math as GPUI's line painting: vertical centering of
        // the shaped line within the cell, identical for every glyph.
        let padding_top = (cell_height - line.ascent - line.descent) / 2.0;
        let baseline_offset = point(px(0.0), padding_top + line.ascent);

        // Walk the run text in parallel with the shaped glyphs: glyph.index
        // is a byte offset into `run.text`; each character before it consumes
        // its terminal cell width, so the owning character's cell column is
        // `run.col + prefix sum`. Glyphs sharing a character (combining
        // marks) keep the same cell column.
        let mut chars = run.text.char_indices();
        let mut next_char = chars.next();
        let mut cell_col = run.col;
        let mut width_index = 0usize;
        for shaped_run in &line.runs {
            for glyph in &shaped_run.glyphs {
                while let Some((byte, _)) = next_char {
                    if byte >= glyph.index {
                        break;
                    }
                    cell_col = cell_col
                        .saturating_add(run.glyph_widths.get(width_index).copied().unwrap_or(1));
                    width_index += 1;
                    next_char = chars.next();
                }
                let glyph_origin = point(
                    origin.x + px(f32::from(cell_col) * self.metrics.width),
                    origin.y + px(f32::from(row) * self.metrics.height),
                );
                let paint_origin =
                    glyph_origin + baseline_offset + point(px(0.0), glyph.position.y);
                if glyph.is_emoji {
                    let _ = window.paint_emoji(
                        paint_origin,
                        shaped_run.font_id,
                        glyph.id,
                        self.font_size,
                    );
                } else {
                    let _ = window.paint_glyph(
                        paint_origin,
                        shaped_run.font_id,
                        glyph.id,
                        self.font_size,
                        color,
                    );
                }
            }
        }

        // Cell-aligned decorations: the run's underline/strikethrough cover
        // the run's cell rectangle (`col..col+len`), not the shaper's width.
        let run_left = origin.x + px(f32::from(run.col) * self.metrics.width);
        let run_top = origin.y + px(f32::from(row) * self.metrics.height);
        let run_width = px(f32::from(run.len) * self.metrics.width);
        if let Some(style) = underline {
            window.paint_underline(
                point(
                    run_left,
                    run_top + baseline_offset.y + (line.descent * 0.618),
                ),
                run_width,
                &style,
            );
        }
        if let Some(style) = strikethrough {
            window.paint_strikethrough(
                point(
                    run_left,
                    run_top + ((line.ascent * 0.5) + baseline_offset.y) * 0.5,
                ),
                run_width,
                &style,
            );
        }
    }
}

/// Resolve a run's paint color and cell-aligned decorations from the frame's
/// style table and the run's attribute flags. Both the shaping pass and the
/// per-glyph paint pass share this one mapping.
fn run_paint_style(
    frame: &FrameDelta,
    run: &RunBatch,
    fallback: Hsla,
    palette: palette::TerminalPalette,
) -> (Hsla, Option<UnderlineStyle>, Option<StrikethroughStyle>) {
    let style = frame.styles.get(usize::from(run.style));
    let mut color = style
        .map(|style| {
            if run.flags & 0x0001 != 0 {
                palette::style_background_with_palette(style, palette)
            } else {
                palette::style_foreground_with_palette(style, palette)
            }
        })
        .unwrap_or(fallback);
    if run.flags & 0x0080 != 0 {
        color.a *= 0.66;
    }
    let underline = (run.flags & 0x7808 != 0).then(|| UnderlineStyle {
        thickness: px(1.0),
        color: style
            .and_then(|style| style.underline.as_ref())
            .map(|color| palette::color_to_hsla_with_palette(color, palette)),
        wavy: run.flags & 0x1000 != 0,
    });
    let strikethrough = (run.flags & 0x0200 != 0).then(|| StrikethroughStyle {
        thickness: px(1.0),
        color: None,
    });
    (color, underline, strikethrough)
}

#[cfg(test)]
mod paint_diagnostics_tests {
    use super::*;

    #[test]
    fn paint_diagnostics_is_opt_in() {
        let metrics = CellMetrics::new(8.0, 16.0).unwrap();
        let element = TerminalElement::empty(metrics);
        assert!(element.paint_diagnostics.is_none());
        let element = element.with_paint_diagnostics(|_| {});
        assert!(element.paint_diagnostics.is_some());
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        self.element_id.clone()
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        if self.focus.is_none() {
            self.focus = Some(cx.focus_handle());
        }
        // Fill the parent content viewport (100% of the parent's content
        // box). The app owns the padding — padding precedes grid
        // calculation — so the element's laid-out box is the padded content
        // box, its origin is the content origin, and the grid derives from
        // these bounds at paint time. Never size from a guessed grid.
        let mut style = GpuiStyle::default();
        style.size.width = gpui::Length::Definite(gpui::DefiniteLength::Fraction(1.0));
        style.size.height = gpui::Length::Definite(gpui::DefiniteLength::Fraction(1.0));
        let layout_id = window.request_layout(style, None, cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        if !self.borrowed_focus
            && let Some(focus) = &self.focus
        {
            window.set_focus_handle(focus, cx);
        }
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(hook) = &self.on_paint {
            hook(cx);
        }
        // Clone the Arc so the frame outlives the element-state closure; the
        // frame itself is only borrowed.
        let Some(frame) = self.frame.clone() else {
            return;
        };
        let element = self;
        // Retained paint state (render cache, shaped lines, resize deduper,
        // font identity) lives in GPUI element storage, keyed by the
        // GlobalElementId that `Element::id` establishes. A fresh element
        // instance with the same id therefore merges a Partial frame into
        // the batches retained from the previous instance's Full frame
        // instead of starting from an empty cache. Without an id there is
        // nothing to retain across frames: paint from scratch state that is
        // dropped, so id-less elements still render standalone.
        window.with_optional_element_state::<PaintState, _>(id, |state, window| match state {
            Some(state) => {
                let mut state = state.unwrap_or_default();
                element.paint_frame(&mut state, &frame, bounds, window, cx);
                ((), Some(state))
            }
            None => {
                let mut scratch = PaintState::default();
                element.paint_frame(&mut scratch, &frame, bounds, window, cx);
                ((), None)
            }
        });
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// Stateful GPUI input handler for committed and marked IME text.
#[derive(Clone)]
pub struct TerminalInputHandler {
    inner: Arc<Mutex<ImeState>>,
    sink: Arc<dyn Fn(&str) + Send + Sync>,
}

#[derive(Default)]
struct ImeState {
    marked: String,
    selected: Range<usize>,
    bounds: Option<Bounds<Pixels>>,
}

impl TerminalInputHandler {
    pub fn new(sink: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ImeState::default())),
            sink: Arc::new(sink),
        }
    }

    fn state(&self) -> MutexGuard<'_, ImeState> {
        self.inner.lock()
    }

    fn set_bounds(&self, bounds: Bounds<Pixels>) {
        self.state().bounds = Some(bounds);
    }
}

impl gpui::InputHandler for TerminalInputHandler {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<gpui::UTF16Selection> {
        let state = self.state();
        Some(gpui::UTF16Selection {
            range: state.selected.clone(),
            reversed: false,
        })
    }

    fn marked_text_range(&mut self, _window: &mut Window, _cx: &mut App) -> Option<Range<usize>> {
        let state = self.state();
        (!state.marked.is_empty()).then(|| 0..state.marked.encode_utf16().count())
    }

    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<String> {
        let state = self.state();
        let encoded: Vec<u16> = state.marked.encode_utf16().collect();
        if range_utf16.start > range_utf16.end || range_utf16.end > encoded.len() {
            return None;
        }
        *adjusted_range = Some(range_utf16.clone());
        String::from_utf16(&encoded[range_utf16]).ok()
    }

    fn replace_text_in_range(
        &mut self,
        _replacement_range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        _cx: &mut App,
    ) {
        {
            let mut state = self.state();
            state.marked.clear();
            state.selected = 0..0;
        }
        (self.sink)(text);
        window.refresh();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        _cx: &mut App,
    ) {
        let mut state = self.state();
        state.marked.clear();
        state.marked.push_str(new_text);
        let len = state.marked.encode_utf16().count();
        state.selected = new_selected_range
            .filter(|range| range.start <= range.end && range.end <= len)
            .unwrap_or(len..len);
        drop(state);
        window.refresh();
    }

    fn unmark_text(&mut self, window: &mut Window, _cx: &mut App) {
        let marked = {
            let mut state = self.state();
            state.selected = 0..0;
            std::mem::take(&mut state.marked)
        };
        if !marked.is_empty() {
            (self.sink)(&marked);
        }
        window.refresh();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        self.state().bounds
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<usize> {
        self.state().bounds.map(|_| 0)
    }
}

/// Kitty APC payload bound for the overlay's command parser, mirroring the
/// S6 APC handler's `Protocol::Kitty` default (`65 MiB`). The protocols
/// layer already caps the payload before it reaches the overlay, so the
/// parser bound is a defense-in-depth match, not a second allocation source.
const KITTY_MAX_PAYLOAD_BYTES: usize = 65 * 1024 * 1024;

/// iTerm2 OSC 1337 upload bounds (mirroring `iterm` crate defaults).
const ITERM_MAX_HEADER_BYTES: usize = 64 * 1024;
const ITERM_MAX_CHUNK_BYTES: usize = 1024 * 1024;
const ITERM_MAX_NAME_BYTES: usize = 1024;

/// iTerm2 image ids start above the kitty protocol id space (protocol ids
/// are caller-chosen small integers; implicit kitty ids count down from
/// `i32::MAX`), so collisions are avoided without touching the store's
/// private allocator.
const ITERM_IMAGE_ID_FLOOR: u32 = 1_000_000_000;

fn apply_iterm_size(
    size: Option<mr_crabs_graphics::iterm::ItermSize>,
    horizontal: bool,
    placement: &mut Placement,
) {
    use mr_crabs_graphics::iterm::ItermSize;
    match (size, horizontal) {
        (None | Some(ItermSize::Auto), _) => {}
        (Some(ItermSize::Cells(value)), true) => placement.columns = value,
        (Some(ItermSize::Cells(value)), false) => placement.rows = value,
        (Some(ItermSize::Pixels(value)), true) => placement.pixel_width = Some(value),
        (Some(ItermSize::Pixels(value)), false) => placement.pixel_height = Some(value),
        (Some(ItermSize::Percent(value)), true) => placement.percent_width = Some(value),
        (Some(ItermSize::Percent(value)), false) => placement.percent_height = Some(value),
    }
}

/// Side effects collected while executing graphics commands (S7). Kept as a
/// separate struct so `ImageStore::execute` can borrow the store and the
/// host state disjointly from the same overlay.
#[derive(Default)]
struct HostState {
    responses: Vec<Vec<u8>>,
    cursor_moves: Vec<(u32, u32)>,
    dirty: bool,
}

impl mr_crabs_graphics::host::GraphicsHost for HostState {
    fn write_response(&mut self, bytes: &[u8]) {
        self.responses.push(bytes.to_vec());
    }

    fn cursor_after_placement(&mut self, rows: u32, col: u32) {
        self.cursor_moves.push((rows, col));
    }

    fn storage_changed(&mut self) {
        self.dirty = true;
    }
}

/// One placement visible in the current viewport, with its resolved paint
/// geometry. Collected per paint and z-sorted; bounded by the store's
/// placement budget.
struct VisiblePlacement {
    z: i32,
    tie: (u64, u64),
    image_id: u32,
    generation: u64,
    image_width: u32,
    image_height: u32,
    pixel_width: u32,
    pixel_height: u32,
    placement: Placement,
}

/// The per-surface graphics overlay (S7 integration seam).
///
/// One bounded overlay/store per active surface: it owns the kitty/iTerm2
/// [`ImageStore`] (byte/count budgets), the deterministic LRU
/// [`TextureCache`] (byte/count budgets), the decoded GPUI `RenderImage`s
/// mirroring the texture cache, the iTerm2 chunked-upload accumulator, and
/// the side-effect queues (protocol responses, cursor movement requests)
/// that the app layer drains back into the PTY and the terminal.
///
/// The pane feeds protocol graphics commands here through
/// [`GraphicsOverlay::ingest_kitty`] (kitty APC payloads) and
/// [`GraphicsOverlay::ingest_iterm`] (OSC 1337 `File=` values); the element
/// calls [`GraphicsOverlay::paint`] once per frame with the grid origin.
/// Everything is bounded: the store evicts deterministically, the texture
/// cache evicts deterministically, and `paint` allocates only the visible
/// placement list (≤ `max_placements`).
pub struct GraphicsOverlay {
    store: ImageStore,
    textures: TextureCache,
    render_images: HashMap<TextureKey, Arc<RenderImage>>,
    host: HostState,
    iterm: ItermUploads,
    ctx: TerminalContext,
    next_iterm_image_id: u32,
}

impl GraphicsOverlay {
    /// A bounded overlay with oracle-default budgets (320 MB decoded images,
    /// 4096 textures, kitty transports limited to the direct medium).
    pub fn new() -> Self {
        Self {
            store: ImageStore::new(StoreConfig::default()),
            textures: TextureCache::default(),
            render_images: HashMap::new(),
            host: HostState::default(),
            iterm: ItermUploads::default(),
            ctx: TerminalContext::default(),
            next_iterm_image_id: ITERM_IMAGE_ID_FLOOR,
        }
    }

    /// Replace the terminal context used for placement pinning (ingest) and
    /// visibility/paint geometry (viewport top row, cursor, grid and pixel
    /// dimensions). The pane refreshes this after every feed and resize.
    pub fn set_context(&mut self, ctx: TerminalContext) {
        self.ctx = ctx;
    }

    pub fn context(&self) -> TerminalContext {
        self.ctx
    }

    // ── ingest ──

    /// Ingest one kitty graphics APC payload (the bytes after `\x1b_G`).
    /// The payload is parsed with the kitty `CommandParser`, executed
    /// against the bounded store, and the resulting responses/cursor moves
    /// are queued for the app layer. Malformed or oversized payloads are
    /// dropped without allocation beyond the parser's own bound.
    pub fn ingest_kitty(&mut self, payload: &[u8], ctx: TerminalContext) {
        let mut parser = kitty::CommandParser::new(KITTY_MAX_PAYLOAD_BYTES);
        if parser.feed_slice(payload).is_err() {
            return;
        }
        let Ok(command) = parser.complete() else {
            return;
        };
        self.ctx = ctx;
        let _ = self.store.execute(&self.ctx, &command, &mut self.host);
        self.finish_ingest();
    }

    /// Ingest one OSC 1337 value (the payload after `1337;`, e.g.
    /// `File=name=x;size=100;inline=1;<base64>`). Single-shot uploads are
    /// decoded and placed at the cursor; chunked uploads accumulate in the
    /// bounded upload table until the declared size arrives. Non-inline
    /// (download-only) uploads are dropped: this product has no file
    /// download surface.
    pub fn ingest_iterm(&mut self, value: &str, ctx: TerminalContext) {
        let Some(rest) = value.strip_prefix("File=") else {
            return;
        };
        let Ok(Some(args)) = iterm::parse_file_value(
            rest,
            ITERM_MAX_HEADER_BYTES,
            ITERM_MAX_CHUNK_BYTES,
            ITERM_MAX_NAME_BYTES,
        ) else {
            return;
        };
        let Ok(Some(upload)) = self.iterm.feed(args) else {
            return;
        };
        if !upload.inline {
            return;
        }
        let Ok(decoded) = iterm::load_upload(
            &upload,
            self.store.config().max_image_size,
            self.store.config().max_dimension,
        ) else {
            return;
        };
        self.ctx = ctx;
        let id = self.alloc_iterm_image_id();
        let image = Image {
            id,
            number: 0,
            width: decoded.width,
            height: decoded.height,
            format: ImageFormat::Rgba,
            compression: mr_crabs_graphics::image::Compression::None,
            data: ImageData::Complete(decoded.rgba),
            transient: false,
            implicit_id: true,
            placement_count: 0,
            generation: 0,
        };
        if self.store.add_image(image).is_err() {
            return;
        }
        let mut placement = Placement {
            location: PlacementLocation::Pin {
                row: ctx
                    .viewport_first_row
                    .saturating_add(u64::from(ctx.cursor.y)),
                col: ctx.cursor.x,
            },
            preserve_aspect: upload.preserve_aspect_ratio,
            ..Placement::default()
        };
        apply_iterm_size(upload.width, true, &mut placement);
        apply_iterm_size(upload.height, false, &mut placement);
        let _ = self.store.add_placement(id, 0, placement);
        self.host.dirty = true;
        self.finish_ingest();
    }

    /// Take the queued protocol response bytes (to write back to the PTY).
    pub fn drain_responses(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.host.responses)
    }

    /// Take the queued cursor-movement requests (`rows` down, then set
    /// column `col`) produced by `C=0` placements; the app applies them by
    /// feeding CSI through the terminal engine.
    pub fn drain_cursor_moves(&mut self) -> Vec<(u32, u32)> {
        std::mem::take(&mut self.host.cursor_moves)
    }

    /// Rebuild the texture set after a store mutation: insert a
    /// `RenderImage` per complete stored image (converting RGB/gray wire
    /// formats to RGBA), then drop textures whose image left the store and
    /// render images the bounded cache evicted. Never called from the paint
    /// path.
    fn sync_textures(&mut self) {
        let mut pending = Vec::new();
        for (id, image) in self.store.images() {
            if image.data.bytes().is_none() {
                continue; // incomplete transmission
            }
            let key = TextureKey::new(*id, image.generation);
            if self.render_images.contains_key(&key) {
                continue;
            }
            pending.push((
                *id,
                image.generation,
                rgba_byte_len(image.format, image.data.len()),
            ));
        }
        for (id, generation, rgba_len) in pending {
            let key = TextureKey::new(id, generation);
            if self.textures.insert(key, rgba_len).is_err() {
                continue; // over the texture budget: leave untextured
            }
            let Some(image) = self.store.image_by_id(id) else {
                self.textures.remove(&key);
                continue;
            };
            if image.generation != generation {
                self.textures.remove(&key);
                continue;
            }
            let Some(bytes) = image.data.bytes() else {
                self.textures.remove(&key);
                continue;
            };
            match build_render_image(image.format, image.width, image.height, bytes) {
                Some(render) => {
                    self.render_images.insert(key, render);
                }
                None => {
                    self.textures.remove(&key);
                }
            }
        }
        // Drop textures whose image left the store (delete/evict) and any
        // render images the bounded cache evicted; `get` touches the LRU so
        // resident entries stay hot.
        self.render_images.retain(|key, _| {
            let resident = self.textures.get(key).is_some()
                && self
                    .store
                    .image_by_id(key.image_id)
                    .is_some_and(|image| image.generation == key.generation);
            if !resident {
                self.textures.remove(key);
            }
            resident
        });
    }

    fn finish_ingest(&mut self) {
        if self.host.dirty {
            self.host.dirty = false;
            self.sync_textures();
        }
    }

    fn alloc_iterm_image_id(&mut self) -> u32 {
        let id = self.next_iterm_image_id;
        self.next_iterm_image_id = self
            .next_iterm_image_id
            .wrapping_add(1)
            .max(ITERM_IMAGE_ID_FLOOR);
        id
    }

    // ── paint ──

    /// Paint every placement intersecting the current viewport, z-sorted,
    /// through GPUI's image pipeline. `origin` is the grid origin in window
    /// coordinates; cell sizes come from the stored terminal context so the
    /// placement math agrees with `mr-crabs-graphics::placement`.
    pub fn paint(&mut self, window: &mut Window, origin: Point<Pixels>) {
        let ctx = self.ctx;
        let vfr = ctx.viewport_first_row;
        let rows = u64::from(ctx.rows);
        if rows == 0 {
            return;
        }
        let cell_w = ctx.width_px.checked_div(ctx.cols).unwrap_or(0);
        let cell_h = ctx.height_px.checked_div(ctx.rows).unwrap_or(0);
        if cell_w == 0 || cell_h == 0 {
            return;
        }

        let mut visible: Vec<VisiblePlacement> = Vec::new();
        for (key, placement) in self.store.placements() {
            let Some(image) = self.store.image_by_id(key.image_id) else {
                continue;
            };
            if image.data.bytes().is_none() {
                continue; // pending transmission
            }
            let Some(rect) = placement.rect(image, &ctx) else {
                continue; // virtual placement or degenerate geometry
            };
            if rect.bottom_right.0 < vfr || rect.top_left.0 >= vfr + rows {
                continue; // scrolled out of the visible window
            }
            let px_size = placement.pixel_size(image, &ctx);
            if px_size.width == 0 || px_size.height == 0 {
                continue;
            }
            let tie = match key.placement_id {
                PlacementId::Internal(id) => (0u64, u64::from(id)),
                PlacementId::External(id) => (1u64, u64::from(id)),
            };
            visible.push(VisiblePlacement {
                z: placement.z,
                tie,
                image_id: key.image_id,
                generation: image.generation,
                image_width: image.width,
                image_height: image.height,
                pixel_width: px_size.width,
                pixel_height: px_size.height,
                placement: *placement,
            });
        }
        // Deterministic z-order: z, then external-before-internal placement
        // id, then image id (stable across map iteration).
        visible.sort_by_key(|v| (v.z, v.tie, v.image_id));

        for item in visible {
            let key = TextureKey::new(item.image_id, item.generation);
            if self.textures.get(&key).is_none() {
                continue; // evicted by the bounded cache
            }
            let Some(render) = self.render_images.get(&key) else {
                continue;
            };
            let PlacementLocation::Pin { row, col } = item.placement.location else {
                continue;
            };
            let screen_row = u32::try_from(row.saturating_sub(vfr)).unwrap_or(u32::MAX);
            // Grid positions are u32-bounded (the store saturates placement
            // math), so the pixel conversions are plain `as f32` casts.
            let x = (col * cell_w + item.placement.x_offset) as f32;
            let y = (screen_row * cell_h + item.placement.y_offset) as f32;
            let dest = Bounds::new(
                point(origin.x + px(x), origin.y + px(y)),
                size(px(item.pixel_width as f32), px(item.pixel_height as f32)),
            );
            // The destination maps the placement's source rectangle
            // (source_width x source_height, or the whole image) onto
            // pixel_size. `image_bounds` positions the full image at the
            // same scale so the sprite-atlas sub-tile sampling selects the
            // source rectangle exactly.
            let src_w = if item.placement.source_width > 0 {
                item.placement.source_width
            } else {
                item.image_width
            };
            let src_h = if item.placement.source_height > 0 {
                item.placement.source_height
            } else {
                item.image_height
            };
            if src_w == 0 || src_h == 0 {
                continue;
            }
            let scale_x = item.pixel_width as f32 / src_w as f32;
            let scale_y = item.pixel_height as f32 / src_h as f32;
            let image_bounds = Bounds::new(
                point(
                    dest.origin.x - px(item.placement.source_x as f32 * scale_x),
                    dest.origin.y - px(item.placement.source_y as f32 * scale_y),
                ),
                size(
                    px(item.image_width as f32 * scale_x),
                    px(item.image_height as f32 * scale_y),
                ),
            );
            let _ = window.paint_image(
                dest,
                image_bounds,
                Corners::default(),
                Arc::clone(render),
                0,
                false,
            );
        }
    }

    pub fn prune_history(&mut self, min_row: u64) {
        self.store.prune_history(min_row);
        self.finish_ingest();
    }

    // ── boundedness accessors (tests) ──

    pub fn image_count(&self) -> usize {
        self.store.image_count()
    }

    pub fn placement_count(&self) -> usize {
        self.store.placement_count()
    }

    pub fn texture_count(&self) -> usize {
        self.render_images.len()
    }

    pub fn texture_bytes(&self) -> usize {
        self.textures.total_bytes()
    }
}

impl Default for GraphicsOverlay {
    fn default() -> Self {
        Self::new()
    }
}

/// Decoded RGBA byte length of a stored image's wire payload; the texture
/// budget counts what is actually uploaded to the atlas.
fn rgba_byte_len(format: ImageFormat, wire_len: usize) -> usize {
    match format {
        ImageFormat::Rgba => wire_len,
        ImageFormat::Rgb => wire_len / 3 * 4,
        ImageFormat::Gray => wire_len * 4,
        ImageFormat::GrayAlpha => wire_len * 2,
        ImageFormat::Png => 0,
    }
}

/// Build a GPUI `RenderImage` (RGBA) from a stored image's wire payload,
/// expanding RGB/gray formats. Returns `None` for unsupported formats or a
/// payload whose length does not match `width * height * 4` after expansion.
fn build_render_image(
    format: ImageFormat,
    width: u32,
    height: u32,
    data: &[u8],
) -> Option<Arc<RenderImage>> {
    let rgba = match format {
        ImageFormat::Rgba => data.to_vec(),
        ImageFormat::Rgb => {
            let mut out = Vec::with_capacity(data.len() / 3 * 4);
            for pixel in data.chunks_exact(3) {
                out.extend_from_slice(&pixel[..3]);
                out.push(255);
            }
            out
        }
        ImageFormat::Gray => {
            let mut out = Vec::with_capacity(data.len() * 4);
            for &gray in data {
                out.extend_from_slice(&[gray, gray, gray, 255]);
            }
            out
        }
        ImageFormat::GrayAlpha => {
            let mut out = Vec::with_capacity(data.len() * 2);
            for channel in data.chunks_exact(2) {
                out.extend_from_slice(&[channel[0], channel[0], channel[0], channel[1]]);
            }
            out
        }
        ImageFormat::Png => return None,
    };
    if rgba.len() != (width as usize) * (height as usize) * 4 {
        return None;
    }
    let image = image::RgbaImage::from_raw(width, height, rgba)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(image)])))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mr_crabs_effects::PointPx;
    use mr_crabs_terminal::{
        CursorShape, CursorState, DamageKind, FramePool, GridSize, RowDelta, Run, SelectionState,
        Style as TermStyle,
    };

    const METRICS: CellMetrics = CellMetrics {
        width: 10.0,
        height: 20.0,
    };

    fn sample_frame() -> FrameDelta {
        // `FrameDelta` carries a crate-private `spare_rows` stash, so the
        // public construction path is `FramePool::acquire` (re-stamps
        // identity) followed by field mutation.
        let mut pool = FramePool::new(1);
        let mut frame = pool.acquire(1, GridSize::new(4, 2));
        frame.damage = DamageKind::Full;
        frame.rows = vec![RowDelta {
            row: 0,
            generation: 1,
            cells: vec![
                mr_crabs_terminal::Cell {
                    content: u32::from('h'),
                    style: 0,
                    flags: 0,
                },
                mr_crabs_terminal::Cell {
                    content: u32::from('i'),
                    style: 0,
                    flags: 0,
                },
            ],
            runs: vec![Run {
                start_col: 0,
                len: 2,
                style: 0,
            }],
        }];
        frame.cursor = CursorState {
            row: 0,
            col: 2,
            shape: CursorShape::Block,
            blinking: false,
            visible: true,
            wrap_pending: false,
        };
        frame.selection = SelectionState {
            start: None,
            end: None,
            active: false,
            kind: mr_crabs_terminal::SelectionKind::Linear,
        };
        frame.styles = vec![TermStyle::default()];
        frame
    }

    #[test]
    fn constructor_owns_frame_and_defaults() {
        let element = TerminalElement::new(sample_frame(), METRICS);
        assert!(element.frame().is_some());
        assert_eq!(element.metrics(), METRICS);
        assert_eq!(element.font_size, px(METRICS.height));
        // The base shaping font defaults to Menlo at the default
        // weight/style; the paint-time identity compares family + features +
        // size + metrics, never a per-instance counter.
        assert_eq!(element.font.family.as_str(), "Menlo");
        assert_eq!(element.font.weight, FontWeight::default());
        assert_eq!(element.font.style, FontStyle::default());
        assert_eq!(element.content_origin(), None);
        assert_eq!(element.focus, None);
        // No element id: without `with_element_id`, paint uses scratch state
        // that is not retained across frames.
        assert_eq!(element.id(), None);
    }

    #[test]
    fn with_shared_keeps_the_same_arc() {
        let shared = Arc::new(sample_frame());
        let element = TerminalElement::with_shared(shared.clone(), METRICS);
        assert!(Arc::ptr_eq(element.frame().unwrap(), &shared));
    }

    #[test]
    fn builders_apply() {
        let origin = point(px(40.0), px(60.0));
        let element = TerminalElement::new(sample_frame(), METRICS)
            .with_content_origin(origin)
            .with_font_size(px(12.0));
        assert_eq!(element.content_origin(), Some(origin));
        assert_eq!(element.font_size, px(12.0));

        // The content origin is optional: an unpinned element derives its
        // origin from its layout bounds at paint time.
        let plain = TerminalElement::new(sample_frame(), METRICS);
        assert_eq!(plain.content_origin(), None);
    }

    #[test]
    fn empty_element_has_no_frame() {
        let element = TerminalElement::empty(METRICS);
        assert!(element.frame().is_none());
    }

    #[test]
    fn set_frame_replaces_and_shares() {
        let mut element = TerminalElement::new(sample_frame(), METRICS);
        let second = Arc::new(sample_frame());
        element.set_shared_frame(second.clone());
        assert!(Arc::ptr_eq(element.frame().unwrap(), &second));
        element.set_frame(sample_frame());
        assert!(element.frame().is_some());
    }
    #[test]
    fn with_element_id_keys_stable_identity_across_instances() {
        let pane_a = ElementId::NamedInteger(SharedString::new_static("mr-crabs-terminal"), 7);
        let pane_b = ElementId::NamedInteger(SharedString::new_static("mr-crabs-terminal"), 8);

        // Instance A and instance B with the same element id report the same
        // id, so GPUI keys their retained paint state together...
        let instance_a =
            TerminalElement::new(sample_frame(), METRICS).with_element_id(pane_a.clone());
        let instance_b =
            TerminalElement::new(sample_frame(), METRICS).with_element_id(pane_a.clone());
        assert_eq!(instance_a.id(), Some(pane_a.clone()));
        assert_eq!(instance_b.id(), instance_a.id());

        // ...while a different id (a different pane) keys separate state and
        // must not inherit the other element's rows.
        let other_pane = TerminalElement::new(sample_frame(), METRICS).with_element_id(pane_b);
        assert_ne!(other_pane.id(), instance_a.id());

        // Constructors without the builder stay id-less and unkeyed.
        let plain = TerminalElement::new(sample_frame(), METRICS);
        assert_eq!(plain.id(), None);
    }

    // ── retained paint state across element reconstruction ──

    /// A Full frame covering rows 0 and 1 (4x2 grid), like the first paint
    /// of a freshly sized surface.
    fn full_two_row_frame(pool: &mut FramePool, sequence: u64) -> FrameDelta {
        text_frame(pool, sequence, DamageKind::Full, vec![(0, "ab"), (1, "cd")])
    }

    /// A Partial one-row delta, like a SIGWINCH/prompt redraw on row 0.
    fn partial_row_zero_frame(pool: &mut FramePool, sequence: u64) -> FrameDelta {
        text_frame(pool, sequence, DamageKind::Partial, vec![(0, "xy")])
    }

    fn text_frame(
        pool: &mut FramePool,
        sequence: u64,
        damage: DamageKind,
        rows: Vec<(u16, &str)>,
    ) -> FrameDelta {
        let mut frame = pool.acquire(sequence, GridSize::new(4, 2));
        frame.damage = damage;
        frame.rows = rows
            .into_iter()
            .map(|(row, text)| RowDelta {
                row,
                generation: 1,
                cells: text
                    .chars()
                    .map(|ch| mr_crabs_terminal::Cell {
                        content: u32::from(ch),
                        style: 0,
                        flags: 0,
                    })
                    .collect(),
                runs: vec![Run {
                    start_col: 0,
                    len: text.chars().count() as u16,
                    style: 0,
                }],
            })
            .collect();
        frame.cursor = CursorState {
            row: 0,
            col: 0,
            shape: CursorShape::Block,
            blinking: false,
            visible: true,
            wrap_pending: false,
        };
        frame.selection = SelectionState {
            start: None,
            end: None,
            active: false,
            kind: mr_crabs_terminal::SelectionKind::Linear,
        };
        frame.styles = vec![TermStyle::default()];
        frame
    }

    fn identity(family: &str, font_size: f32, metrics: CellMetrics) -> FontIdentity {
        FontIdentity {
            font: font(family),
            font_size: px(font_size),
            metrics,
        }
    }

    #[test]
    fn effects_model_rebuilds_when_cell_metrics_change() {
        let mut state = PaintState::default();
        let config = EffectsConfig::default();
        let size = GridSize::new(80, 24);
        let initial = CellPx::new(10.0, 20.0);
        let resized = CellPx::new(12.0, 24.0);

        assert_eq!(state.effects_model(config, size, initial).cell(), initial);
        assert_eq!(state.effects_model(config, size, resized).cell(), resized);
    }

    #[test]
    fn retained_state_merges_partial_into_full_across_reconstructed_instances() {
        let id = identity("Menlo", 20.0, METRICS);
        let mut pool = FramePool::new(2);

        // Instance A paints a Full frame and stores the paint state in GPUI
        // element storage keyed by its element id.
        let mut retained = PaintState::default();
        retained.apply_frame(&full_two_row_frame(&mut pool, 1), &id);
        assert_eq!(retained.cache.batches().len(), 2);

        // Instance B is a fresh TerminalElement with the SAME element id;
        // GPUI hands back the retained state, so a Partial one-row delta
        // merges into the prior Full rows instead of starting empty.
        let action = retained.apply_frame(&partial_row_zero_frame(&mut pool, 2), &id);
        assert!(action.needs_redraw);
        assert_eq!(
            retained.cache.batches().len(),
            2,
            "untouched rows from instance A's Full frame must survive"
        );
        assert_eq!(retained.cache.batches()[0].runs[0].text.as_str(), "xy");
        assert_eq!(retained.cache.batches()[1].row, 1);
        assert_eq!(retained.cache.batches()[1].runs[0].text.as_str(), "cd");

        // The shaped-line array is kept parallel to the retained batches by
        // the same sync the paint path runs after a rebuild.
        retained.sync_shaped_rows();
        assert_eq!(retained.shaped_lines.len(), 2);
    }

    #[test]
    fn different_element_id_does_not_inherit_retained_rows() {
        let id = identity("Menlo", 20.0, METRICS);
        let mut pool = FramePool::new(2);

        // One element id retains a Full frame...
        let mut retained = PaintState::default();
        retained.apply_frame(&full_two_row_frame(&mut pool, 1), &id);
        assert_eq!(retained.cache.batches().len(), 2);

        // ...but a different element id keys different GPUI element state: a
        // fresh PaintState starts empty, so the same Partial delta alone
        // paints only its own row. Nothing is inherited across ids.
        let mut other = PaintState::default();
        other.apply_frame(&partial_row_zero_frame(&mut pool, 2), &id);
        assert_eq!(other.cache.batches().len(), 1);
        assert_eq!(other.cache.batches()[0].row, 0);
        assert_eq!(other.cache.batches()[0].runs[0].text.as_str(), "xy");
    }

    #[test]
    fn same_font_identity_never_resets_retained_rows() {
        let id = identity("Menlo", 20.0, METRICS);
        let mut pool = FramePool::new(3);
        let mut retained = PaintState::default();
        retained.apply_frame(&full_two_row_frame(&mut pool, 1), &id);

        // A reconstructed instance painting with the same shaping identity:
        // no reset, so the Partial row merges and the untouched Full rows
        // survive.
        let action = retained.apply_frame(&partial_row_zero_frame(&mut pool, 2), &id);
        assert!(action.needs_redraw);
        assert_eq!(retained.painted_font.as_ref(), Some(&id));
        assert_eq!(retained.cache.batches().len(), 2);
        assert_eq!(retained.cache.batches()[1].runs[0].text.as_str(), "cd");

        // A Clean frame with the retained sequence is a pure repaint no-op:
        // a spurious reset would force a rebuild and request redraw.
        let clean = text_frame(&mut pool, 2, DamageKind::Clean, Vec::new());
        let action = retained.apply_frame(&clean, &id);
        assert!(!action.needs_redraw);
        assert_eq!(retained.cache.batches().len(), 2);
    }

    #[test]
    fn changed_font_identity_invalidates_shaping_once_without_dropping_rows() {
        let menlo = identity("Menlo", 20.0, METRICS);
        let monaco = identity("Monaco", 20.0, METRICS);
        let mut pool = FramePool::new(3);
        let mut retained = PaintState::default();
        retained.apply_frame(&full_two_row_frame(&mut pool, 1), &menlo);
        assert_eq!(retained.cache.batches().len(), 2);

        // A different family changes the shaping identity. Cached terminal
        // rows are font-independent and remain intact while the next paint
        // reshapes every retained run with the new font.
        let action = retained.apply_frame(&partial_row_zero_frame(&mut pool, 2), &monaco);
        assert!(action.needs_redraw);
        assert_eq!(retained.painted_font.as_ref(), Some(&monaco));
        assert_eq!(retained.cache.batches().len(), 2);
        assert_eq!(retained.cache.batches()[0].runs[0].text.as_str(), "xy");
        assert_eq!(retained.cache.batches()[1].runs[0].text.as_str(), "cd");
        retained.sync_shaped_rows();
        assert_eq!(retained.shaped_lines.len(), 2);

        // The same identity again does not invalidate a second time: a Clean
        // frame with the retained sequence stays a repaint no-op.
        let clean = text_frame(&mut pool, 2, DamageKind::Clean, Vec::new());
        let action = retained.apply_frame(&clean, &monaco);
        assert!(!action.needs_redraw);

        // Font size and cell metrics are part of the identity: each change
        // invalidates once, forcing a rebuild even for a Clean frame.
        let bigger = identity("Monaco", 24.0, METRICS);
        let action = retained.apply_frame(&clean, &bigger);
        assert!(action.needs_redraw, "a font-size change must invalidate");
        assert_eq!(retained.painted_font.as_ref(), Some(&bigger));
        let narrower = identity(
            "Monaco",
            24.0,
            CellMetrics {
                width: 9.0,
                height: 18.0,
            },
        );
        let action = retained.apply_frame(&clean, &narrower);
        assert!(action.needs_redraw, "a cell-metrics change must invalidate");
        assert_eq!(retained.painted_font.as_ref(), Some(&narrower));
    }

    #[test]
    fn font_features_are_part_of_the_shaping_identity() {
        let plain = identity("Menlo", 20.0, METRICS);
        let mut liga = plain.clone();
        liga.font.features = FontFeatures(Arc::new(vec![(String::from("liga"), 0u32)]));
        assert_ne!(liga, plain, "the test fixture must differ in features");

        let mut pool = FramePool::new(2);
        let mut retained = PaintState::default();
        retained.apply_frame(&full_two_row_frame(&mut pool, 1), &plain);

        // Same family and size but different OpenType features: the identity
        // differs, so every retained run is reshaped without dropping
        // undamaged terminal rows.
        let action = retained.apply_frame(&partial_row_zero_frame(&mut pool, 2), &liga);
        assert!(action.needs_redraw);
        assert_eq!(retained.painted_font.as_ref(), Some(&liga));
        assert_eq!(retained.cache.batches().len(), 2);
        assert_eq!(retained.cache.batches()[1].runs[0].text.as_str(), "cd");
    }

    #[test]
    fn resize_deduper_is_retained_across_reconstructed_instances() {
        let mut retained = PaintState::default();
        // Instance A's paint offers the measured grid...
        assert!(retained.deduper.offer(GridSize::new(80, 24)));
        // Instance B's paint with the same grid must not re-emit the resize
        // (no resize spam from element reconstruction)...
        assert!(!retained.deduper.offer(GridSize::new(80, 24)));
        // ...and a real change still emits exactly once.
        assert!(retained.deduper.offer(GridSize::new(100, 30)));
        assert!(!retained.deduper.offer(GridSize::new(100, 30)));
    }

    #[test]
    fn input_handler_is_stateful_and_registrable() {
        fn assert_input_handler<T: gpui::InputHandler>() {}
        assert_input_handler::<TerminalInputHandler>();
        let committed = Arc::new(Mutex::new(String::new()));
        let output = Arc::clone(&committed);
        let handler = TerminalInputHandler::new(move |text| {
            output.lock().push_str(text);
        });
        assert!(handler.state().marked.is_empty());
        assert!(committed.lock().is_empty());
    }

    #[test]
    fn element_bounds_are_satisfied() {
        fn assert_element<E: Element>() {}
        fn assert_into_element<E: IntoElement>() {}
        assert_element::<TerminalElement>();
        assert_into_element::<TerminalElement>();
    }

    // ── S7 graphics overlay ──

    use base64::Engine;

    /// Raw 20x15 RGB corpus fixture (shared with mr-crabs-graphics tests).
    const RGB_20X15: &[u8] = include_bytes!(
        "../../../verification/graphics-corpus/fixtures/image-rgb-none-20x15-2147483647-raw.data"
    );

    /// 4x4 PNG, base64 (mirrors the iterm crate test fixture).
    const PNG_4X4_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAYAAACp8Z5+AAAAPUlEQVR42g3KMQHAMBACQOREBCJeDiNSXgQiIicOaG8+ADBxKowDFeApORbVcA1oTKnSOrr/iMqsldvk+QNmXR65+p5O5AAAAABJRU5ErkJggg==";

    fn gfx_ctx() -> TerminalContext {
        TerminalContext {
            viewport_first_row: 10,
            cursor: mr_crabs_graphics::placement::Point { x: 2, y: 3 },
            cols: 80,
            rows: 24,
            width_px: 800,
            height_px: 600,
        }
    }

    #[test]
    fn overlay_kitty_transmit_display_ingests_and_textures() {
        let mut overlay = GraphicsOverlay::new();
        let ctx = gfx_ctx();

        // Transmit-and-display (a=T) a 20x15 RGB image with grid c=2,r=3.
        let b64 = base64::engine::general_purpose::STANDARD.encode(RGB_20X15);
        let payload = format!("a=T,t=d,f=24,s=20,v=15,i=1,c=2,r=3;{b64}");
        overlay.ingest_kitty(payload.as_bytes(), ctx);

        assert_eq!(overlay.image_count(), 1);
        assert_eq!(overlay.placement_count(), 1);
        // The texture mirrors the store: one RGBA entry (20*15*4 bytes).
        assert_eq!(overlay.texture_count(), 1);
        assert_eq!(overlay.texture_bytes(), 20 * 15 * 4);

        // The response is queued for the pane to write back to the PTY.
        let responses = overlay.drain_responses();
        assert_eq!(responses, vec![b"\x1b_Gi=1;OK\x1b\\".to_vec()]);
        // C=0 (default): cursor moves 3 rows down and to column 5
        // (0-based; placement col 2 + width 2 + 1).
        assert_eq!(overlay.drain_cursor_moves(), vec![(3, 5)]);
    }

    #[test]
    fn overlay_kitty_delete_prunes_textures() {
        let mut overlay = GraphicsOverlay::new();
        let ctx = gfx_ctx();
        let b64 = base64::engine::general_purpose::STANDARD.encode(RGB_20X15);
        overlay.ingest_kitty(format!("a=t,t=d,f=24,s=20,v=15,i=1;{b64}").as_bytes(), ctx);
        assert_eq!(overlay.texture_count(), 1);

        // d=A deletes every placement and image; the texture set follows.
        overlay.ingest_kitty(b"a=d,d=A", ctx);
        assert_eq!(overlay.image_count(), 0);
        assert_eq!(overlay.placement_count(), 0);
        assert_eq!(overlay.texture_count(), 0);
        assert_eq!(overlay.texture_bytes(), 0);
    }

    #[test]
    fn overlay_kitty_malformed_payload_is_dropped() {
        let mut overlay = GraphicsOverlay::new();
        overlay.ingest_kitty(b"a=t,t=d,f=24,s=20,v=15,i=1;!!!not-base64!!!", gfx_ctx());
        assert_eq!(overlay.image_count(), 0);
        assert_eq!(overlay.placement_count(), 0);
        assert_eq!(overlay.texture_count(), 0);
        assert!(overlay.drain_responses().is_empty());
        assert!(overlay.drain_cursor_moves().is_empty());
    }

    #[test]
    fn overlay_iterm_inline_upload_places_at_cursor() {
        let mut overlay = GraphicsOverlay::new();
        let ctx = gfx_ctx();
        let value = format!("File=name=test.png;size=118;inline=1;{PNG_4X4_B64}");
        overlay.ingest_iterm(&value, ctx);

        assert_eq!(overlay.image_count(), 1);
        assert_eq!(overlay.placement_count(), 1);
        assert_eq!(overlay.texture_count(), 1);
        // 4x4 RGBA.
        assert_eq!(overlay.texture_bytes(), 4 * 4 * 4);
        // Inline uploads never respond and never move the cursor.
        assert!(overlay.drain_responses().is_empty());
        assert!(overlay.drain_cursor_moves().is_empty());
    }

    #[test]
    fn overlay_iterm_download_only_upload_is_dropped() {
        let mut overlay = GraphicsOverlay::new();
        let value = format!("File=name=test.png;size=118;inline=0;{PNG_4X4_B64}");
        overlay.ingest_iterm(&value, gfx_ctx());
        assert_eq!(overlay.image_count(), 0);
        assert_eq!(overlay.placement_count(), 0);
    }

    #[test]
    fn build_render_image_expands_rgb_to_rgba() {
        let mut rgb = Vec::new();
        for i in 0..(2 * 2) {
            rgb.extend_from_slice(&[i as u8, 128, 255]);
        }
        let render = build_render_image(ImageFormat::Rgb, 2, 2, &rgb).expect("render image");
        let bytes = render.as_bytes(0).expect("frame bytes");
        assert_eq!(bytes.len(), 2 * 2 * 4);
        assert_eq!(&bytes[0..4], &[0, 128, 255, 255]);
        assert_eq!(&bytes[4..8], &[1, 128, 255, 255]);
        assert_eq!(render.frame_count(), 1);
        assert_eq!(
            render.size(0),
            gpui::size(gpui::DevicePixels(2), gpui::DevicePixels(2))
        );
    }

    #[test]
    fn overlay_context_is_stored_and_replaceable() {
        let mut overlay = GraphicsOverlay::new();
        assert_eq!(overlay.context(), TerminalContext::default());
        let ctx = gfx_ctx();
        overlay.set_context(ctx);
        assert_eq!(overlay.context(), ctx);
    }

    #[test]
    fn trail_glow_translates_by_origin() {
        let glow = RectPx::new(12.0, 8.0, 10.0, 20.0);
        let origin = point(px(5.0), px(7.0));
        let bounds = trail_glow_bounds(glow, origin).expect("bounds");
        assert_eq!(bounds.origin.x, px(17.0));
        assert_eq!(bounds.origin.y, px(15.0));
        assert_eq!(bounds.size.width, px(10.0));
        assert_eq!(bounds.size.height, px(20.0));
    }

    #[test]
    fn trail_glow_reports_degenerate_as_none() {
        assert!(
            trail_glow_bounds(RectPx::new(0.0, 0.0, 0.0, 10.0), point(px(0.0), px(0.0))).is_none()
        );
        assert!(
            trail_glow_bounds(RectPx::new(0.0, 0.0, 10.0, 0.0), point(px(0.0), px(0.0))).is_none()
        );
    }

    #[test]
    fn trail_segment_points_translate_by_origin() {
        let origin = point(px(3.0), px(4.0));
        let seg = LinePx::new(PointPx::new(1.0, 2.0), PointPx::new(10.0, 12.0));
        let (from, to) = trail_segment_points(seg, origin);
        assert_eq!(from, point(px(4.0), px(6.0)));
        assert_eq!(to, point(px(13.0), px(16.0)));
    }

    #[test]
    fn trail_stroke_width_clamps_at_one() {
        assert_eq!(trail_stroke_width(10.0), px(5.0));
        assert_eq!(trail_stroke_width(0.2), px(1.0));
        assert_eq!(trail_stroke_width(1.0), px(1.0));
    }

    #[test]
    fn trail_segment_stroke_builds_nonempty_path() {
        let origin = point(px(0.0), px(0.0));
        let seg = LinePx::new(PointPx::new(0.0, 0.0), PointPx::new(20.0, 0.0));
        let (from, to) = trail_segment_points(seg, origin);
        let mut builder = PathBuilder::stroke(px(4.0));
        builder.move_to(from);
        builder.line_to(to);
        let path = builder.build().expect("path");
        assert_ne!(
            format!("{path:?}"),
            format!("{:?}", PathBuilder::stroke(px(1.0)).build().unwrap())
        );
    }
}
