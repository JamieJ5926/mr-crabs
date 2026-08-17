//! macOS memory measurement helpers for S12 workloads.
//!
//! Each S12 measured run executes in a fresh benchmark process, so the
//! process-wide `getrusage` peak is also the run peak. Current RSS and the
//! physical footprint come from `task_info(TASK_VM_INFO)`; `phys_footprint`
//! is the metric the S0 oracle baseline recorded as `peak_footprint_bytes`.
//!
//! On non-macOS targets every helper returns `None`; the gate driver treats
//! a missing RSS/footprint value as comparison-ineligible and fails closed.

/// Current resident set size in bytes, or `None` when unavailable.
#[cfg(target_os = "macos")]
pub fn current_rss_bytes() -> Option<u64> {
    let info = task_vm_info()?;
    Some(info.resident_size)
}

/// Current physical footprint in bytes (the S0 baseline's
/// `peak_footprint_bytes` metric), or `None` when unavailable.
#[cfg(target_os = "macos")]
pub fn current_phys_footprint_bytes() -> Option<u64> {
    let info = task_vm_info()?;
    Some(info.phys_footprint)
}

/// Process peak RSS in bytes (`ru_maxrss`), or `None` when unavailable.
#[cfg(target_os = "macos")]
pub fn peak_rss_bytes() -> Option<u64> {
    // SAFETY: `usage` points to writable storage for `getrusage`, and a
    // successful call initializes the complete structure.
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if result == 0 {
        // SAFETY: established by the successful `getrusage` call above.
        let usage = unsafe { usage.assume_init() };
        Some(usage.ru_maxrss.try_into().unwrap_or(0))
    } else {
        None
    }
}

/// Read the current `task_vm_info` record.
#[cfg(target_os = "macos")]
fn task_vm_info() -> Option<mach2::task_info::task_vm_info> {
    use mach2::kern_return::KERN_SUCCESS;
    use mach2::task::task_info;
    use mach2::task_info::{TASK_VM_INFO, task_info_t, task_vm_info};
    use mach2::traps::mach_task_self;
    use mach2::vm_types::natural_t;

    // SAFETY: `info` is writable storage for one `task_vm_info`; the Mach
    // count is measured in `natural_t` units.
    let mut info = std::mem::MaybeUninit::<task_vm_info>::zeroed();
    let mut count = (std::mem::size_of::<task_vm_info>() / std::mem::size_of::<natural_t>()) as u32;
    let result = unsafe {
        task_info(
            mach_task_self(),
            TASK_VM_INFO,
            info.as_mut_ptr().cast::<i32>() as task_info_t,
            &mut count,
        )
    };
    if result != KERN_SUCCESS {
        return None;
    }
    // SAFETY: a successful `task_info` call initializes the structure.
    Some(unsafe { info.assume_init() })
}

#[cfg(not(target_os = "macos"))]
pub fn current_rss_bytes() -> Option<u64> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn current_phys_footprint_bytes() -> Option<u64> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn peak_rss_bytes() -> Option<u64> {
    None
}

/// Periodic physical-footprint sampler.
///
/// Long workloads call [`tick`](FootprintTracker::tick) from their loop so
/// the reported peak tracks intra-run spikes instead of only the run-end
/// value. Sampling is deliberately cheap (once every 64 ticks).
pub struct FootprintTracker {
    peak: Option<u64>,
    ticks: u64,
}

impl FootprintTracker {
    pub fn new() -> Self {
        Self {
            peak: crate::memory::current_phys_footprint_bytes(),
            ticks: 0,
        }
    }

    /// Sample the footprint; call from workload loops.
    pub fn tick(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);
        if self.ticks % 64 == 0 {
            self.sample_now();
        }
    }

    /// Take a final sample and return the observed peak in bytes.
    pub fn peak_bytes(&mut self) -> Option<u64> {
        self.sample_now();
        self.peak
    }

    fn sample_now(&mut self) {
        if let Some(bytes) = crate::memory::current_phys_footprint_bytes() {
            self.peak = Some(self.peak.map_or(bytes, |peak| peak.max(bytes)));
        }
    }
}

impl Default for FootprintTracker {
    fn default() -> Self {
        Self::new()
    }
}
