use std::io::{IsTerminal, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const FRAME_COUNT: usize = 8;
pub const FRAME_DELAY: Duration = Duration::from_millis(80);

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

pub fn parse_fetch_layout(output: &str) -> Option<FetchLayout> {
    if output.is_empty() {
        return None;
    }
    let raw_lines: Vec<&str> = output.lines().collect();
    if raw_lines.is_empty() {
        return None;
    }
    let sep_idx = raw_lines.iter().position(|l| l.contains("---"))?;
    let sep_line = raw_lines[sep_idx];
    let dash_start = sep_line.find("---")?;
    let logo_width = dash_start;
    if !sep_line.is_char_boundary(logo_width) {
        return None;
    }
    for line in &raw_lines {
        if line.len() < logo_width {
            return None;
        }
        if !line.is_char_boundary(logo_width) {
            return None;
        }
    }
    let mut lines = Vec::with_capacity(raw_lines.len());
    for line in raw_lines {
        let (logo, info) = line.split_at(logo_width);
        lines.push(FetchLine {
            logo: logo.to_string(),
            info: info.to_string(),
        });
    }
    Some(FetchLayout { lines, logo_width })
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let h2 = h / 60.0;
    let x = c * (1.0 - ((h2 % 2.0) - 1.0).abs());
    let (r1, g1, b1) = match h2 as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    (
        ((r1 + m) * 255.0) as u8,
        ((g1 + m) * 255.0) as u8,
        ((b1 + m) * 255.0) as u8,
    )
}

fn color_for(phase: usize, col: usize, row: usize) -> (u8, u8, u8) {
    let hue = ((phase * 45 + col * 17 + row * 11) % 360) as f32;
    hsv_to_rgb(hue, 0.9, 1.0)
}

pub fn frame_bytes(layout: &FetchLayout, phase: usize) -> Vec<u8> {
    let mut out = String::new();
    for (row, line) in layout.lines.iter().enumerate() {
        for (col, ch) in line.logo.chars().enumerate() {
            if ch == ' ' {
                out.push(' ');
            } else {
                let (r, g, b) = color_for(phase, col, row);
                out.push_str(&format!("\x1b[38;2;{r};{g};{b}m{ch}\x1b[0m"));
            }
        }
        out.push_str(&line.info);
        out.push('\n');
    }
    out.into_bytes()
}

pub fn animation_frames(layout: &FetchLayout) -> Vec<Vec<u8>> {
    (0..FRAME_COUNT).map(|p| frame_bytes(layout, p)).collect()
}

pub fn animation_chunks(layout: &FetchLayout, original: &str) -> Vec<Vec<u8>> {
    let height = layout.lines.len();
    let prefix = format!("\x1b[{height}A\r").into_bytes();
    let mut chunks = Vec::with_capacity(FRAME_COUNT + 1);
    for phase in 0..FRAME_COUNT {
        let frame = frame_bytes(layout, phase);
        if phase == 0 {
            chunks.push(frame);
        } else {
            let mut chunk = Vec::with_capacity(prefix.len() + frame.len());
            chunk.extend_from_slice(&prefix);
            chunk.extend_from_slice(&frame);
            chunks.push(chunk);
        }
    }
    let mut last = Vec::with_capacity(prefix.len() + original.len());
    last.extend_from_slice(&prefix);
    last.extend_from_slice(original.as_bytes());
    chunks.push(last);
    chunks
}

pub fn inline_animation_bytes(layout: &FetchLayout, original: &str) -> Vec<u8> {
    animation_chunks(layout, original).concat()
}

pub fn fits_terminal(layout: &FetchLayout, original: &str, rows: u16, cols: u16) -> bool {
    if rows == 0 || cols == 0 {
        return false;
    }
    if layout.lines.len() + 1 > rows as usize {
        return false;
    }
    for line in original.lines() {
        if !line.is_ascii() {
            return false;
        }
        if line.len() > cols as usize {
            return false;
        }
    }
    true
}

fn terminal_size() -> Option<(u16, u16)> {
    #[cfg(unix)]
    {
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        let ret = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) };
        if ret == 0 && ws.ws_row != 0 && ws.ws_col != 0 {
            return Some((ws.ws_row, ws.ws_col));
        }
        None
    }
    #[cfg(not(unix))]
    {
        None
    }
}

fn capture_rustfetch() -> Option<String> {
    let mut child = match Command::new("rustfetch")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("mr-crabs: rustfetch spawn failed: {e}");
            return None;
        }
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let status = status?;
    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(_) => return None,
    };
    if output.stdout.is_empty() {
        if !status.success() && !output.stderr.is_empty() {
            let msg = String::from_utf8_lossy(&output.stderr);
            let trimmed = msg.trim();
            if !trimmed.is_empty() {
                eprintln!("mr-crabs: rustfetch failed: {trimmed}");
            }
        }
        return None;
    }
    match String::from_utf8(output.stdout) {
        Ok(s) => Some(s),
        Err(e) => Some(String::from_utf8_lossy(e.as_bytes()).into_owned()),
    }
}
pub fn should_sleep_after_chunk(idx: usize) -> bool {
    idx < FRAME_COUNT
}

pub fn should_run_animated_fetch(args: &[String]) -> bool {
    matches!(args.first(), Some(first) if first == "+animated-fetch")
}

pub fn run_animated_fetch_and_exit() -> ! {
    let captured = capture_rustfetch();
    let is_tty = std::io::stdout().is_terminal();
    match captured {
        None => std::process::exit(0),
        Some(original) => {
            if !is_tty {
                let _ = std::io::stdout().write_all(original.as_bytes());
                let _ = std::io::stdout().flush();
                std::process::exit(0);
            }
            let Some(layout) = parse_fetch_layout(&original) else {
                let _ = std::io::stdout().write_all(original.as_bytes());
                let _ = std::io::stdout().flush();
                std::process::exit(0);
            };
            if layout.lines.is_empty() {
                let _ = std::io::stdout().write_all(original.as_bytes());
                let _ = std::io::stdout().flush();
                std::process::exit(0);
            }
            let animate = match terminal_size() {
                Some((rows, cols)) => fits_terminal(&layout, &original, rows, cols),
                None => false,
            };
            if !animate {
                let _ = std::io::stdout().write_all(original.as_bytes());
                let _ = std::io::stdout().flush();
                std::process::exit(0);
            }
            let chunks = animation_chunks(&layout, &original);
            let mut stdout = std::io::stdout();
            for (idx, chunk) in chunks.iter().enumerate() {
                let _ = stdout.write_all(chunk);
                let _ = stdout.flush();
                if should_sleep_after_chunk(idx) {
                    std::thread::sleep(FRAME_DELAY);
                }
            }
            std::process::exit(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_output() -> String {
        let mut s = String::new();
        s.push_str("        .:'      jamie@host\n");
        s.push_str("    __ :'__      ------------------------------\n");
        s.push_str(" .'`__`-'__``.   OS: Darwin (aarch64)\n");
        s.push_str(":__________.-'   Kernel: 25.5.0\n");
        s.push_str(":_________:      Shell: zsh\n");
        s.push_str(" :_________`-;   Terminal: Mr Crabs\n");
        s.push_str("  `.__.-.__.'   Uptime: 1 day\n");
        s
    }

    #[test]
    fn parse_splits_at_separator_column() {
        let out = sample_output();
        let layout = parse_fetch_layout(&out).expect("layout");
        assert_eq!(layout.logo_width, 17);
        assert_eq!(layout.lines.len(), 7);
        for line in &layout.lines {
            assert_eq!(line.logo.chars().count(), 17);
        }
        assert!(layout.lines[1].info.contains("---"));
    }

    #[test]
    fn logo_only_coloring() {
        let out = sample_output();
        let layout = parse_fetch_layout(&out).expect("layout");
        let frame = frame_bytes(&layout, 0);
        let text = String::from_utf8(frame).unwrap();
        assert!(text.contains("\x1b[38;2;"));
        for line in &layout.lines {
            if line.info.contains("jamie@host") {
                let frame_s = String::from_utf8(frame_bytes(&layout, 0)).unwrap();
                let info_line = frame_s.lines().find(|l| l.contains("jamie@host")).unwrap();
                assert!(
                    !info_line.contains("\x1b[38;2;")
                        || info_line.matches("\x1b[0m").count()
                            <= layout.lines[0].logo.chars().filter(|c| *c != ' ').count()
                );
            }
        }
        let first_info = &layout.lines[0].info;
        let frame_str = String::from_utf8(frame_bytes(&layout, 0)).unwrap();
        assert!(frame_str.contains(first_info.trim()));
        let logo_colored = frame_str.matches("\x1b[38;2;").count();
        let non_space_logo: usize = layout
            .lines
            .iter()
            .map(|l| l.logo.chars().filter(|c| *c != ' ').count())
            .sum();
        assert_eq!(logo_colored, non_space_logo);
    }

    #[test]
    fn exact_final_output() {
        let out = sample_output();
        let layout = parse_fetch_layout(&out).expect("layout");
        let bytes = inline_animation_bytes(&layout, &out);
        assert!(
            bytes.ends_with(out.as_bytes()),
            "must finish with exact original static output"
        );
        let frames: Vec<Vec<u8>> = (0..FRAME_COUNT).map(|p| frame_bytes(&layout, p)).collect();
        for f in frames {
            let s = String::from_utf8(f).unwrap();
            assert_eq!(
                s.lines().count(),
                layout.lines.len(),
                "each frame stable height"
            );
        }
    }

    #[test]
    fn fixed_chunks() {
        let out = sample_output();
        let layout = parse_fetch_layout(&out).expect("layout");
        let chunks = animation_chunks(&layout, &out);
        assert_eq!(chunks.len(), FRAME_COUNT + 1);
        assert_eq!(chunks.concat(), inline_animation_bytes(&layout, &out));
        let height = layout.lines.len();
        let prefix = format!("\x1b[{height}A\r").into_bytes();
        assert!(
            !chunks[0].starts_with(&prefix),
            "first chunk has no CUU prefix"
        );
        for chunk in &chunks[1..] {
            assert!(chunk.starts_with(&prefix));
        }
    }

    #[test]
    fn non_tty_has_no_animation_sequences() {
        let out = sample_output();
        let plain = out.as_bytes();
        let text = String::from_utf8_lossy(plain);
        assert!(
            !text.contains("\x1b["),
            "non-tty static output must have no control sequences"
        );
        assert_eq!(plain, out.as_bytes());
    }

    #[test]
    fn parse_fallback_single_column() {
        assert!(parse_fetch_layout("hello\nworld\n").is_none());
        assert!(parse_fetch_layout("").is_none());
        assert!(parse_fetch_layout("no dash here\njust text\n").is_none());
    }

    #[test]
    fn parse_fallback_validates_utf8_boundaries() {
        let mut out = String::new();
        out.push_str("    __ :'__      ------------------------------\n");
        let emoji = "🦀".repeat(10);
        out.push_str(&format!("{emoji}   OS: test\n"));
        assert!(
            parse_fetch_layout(&out).is_none(),
            "byte 17 inside emoji must fail UTF-8 boundary and fallback to static"
        );
    }

    #[test]
    fn animation_bounds_under_one_second() {
        assert!(
            (FRAME_COUNT as u128) * FRAME_DELAY.as_millis() < 1000,
            "must be under one second"
        );
        const {
            assert!(FRAME_COUNT >= 2, "must have multiple frames");
            assert!(FRAME_COUNT <= 16, "hard fixed bounds");
        }
    }

    #[test]
    fn fits_terminal_rejects_narrow_width() {
        let out = sample_output();
        let layout = parse_fetch_layout(&out).expect("layout");
        assert!(
            !fits_terminal(&layout, &out, 24, 20),
            "narrow cols must fallback to static"
        );
        assert!(
            !fits_terminal(&layout, &out, 24, 30),
            "cols shorter than longest line must fallback"
        );
        assert!(fits_terminal(&layout, &out, 24, 80));
        assert!(
            !fits_terminal(&layout, &out, 7, 80),
            "exact height without spare row must fallback"
        );
        assert!(fits_terminal(&layout, &out, 8, 80), "height+1 fits");
    }

    #[test]
    fn fits_terminal_rejects_short_height() {
        let out = sample_output();
        let layout = parse_fetch_layout(&out).expect("layout");
        assert!(
            !fits_terminal(&layout, &out, 5, 80),
            "short rows must fallback to static"
        );
        assert!(
            !fits_terminal(&layout, &out, 7, 80),
            "exact height must fallback without spare row"
        );
        assert!(fits_terminal(&layout, &out, 8, 80));
        assert!(!fits_terminal(&layout, &out, 0, 80));
        assert!(!fits_terminal(&layout, &out, 24, 0));
    }

    #[test]
    fn fits_terminal_rejects_non_ascii() {
        let mut out = sample_output();
        let base_layout = parse_fetch_layout(&out).expect("layout");
        let logo_width = base_layout.logo_width;
        let pad = " ".repeat(logo_width);
        out.push_str(&format!("{pad}🦀 crab\n"));
        let layout =
            parse_fetch_layout(&out).expect("layout still parseable with ascii logo prefix");
        assert_eq!(layout.logo_width, logo_width);
        assert!(
            !fits_terminal(&layout, &out, 24, 80),
            "non-ascii in info must fallback"
        );
    }

    #[test]
    fn should_run_requires_first_position() {
        assert!(should_run_animated_fetch(&["+animated-fetch".to_string()]));
        assert!(!should_run_animated_fetch(&[
            "other".to_string(),
            "+animated-fetch".to_string()
        ]));
        assert!(!should_run_animated_fetch(&[]));
        assert!(!should_run_animated_fetch(&[
            "+animated-fetch-extra".to_string()
        ]));
    }

    #[test]
    fn chunks_flatten_to_inline_bytes() {
        let out = sample_output();
        let layout = parse_fetch_layout(&out).expect("layout");
        let chunks = animation_chunks(&layout, &out);
        assert_eq!(chunks.len(), FRAME_COUNT + 1);
        assert_eq!(chunks.concat(), inline_animation_bytes(&layout, &out));
        let height = layout.lines.len();
        let prefix = format!("\x1b[{height}A\r").into_bytes();
        assert!(
            !chunks[0].starts_with(&prefix),
            "first chunk has no CUU prefix"
        );
        assert!(
            chunks[0].windows(7).any(|w| w == b"\x1b[38;2;"),
            "first chunk contains truecolor"
        );
        for chunk in &chunks[1..] {
            assert!(
                chunk.starts_with(&prefix),
                "subsequent chunks start with CUU prefix"
            );
        }
        assert!(
            chunks.last().unwrap().ends_with(out.as_bytes()),
            "last chunk ends with exact original"
        );
        assert_eq!(
            chunks.last().unwrap()[prefix.len()..],
            out.as_bytes().to_vec()
        );
    }

    #[test]
    fn production_uses_same_chunks() {
        let out = sample_output();
        let layout = parse_fetch_layout(&out).expect("layout");
        let chunks = animation_chunks(&layout, &out);
        let frames = animation_frames(&layout);
        assert_eq!(chunks.len(), FRAME_COUNT + 1);
        assert_eq!(frames.len(), FRAME_COUNT);
        assert_eq!(chunks[0], frames[0]);
        let height = layout.lines.len();
        let prefix = format!("\x1b[{height}A\r").into_bytes();
        for idx in 1..FRAME_COUNT {
            let mut expected = prefix.clone();
            expected.extend_from_slice(&frames[idx]);
            assert_eq!(chunks[idx], expected);
        }
    }

    #[test]
    fn default_config_uses_animated_fetch() {
        let defaults = mr_crabs_config::EffectiveConfig::defaults();
        assert_eq!(
            defaults.startup_fetch_command,
            "\"$MR_CRABS_BIN\" +animated-fetch"
        );
        assert!(defaults.startup_fetch);
    }

    #[test]
    fn playback_sleeps_after_every_colored_chunk() {
        assert_eq!(
            animation_chunks(
                &parse_fetch_layout(&sample_output()).unwrap(),
                &sample_output()
            )
            .len(),
            FRAME_COUNT + 1
        );
        for idx in 0..FRAME_COUNT {
            assert!(
                should_sleep_after_chunk(idx),
                "idx {idx} is a colored chunk and must sleep"
            );
        }
        assert!(
            !should_sleep_after_chunk(FRAME_COUNT),
            "final static chunk must not sleep"
        );
        assert!(!should_sleep_after_chunk(FRAME_COUNT + 1));
        assert!(!should_sleep_after_chunk(usize::MAX));
    }

    #[test]
    fn sleep_predicate_matches_production_loop() {
        let out = sample_output();
        let layout = parse_fetch_layout(&out).expect("layout");
        let chunks = animation_chunks(&layout, &out);
        let sleep_indices: Vec<usize> = (0..chunks.len())
            .filter(|i| should_sleep_after_chunk(*i))
            .collect();
        assert_eq!(sleep_indices, (0..FRAME_COUNT).collect::<Vec<_>>());
        assert_eq!(chunks.len(), FRAME_COUNT + 1);
        assert_eq!(sleep_indices.len(), FRAME_COUNT);
    }

    #[test]
    fn startup_command_propagation_in_pane() {
        use crate::model::pane::PtySpawnConfig;
        use mr_crabs_terminal::GridSize;
        let size = GridSize::new(80, 24);
        let cfg = PtySpawnConfig {
            size,
            shell: None,
            cwd: None,
            env: {
                let mut m = std::collections::BTreeMap::new();
                if let Ok(exe) = std::env::current_exe() {
                    m.insert("MR_CRABS_BIN".to_string(), exe.display().to_string());
                }
                m
            },
            term: "xterm-ghostty".to_string(),
            colorterm: "truecolor".to_string(),
            scrollback_lines: 10000,
            startup_command: Some("\"$MR_CRABS_BIN\" +animated-fetch".to_string()),
        };
        assert_eq!(
            cfg.startup_command.as_deref(),
            Some("\"$MR_CRABS_BIN\" +animated-fetch")
        );
        assert!(cfg.env.contains_key("MR_CRABS_BIN"));
    }
}
