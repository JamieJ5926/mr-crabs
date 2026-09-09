//! System facts for the startup fetch. Every reader degrades to omission, so
//! `collect` is infallible and always yields at least a title.

use std::ffi::CString;

pub struct SysInfo {
    pub title: String,
    pub lines: Vec<(String, String)>,
}

pub fn collect() -> SysInfo {
    let user = std::env::var("USER").unwrap_or_else(|_| "user".to_string());
    let host = sysctl_string("kern.hostname").unwrap_or_else(|| "localhost".to_string());
    let mut lines = Vec::new();

    if let Some(arch) = sysctl_string("hw.machine") {
        lines.push(("OS".to_string(), format!("Darwin ({arch})")));
    }
    if let Some(kernel) = sysctl_string("kern.osrelease") {
        lines.push(("Kernel".to_string(), format!("MacOS {kernel}")));
    }
    if let Some(cpu) = sysctl_string("machdep.cpu.brand_string") {
        lines.push(("CPU".to_string(), cpu));
    }
    if let Some(total) = sysctl_u64("hw.memsize") {
        lines.push(("RAM".to_string(), format_bytes(total)));
    }
    if let Some(swap) = swap_total() {
        lines.push(("Swap".to_string(), format_bytes(swap)));
    }
    if let Some(secs) = uptime_seconds() {
        lines.push(("Uptime".to_string(), format_duration(secs)));
    }
    if let Some((total, avail)) = root_capacity() {
        lines.push((
            "Disk (/)".to_string(),
            format!(
                "{} / {}",
                format_bytes(total.saturating_sub(avail)),
                format_bytes(total)
            ),
        ));
    }

    lines.retain(|(_, value)| !value.is_empty() && !value.contains('\n'));

    SysInfo {
        title: format!("{user}@{host}"),
        lines,
    }
}

fn sysctl_string(name: &str) -> Option<String> {
    let cname = CString::new(name).ok()?;
    let mut len: libc::size_t = 0;
    let probe = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            std::ptr::null_mut(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if probe != 0 || len == 0 {
        return None;
    }
    let mut buf = vec![0u8; len];
    let read = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 {
        return None;
    }
    buf.truncate(len);
    while buf.last() == Some(&0) {
        buf.pop();
    }
    let text = String::from_utf8(buf).ok()?;
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn sysctl_u64(name: &str) -> Option<u64> {
    let cname = CString::new(name).ok()?;
    let mut value: u64 = 0;
    let mut len = std::mem::size_of::<u64>() as libc::size_t;
    let read = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            (&mut value as *mut u64).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (read == 0).then_some(value)
}

fn swap_total() -> Option<u64> {
    let raw = sysctl_string("vm.swapusage")?;
    let token = raw.split_whitespace().nth(2)?;
    parse_mib(token)
}

fn parse_mib(token: &str) -> Option<u64> {
    let digits: String = token
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let value: f64 = digits.parse().ok()?;
    let scale = if token.ends_with('G') {
        1024.0 * 1024.0 * 1024.0
    } else if token.ends_with('M') {
        1024.0 * 1024.0
    } else {
        1024.0
    };
    Some((value * scale) as u64)
}

fn uptime_seconds() -> Option<u64> {
    let cname = CString::new("kern.boottime").ok()?;
    let mut tv = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    let mut len = std::mem::size_of::<libc::timeval>() as libc::size_t;
    let read = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            (&mut tv as *mut libc::timeval).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 || tv.tv_sec <= 0 {
        return None;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    now.checked_sub(tv.tv_sec as u64)
}

fn root_capacity() -> Option<(u64, u64)> {
    let path = CString::new("/").ok()?;
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    let read = unsafe { libc::statfs(path.as_ptr(), &mut stat) };
    if read != 0 || stat.f_blocks == 0 {
        return None;
    }
    let block = stat.f_bsize as u64;
    Some((stat.f_blocks * block, stat.f_bavail * block))
}

pub fn format_bytes(bytes: u64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let value = bytes as f64;
    if value >= GB {
        format!("{:.2} GB", value / GB)
    } else if value >= MB {
        format!("{:.0} MB", value / MB)
    } else {
        format!("{bytes} B")
    }
}

pub fn format_duration(total_secs: u64) -> String {
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    format!("{hours:02}h {minutes:02}m {seconds:02}s")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_has_title() {
        assert!(collect().title.contains('@'), "title is user@host");
    }

    #[test]
    fn collect_has_lines() {
        assert!(collect().lines.len() >= 3, "at least three readable facts");
    }

    #[test]
    fn values_are_single_line() {
        for (label, value) in collect().lines {
            assert!(!value.is_empty(), "{label} has a value");
            assert!(!value.contains('\n'), "{label} value stays on one row");
        }
    }

    #[test]
    fn formatting_boundaries() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1024 * 1024), "1 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GB");
        assert_eq!(format_duration(0), "00h 00m 00s");
        assert_eq!(format_duration(3661), "01h 01m 01s");
    }
}
