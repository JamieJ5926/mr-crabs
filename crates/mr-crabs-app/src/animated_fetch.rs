use std::io::{IsTerminal, Write};
use std::path::Path;
use std::sync::mpsc::{RecvTimeoutError, TryRecvError};
use std::time::{Duration, Instant};

use mr_crabs_pty::{CommandBuilder, PtyConfig, PtySession, PtySize};

pub const FRAME_COUNT: usize = 8;
pub const FRAME_DELAY: Duration = Duration::from_millis(80);
pub const RUSTFETCH_CAPTURE_MAX_BYTES: usize = 64 * 1024;
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

fn ansi_sequence_end(bytes: &[u8], start: usize) -> usize {
    let mut index = start + 1;
    if index >= bytes.len() {
        return bytes.len();
    }
    match bytes[index] {
        b'[' => {
            index += 1;
            while index < bytes.len() {
                let byte = bytes[index];
                index += 1;
                if (0x40..=0x7e).contains(&byte) {
                    break;
                }
            }
        }
        b']' => {
            index += 1;
            while index < bytes.len() {
                if bytes[index] == 0x07 {
                    index += 1;
                    break;
                }
                if bytes[index] == 0x1b && index + 1 < bytes.len() && bytes[index + 1] == b'\\' {
                    index += 2;
                    break;
                }
                index += 1;
            }
        }
        _ => index += 1,
    }
    index
}

fn visible_text(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == 0x1b {
            index = ansi_sequence_end(bytes, index);
            continue;
        }
        let ch = input[index..].chars().next().expect("character boundary");
        output.push(ch);
        index += ch.len_utf8();
    }
    output
}

fn char_cell_width(ch: char) -> Option<usize> {
    match ch {
        '\u{00}'..='\u{1F}' | '\u{7F}'..='\u{9F}' => None,
        '\u{0300}'..='\u{036F}' | '\u{1AB0}'..='\u{1AFF}' | '\u{1DC0}'..='\u{1DFF}' => Some(0),
        '\u{1100}'..='\u{115F}'
        | '\u{2329}'..='\u{232A}'
        | '\u{2E80}'..='\u{A4CF}'
        | '\u{AC00}'..='\u{D7A3}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{FE10}'..='\u{FE19}'
        | '\u{FE30}'..='\u{FE6F}'
        | '\u{FF00}'..='\u{FF60}'
        | '\u{FFE0}'..='\u{FFE6}'
        | '\u{1F300}'..='\u{1FAFF}'
        | '\u{20000}'..='\u{3FFFD}' => Some(2),
        _ => Some(1),
    }
}

fn visible_width(input: &str) -> usize {
    visible_text(input)
        .chars()
        .filter_map(char_cell_width)
        .sum()
}

fn is_sgr_reset(sequence: &[u8]) -> bool {
    sequence == b"\x1b[m" || sequence == b"\x1b[0m"
}

fn split_at_visible_column(input: &str, column: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    let mut index = 0;
    let mut cells = 0;
    while cells < column {
        if index >= bytes.len() {
            return None;
        }
        if bytes[index] == 0x1b {
            index = ansi_sequence_end(bytes, index);
            continue;
        }
        let ch = input[index..].chars().next()?;
        let width = char_cell_width(ch)?;
        if cells + width > column {
            return None;
        }
        index += ch.len_utf8();
        cells += width;
    }
    while index < bytes.len() && bytes[index] == 0x1b {
        let end = ansi_sequence_end(bytes, index);
        if !is_sgr_reset(&bytes[index..end]) {
            break;
        }
        index = end;
    }
    Some(index)
}

pub fn parse_fetch_layout(output: &str) -> Option<FetchLayout> {
    let raw_lines: Vec<&str> = output.lines().collect();
    let logo_width = raw_lines.iter().find_map(|line| {
        let visible = visible_text(line);
        let dash_byte = visible.find("---")?;
        Some(
            visible[..dash_byte]
                .chars()
                .filter_map(char_cell_width)
                .sum(),
        )
    })?;
    let mut lines = Vec::with_capacity(raw_lines.len());
    for line in raw_lines {
        let split = split_at_visible_column(line, logo_width)?;
        let (logo_raw, info) = line.split_at(split);
        lines.push(FetchLine {
            logo: visible_text(logo_raw),
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

fn positioned_frame(layout: &FetchLayout, phase: usize, top: u16, left: u16) -> Vec<u8> {
    let mut out = Vec::new();
    for (row, line) in layout.lines.iter().enumerate() {
        out.extend_from_slice(format!("\x1b[{};{}H\x1b[0m", top as usize + row, left).as_bytes());
        for (col, ch) in line.logo.chars().enumerate() {
            if ch == ' ' {
                out.push(b' ');
            } else {
                let (r, g, b) = color_for(phase, col, row);
                out.extend_from_slice(format!("\x1b[38;2;{r};{g};{b}m{ch}\x1b[0m").as_bytes());
            }
        }
        out.extend_from_slice(line.info.as_bytes());
        out.extend_from_slice(b"\x1b[0m");
    }
    out
}

fn dimmed_frame(layout: &FetchLayout, level: u8, top: u16, left: u16) -> Vec<u8> {
    let mut out = Vec::new();
    for (row, line) in layout.lines.iter().enumerate() {
        out.extend_from_slice(
            format!(
                "\x1b[{};{}H\x1b[2m\x1b[38;2;{level};{level};{level}m",
                top as usize + row,
                left
            )
            .as_bytes(),
        );
        out.extend_from_slice(line.logo.as_bytes());
        out.extend_from_slice(visible_text(&line.info).as_bytes());
        out.extend_from_slice(b"\x1b[0m");
    }
    out
}

pub fn centered_animation_chunks(layout: &FetchLayout, rows: u16, cols: u16) -> Vec<Vec<u8>> {
    let height = layout.lines.len() as u16;
    let width = layout
        .lines
        .iter()
        .map(|line| visible_width(&line.logo) + visible_width(&line.info))
        .max()
        .unwrap_or(0) as u16;
    let top = ((rows.saturating_sub(height)) / 2).saturating_add(1);
    let left = ((cols.saturating_sub(width)) / 2).saturating_add(1);
    let mut chunks = Vec::with_capacity(FRAME_COUNT + 5);
    chunks.push(b"\x1b[?25l".to_vec());
    for phase in 0..FRAME_COUNT {
        chunks.push(positioned_frame(layout, phase, top, left));
    }
    for level in [160, 96, 40] {
        chunks.push(dimmed_frame(layout, level, top, left));
    }
    let mut final_frame = Vec::new();
    for (row, line) in layout.lines.iter().enumerate() {
        final_frame
            .extend_from_slice(format!("\x1b[{};{}H\x1b[0m", top as usize + row, left).as_bytes());
        final_frame.extend_from_slice(line.logo.as_bytes());
        final_frame.extend_from_slice(line.info.as_bytes());
        final_frame.extend_from_slice(b"\x1b[0m");
    }
    let prompt_row = top.saturating_add(height).min(rows);
    final_frame.extend_from_slice(format!("\x1b[{};1H\x1b[0m\x1b[?25h", prompt_row).as_bytes());
    chunks.push(final_frame);
    chunks
}

pub fn centered_inline_animation_bytes(layout: &FetchLayout, rows: u16, cols: u16) -> Vec<u8> {
    centered_animation_chunks(layout, rows, cols).concat()
}

pub fn fits_centered_terminal(layout: &FetchLayout, rows: u16, cols: u16) -> bool {
    if rows == 0 || cols == 0 || layout.lines.len() + 2 > rows as usize {
        return false;
    }
    layout
        .lines
        .iter()
        .all(|line| visible_width(&line.logo) + visible_width(&line.info) + 2 <= cols as usize)
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

fn capture_rustfetch_from(executable: &Path) -> Option<String> {
    let mut command = CommandBuilder::new(executable);
    command
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor");
    let size = PtySize::new(120, 40, 0, 0).ok()?;
    let (mut session, output_rx, exit_rx) = match PtySession::spawn(PtyConfig::new(command, size)) {
        Ok(spawned) => spawned,
        Err(e) => {
            eprintln!("mr-crabs: rustfetch spawn failed: {e}");
            return None;
        }
    };

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut output = Vec::new();
    let mut status = None;
    let mut output_disconnected = false;
    while !output_disconnected {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            let _ = session.shutdown_and_reap(Duration::from_millis(100));
            return None;
        }
        match output_rx.recv_timeout(remaining.min(Duration::from_millis(50))) {
            Ok(chunk) => {
                if output.len().saturating_add(chunk.len()) > RUSTFETCH_CAPTURE_MAX_BYTES {
                    let _ = session.shutdown_and_reap(Duration::from_millis(100));
                    return None;
                }
                output.extend_from_slice(&chunk);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => output_disconnected = true,
        }

        if status.is_none() {
            match exit_rx.try_recv() {
                Ok(exit) => status = Some(exit),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    let _ = session.shutdown_and_reap(Duration::from_millis(100));
                    return None;
                }
            }
        }
    }

    if status.is_none() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            let _ = session.shutdown_and_reap(Duration::from_millis(100));
            return None;
        }
        status = exit_rx.recv_timeout(remaining).ok();
    }
    let status = status?;
    if output.is_empty() {
        if status.code() != Some(0) {
            eprintln!(
                "mr-crabs: rustfetch failed (exit status {:?})",
                status.code()
            );
        }
        return None;
    }

    match String::from_utf8(output) {
        Ok(s) => Some(s),
        Err(e) => Some(String::from_utf8_lossy(e.as_bytes()).into_owned()),
    }
}

fn capture_rustfetch() -> Option<String> {
    capture_rustfetch_from(Path::new("rustfetch"))
}

pub fn should_sleep_after_chunk(idx: usize) -> bool {
    idx < FRAME_COUNT
}

pub fn should_run_animated_fetch(args: &[String]) -> bool {
    args.first()
        .is_some_and(|arg| matches!(arg.as_str(), "+animated-fetch" | "+rustfetch"))
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
            let terminal = terminal_size();
            let animate = terminal
                .map(|(rows, cols)| fits_centered_terminal(&layout, rows, cols))
                .unwrap_or(false);
            if !animate {
                let _ = std::io::stdout().write_all(original.as_bytes());
                let _ = std::io::stdout().flush();
                std::process::exit(0);
            }
            let (rows, cols) = terminal.expect("terminal size checked above");
            let chunks = centered_animation_chunks(&layout, rows, cols);
            let mut stdout = std::io::stdout();
            for (idx, chunk) in chunks.iter().enumerate() {
                let _ = stdout.write_all(chunk);
                let _ = stdout.flush();
                if idx > 0 && idx <= FRAME_COUNT {
                    std::thread::sleep(FRAME_DELAY);
                } else if idx > FRAME_COUNT && idx < chunks.len() - 1 {
                    std::thread::sleep(Duration::from_millis(50));
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
                assert!(text.contains("jamie@host"));
            }
        }
    }

    #[test]
    fn exact_capture_after_immediate_exit() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::time::{SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "mr-crabs-animated-fetch-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&dir).expect("create fixture directory");
        let fixture = dir.join("rustfetch-fixture");
        fs::write(
            &fixture,
            "#!/bin/sh\ni=0\nwhile [ \"$i\" -lt 9000 ]; do printf x; i=$((i + 1)); done\nprintf 'FINAL-TAIL\\n'\n",
        )
        .expect("write fixture");
        fs::set_permissions(&fixture, fs::Permissions::from_mode(0o755))
            .expect("make fixture executable");

        let captured = capture_rustfetch_from(&fixture).expect("capture fixture output");
        let mut expected = "x".repeat(9000);
        expected.push_str("FINAL-TAIL\r\n");
        assert_eq!(captured, expected);

        fs::remove_dir_all(dir).expect("remove fixture directory");
    }
    #[test]
    fn exact_capture_preserves_tiny_output() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::time::{SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "mr-crabs-animated-fetch-tiny-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&dir).expect("create fixture directory");
        let fixture = dir.join("rustfetch-fixture");
        fs::write(&fixture, "#!/bin/sh\nprintf 'tiny-tail\\n'\n").expect("write fixture");
        fs::set_permissions(&fixture, fs::Permissions::from_mode(0o755))
            .expect("make fixture executable");

        assert_eq!(
            capture_rustfetch_from(&fixture).expect("capture tiny fixture"),
            "tiny-tail\r\n"
        );
        fs::remove_dir_all(dir).expect("remove fixture directory");
    }

    #[test]
    fn capture_overflow_reaps_and_returns_none() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::time::{SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "mr-crabs-animated-fetch-overflow-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&dir).expect("create fixture directory");
        let fixture = dir.join("rustfetch-fixture");
        fs::write(
            &fixture,
            format!(
                "#!/bin/sh\ndd if=/dev/zero bs={} count=2 2>/dev/null\n",
                RUSTFETCH_CAPTURE_MAX_BYTES
            ),
        )
        .expect("write fixture");
        fs::set_permissions(&fixture, fs::Permissions::from_mode(0o755))
            .expect("make fixture executable");

        assert_eq!(capture_rustfetch_from(&fixture), None);
        fs::remove_dir_all(dir).expect("remove fixture directory");
    }

    #[test]
    fn centered_animation_leaves_final_fetch_visible_with_prompt_below() {
        let out = sample_output();
        let layout = parse_fetch_layout(&out).expect("layout");
        let chunks = centered_animation_chunks(&layout, 24, 80);
        let text = String::from_utf8(chunks.concat()).expect("ansi bytes");
        assert!(text.starts_with("\x1b[?25l"));
        assert!(!text.contains("\x1b[2K"));
        assert!(text.ends_with("\x1b[0m\x1b[16;1H\x1b[0m\x1b[?25h"));
        let final_frame = String::from_utf8(chunks.last().expect("final frame").clone())
            .expect("final frame text");
        for line in &layout.lines {
            assert!(final_frame.contains(&format!("{}{}", line.logo, line.info)));
        }
    }

    #[test]
    fn centered_overlay_uses_absolute_positions_and_fade_frames() {
        let out = sample_output();
        let layout = parse_fetch_layout(&out).expect("layout");
        let chunks = centered_animation_chunks(&layout, 24, 80);
        assert_eq!(chunks.len(), FRAME_COUNT + 5);
        for chunk in chunks.iter().skip(1).take(FRAME_COUNT + 3) {
            assert!(chunk.starts_with(b"\x1b["));
        }
        assert!(
            chunks[FRAME_COUNT + 1]
                .windows(5)
                .any(|window| window == b"\x1b[2m\x1b")
        );
    }

    #[test]
    fn unicode_cell_width_controls_fit_and_final_prompt_position() {
        assert_eq!(visible_width("A中"), 3);
        assert_eq!(visible_width("e\u{0301}"), 1);
        let layout = FetchLayout {
            lines: vec![FetchLine {
                logo: "中".to_string(),
                info: "e\u{0301}".to_string(),
            }],
            logo_width: 2,
        };
        assert!(fits_centered_terminal(&layout, 4, 5));
        assert!(!fits_centered_terminal(&layout, 4, 4));
        let final_frame = String::from_utf8(
            centered_animation_chunks(&layout, 4, 5)
                .last()
                .expect("final frame")
                .clone(),
        )
        .expect("final frame text");
        assert_eq!(
            final_frame,
            "\x1b[2;2H\x1b[0m中e\u{0301}\x1b[0m\x1b[3;1H\x1b[0m\x1b[?25h"
        );
    }

    #[test]
    fn positioned_frame_resets_each_info_row() {
        let layout = FetchLayout {
            lines: vec![
                FetchLine {
                    logo: "A".to_string(),
                    info: "\x1b[31mone".to_string(),
                },
                FetchLine {
                    logo: "B".to_string(),
                    info: "two".to_string(),
                },
            ],
            logo_width: 1,
        };
        let frame = String::from_utf8(positioned_frame(&layout, 0, 2, 3)).expect("frame");
        assert!(frame.contains("\x1b[31mone\x1b[0m\x1b[3;3H\x1b[0m"));
        assert!(frame.ends_with("two\x1b[0m"));
    }

    #[test]
    fn small_terminal_uses_static_fallback() {
        let out = sample_output();
        let layout = parse_fetch_layout(&out).expect("layout");
        assert!(!fits_centered_terminal(&layout, 8, 80));
        assert!(!fits_centered_terminal(&layout, 24, 20));
        assert!(fits_centered_terminal(&layout, 24, 80));
    }

    #[test]
    fn fade_strips_info_color_overrides() {
        let colored = sample_output().replace(
            "OS: Darwin (aarch64)",
            "\x1b[38;2;255;0;0mOS: Darwin (aarch64)\x1b[0m",
        );
        let layout = parse_fetch_layout(&colored).expect("layout");
        let fade = String::from_utf8(dimmed_frame(&layout, 40, 9, 10)).expect("fade");
        assert!(fade.contains("\x1b[38;2;40;40;40m"));
        assert!(!fade.contains("\x1b[38;2;255;0;0m"));
        assert!(fade.contains("OS: Darwin (aarch64)"));
    }

    #[test]
    fn colored_info_uses_visible_columns_for_split_fit_and_final_frame() {
        let colored = sample_output().replace(
            "------------------------------",
            "\x1b[38;2;255;0;0m------------------------------\x1b[0m",
        );
        let layout = parse_fetch_layout(&colored).expect("colored layout");
        assert_eq!(layout.logo_width, 17);
        assert!(fits_centered_terminal(&layout, 24, 80));
        let final_frame = String::from_utf8(
            centered_animation_chunks(&layout, 24, 80)
                .last()
                .expect("final frame")
                .clone(),
        )
        .expect("final frame text");
        assert!(final_frame.starts_with("\x1b[9;17H\x1b[0m"));
        assert!(final_frame.contains("\x1b[38;2;255;0;0m------------------------------\x1b[0m"));
        assert!(final_frame.ends_with("\x1b[0m\x1b[16;1H\x1b[0m\x1b[?25h"));
        assert!(!final_frame.contains("\x1b[2K"));
    }

    #[test]
    fn ansi_adjacent_to_split_keeps_logo_visible_and_info_styled() {
        let layout = parse_fetch_layout("AB\x1b[0m\x1b[32m--- info").expect("layout");
        assert_eq!(layout.logo_width, 2);
        assert_eq!(layout.lines[0].logo, "AB");
        assert_eq!(visible_width(&layout.lines[0].logo), 2);
        assert!(layout.lines[0].info.starts_with("\x1b[32m---"));
    }

    #[test]
    fn should_run_requires_supported_first_argument() {
        assert!(should_run_animated_fetch(&["+animated-fetch".to_string()]));
        assert!(should_run_animated_fetch(&["+rustfetch".to_string()]));
        assert!(!should_run_animated_fetch(&["normal".to_string()]));
        assert!(!should_run_animated_fetch(&[
            "normal".to_string(),
            "+rustfetch".to_string(),
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
}
