//! Stable per-cell render keys and the bounded change tracker.
//!
//! Port of the oracle's `cellRenderKey` + `TextAnimationState` change
//! detection (`verification/manifests/dirty-oracle-v2.patch`,
//! `src/renderer/generic.zig`, new-file lines 48-80 and 730-811): a hash of
//! the rendered-relevant content of a terminal cell decides which cells
//! changed between rebuilds so the built-in text animation only animates
//! newly changed glyph output. When a rebuilt row has a new generation but
//! the same final render keys, drawable glyphs are timestamped again: the
//! generation proves a fresh write cycle occurred even when repeated output
//! collapsed to an identical final snapshot.
//!
//! The Rust compact cell (`mr_crabs_terminal::Cell`) is exactly 8 bytes —
//! `content: u32, style: u16, flags: u16` — so the render key is the raw
//! byte pattern as a `u64`, covering the glyph scalar, the interned style
//! index (colors and flags), and the wide/spacer/combining properties.
//! Cursor, selection, and search-highlight state are not part of the cell
//! and never animate, matching the oracle.

use crate::schedule::TypewriterSchedule;
use mr_crabs_terminal::{Cell, RowDelta};

/// Sentinel change time (milliseconds) for cells that never changed:
/// -1000 shader seconds (oracle `TextAnimationState.never = -1000`), so
/// the elapsed time is always far past any configured reveal duration
/// (maximum 5 s) and never-changed cells pass through unchanged.
pub const NEVER_MS: f64 = -1_000_000.0;

/// Pack a change time in milliseconds into the IEEE-754 `f32` bit pattern
/// of shader-time seconds, matching the oracle's change-texture texel
/// format (each texel holds the bit pattern of the shader time at which the
/// cell's content last changed).
pub const fn pack_time_ms(ms: f64) -> u32 {
    f32::to_bits((ms / 1000.0) as f32)
}

/// Unpack a change-texture texel bit pattern back into milliseconds.
///
/// Test-only round-trip helper: the packed texels are consumed by the GPU
/// path, so this is never used outside `#[cfg(test)]` verification.
#[cfg(test)]
pub const fn unpack_time_bits(bits: u32) -> f64 {
    f32::from_bits(bits) as f64 * 1000.0
}

/// The bit pattern of the never-changed sentinel texel.
pub const NEVER_BITS: u32 = pack_time_ms(NEVER_MS);

/// A stable render key for a terminal cell: the raw 8-byte cell pattern
/// (content scalar, style index, flags). Equal keys mean the final rendered
/// content is identical; a new row generation can still re-time drawable
/// glyphs when repeated output produced that same final snapshot.
pub fn cell_render_key(cell: Cell) -> u64 {
    u64::from(cell.content) | (u64::from(cell.style) << 32) | (u64::from(cell.flags) << 48)
}

/// True when a cell contributes a drawable glyph to the text pass.
///
/// This mirrors the render cache's glyph filter: spaces, NULs, and the
/// trailing spacer of a wide character do not consume a repeated-write
/// timestamp. Genuine key changes continue to stamp every changed cell,
/// including spaces, exactly as before.
fn cell_paints_glyph(cell: Cell) -> bool {
    cell.flags & Cell::WIDE_SPACER == 0
        && char::from_u32(cell.content).is_some_and(|ch| ch != ' ' && ch != '\0')
}

/// Bounded, dense per-cell change store.
///
/// One entry per tracked grid cell, row-major, row 0 is the top row:
/// * `snapshot` — the render key of the previous rebuild (sentinel
///   `u64::MAX` = never seen),
/// * `change_times` — packed change timestamps (sentinel
///   [`NEVER_BITS`] = never changed),
/// * `packed` — the same timestamps repacked as rgba8 texel bytes for GPU
///   upload (little-endian bit pattern, oracle `textAnimationUpload`),
/// * `row_generations` — the last-seen row generation for both the fast path
///   (an unchanged generation is skipped) and repeated-write detection (a
///   new generation with identical final keys re-times drawable glyphs).
///
/// The tracker is explicitly bounded: it never stores more than
/// `min(cols * rows, max_cells)` cells (the row-major prefix), so every
/// payload has a documented count bound. Resizes preserve the stored
/// prefix; cells beyond the previous grid are marked changed at the resize
/// time so they reveal like any other new content (oracle
/// `textAnimationEnsureBuffers`).
pub struct ChangeTracker {
    cols: usize,
    rows: usize,
    cap: usize,
    max_cells: usize,
    snapshot: Vec<u64>,
    change_times: Vec<u32>,
    /// Exact change timestamps in milliseconds, parallel to
    /// `change_times`. The model's deterministic clock runs at f64
    /// millisecond precision; the packed `u32`/rgba8 texels carry the
    /// oracle's IEEE-754 `f32` shader-seconds approximation (≤ 1.5e-5 ms
    /// error, far below any reveal window) for GPU upload.
    change_ms: Vec<f64>,
    packed: Vec<u8>,
    row_generations: Vec<u64>,
    last_change_ms: f64,
    upload_dirty: bool,
}

impl ChangeTracker {
    /// Create a tracker for a grid, capped at `max_cells` tracked cells.
    pub fn new(cols: usize, rows: usize, max_cells: usize) -> Self {
        let cap = Self::cap_for(cols, rows, max_cells);
        let mut tracker = Self {
            cols,
            rows,
            cap,
            max_cells,
            snapshot: vec![u64::MAX; cap],
            change_times: vec![NEVER_BITS; cap],
            change_ms: vec![NEVER_MS; cap],
            packed: vec![0; cap * 4],
            row_generations: vec![u64::MAX; rows],
            last_change_ms: NEVER_MS,
            upload_dirty: true,
        };
        tracker.repack();
        tracker
    }

    /// The tracked-cell bound for a grid: `min(cols * rows, max_cells)`,
    /// at least one cell.
    pub const fn cap_for(cols: usize, rows: usize, max_cells: usize) -> usize {
        // `Ord::min`/`Ord::max` are not const-stable yet (rust#143874), so
        // the clamp is written out.
        let max_cells = if max_cells < 1 { 1 } else { max_cells };
        let product = cols.saturating_mul(rows);
        if product < max_cells {
            product
        } else {
            max_cells
        }
    }

    /// Resize the tracker. Stored keys/timestamps for the preserved prefix
    /// survive; cells beyond the previous grid are marked changed at
    /// `now_ms` (they reveal like newly written content); new rows have no
    /// stored generation and are diffed on their next rebuild.
    pub fn resize(&mut self, cols: usize, rows: usize, now_ms: f64) {
        let new_cap = Self::cap_for(cols, rows, self.max_cells);
        self.cols = cols;
        self.rows = rows;
        if new_cap != self.cap {
            let preserve = self.cap.min(new_cap);
            let mut snapshot = vec![u64::MAX; new_cap];
            let mut change_times = vec![NEVER_BITS; new_cap];
            let mut change_ms = vec![NEVER_MS; new_cap];
            let packed = vec![0u8; new_cap * 4];
            snapshot[..preserve].copy_from_slice(&self.snapshot[..preserve]);
            change_times[..preserve].copy_from_slice(&self.change_times[..preserve]);
            change_ms[..preserve].copy_from_slice(&self.change_ms[..preserve]);
            let now_bits = pack_time_ms(now_ms);
            for (bits, ms) in change_times[preserve..]
                .iter_mut()
                .zip(&mut change_ms[preserve..])
            {
                *bits = now_bits;
                *ms = now_ms;
            }
            self.snapshot = snapshot;
            self.change_times = change_times;
            self.change_ms = change_ms;
            self.packed = packed;
            self.cap = new_cap;
        }
        self.row_generations.resize(rows, u64::MAX);
        self.upload_dirty = true;
        self.repack();
    }

    /// Diff one rebuilt row against the previous snapshot and timestamp
    /// changed cells. `anim_ms` is the rebuild time shared by every change
    /// in streaming mode; when `schedule` is active (typewriter), changed
    /// cells consume staggered timestamps from the persistent burst
    /// schedule instead. If a new row generation has no final key changes,
    /// drawable glyphs are re-timestamped so repeated identical output still
    /// receives one reveal. Only rebuilt rows are diffed — rows absent from
    /// the frame are unchanged by definition (oracle
    /// `textAnimationUpdateRow`).
    pub fn update_row(
        &mut self,
        row: u16,
        generation: u64,
        cells: &[Cell],
        anim_ms: f64,
        schedule: &mut TypewriterSchedule,
    ) {
        let row = usize::from(row);
        if row >= self.rows {
            return;
        }
        if self.row_generations[row] == generation {
            return;
        }
        self.row_generations[row] = generation;
        let base = row * self.cols;
        if base >= self.cap {
            // Beyond the tracked prefix: cells here are never stamped and
            // pass through (their texels stay sentinel).
            return;
        }
        let len = cells.len().min(self.cols).min(self.cap - base);
        let mut last = self.last_change_ms;
        let mut changed = false;
        for (x, cell) in cells.iter().take(len).enumerate() {
            let i = base + x;
            let key = cell_render_key(*cell);
            if key == self.snapshot[i] {
                continue;
            }
            self.snapshot[i] = key;
            self.stamp_change(i, anim_ms, schedule, &mut last);
            changed = true;
        }

        // A terminal row generation advances when the engine observes a new
        // write cycle. If that cycle produced the same final cell keys (for
        // example, running the same printf twice), the key diff alone cannot
        // see it. Re-time drawable glyph cells and the spacer that carries
        // the trailing half of a wide glyph. Ordinary spaces remain untouched,
        // and the same generation still returns through the fast path above.
        if !changed {
            for (x, cell) in cells.iter().take(len).enumerate() {
                if !cell_paints_glyph(*cell) {
                    continue;
                }
                self.stamp_change(base + x, anim_ms, schedule, &mut last);
                if cell.flags & Cell::WIDE != 0
                    && x + 1 < len
                    && cells[x + 1].flags & Cell::WIDE_SPACER != 0
                {
                    self.stamp_change(base + x + 1, anim_ms, schedule, &mut last);
                }
                changed = true;
            }
        }

        if changed {
            self.last_change_ms = last;
            self.upload_dirty = true;
            self.repack();
        }
    }

    fn stamp_change(
        &mut self,
        index: usize,
        anim_ms: f64,
        schedule: &mut TypewriterSchedule,
        last: &mut f64,
    ) {
        let timestamp = if schedule.is_active() {
            schedule.next_timestamp()
        } else {
            anim_ms
        };
        self.change_times[index] = pack_time_ms(timestamp);
        self.change_ms[index] = timestamp;
        if timestamp > *last {
            *last = timestamp;
        }
    }

    pub fn can_translate_up_one(&self, rows: &[RowDelta]) -> bool {
        if self.cap != self.cols.saturating_mul(self.rows) || rows.len() != self.rows {
            return false;
        }
        rows.iter().take(self.rows.saturating_sub(1)).all(|row| {
            let dst = usize::from(row.row);
            dst < self.rows.saturating_sub(1)
                && row.cells.len() >= self.cols
                && row
                    .cells
                    .iter()
                    .take(self.cols)
                    .enumerate()
                    .all(|(col, cell)| {
                        cell_render_key(*cell) == self.snapshot[(dst + 1) * self.cols + col]
                    })
        })
    }

    pub fn translate_up_one(&mut self) {
        let shift = self.cols.min(self.cap);
        self.snapshot.copy_within(shift..self.cap, 0);
        self.change_times.copy_within(shift..self.cap, 0);
        self.change_ms.copy_within(shift..self.cap, 0);
        self.snapshot[self.cap - shift..].fill(u64::MAX);
        self.change_times[self.cap - shift..].fill(NEVER_BITS);
        self.change_ms[self.cap - shift..].fill(NEVER_MS);
        self.row_generations.fill(u64::MAX);
        self.last_change_ms = self
            .change_ms
            .iter()
            .copied()
            .filter(|value| *value != NEVER_MS)
            .fold(NEVER_MS, f64::max);
        self.upload_dirty = true;
        self.repack();
    }

    /// Synchronize bypassed rows without assigning fresh reveal stamps.
    ///
    /// Existing timestamps survive where the final render key is
    /// unchanged, so an in-flight reveal can finish across a large or
    /// Full frame. Cells whose bypassed content changed have their old
    /// timestamp cleared so stale overlays never conceal replacement
    /// text. New cells therefore enter the snapshot without animating.
    pub fn sync_rows_without_stamping(&mut self, rows: &[RowDelta]) {
        let mut timestamps_changed = false;
        for row in rows {
            let row_index = usize::from(row.row);
            if row_index >= self.rows {
                continue;
            }
            self.row_generations[row_index] = row.generation;
            let base = row_index * self.cols;
            let len = row
                .cells
                .len()
                .min(self.cols)
                .min(self.cap.saturating_sub(base));
            for (col, cell) in row.cells.iter().take(len).enumerate() {
                let index = base + col;
                let key = cell_render_key(*cell);
                if self.snapshot[index] == key {
                    continue;
                }
                self.snapshot[index] = key;
                if self.change_times[index] != NEVER_BITS {
                    self.change_times[index] = NEVER_BITS;
                    self.change_ms[index] = NEVER_MS;
                    timestamps_changed = true;
                }
            }
        }
        if timestamps_changed {
            self.last_change_ms = self
                .change_ms
                .iter()
                .copied()
                .filter(|value| *value != NEVER_MS)
                .fold(NEVER_MS, f64::max);
            self.upload_dirty = true;
            self.repack();
        }
    }

    /// Clear every retained reveal timestamp while preserving cell
    /// snapshots and row generations. Used when a resize or alternate
    /// screen transition invalidates the old cell coordinate space.
    pub fn clear_changes(&mut self) {
        self.change_times.fill(NEVER_BITS);
        self.change_ms.fill(NEVER_MS);
        self.last_change_ms = NEVER_MS;
        self.upload_dirty = true;
        self.repack();
    }

    pub fn adopt_rows(&mut self, rows: &[RowDelta]) {
        for row in rows {
            let row_index = usize::from(row.row);
            if row_index >= self.rows {
                continue;
            }
            self.row_generations[row_index] = row.generation;
            let base = row_index * self.cols;
            let len = row
                .cells
                .len()
                .min(self.cols)
                .min(self.cap.saturating_sub(base));
            for (col, cell) in row.cells.iter().take(len).enumerate() {
                self.snapshot[base + col] = cell_render_key(*cell);
                self.change_times[base + col] = NEVER_BITS;
                self.change_ms[base + col] = NEVER_MS;
            }
        }
        self.last_change_ms = NEVER_MS;
        self.upload_dirty = true;
        self.repack();
    }

    /// Repack every change time into the rgba8 texel byte layout.
    fn repack(&mut self) {
        for (i, &bits) in self.change_times.iter().enumerate() {
            let b = i * 4;
            self.packed[b] = bits as u8;
            self.packed[b + 1] = (bits >> 8) as u8;
            self.packed[b + 2] = (bits >> 16) as u8;
            self.packed[b + 3] = (bits >> 24) as u8;
        }
    }

    /// The packed change texture (one rgba8 texel per tracked cell,
    /// little-endian IEEE-754 bit pattern of shader-time seconds).
    pub fn change_texture(&self) -> &[u8] {
        &self.packed
    }

    /// True when the change texture needs re-uploading.
    pub const fn upload_dirty(&self) -> bool {
        self.upload_dirty
    }

    /// Acknowledge a texture upload.
    pub fn clear_upload_dirty(&mut self) {
        self.upload_dirty = false;
    }

    /// The most recent change timestamp handed out, or [`NEVER_MS`] when
    /// no cell has changed yet.
    pub const fn last_change_ms(&self) -> f64 {
        self.last_change_ms
    }

    /// Grid columns.
    pub const fn cols(&self) -> usize {
        self.cols
    }

    /// Grid rows.
    pub const fn rows(&self) -> usize {
        self.rows
    }

    /// The number of tracked cells (the explicit count bound).
    pub const fn tracked_cells(&self) -> usize {
        self.cap
    }

    /// The packed bit pattern of the change time at a tracked-cell index.
    pub fn bits_at(&self, index: usize) -> u32 {
        self.change_times[index]
    }

    /// The exact change timestamp (milliseconds) at a tracked-cell index,
    /// or [`NEVER_MS`] for cells that never changed.
    pub fn change_ms_at(&self, index: usize) -> f64 {
        self.change_ms[index]
    }

    /// Retained heap bytes (snapshot keys, packed times, exact
    /// millisecond times, texel bytes, and row generations). All arrays
    /// are exactly `cap`/`rows` sized.
    pub fn retained_capacity(&self) -> usize {
        self.snapshot.capacity() * 8
            + self.change_times.capacity() * 4
            + self.change_ms.capacity() * 8
            + self.packed.capacity()
            + self.row_generations.capacity() * 8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(content: u32, style: u16, flags: u16) -> Cell {
        Cell {
            content,
            style,
            flags,
        }
    }

    #[test]
    fn render_key_covers_content_style_flags() {
        let a = cell(65, 0, 0);
        let b = cell(66, 0, 0);
        let c = cell(65, 1, 0);
        let d = cell(65, 0, Cell::WIDE);
        assert_ne!(cell_render_key(a), cell_render_key(b));
        assert_ne!(cell_render_key(a), cell_render_key(c));
        assert_ne!(cell_render_key(a), cell_render_key(d));
        assert_eq!(cell_render_key(a), cell_render_key(cell(65, 0, 0)));
    }

    #[test]
    fn streaming_stamps_changes_and_repeated_identical_glyph_writes() {
        let mut t = ChangeTracker::new(4, 2, 16);
        let mut sched = TypewriterSchedule::new(0.0);
        let row = vec![
            cell(65, 0, 0),
            cell(66, 0, 0),
            cell(32, 0, 0),
            cell(32, 0, 0),
        ];
        // A fresh tracker's snapshot is all-sentinel (oracle
        // textAnimationEnsureBuffers marks never-seen cells changed), so
        // the first rebuild stamps every cell of the rebuilt row —
        // including spaces — and the rebuild time is shared by all.
        t.update_row(0, 1, &row, 1000.0, &mut sched);
        assert_eq!(t.last_change_ms(), 1000.0);
        assert_eq!(t.change_ms_at(0), 1000.0);
        assert_eq!(t.change_ms_at(1), 1000.0);
        assert_eq!(t.change_ms_at(2), 1000.0);
        assert_eq!(t.change_ms_at(3), 1000.0);
        assert!(t.upload_dirty());

        // Same generation: no diff, no restamp.
        t.update_row(0, 1, &row, 2000.0, &mut sched);
        assert_eq!(t.last_change_ms(), 1000.0);

        // A new generation with the same final cells represents a fresh
        // write cycle. Re-time drawable glyphs, but not spaces.
        t.update_row(0, 2, &row, 2000.0, &mut sched);
        assert_eq!(t.last_change_ms(), 2000.0);
        assert_eq!(t.change_ms_at(0), 2000.0);
        assert_eq!(t.change_ms_at(1), 2000.0);
        assert_eq!(t.change_ms_at(2), 1000.0);
        assert_eq!(t.change_ms_at(3), 1000.0);

        // A real key change still restamps only the changed cell.
        let row2 = vec![
            cell(90, 0, 0),
            cell(66, 0, 0),
            cell(32, 0, 0),
            cell(32, 0, 0),
        ];
        t.update_row(0, 3, &row2, 3000.0, &mut sched);
        assert_eq!(t.last_change_ms(), 3000.0);
        assert_eq!(t.change_ms_at(0), 3000.0);
        assert_eq!(t.change_ms_at(1), 2000.0);
        assert_eq!(t.change_ms_at(2), 1000.0);
    }

    #[test]
    fn identical_blank_row_does_not_restart_animation() {
        let mut t = ChangeTracker::new(4, 1, 16);
        let mut sched = TypewriterSchedule::new(0.0);
        let row = vec![cell(32, 0, 0); 4];
        t.update_row(0, 1, &row, 1000.0, &mut sched);
        t.update_row(0, 2, &row, 2000.0, &mut sched);
        assert_eq!(t.last_change_ms(), 1000.0);
        assert!(
            (0..4).all(|index| t.change_ms_at(index) == 1000.0),
            "spaces must not be re-timed by the identical-row fallback"
        );
    }

    #[test]
    fn typewriter_stamps_consume_schedule_slots_in_reading_order() {
        let mut t = ChangeTracker::new(4, 1, 16);
        let mut sched = TypewriterSchedule::new(15.0);
        sched.begin_build(1000.0, 120.0);
        // Fresh tracker: every cell of the rebuilt row consumes a slot,
        // spaces included (oracle never-seen behavior).
        let row = vec![
            cell(65, 0, 0),
            cell(66, 0, 0),
            cell(67, 0, 0),
            cell(32, 0, 0),
        ];
        t.update_row(0, 1, &row, 1000.0, &mut sched);
        assert_eq!(t.change_ms_at(0), 1000.0);
        assert_eq!(t.change_ms_at(1), 1015.0);
        assert_eq!(t.change_ms_at(2), 1030.0);
        assert_eq!(t.change_ms_at(3), 1045.0);
        assert_eq!(t.last_change_ms(), 1045.0);

        // A later rebuild stamps only genuinely changed cells, one slot
        // after the last handed-out timestamp.
        let row2 = vec![
            cell(65, 0, 0),
            cell(90, 0, 0),
            cell(67, 0, 0),
            cell(32, 0, 0),
        ];
        t.update_row(0, 2, &row2, 2000.0, &mut sched);
        assert_eq!(t.change_ms_at(1), 1060.0);
        assert_eq!(t.change_ms_at(0), 1000.0); // unchanged cells keep stamps
        assert_eq!(t.last_change_ms(), 1060.0);
    }

    #[test]
    fn resize_preserves_prefix_and_stamps_new_cells() {
        let mut t = ChangeTracker::new(2, 1, 16);
        let mut sched = TypewriterSchedule::new(0.0);
        t.update_row(0, 1, &[cell(65, 0, 0), cell(66, 0, 0)], 1000.0, &mut sched);
        assert_eq!(unpack_time_bits(t.bits_at(0)), 1000.0);

        t.resize(3, 2, 1500.0);
        assert_eq!(t.tracked_cells(), 6);
        assert_eq!(t.change_ms_at(0), 1000.0); // preserved
        assert_eq!(t.change_ms_at(2), 1500.0); // new cell stamped
        assert_eq!(t.change_ms_at(4), 1500.0);
        // New rows have no stored generation: a rebuild diffes and stamps.
        t.update_row(
            1,
            1,
            &[cell(88, 0, 0), cell(32, 0, 0), cell(32, 0, 0)],
            1600.0,
            &mut sched,
        );
        assert_eq!(t.change_ms_at(3), 1600.0);
    }

    #[test]
    fn max_cells_bounds_tracking() {
        let mut t = ChangeTracker::new(4, 2, 4);
        assert_eq!(t.tracked_cells(), 4);
        let mut sched = TypewriterSchedule::new(0.0);
        // Row 1 is beyond the row-major prefix of 4 cells: its rebuild
        // never stamps anything.
        t.update_row(
            1,
            1,
            &[
                cell(88, 0, 0),
                cell(32, 0, 0),
                cell(32, 0, 0),
                cell(32, 0, 0),
            ],
            1000.0,
            &mut sched,
        );
        assert_eq!(t.last_change_ms(), NEVER_MS);
        // Row 0 is within the prefix and stamps.
        t.update_row(
            0,
            1,
            &[
                cell(65, 0, 0),
                cell(66, 0, 0),
                cell(67, 0, 0),
                cell(68, 0, 0),
            ],
            1000.0,
            &mut sched,
        );
        assert_eq!(t.change_ms_at(0), 1000.0);
        assert_eq!(t.change_ms_at(3), 1000.0);
    }

    #[test]
    fn texture_packing_matches_shader_byte_order() {
        let mut t = ChangeTracker::new(1, 1, 16);
        let mut sched = TypewriterSchedule::new(0.0);
        t.update_row(0, 1, &[cell(65, 0, 0)], 1000.0, &mut sched);
        // 1000 ms -> 1.0 s -> 0x3F800000, little-endian bytes.
        assert_eq!(t.change_texture(), &[0x00, 0x00, 0x80, 0x3F]);
        // Never-changed texel: -1000.0 s -> 0xC47A0000.
        let t2 = ChangeTracker::new(1, 1, 16);
        assert_eq!(t2.change_texture(), &[0x00, 0x00, 0x7A, 0xC4]);
        assert_eq!(t2.bits_at(0), NEVER_BITS);
    }

    #[test]
    fn retained_capacity_is_exact() {
        let t = ChangeTracker::new(4, 2, 16);
        // 8 snapshot keys * 8B + 8 packed times * 4B + 8 exact ms * 8B
        // + 32 texel bytes + 2 row generations * 8B.
        assert_eq!(t.retained_capacity(), 8 * 8 + 8 * 4 + 8 * 8 + 32 + 2 * 8);
    }
}
