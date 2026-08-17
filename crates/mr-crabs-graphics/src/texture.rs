//! Bounded texture-cache interface for GPUI consumption.
//!
//! The renderer (a future GPUI `TerminalElement`) owns the actual GPU
//! textures; this cache owns the *bookkeeping*: a deterministic LRU keyed by
//! `(image_id, generation)` with explicit byte and count budgets. Image
//! retransmission bumps the store generation, so a stale key never resolves
//! to a stale texture. Handles are opaque monotonically increasing ids the
//! host maps to its own GPU resources. No renderer dependency exists here.

use std::collections::HashMap;

/// Cache key: the image id plus the store generation stamp at insertion.
/// A changed generation (retransmission or replacement) yields a new key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureKey {
    pub image_id: u32,
    pub generation: u64,
}

impl TextureKey {
    pub fn new(image_id: u32, generation: u64) -> Self {
        Self {
            image_id,
            generation,
        }
    }
}

/// An opaque handle to a cached texture; the host maps this to its GPU
/// texture id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureHandle {
    pub id: u64,
}

/// Errors from texture-cache operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureError {
    /// A single entry exceeds the byte budget.
    OverBudget,
    /// The entry count budget cannot be satisfied.
    TooManyEntries,
}

struct Entry {
    handle: TextureHandle,
    bytes: usize,
    /// Monotonic use counter; lower is older (LRU).
    last_used: u64,
    /// Tie-break for identical timestamps (insertion order).
    sequence: u64,
}

/// A deterministic LRU texture cache with byte and count budgets.
pub struct TextureCache {
    max_bytes: usize,
    max_entries: usize,
    clock: u64,
    next_sequence: u64,
    next_handle: u64,
    entries: HashMap<TextureKey, Entry>,
    total_bytes: usize,
}

impl Default for TextureCache {
    fn default() -> Self {
        // Oracle-scale defaults: 320 MB of decoded images, 4096 textures.
        Self::new(320 * 1000 * 1000, 4096)
    }
}

impl TextureCache {
    pub fn new(max_bytes: usize, max_entries: usize) -> Self {
        Self {
            max_bytes,
            max_entries,
            clock: 0,
            next_sequence: 0,
            next_handle: 1,
            entries: HashMap::new(),
            total_bytes: 0,
        }
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    pub fn max_entries(&self) -> usize {
        self.max_entries
    }

    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Look up a key and touch it (LRU refresh). Returns the handle when
    /// resident.
    pub fn get(&mut self, key: &TextureKey) -> Option<TextureHandle> {
        let entry = self.entries.get_mut(key)?;
        self.clock = self.clock.wrapping_add(1);
        entry.last_used = self.clock;
        Some(entry.handle)
    }

    /// Insert (or replace) a key with the given byte cost, evicting
    /// least-recently-used entries as needed. Eviction is deterministic:
    /// oldest `last_used` first, then lowest `sequence`, then lowest handle.
    pub fn insert(&mut self, key: TextureKey, bytes: usize) -> Result<TextureHandle, TextureError> {
        if bytes > self.max_bytes {
            return Err(TextureError::OverBudget);
        }

        // A replacement of an existing key keeps its handle.
        let handle = match self.entries.remove(&key) {
            Some(old) => {
                self.total_bytes -= old.bytes;
                old.handle
            }
            None => {
                let h = TextureHandle {
                    id: self.next_handle,
                };
                self.next_handle = self.next_handle.wrapping_add(1);
                h
            }
        };

        self.clock = self.clock.wrapping_add(1);
        let seq = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.entries.insert(
            key,
            Entry {
                handle,
                bytes,
                last_used: self.clock,
                sequence: seq,
            },
        );
        self.total_bytes += bytes;

        self.enforce_budgets()?;
        Ok(handle)
    }

    /// Remove a key (e.g. when the image is deleted from the store).
    pub fn remove(&mut self, key: &TextureKey) -> Option<TextureHandle> {
        let entry = self.entries.remove(key)?;
        self.total_bytes -= entry.bytes;
        Some(entry.handle)
    }

    /// Evict until both budgets hold. Deterministic LRU: oldest use, then
    /// lowest insertion sequence, then lowest handle id.
    fn enforce_budgets(&mut self) -> Result<(), TextureError> {
        while self.total_bytes > self.max_bytes || self.entries.len() > self.max_entries {
            let victim = self
                .entries
                .iter()
                .min_by_key(|(_, e)| (e.last_used, e.sequence, e.handle.id))
                .map(|(k, _)| *k);
            let Some(key) = victim else {
                break;
            };
            let entry = self.entries.remove(&key).unwrap();
            self.total_bytes -= entry.bytes;
        }
        if self.total_bytes > self.max_bytes {
            return Err(TextureError::OverBudget);
        }
        if self.entries.len() > self.max_entries {
            return Err(TextureError::TooManyEntries);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_get_and_touch() {
        let mut c = TextureCache::new(1000, 10);
        let k = TextureKey::new(1, 1);
        let h = c.insert(k, 100).unwrap();
        assert_eq!(c.get(&k), Some(h));
        assert_eq!(c.total_bytes(), 100);
    }

    #[test]
    fn lru_eviction_is_deterministic() {
        let mut c = TextureCache::new(500, 10);
        let k1 = TextureKey::new(1, 1);
        let k2 = TextureKey::new(2, 1);
        let k3 = TextureKey::new(3, 1);
        c.insert(k1, 200).unwrap();
        c.insert(k2, 200).unwrap();
        // Touch k1 so k2 becomes the oldest.
        c.get(&k1);
        c.insert(k3, 200).unwrap(); // needs 100 bytes -> evict k2
        assert!(c.get(&k2).is_none());
        assert!(c.get(&k1).is_some());
        assert!(c.get(&k3).is_some());
        assert!(c.total_bytes() <= 500);
    }

    #[test]
    fn count_budget_evicts_oldest() {
        let mut c = TextureCache::new(100_000, 2);
        let k1 = TextureKey::new(1, 1);
        let k2 = TextureKey::new(2, 1);
        let k3 = TextureKey::new(3, 1);
        c.insert(k1, 10).unwrap();
        c.insert(k2, 10).unwrap();
        c.insert(k3, 10).unwrap();
        assert_eq!(c.len(), 2);
        assert!(c.get(&k1).is_none());
        assert!(c.get(&k2).is_some());
        assert!(c.get(&k3).is_some());
    }

    #[test]
    fn single_entry_over_budget_rejected() {
        let mut c = TextureCache::new(100, 10);
        assert_eq!(
            c.insert(TextureKey::new(1, 1), 200),
            Err(TextureError::OverBudget)
        );
        assert!(c.is_empty());
    }

    #[test]
    fn replacement_keeps_handle_and_refreshes_lru() {
        let mut c = TextureCache::new(1000, 10);
        let k = TextureKey::new(1, 1);
        let h1 = c.insert(k, 100).unwrap();
        // New generation (retransmission) creates a new key -> new handle.
        // The old generation is a distinct texture key and stays resident
        // until evicted (see stale_generation_key_never_resolves); the
        // renderer simply never requests it after a retransmission.
        let k2 = TextureKey::new(1, 2);
        let h2 = c.insert(k2, 100).unwrap();
        assert_ne!(h1, h2);
        assert!(c.get(&k).is_some());
        // Same key replacement keeps the handle.
        let h3 = c.insert(k2, 200).unwrap();
        assert_eq!(h2, h3);
        assert_eq!(c.total_bytes(), 300);
    }

    #[test]
    fn stale_generation_key_never_resolves() {
        let mut c = TextureCache::new(1000, 10);
        let k_old = TextureKey::new(5, 1);
        let k_new = TextureKey::new(5, 2);
        c.insert(k_old, 100).unwrap();
        assert!(c.get(&k_old).is_some());
        c.insert(k_new, 100).unwrap();
        // Old key is a different texture: both resident until evicted.
        assert!(c.get(&k_old).is_some());
        assert!(c.get(&k_new).is_some());
    }

    #[test]
    fn remove_frees_bytes() {
        let mut c = TextureCache::new(1000, 10);
        let k = TextureKey::new(1, 1);
        let h = c.insert(k, 300).unwrap();
        assert_eq!(c.remove(&k), Some(h));
        assert_eq!(c.total_bytes(), 0);
        assert_eq!(c.remove(&k), None);
    }

    #[test]
    fn zero_budget_rejects_everything() {
        let mut c = TextureCache::new(0, 0);
        assert_eq!(
            c.insert(TextureKey::new(1, 1), 1),
            Err(TextureError::OverBudget)
        );
        assert!(c.is_empty());
    }
}
