//! Bounded frame pool: recycles [`FrameDelta`] allocations across builds so
//! steady-state rendering performs no allocation growth.

use crate::delta::FrameDelta;
use crate::{
    CursorState, DamageKind, GridSize, ImageDeltaPlaceholder, SelectionState, TerminalViewport,
};

/// Default pool bound, matching the contract's example capacity.
pub const DEFAULT_FRAME_POOL_CAPACITY: usize = 4;

/// A bounded pool of reusable [`FrameDelta`]s.
///
/// [`acquire`](FramePool::acquire) pops the most recently released frame with
/// all allocations intact; [`release`](FramePool::release) clears it for reuse
/// without shrinking. The pool never holds more frames than the bound given
/// to [`FramePool::new`]; frames released beyond the bound are dropped.
pub struct FramePool {
    pool: Vec<FrameDelta>,
    bound: usize,
}

impl FramePool {
    /// Create a pool that holds at most `cap` frames.
    pub fn new(cap: usize) -> Self {
        Self {
            pool: Vec::with_capacity(cap),
            bound: cap,
        }
    }

    /// Take a frame to build into, reusing a pooled frame when one is
    /// available. The frame is re-stamped with fresh identity fields but
    /// retains every recycled allocation.
    pub fn acquire(&mut self, sequence: u64, size: GridSize) -> FrameDelta {
        let mut frame = self.pool.pop().unwrap_or_else(|| FrameDelta {
            sequence,
            style_epoch: 0,
            size,
            damage: DamageKind::Clean,
            rows: Vec::new(),
            cursor: CursorState::default(),
            selection: SelectionState::default(),
            styles: Vec::new(),
            images: ImageDeltaPlaceholder::default(),
            viewport: TerminalViewport::default(),
            search_matches: Vec::new(),
            hyperlinks: Vec::new(),
            animation_region: None,
            spare_rows: Vec::new(),
        });
        // Re-stamp identity regardless of prior reuse.
        frame.sequence = sequence;
        frame.style_epoch = 0;
        frame.size = size;
        frame.damage = DamageKind::Clean;
        frame.cursor = CursorState::default();
        frame.selection = SelectionState::default();
        frame.images = ImageDeltaPlaceholder::default();
        frame.viewport = TerminalViewport::default();
        frame.search_matches.clear();
        frame.hyperlinks.clear();
        frame.animation_region = None;
        frame
    }

    /// Return a frame to the pool, retaining its allocations for a later
    /// [`acquire`](FramePool::acquire). Frames beyond the pool bound are
    /// dropped (their allocations are freed).
    pub fn release(&mut self, frame: FrameDelta) {
        let mut frame = frame;
        frame.clear_for_reuse();
        if self.pool.len() < self.bound {
            self.pool.push(frame);
        }
    }

    /// The pool's capacity bound.
    pub fn capacity(&self) -> usize {
        self.bound
    }

    /// Number of frames currently pooled.
    pub fn len(&self) -> usize {
        self.pool.len()
    }

    /// Whether the pool currently holds no frames.
    pub fn is_empty(&self) -> bool {
        self.pool.is_empty()
    }
}

/// Default [`FramePool`] for the terminal
/// (capacity [`DEFAULT_FRAME_POOL_CAPACITY`]).
pub fn frame_pool_default() -> FramePool {
    FramePool::new(DEFAULT_FRAME_POOL_CAPACITY)
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_FRAME_POOL_CAPACITY, FramePool, frame_pool_default};
    use crate::{Cell, CursorState, DamageKind, GridSize, RowDelta, Run, Style};

    #[test]
    fn default_capacity_is_four() {
        assert_eq!(DEFAULT_FRAME_POOL_CAPACITY, 4);
        assert_eq!(frame_pool_default().capacity(), DEFAULT_FRAME_POOL_CAPACITY);
    }

    #[test]
    fn pool_is_bounded() {
        let mut pool = FramePool::new(4);
        assert_eq!(pool.capacity(), 4);
        for i in 0..8 {
            pool.release(crate::FrameDelta {
                sequence: i,
                style_epoch: 0,
                size: GridSize::new(8, 3),
                damage: DamageKind::Clean,
                rows: Vec::new(),
                cursor: CursorState::default(),
                selection: crate::SelectionState::default(),
                styles: Vec::new(),
                images: crate::ImageDeltaPlaceholder::default(),
                viewport: crate::TerminalViewport::default(),
                search_matches: Vec::new(),
                hyperlinks: Vec::new(),
                animation_region: None,
                spare_rows: Vec::new(),
            });
        }
        assert_eq!(pool.len(), 4, "pool never exceeds its bound");
        // Acquiring all pooled frames works and re-stamps identity.
        let frame = pool.acquire(99, GridSize::new(2, 1));
        assert_eq!(frame.sequence, 99);
        assert_eq!(frame.size, GridSize::new(2, 1));
        assert_eq!(pool.len(), 3);
    }

    #[test]
    fn release_reacquire_keeps_capacities() {
        let mut pool = frame_pool_default();
        let mut frame = pool.acquire(1, GridSize::new(8, 3));
        frame.rows.push(RowDelta {
            row: 0,
            generation: 1,
            cells: vec![Cell::default(); 8],
            runs: vec![Run {
                start_col: 0,
                len: 8,
                style: 0,
            }],
        });
        frame.styles.push(Style::default());
        frame.viewport = crate::TerminalViewport {
            scroll_offset: 3,
            history_rows: 9,
            alternate_screen: true,
        };
        frame.search_matches.push(crate::FrameSearchMatch {
            range: crate::FrameRange {
                start: crate::FramePoint { row: 0, col: 0 },
                end: crate::FramePoint { row: 0, col: 2 },
            },
            current: true,
        });
        frame.hyperlinks.push(crate::FrameHyperlink {
            range: crate::FrameRange {
                start: crate::FramePoint { row: 1, col: 0 },
                end: crate::FramePoint { row: 1, col: 3 },
            },
            id: None,
            uri: "https://example.com".to_owned(),
        });
        let rows_cap = frame.rows.capacity();
        let styles_cap = frame.styles.capacity();
        let cells_cap = frame.rows[0].cells.capacity();
        let runs_cap = frame.rows[0].runs.capacity();
        let search_cap = frame.search_matches.capacity();
        let hyperlink_cap = frame.hyperlinks.capacity();

        pool.release(frame);
        assert_eq!(pool.len(), 1);

        // Re-acquire and rebuild the same shape: no growth beyond retained
        // capacities.
        let mut frame = pool.acquire(2, GridSize::new(8, 3));
        assert!(frame.rows.capacity() >= rows_cap);
        assert!(frame.styles.capacity() >= styles_cap);
        assert!(
            frame.rows.is_empty(),
            "released frame carries no stale rows"
        );
        assert_eq!(
            frame.viewport,
            crate::TerminalViewport::default(),
            "reacquire clears stale viewport metadata"
        );
        assert!(frame.search_matches.is_empty());
        assert!(frame.hyperlinks.is_empty());
        assert!(frame.search_matches.capacity() >= search_cap);
        assert!(frame.hyperlinks.capacity() >= hyperlink_cap);

        let mut row = frame.take_row();
        row.cells.push(Cell::default());
        row.cells.push(Cell::default());
        row.runs.push(Run {
            start_col: 0,
            len: 2,
            style: 0,
        });
        assert!(
            row.cells.capacity() >= cells_cap,
            "cells capacity retained across release/acquire"
        );
        assert!(
            row.runs.capacity() >= runs_cap,
            "runs capacity retained across release/acquire"
        );
        frame.rows.push(row);
        pool.release(frame);
    }
}
