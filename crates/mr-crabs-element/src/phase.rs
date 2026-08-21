//! Phase probes for RenderCache (kept additive; no per-cell work).

#[cfg(feature = "phase-timing")]
mod imp {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    static ENABLED: OnceLock<bool> = OnceLock::new();
    static REGISTRY: OnceLock<Mutex<HashMap<&'static str, (u64, u64)>>> = OnceLock::new();

    pub(crate) fn enabled_for_env(
        phase: Option<&std::ffi::OsStr>,
        timing: Option<&std::ffi::OsStr>,
    ) -> bool {
        phase.is_some() || timing.is_some()
    }

    fn enabled_inner() -> bool {
        *ENABLED.get_or_init(|| {
            enabled_for_env(
                std::env::var_os("MR_CRABS_PHASE").as_deref(),
                std::env::var_os("MR_CRABS_PHASE_TIMING").as_deref(),
            )
        })
    }

    pub fn enabled() -> bool {
        enabled_inner()
    }

    pub(crate) fn record_into(
        registry: &Mutex<HashMap<&'static str, (u64, u64)>>,
        phase: &'static str,
        nanos: u64,
    ) {
        if let Ok(mut g) = registry.lock() {
            let e = g.entry(phase).or_insert((0, 0));
            e.0 += 1;
            e.1 += nanos;
        }
    }

    pub fn record(phase: &'static str, nanos: u64) {
        if !enabled_inner() {
            return;
        }
        let registry = REGISTRY.get_or_init(|| Mutex::new(HashMap::new()));
        record_into(registry, phase, nanos);
    }

    pub(crate) fn snapshot_from(
        registry: &Mutex<HashMap<&'static str, (u64, u64)>>,
    ) -> Vec<(&'static str, u64, u64)> {
        let Ok(g) = registry.lock() else {
            return Vec::new();
        };
        g.iter().map(|(k, (c, n))| (*k, *c, *n)).collect()
    }

    pub fn snapshot() -> Vec<(&'static str, u64, u64)> {
        if !enabled_inner() {
            return Vec::new();
        }
        let Some(reg) = REGISTRY.get() else {
            return Vec::new();
        };
        snapshot_from(reg)
    }

    pub(crate) fn snapshot_map_from(
        registry: &Mutex<HashMap<&'static str, (u64, u64)>>,
    ) -> HashMap<&'static str, (u64, u64)> {
        let Ok(g) = registry.lock() else {
            return HashMap::new();
        };
        g.iter().map(|(k, (c, n))| (*k, (*c, *n))).collect()
    }

    pub fn snapshot_map() -> HashMap<&'static str, (u64, u64)> {
        if !enabled_inner() {
            return HashMap::new();
        }
        let Some(reg) = REGISTRY.get() else {
            return HashMap::new();
        };
        snapshot_map_from(reg)
    }

    pub(crate) fn delta_between(
        prev: &HashMap<&'static str, (u64, u64)>,
        cur: &HashMap<&'static str, (u64, u64)>,
    ) -> Vec<(&'static str, u64, u64)> {
        let mut out = Vec::new();
        for (&k, &(c2, n2)) in cur {
            let (c1, n1) = prev.get(k).copied().unwrap_or((0, 0));
            if c2 > c1 || n2 > n1 {
                out.push((k, c2 - c1, n2 - n1));
            }
        }
        out
    }

    pub fn delta_since(prev: &HashMap<&'static str, (u64, u64)>) -> Vec<(&'static str, u64, u64)> {
        let cur = snapshot_map();
        delta_between(prev, &cur)
    }

    pub fn flush_sidecar() {}

    pub struct Guard {
        phase: &'static str,
        start: Option<Instant>,
    }

    impl Guard {
        #[inline]
        pub fn new(phase: &'static str) -> Self {
            if !enabled_inner() {
                return Self { phase, start: None };
            }
            Self {
                phase,
                start: Some(Instant::now()),
            }
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            if let Some(s) = self.start.take() {
                record(self.phase, s.elapsed().as_nanos() as u64);
            }
        }
    }
}

#[cfg(not(feature = "phase-timing"))]
mod imp {
    pub fn enabled() -> bool {
        false
    }

    pub fn record(_phase: &'static str, _nanos: u64) {}

    pub fn snapshot() -> Vec<(&'static str, u64, u64)> {
        Vec::new()
    }

    pub fn snapshot_map() -> std::collections::HashMap<&'static str, (u64, u64)> {
        std::collections::HashMap::new()
    }

    pub fn delta_since(
        _previous: &std::collections::HashMap<&'static str, (u64, u64)>,
    ) -> Vec<(&'static str, u64, u64)> {
        Vec::new()
    }

    pub fn flush_sidecar() {}

    pub struct Guard;

    impl Guard {
        #[inline]
        pub fn new(_phase: &'static str) -> Self {
            Self
        }
    }
}

#[cfg(feature = "phase-timing")]
pub use imp::{delta_since, enabled, flush_sidecar, record, snapshot, snapshot_map, Guard};
#[cfg(not(feature = "phase-timing"))]
pub use imp::{delta_since, enabled, flush_sidecar, record, snapshot, snapshot_map, Guard};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_api_does_not_panic() {
        let _ = enabled();
        record("element::test", 42);
        let _ = snapshot();
        let _ = snapshot_map();
        let prev = snapshot_map();
        let _ = delta_since(&prev);
        flush_sidecar();
        let _guard = Guard::new("element::test");
    }

    #[test]
    fn guard_drop_without_record_is_noop() {
        {
            let _guard = Guard::new("element::guard_noop");
        }
    }

    #[cfg(not(feature = "phase-timing"))]
    mod disabled {
        use super::*;

        #[test]
        fn enabled_is_false() {
            assert!(!enabled(), "without phase-timing feature enabled() must be false");
        }

        #[test]
        fn snapshot_is_empty() {
            assert!(snapshot().is_empty());
            assert!(snapshot_map().is_empty());
        }

        #[test]
        fn delta_since_is_empty() {
            let mut prev = std::collections::HashMap::new();
            prev.insert("a", (1, 100));
            let delta = delta_since(&prev);
            assert!(
                delta.is_empty(),
                "disabled delta_since must return empty vec"
            );
        }

        #[test]
        fn record_is_noop() {
            record("a", 1);
            record("a", 2);
            assert!(snapshot().is_empty());
            assert!(snapshot_map().is_empty());
        }
    }

    #[cfg(feature = "phase-timing")]
    mod enabled_helpers {
        use super::super::imp::{
            delta_between, enabled_for_env, record_into, snapshot_from, snapshot_map_from,
        };
        use std::collections::HashMap;
        use std::ffi::OsStr;
        use std::sync::Mutex;

        #[test]
        fn enabled_for_env_is_pure() {
            assert!(!enabled_for_env(None, None));
            assert!(enabled_for_env(Some(OsStr::new("1")), None));
            assert!(enabled_for_env(None, Some(OsStr::new("1"))));
            assert!(
                enabled_for_env(Some(OsStr::new("")), None),
                "presence not value matters"
            );
            assert!(enabled_for_env(
                Some(OsStr::new("x")),
                Some(OsStr::new("y"))
            ));
        }

        #[test]
        fn record_into_aggregates_counts_and_nanos() {
            let reg = Mutex::new(HashMap::new());
            record_into(&reg, "element::render", 10);
            record_into(&reg, "element::render", 20);
            record_into(&reg, "element::cache", 5);
            let map = snapshot_map_from(&reg);
            assert_eq!(map.get("element::render"), Some(&(2, 30)));
            assert_eq!(map.get("element::cache"), Some(&(1, 5)));
        }

        #[test]
        fn snapshot_from_reflects_registry() {
            let reg = Mutex::new(HashMap::new());
            assert!(snapshot_from(&reg).is_empty());
            record_into(&reg, "a", 7);
            let snap = snapshot_from(&reg);
            assert_eq!(snap.len(), 1);
            assert_eq!(snap[0], ("a", 1, 7));
        }

        #[test]
        fn delta_between_computes_deltas() {
            let mut prev = HashMap::new();
            prev.insert("a", (1, 10));
            prev.insert("b", (2, 20));
            let mut cur = HashMap::new();
            cur.insert("a", (2, 15)); // +1, +5
            cur.insert("b", (2, 20)); // unchanged
            cur.insert("c", (1, 99)); // new
            let mut delta = delta_between(&prev, &cur);
            delta.sort_by_key(|(k, _, _)| *k);
            assert_eq!(delta, vec![("a", 1, 5), ("c", 1, 99)]);
        }

        #[test]
        fn delta_between_empty_prev_returns_all() {
            let prev = HashMap::new();
            let mut cur = HashMap::new();
            cur.insert("x", (3, 300));
            assert_eq!(delta_between(&prev, &cur), vec![("x", 3, 300)]);
        }

        #[test]
        fn delta_between_no_change_is_empty() {
            let mut m = HashMap::new();
            m.insert("a", (5, 50));
            assert!(delta_between(&m, &m).is_empty());
        }

        #[test]
        fn delta_between_nanos_only_counts() {
            let mut prev = HashMap::new();
            prev.insert("a", (1, 10));
            let mut cur = HashMap::new();
            cur.insert("a", (1, 15)); // same count, more nanos
            assert_eq!(delta_between(&prev, &cur), vec![("a", 0, 5)]);
        }

        #[test]
        fn record_into_handles_zero_nanos() {
            let reg = Mutex::new(HashMap::new());
            record_into(&reg, "z", 0);
            let map = snapshot_map_from(&reg);
            assert_eq!(map.get("z"), Some(&(1, 0)));
        }
    }
}
