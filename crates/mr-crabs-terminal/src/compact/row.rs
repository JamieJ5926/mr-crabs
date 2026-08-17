//! Shared compact row/page descriptors for the live grid and scrollback.
//!
//! Full-screen scroll moves these descriptors (an `Arc` bump) into history.
//! Cells are not scanned or copied solely to enter history.

use std::collections::HashMap;
use std::sync::Arc;

use crate::Cell;

/// Rare per-column payload that travels with a row descriptor.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RowExtras {
    /// Column → interned grapheme-cluster id (`GraphemeTable`).
    pub combining: HashMap<u16, u32>,
    /// Column → interned hyperlink id (`HyperlinkTable`).
    pub hyperlinks: HashMap<u16, u32>,
}

impl RowExtras {
    pub fn is_empty(&self) -> bool {
        self.combining.is_empty() && self.hyperlinks.is_empty()
    }
}

/// One terminal row. Live screen and history share this type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactRow {
    pub cells: Arc<[Cell]>,
    pub cols: u16,
    pub occupancy: u16,
    pub first_occupied: u16,
    pub wrapped: bool,
    pub generation: u64,
    pub extras: Option<Arc<RowExtras>>,
}

impl CompactRow {
    /// Build a row from an owned cell allocation. Occupancy is computed once
    /// at construction; later mutations keep it incrementally.
    pub fn new(cells: impl Into<Arc<[Cell]>>, wrapped: bool) -> Self {
        let cells = cells.into();
        let cols = u16::try_from(cells.len()).unwrap_or(u16::MAX);
        let occupancy = occupancy_of(&cells);
        let first_occupied = cells
            .iter()
            .position(|cell| !cell.is_default())
            .map_or(cols, |index| index as u16);
        Self {
            cells,
            cols,
            occupancy,
            first_occupied,
            wrapped,
            generation: 1,
            extras: None,
        }
    }

    pub fn blank(cols: u16) -> Self {
        Self {
            cells: blank_cells(cols),
            cols,
            occupancy: 0,
            first_occupied: cols,
            wrapped: false,
            generation: 1,
            extras: None,
        }
    }

    pub fn with_generation(mut self, generation: u64) -> Self {
        self.generation = generation;
        self
    }

    #[cfg(test)]
    #[inline]
    pub fn is_visually_empty(&self) -> bool {
        self.occupancy == 0 && !self.wrapped
    }

    pub(crate) fn cells_mut(&mut self) -> &mut [Cell] {
        let first = usize::from(self.first_occupied);
        let occupancy = usize::from(self.occupancy);
        let len = self.cells.len();
        let cells = Arc::make_mut(&mut self.cells);
        cells[..first.min(len)].fill(Cell::default());
        if occupancy < len {
            cells[occupancy..].fill(Cell::default());
        }
        cells
    }

    /// Mutate an engine-owned active row without an atomic Arc uniqueness check.
    ///
    /// # Safety
    /// The caller must guarantee that `self.cells` has no other strong owner.
    pub(crate) unsafe fn cells_for_unique_write_range(
        &mut self,
        start: usize,
        end: usize,
    ) -> &mut [Cell] {
        debug_assert_eq!(Arc::strong_count(&self.cells), 1);
        let len = self.cells.len();
        let start = start.min(len);
        let end = end.min(len).max(start);
        let old_first = usize::from(self.first_occupied);
        let old_end = usize::from(self.occupancy);
        if start < end {
            self.first_occupied = if old_end == 0 {
                start as u16
            } else {
                self.first_occupied.min(start as u16)
            };
            self.occupancy = self.occupancy.max(end as u16);
            self.generation = self.generation.wrapping_add(1);
        }
        let cells = unsafe { std::slice::from_raw_parts_mut(self.cells.as_ptr().cast_mut(), len) };
        if old_end != 0 {
            if end < old_first {
                cells[end..old_first].fill(Cell::default());
            } else if start > old_end {
                cells[old_end..start].fill(Cell::default());
            }
        }
        cells
    }

    pub(crate) fn extras_mut(&mut self) -> &mut RowExtras {
        let extras = self
            .extras
            .get_or_insert_with(|| Arc::new(RowExtras::default()));
        Arc::make_mut(extras)
    }

    pub(crate) fn clear_extras_at(&mut self, col: u16) {
        if let Some(extras) = self.extras.as_mut() {
            let extras = Arc::make_mut(extras);
            extras.combining.remove(&col);
            extras.hyperlinks.remove(&col);
            if extras.is_empty() {
                self.extras = None;
            }
        }
    }

    pub(crate) fn bump(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    pub(crate) fn recompute_occupancy(&mut self) {
        self.first_occupied = self
            .cells
            .iter()
            .position(|cell| !cell.is_default())
            .map_or(self.cols, |index| index as u16);
        self.occupancy = occupancy_of(&self.cells);
        if self.wrapped && self.occupancy < self.cols {
            self.occupancy = self.cols;
            self.first_occupied = self.first_occupied.min(self.cols.saturating_sub(1));
        }
    }

    pub(crate) fn reset_blank(&mut self, cols: u16) {
        self.cells = blank_cells(cols);
        self.cols = cols;
        self.occupancy = 0;
        self.first_occupied = cols;
        self.wrapped = false;
        self.extras = None;
        self.bump();
    }

    pub(crate) fn fill_erased(&mut self, start: usize, end: usize, erased: Cell) {
        if start >= end {
            return;
        }
        let end = end.min(self.cells.len());
        let start = start.min(end);
        {
            let cells = self.cells_mut();
            for cell in &mut cells[start..end] {
                *cell = erased;
            }
        }
        for col in start..end {
            self.clear_extras_at(col as u16);
        }
        if erased.is_default() {
            if usize::from(self.occupancy) > start {
                self.recompute_occupancy();
            }
        } else {
            self.occupancy = self.occupancy.max(end as u16);
            self.first_occupied = self.first_occupied.min(start as u16);
        }
        self.wrapped = false;
        self.bump();
    }

    pub(crate) fn set_wrapped(&mut self, wrapped: bool) {
        self.wrapped = wrapped;
        if let Some(last) = self.cells_mut().last_mut() {
            if wrapped {
                last.flags |= crate::compact::flags::WRAPLINE;
            } else {
                last.flags &= !crate::compact::flags::WRAPLINE;
            }
        }
        if wrapped {
            self.occupancy = self.cols;
            self.first_occupied = self.first_occupied.min(self.cols.saturating_sub(1));
        }
        self.bump();
    }
}

#[cfg(test)]
/// A frozen page of row descriptors. Compression later shares this `Arc`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactPage {
    pub rows: Arc<[CompactRow]>,
    pub cols: u16,
    pub generation: u64,
}

#[cfg(test)]
impl CompactPage {
    pub fn new(rows: impl Into<Arc<[CompactRow]>>, cols: u16, generation: u64) -> Self {
        Self {
            rows: rows.into(),
            cols,
            generation,
        }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

#[cfg(test)]
pub(crate) const PAGE_LINES: usize = 128;

fn blank_cells(cols: u16) -> Arc<[Cell]> {
    let n = usize::from(cols);
    let mut cells = Vec::with_capacity(n);
    cells.resize(n, Cell::default());
    cells.into()
}

fn occupancy_of(cells: &[Cell]) -> u16 {
    let mut occ = 0u16;
    for (index, cell) in cells.iter().enumerate() {
        if !cell.is_default() {
            occ = (index + 1) as u16;
        }
    }
    occ
}
