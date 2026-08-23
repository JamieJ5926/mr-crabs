use std::path::Path;
use std::time::Duration;

use image::codecs::gif::GifDecoder;
use image::AnimationDecoder;

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
    let decoder = GifDecoder::new(bytes).map_err(|e| format!("gif decode: {e}"))?;
    let frames = decoder.into_frames().collect_frames().map_err(|e| format!("gif frames: {e}"))?;
    if frames.is_empty() {
        return Err("no frames".to_string());
    }
    if frames.len() > MAX_FRAMES {
        return Err("too many frames".to_string());
    }
    let mut loop_policy = LoopPolicy::Forever;
    if let Ok(decoder2) = GifDecoder::new(bytes) {
        use image::codecs::gif::Repeat;
        if let Some(repeat) = decoder2.repeat().ok().flatten() {
            loop_policy = match repeat {
                Repeat::Infinite => LoopPolicy::Forever,
                Repeat::Finite(n) => {
                    if n == 0 {
                        LoopPolicy::Forever
                    } else if n == 1 {
                        LoopPolicy::Once
                    } else {
                        LoopPolicy::Count(n)
                    }
                }
            };
        }
    }
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
    let target_h = ((target_h_px + 1) / 2) as u16;
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
                let bot = if fh > 1 { buf.get_pixel(src_x, bot_y) } else { top };
                let top_a = top[3];
                let bot_a = bot[3];
                let (ch, style) = if top_a < 128 && bot_a < 128 {
                    (' ', Style::default())
                } else if top_a >= 128 && bot_a < 128 {
                    (
                        '\u{2580}',
                        Style {
                            foreground: NormalizedColor::Rgb([top[0], top[1], top[2]]),
                            background: NormalizedColor::Named(mr_crabs_terminal::NamedColorValue::Background),
                            underline: None,
                        },
                    )
                } else if top_a < 128 && bot_a >= 128 {
                    (
                        '\u{2584}',
                        Style {
                            foreground: NormalizedColor::Rgb([bot[0], bot[1], bot[2]]),
                            background: NormalizedColor::Named(mr_crabs_terminal::NamedColorValue::Background),
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
        let remaining = animation.as_ref().map(|a| match a.loop_policy {
            LoopPolicy::Forever => None,
            LoopPolicy::Once => Some(1),
            LoopPolicy::Count(n) => Some(n),
        }).unwrap_or(None).flatten();
        let next_deadline = 0;
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
                    let mut cells = Vec::with_capacity(usize::from(new_cols) * usize::from(new_rows));
                    for r in 0..new_rows {
                        for c in 0..new_cols {
                            let idx = r as usize * anim.size.cols as usize + c as usize;
                            cells.push(f.cells.get(idx).copied().unwrap_or_default());
                        }
                    }
                    new_frames.push(FetchFrame { delay_ms: f.delay_ms, cells });
                }
                self.animation = Some(FetchAnimation {
                    size: new_size,
                    loop_policy: anim.loop_policy,
                    frames: new_frames,
                    styles: anim.styles,
                });
                self.index = 0;
                self.next_deadline = self.now_ms + self.animation.as_ref().unwrap().frames[0].delay_ms;
                return;
            }
            if let Some(rerastered) = load_fetch_animation(path, grid) {
                self.animation = Some(rerastered);
                self.index = 0;
                self.next_deadline = self.now_ms + self.animation.as_ref().unwrap().frames[0].delay_ms;
                return;
            }
        }
        self.animation = Some(anim);
    }

    pub fn tick(&mut self, now_ms: u64) -> Option<(GridSize, Vec<Cell>)> {
        if self.invalidated {
            return None;
        }
        let anim = self.animation.as_ref()?;
        if !self.playing || anim.frames.is_empty() {
            return None;
        }
        let now = now_ms.max(self.now_ms);
        self.now_ms = now;
        if now < self.next_deadline && self.index == 0 && self.now_ms != 0 {
            return None;
        }
        if now < self.next_deadline {
            return None;
        }
        let current = self.index;
        let frame = &anim.frames[current];
        let cells = frame.cells.clone();
        let size = anim.size;
        let delay = frame.delay_ms;
        let mut next_index = current + 1;
        let mut finished = false;
        if next_index >= anim.frames.len() {
            match anim.loop_policy {
                LoopPolicy::Forever => next_index = 0,
                LoopPolicy::Once => {
                    finished = true;
                    self.playing = false;
                }
                LoopPolicy::Count(n) => {
                    if let Some(rem) = self.remaining.as_mut() {
                        if *rem > 1 {
                            *rem -= 1;
                            next_index = 0;
                        } else {
                            finished = true;
                            self.playing = false;
                        }
                    } else {
                        next_index = 0;
                    }
                    let _ = n;
                }
            }
        }
        if !finished {
            self.index = next_index;
            self.next_deadline = now + anim.frames[next_index % anim.frames.len()].delay_ms;
            let _ = delay;
        } else {
            self.next_deadline = u64::MAX;
        }
        Some((size, cells))
    }

    pub fn current_frame_cells(&self) -> Option<(GridSize, Vec<Cell>)> {
        let anim = self.animation.as_ref()?;
        let frame = anim.frames.get(self.index)?;
        Some((anim.size, frame.cells.clone()))
    }

    pub fn styles(&self) -> Option<Vec<Style>> {
        self.animation.as_ref().map(|a| a.styles.clone())
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
    fn loop_clock() {
        let bytes = tiny_gif_bytes();
        let anim = decode_gif_bytes(&bytes, GridSize::new(80, 24)).unwrap();
        let mut driver = FetchDriver::new(Some(anim));
        assert!(driver.needs_frame());
        let first = driver.tick(60).unwrap();
        assert_eq!(first.0, GridSize::new(2, 1));
        let second = driver.tick(120).unwrap();
        assert_ne!(first.1[0].style, second.1[0].style);
        let third = driver.tick(180).unwrap();
        assert_eq!(third.1[0].style, first.1[0].style);
    }
}
