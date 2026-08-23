use std::path::Path;

use image::AnimationDecoder;
use image::codecs::gif::GifDecoder;

use mr_crabs_terminal::{Cell, GridSize, NormalizedColor, Style};

const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
const MAX_FRAMES: usize = 256;
const MAX_CANVAS: u32 = 512;
const MIN_DELAY_MS: u64 = 16;
const MAX_DELAY_MS: u64 = 5000;
const FALLBACK_DELAY_MS: u64 = 100;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoopPolicy {
    Forever,
    Once,
    Count(u16),
}

#[derive(Clone, Debug)]
pub struct FetchFrame {
    pub delay_ms: u64,
    pub cells: Vec<Cell>,
}

#[derive(Clone, Debug)]
pub struct FetchAnimation {
    pub size: GridSize,
    pub loop_policy: LoopPolicy,
    pub frames: Vec<FetchFrame>,
    pub styles: Vec<Style>,
}

#[derive(Clone, Debug)]
pub struct FetchDriver {
    animation: Option<FetchAnimation>,
    index: usize,
    next_deadline: u64,
    now_ms: u64,
    playing: bool,
    origin: (u16, u16),
    remaining: Option<u16>,
    invalidated: bool,
}

fn clamp_delay(raw_ms: u64) -> u64 {
    if raw_ms == 0 {
        return FALLBACK_DELAY_MS;
    }
    raw_ms.clamp(MIN_DELAY_MS, MAX_DELAY_MS)
}

fn validate_bytes(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err("gif too large".to_string());
    }
    if bytes.is_empty() {
        return Err("empty gif".to_string());
    }
    Ok(())
}

pub fn decode_gif_bytes(bytes: &[u8], grid: GridSize) -> Result<FetchAnimation, String> {
    validate_bytes(bytes)?;
    let decoder =
        GifDecoder::new(std::io::Cursor::new(bytes)).map_err(|e| format!("gif decode: {e}"))?;
    let frames = decoder
        .into_frames()
        .collect_frames()
        .map_err(|e| format!("gif frames: {e}"))?;
    if frames.is_empty() {
        return Err("no frames".to_string());
    }
    if frames.len() > MAX_FRAMES {
        return Err("too many frames".to_string());
    }
    let loop_policy = LoopPolicy::Forever;
    let first = &frames[0];
    let (w, h) = (first.buffer().width(), first.buffer().height());
    if w == 0 || h == 0 || w > MAX_CANVAS || h > MAX_CANVAS {
        return Err("canvas too large".to_string());
    }
    let grid_cols = grid.cols as u32;
    let grid_rows = grid.rows as u32;
    if grid_cols == 0 || grid_rows == 0 {
        return Err("zero grid".to_string());
    }
    let scale = {
        let sx = grid_cols as f32 / w as f32;
        let sy = (grid_rows * 2) as f32 / h as f32;
        sx.min(sy).min(1.0)
    };
    let target_w = ((w as f32 * scale) as u32).max(1).min(grid_cols) as u16;
    let target_h_px = ((h as f32 * scale) as u32).max(1).min(grid_rows * 2);
    let target_h = target_h_px.div_ceil(2) as u16;
    let size = GridSize::new(target_w, target_h);
    let mut styles = vec![Style::default()];
    use std::collections::HashMap;
    let mut style_map: HashMap<Style, u16> = HashMap::new();
    style_map.insert(Style::default(), 0);
    let mut fetch_frames = Vec::with_capacity(frames.len());
    for frame in frames {
        let delay = frame.delay();
        let raw_ms = delay.numer_denom_ms().0 as u64;
        let delay_ms = clamp_delay(raw_ms);
        let buf = frame.buffer();
        let (fw, fh) = (buf.width(), buf.height());
        let mut cells = Vec::with_capacity(usize::from(target_w) * usize::from(target_h));
        for row in 0..target_h {
            for col in 0..target_w {
                let src_x = (col as u32 * fw / target_w as u32).min(fw - 1);
                let top_y = (row as u32 * 2 * fh / (target_h as u32 * 2)).min(fh - 1);
                let bot_y = (row as u32 * 2 + 1) * fh / (target_h as u32 * 2);
                let bot_y = bot_y.min(fh - 1);
                let top = buf.get_pixel(src_x, top_y);
                let bot = if fh > 1 {
                    buf.get_pixel(src_x, bot_y)
                } else {
                    top
                };
                let top_a = top[3];
                let bot_a = bot[3];
                let (ch, style) = if top_a < 128 && bot_a < 128 {
                    (' ', Style::default())
                } else if top_a >= 128 && bot_a < 128 {
                    (
                        '\u{2580}',
                        Style {
                            foreground: NormalizedColor::Rgb([top[0], top[1], top[2]]),
                            background: NormalizedColor::Named(
                                mr_crabs_terminal::NamedColorValue::Background,
                            ),
                            underline: None,
                        },
                    )
                } else if top_a < 128 && bot_a >= 128 {
                    (
                        '\u{2584}',
                        Style {
                            foreground: NormalizedColor::Rgb([bot[0], bot[1], bot[2]]),
                            background: NormalizedColor::Named(
                                mr_crabs_terminal::NamedColorValue::Background,
                            ),
                            underline: None,
                        },
                    )
                } else {
                    (
                        '\u{2580}',
                        Style {
                            foreground: NormalizedColor::Rgb([top[0], top[1], top[2]]),
                            background: NormalizedColor::Rgb([bot[0], bot[1], bot[2]]),
                            underline: None,
                        },
                    )
                };
                let style_id = if style == Style::default() {
                    0
                } else if let Some(&id) = style_map.get(&style) {
                    id
                } else {
                    let id = styles.len() as u16;
                    if styles.len() >= 65535 {
                        return Err("style overflow".to_string());
                    }
                    styles.push(style.clone());
                    style_map.insert(style.clone(), id);
                    id
                };
                cells.push(Cell {
                    content: ch as u32,
                    style: style_id,
                    flags: 0,
                });
            }
        }
        fetch_frames.push(FetchFrame { delay_ms, cells });
    }
    Ok(FetchAnimation {
        size,
        loop_policy,
        frames: fetch_frames,
        styles,
    })
}

pub fn load_fetch_animation(path: &Path, grid: GridSize) -> Option<FetchAnimation> {
    if path.as_os_str().is_empty() {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() > MAX_FILE_BYTES {
        return None;
    }
    decode_gif_bytes(&bytes, grid).ok()
}

impl FetchDriver {
    pub fn new(animation: Option<FetchAnimation>) -> Self {
        let playing = animation.as_ref().is_some_and(|a| a.frames.len() > 1);
        let remaining = animation
            .as_ref()
            .and_then(|animation| match animation.loop_policy {
                LoopPolicy::Forever => None,
                LoopPolicy::Once => Some(1),
                LoopPolicy::Count(count) => Some(count),
            });
        let next_deadline = animation
            .as_ref()
            .filter(|_| playing)
            .and_then(|animation| animation.frames.first())
            .map_or(u64::MAX, |frame| frame.delay_ms);
        Self {
            animation,
            index: 0,
            next_deadline,
            now_ms: 0,
            playing,
            origin: (0, 0),
            remaining,
            invalidated: false,
        }
    }

    pub fn from_path(path: &Path, grid: GridSize) -> Self {
        let anim = load_fetch_animation(path, grid);
        Self::new(anim)
    }

    pub fn is_active(&self) -> bool {
        !self.invalidated && self.animation.is_some() && self.playing
    }

    pub fn needs_frame(&self) -> bool {
        self.is_active()
    }

    pub fn next_deadline_ms(&self) -> Option<u64> {
        self.is_active().then_some(self.next_deadline)
    }

    pub fn size(&self) -> Option<GridSize> {
        self.animation.as_ref().map(|a| a.size)
    }

    pub fn origin(&self) -> (u16, u16) {
        self.origin
    }

    pub fn animation_region(&self) -> Option<(u16, u16, GridSize)> {
        if self.invalidated {
            return None;
        }
        let size = self.size()?;
        Some((self.origin.0, self.origin.1, size))
    }

    pub fn invalidate(&mut self) {
        self.invalidated = true;
        self.playing = false;
    }

    pub fn on_resize(&mut self, grid: GridSize, path: &Path) {
        if self.invalidated {
            return;
        }
        let Some(anim) = self.animation.take() else {
            return;
        };
        if grid.cols == 0 || grid.rows == 0 {
            self.animation = Some(anim);
            return;
        }
        let clamped_origin = (
            self.origin.0.min(grid.rows.saturating_sub(anim.size.rows)),
            self.origin.1.min(grid.cols.saturating_sub(anim.size.cols)),
        );
        self.origin = clamped_origin;
        if anim.size.cols > grid.cols || anim.size.rows > grid.rows {
            if path.as_os_str().is_empty() {
                let new_cols = anim.size.cols.min(grid.cols);
                let new_rows = anim.size.rows.min(grid.rows);
                let new_size = GridSize::new(new_cols, new_rows);
                let mut new_frames = Vec::new();
                for f in anim.frames {
                    let mut cells =
                        Vec::with_capacity(usize::from(new_cols) * usize::from(new_rows));
                    for r in 0..new_rows {
                        for c in 0..new_cols {
                            let idx = r as usize * anim.size.cols as usize + c as usize;
                            cells.push(f.cells.get(idx).copied().unwrap_or_default());
                        }
                    }
                    new_frames.push(FetchFrame {
                        delay_ms: f.delay_ms,
                        cells,
                    });
                }
                self.animation = Some(FetchAnimation {
                    size: new_size,
                    loop_policy: anim.loop_policy,
                    frames: new_frames,
                    styles: anim.styles,
                });
                self.index = 0;
                self.next_deadline =
                    self.now_ms + self.animation.as_ref().unwrap().frames[0].delay_ms;
                return;
            }
            if let Some(rerastered) = load_fetch_animation(path, grid) {
                self.animation = Some(rerastered);
                self.index = 0;
                self.next_deadline =
                    self.now_ms + self.animation.as_ref().unwrap().frames[0].delay_ms;
                return;
            }
        }
        self.animation = Some(anim);
    }

    pub fn tick(&mut self, now_ms: u64) -> Option<usize> {
        if !self.is_active() {
            return None;
        }
        let now = now_ms.max(self.now_ms);
        self.now_ms = now;
        if now < self.next_deadline {
            return None;
        }
        let anim = self.animation.as_ref()?;
        let current = self.index;
        let mut next_index = current + 1;
        let mut finished = false;
        if next_index >= anim.frames.len() {
            match anim.loop_policy {
                LoopPolicy::Forever => next_index = 0,
                LoopPolicy::Once => {
                    finished = true;
                    self.playing = false;
                }
                LoopPolicy::Count(_) => {
                    if let Some(remaining) = self.remaining.as_mut() {
                        if *remaining > 1 {
                            *remaining -= 1;
                            next_index = 0;
                        } else {
                            finished = true;
                            self.playing = false;
                        }
                    } else {
                        next_index = 0;
                    }
                }
            }
        }
        if finished {
            self.next_deadline = u64::MAX;
        } else {
            self.index = next_index;
            self.next_deadline = now.saturating_add(anim.frames[next_index].delay_ms);
        }
        Some(current)
    }

    pub fn frame(&self, index: usize) -> Option<(GridSize, &[Cell], &[Style])> {
        let animation = self.animation.as_ref()?;
        let frame = animation.frames.get(index)?;
        Some((animation.size, &frame.cells, &animation.styles))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mr_crabs_terminal::GridSize;

    fn tiny_gif_bytes() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            let frame1 = image::Frame::from_parts(
                image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255])),
                0,
                0,
                image::Delay::from_numer_denom_ms(50, 1),
            );
            let frame2 = image::Frame::from_parts(
                image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 255, 0, 255])),
                0,
                0,
                image::Delay::from_numer_denom_ms(50, 1),
            );
            encoder.encode_frames(vec![frame1, frame2]).unwrap();
        }
        bytes
    }

    #[test]
    fn delay_clamp() {
        assert_eq!(clamp_delay(0), 100);
        assert_eq!(clamp_delay(1), 16);
        assert_eq!(clamp_delay(10), 16);
        assert_eq!(clamp_delay(100), 100);
        assert_eq!(clamp_delay(6000), 5000);
    }

    #[test]
    fn decode_bounds() {
        let bytes = vec![0u8; 9 * 1024 * 1024];
        assert!(decode_gif_bytes(&bytes, GridSize::new(80, 24)).is_err());
    }

    #[test]
    fn raster_colors() {
        let bytes = tiny_gif_bytes();
        let anim = decode_gif_bytes(&bytes, GridSize::new(80, 24)).unwrap();
        assert_eq!(anim.frames.len(), 2);
        assert_eq!(anim.size, GridSize::new(2, 1));
        let first = &anim.frames[0];
        assert!(first.cells[0].content == '\u{2580}' as u32);
        assert!(first.cells[0].style != 0);
    }

    #[test]
    fn transparent_raster() {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            let frame = image::Frame::from_parts(
                image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 0, 0])),
                0,
                0,
                image::Delay::from_numer_denom_ms(50, 1),
            );
            encoder.encode_frames(vec![frame]).unwrap();
        }
        let anim = decode_gif_bytes(&bytes, GridSize::new(80, 24)).unwrap();
        assert_eq!(anim.frames[0].cells[0].content, ' ' as u32);
        assert_eq!(anim.frames[0].cells[0].style, 0);
    }

    #[test]
    fn frames_advance_only_at_explicit_deadlines() {
        let animation = decode_gif_bytes(&tiny_gif_bytes(), GridSize::new(80, 24)).unwrap();
        let mut driver = FetchDriver::new(Some(animation));

        assert_eq!(driver.next_deadline_ms(), Some(50));
        assert!(driver.tick(49).is_none());
        let first = driver.tick(50).expect("first frame at deadline");
        let first_style = driver.frame(first).expect("first frame").1[0].style;
        assert!(driver.tick(50).is_none());
        let second = driver.tick(100).expect("one next frame at deadline");
        let second_style = driver.frame(second).expect("second frame").1[0].style;
        assert_ne!(first_style, second_style);
    }

    #[test]
    fn once_loop_completion_removes_deadline() {
        let mut animation = decode_gif_bytes(&tiny_gif_bytes(), GridSize::new(80, 24)).unwrap();
        animation.loop_policy = LoopPolicy::Once;
        let mut driver = FetchDriver::new(Some(animation));

        assert!(driver.tick(50).is_some());
        assert!(driver.tick(100).is_some());
        assert_eq!(driver.next_deadline_ms(), None);
        assert!(driver.tick(150).is_none());
    }

    #[test]
    fn invalidation_removes_deadline() {
        let animation = decode_gif_bytes(&tiny_gif_bytes(), GridSize::new(80, 24)).unwrap();
        let mut driver = FetchDriver::new(Some(animation));
        assert!(driver.next_deadline_ms().is_some());
        driver.invalidate();
        assert_eq!(driver.next_deadline_ms(), None);
    }

    #[test]
    fn resize_can_move_the_next_deadline_earlier() {
        let mut animation = decode_gif_bytes(&tiny_gif_bytes(), GridSize::new(80, 24)).unwrap();
        animation.frames[0].delay_ms = 16;
        animation.frames[1].delay_ms = 5000;
        let mut driver = FetchDriver::new(Some(animation));

        assert!(driver.tick(16).is_some());
        assert_eq!(driver.next_deadline_ms(), Some(5016));
        driver.on_resize(GridSize::new(1, 1), Path::new(""));
        assert_eq!(driver.next_deadline_ms(), Some(32));
    }
}
