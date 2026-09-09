use std::io::Write;
use std::time::Duration;

use mr_crabs_config::{FetchArrangement, StartupArt};

use crate::{art, sysinfo};

pub const FRAME_COUNT: usize = 8;
pub const FRAME_DELAY: Duration = Duration::from_millis(80);

fn startup_art() -> StartupArt {
    match std::env::var("MR_CRABS_STARTUP_ART").ok().as_deref() {
        Some("none") => StartupArt::None,
        Some("apple") => StartupArt::Apple,
        Some(value) if value.starts_with("file:") => StartupArt::parse(value).unwrap_or_default(),
        _ => StartupArt::Native,
    }
}

fn fetch_arrangement() -> FetchArrangement {
    match std::env::var("MR_CRABS_FETCH_ARRANGEMENT").ok().as_deref() {
        Some("beside") => FetchArrangement::Beside,
        Some("hidden") => FetchArrangement::Hidden,
        _ => FetchArrangement::Below,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchLine {
    pub logo: String,
    pub info: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchLayout {
    pub lines: Vec<FetchLine>,
    pub logo_width: usize,
}

fn compose(arrangement: FetchArrangement, selected: &StartupArt) -> FetchLayout {
    let logo = if matches!(selected, StartupArt::None) {
        None
    } else {
        art::art_for_startup(selected)
    };
    let info = sysinfo::collect();
    let art_rows = logo.as_ref().map(|a| a.rows.clone()).unwrap_or_default();
    let width = logo.as_ref().map_or(0, |a| a.width);
    let mut lines = Vec::new();
    match arrangement {
        FetchArrangement::Hidden => lines.extend(art_rows.into_iter().map(|logo| FetchLine {
            logo,
            info: String::new(),
        })),
        FetchArrangement::Beside => {
            let facts: Vec<String> = std::iter::once(info.title)
                .chain(info.lines.into_iter().map(|(k, v)| format!("{k}: {v}")))
                .collect();
            let gutter = width + 2;
            let n = art_rows.len().max(facts.len());
            for i in 0..n {
                let row = art_rows.get(i).cloned().unwrap_or_default();
                let pad = gutter.saturating_sub(row.chars().count());
                lines.push(FetchLine {
                    logo: format!("{row}{}", " ".repeat(pad)),
                    info: facts.get(i).cloned().unwrap_or_default(),
                });
            }
        }
        FetchArrangement::Below => {
            lines.extend(art_rows.into_iter().map(|logo| FetchLine {
                logo,
                info: String::new(),
            }));
            lines.push(FetchLine {
                logo: String::new(),
                info: info.title,
            });
            lines.extend(info.lines.into_iter().map(|(k, v)| FetchLine {
                logo: String::new(),
                info: format!("{k}: {v}"),
            }));
        }
    }
    FetchLayout {
        lines,
        logo_width: width,
    }
}

pub fn composed_layout(arrangement: FetchArrangement, selected: &StartupArt) -> FetchLayout {
    compose(arrangement, selected)
}
pub fn logo_only_bytes(layout: &FetchLayout, _: FetchArrangement) -> Vec<u8> {
    layout
        .lines
        .iter()
        .map(|l| format!("{}{}\n", l.logo, l.info))
        .collect::<String>()
        .into_bytes()
}
fn hsv(phase: usize, col: usize, row: usize) -> (u8, u8, u8) {
    let h = ((phase * 45 + col * 17 + row * 11) % 360) as f32;
    let c = 0.9;
    let x = c * (1.0 - ((h / 60.0 % 2.0) - 1.0).abs());
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (
        ((r + 0.1) * 255.0) as u8,
        ((g + 0.1) * 255.0) as u8,
        ((b + 0.1) * 255.0) as u8,
    )
}
fn frame(layout: &FetchLayout, phase: usize) -> Vec<u8> {
    let mut s = String::new();
    for (r, l) in layout.lines.iter().enumerate() {
        for (c, ch) in l.logo.chars().enumerate() {
            if ch == ' ' {
                s.push(ch)
            } else {
                let (a, b, d) = hsv(phase, c, r);
                s.push_str(&format!("\x1b[38;2;{a};{b};{d}m{ch}\x1b[0m"));
            }
        }
        s.push_str(&l.info);
        s.push('\n');
    }
    s.into_bytes()
}
pub fn animation_frames(layout: &FetchLayout, _: FetchArrangement) -> Vec<Vec<u8>> {
    (0..FRAME_COUNT).map(|p| frame(layout, p)).collect()
}
pub fn animation_chunks(layout: &FetchLayout, _: &str, _: FetchArrangement) -> Vec<Vec<u8>> {
    let mut out = animation_frames(layout, FetchArrangement::Hidden);
    let prefix = format!("\x1b[{}A\r", layout.lines.len()).into_bytes();
    for x in out.iter_mut().skip(1) {
        let mut y = prefix.clone();
        y.append(x);
        *x = y;
    }
    let mut final_frame = prefix.clone();
    final_frame.extend_from_slice(&logo_only_bytes(layout, FetchArrangement::Hidden));
    out.push(final_frame);
    out
}
pub fn inline_animation_bytes(
    layout: &FetchLayout,
    original: &str,
    a: FetchArrangement,
) -> Vec<u8> {
    animation_chunks(layout, original, a).concat()
}
pub fn should_sleep_after_chunk(i: usize) -> bool {
    i < FRAME_COUNT
}
pub fn should_run_animated_fetch(args: &[String]) -> bool {
    args.first().is_some_and(|a| a == "+fetch")
}
fn terminal_size() -> Option<(u16, u16)> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let read = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) };
    (read == 0 && ws.ws_row > 0 && ws.ws_col > 0).then_some((ws.ws_row, ws.ws_col))
}

fn centered(layout: &FetchLayout) -> FetchLayout {
    let Some((rows, cols)) = terminal_size() else {
        return layout.clone();
    };
    let block_width = layout
        .lines
        .iter()
        .map(|l| l.logo.chars().count() + l.info.chars().count())
        .max()
        .unwrap_or(0);
    let left = (usize::from(cols).saturating_sub(block_width)) / 2;
    let top = (usize::from(rows).saturating_sub(layout.lines.len())) / 2;
    let margin = " ".repeat(left);
    let mut lines: Vec<FetchLine> = (0..top)
        .map(|_| FetchLine {
            logo: String::new(),
            info: String::new(),
        })
        .collect();
    lines.extend(layout.lines.iter().map(|l| FetchLine {
        logo: format!("{margin}{}", l.logo),
        info: l.info.clone(),
    }));
    FetchLayout {
        lines,
        logo_width: layout.logo_width + left,
    }
}

pub fn run_animated_fetch_and_exit() -> ! {
    let layout = centered(&compose(fetch_arrangement(), &startup_art()));
    let chunks = animation_chunks(&layout, "", FetchArrangement::Hidden);
    let mut out = std::io::stdout();
    for (index, chunk) in chunks.iter().enumerate() {
        let _ = out.write_all(chunk);
        let _ = out.flush();
        if should_sleep_after_chunk(index) {
            std::thread::sleep(FRAME_DELAY);
        }
    }
    std::process::exit(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composed_rows_never_shift_between_frames() {
        let layout = compose(FetchArrangement::Below, &StartupArt::Native);
        let frames = animation_frames(&layout, FetchArrangement::Below);
        let rows = |f: &Vec<u8>| f.iter().filter(|b| **b == b'\n').count();
        assert!(
            frames.windows(2).all(|w| rows(&w[0]) == rows(&w[1])),
            "every animation frame keeps the same row count, so the block cannot move"
        );
    }

    #[test]
    fn arrangements_have_expected_shapes() {
        let hidden = compose(FetchArrangement::Hidden, &StartupArt::Native);
        let below = compose(FetchArrangement::Below, &StartupArt::Native);
        let beside = compose(FetchArrangement::Beside, &StartupArt::Native);
        assert!(
            hidden.lines.iter().all(|l| l.info.is_empty()),
            "hidden shows art only"
        );
        assert!(
            below.lines.len() > hidden.lines.len(),
            "below adds info rows beneath the art"
        );
        assert!(
            beside.lines.len() <= below.lines.len(),
            "beside is shorter than stacked"
        );
        assert!(
            beside
                .lines
                .iter()
                .any(|l| !l.logo.is_empty() && !l.info.is_empty()),
            "beside pairs art and info on one row"
        );
    }

    #[test]
    fn missing_custom_art_does_not_panic() {
        let art = StartupArt::parse("file:/nonexistent/mr-crabs-art.txt").unwrap_or_default();
        let layout = compose(FetchArrangement::Below, &art);
        assert!(!layout.lines.is_empty(), "falls back with content");
    }

    #[test]
    fn composition_carries_real_system_facts() {
        let layout = compose(FetchArrangement::Below, &StartupArt::Native);
        let text: String = layout.lines.iter().map(|l| l.info.clone()).collect();
        assert!(text.contains('@'), "title row present");
        assert!(
            text.contains("Kernel") || text.contains("CPU"),
            "facts present"
        );
    }
}
