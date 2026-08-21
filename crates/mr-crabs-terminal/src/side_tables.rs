use std::collections::HashMap;

use crate::{Style, TerminalError};

/// Interned style table: dedup via Vec<Style> + HashMap. ID 0 is always Style::default().
/// IDs are limited to u16 for the 8-byte Cell invariant.
#[derive(Clone, Debug)]
pub struct StyleTable {
    styles: Vec<Style>,
    index: HashMap<Style, u16>,
    epoch: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StyleRemap {
    pub(crate) old_to_new: Vec<u16>,
    pub(crate) new_styles: Vec<Style>,
    pub(crate) old_len: usize,
}

impl StyleRemap {
    pub(crate) fn new(
        old_styles: &[Style],
        live_ids: &std::collections::BTreeSet<u16>,
    ) -> Result<Self, TerminalError> {
        if old_styles.is_empty()
            || old_styles[0] != Style::default()
            || live_ids.first() != Some(&0)
        {
            return Err(TerminalError::StyleCompactionCorrupt);
        }

        let mut old_to_new = vec![u16::MAX; old_styles.len()];
        let mut new_styles = Vec::with_capacity(live_ids.len());
        for &old_id in live_ids {
            let old_index = usize::from(old_id);
            let Some(style) = old_styles.get(old_index) else {
                return Err(TerminalError::StyleCompactionCorrupt);
            };
            let new_id = u16::try_from(new_styles.len())
                .map_err(|_| TerminalError::StyleCompactionCapacity)?;
            old_to_new[old_index] = new_id;
            new_styles.push(style.clone());
        }

        if new_styles.first() != Some(&Style::default()) {
            return Err(TerminalError::StyleCompactionCorrupt);
        }
        Ok(Self {
            old_to_new,
            new_styles,
            old_len: old_styles.len(),
        })
    }

    pub(crate) fn validate_for(&self, old_styles: &[Style]) -> Result<(), TerminalError> {
        if self.old_len != old_styles.len()
            || self.old_to_new.len() != self.old_len
            || self.new_styles.is_empty()
            || self.new_styles.len() > usize::from(u16::MAX) + 1
            || self.new_styles[0] != Style::default()
            || self.old_to_new.first() != Some(&0)
        {
            return Err(TerminalError::StyleCompactionCorrupt);
        }

        // Dense monotonic check: live entries in old order must map to 0..new_len-1 sequentially.
        let mut expected: u16 = 0;
        let mut seen = vec![false; self.new_styles.len()];
        for (old_id, &mapped) in self.old_to_new.iter().enumerate() {
            if mapped == u16::MAX {
                continue;
            }
            if usize::from(mapped) >= self.new_styles.len() {
                return Err(TerminalError::StyleCompactionCorrupt);
            }
            if mapped != expected {
                return Err(TerminalError::StyleCompactionCorrupt);
            }
            if seen[usize::from(mapped)] {
                return Err(TerminalError::StyleCompactionCorrupt);
            }
            seen[usize::from(mapped)] = true;
            if old_styles.get(old_id) != Some(&self.new_styles[usize::from(mapped)]) {
                return Err(TerminalError::StyleCompactionCorrupt);
            }
            expected = expected.wrapping_add(1);
        }
        if usize::from(expected) != self.new_styles.len() {
            return Err(TerminalError::StyleCompactionCorrupt);
        }
        if seen.iter().any(|&v| !v) {
            return Err(TerminalError::StyleCompactionCorrupt);
        }
        Ok(())
    }

    pub(crate) fn map(&self, old_id: u16) -> Result<u16, TerminalError> {
        self.old_to_new
            .get(usize::from(old_id))
            .copied()
            .filter(|&mapped| mapped != u16::MAX)
            .ok_or(TerminalError::StyleCompactionCorrupt)
    }
}

impl Default for StyleTable {
    fn default() -> Self {
        Self::new()
    }
}

impl StyleTable {
    pub fn new() -> Self {
        let default = Style::default();
        let mut index = HashMap::new();
        index.insert(default.clone(), 0);
        Self {
            styles: vec![default],
            index,
            epoch: 0,
        }
    }

    pub fn from_exact(styles: &[Style]) -> Result<Self, TerminalError> {
        if styles.is_empty() {
            return Err(TerminalError::RestoreStyleTable);
        }
        if styles.len() > usize::from(u16::MAX) + 1 {
            return Err(TerminalError::RestoreStyleTable);
        }
        if styles[0] != Style::default() {
            return Err(TerminalError::RestoreStyleTable);
        }
        let styled = styles.to_vec();
        let mut index: HashMap<Style, u16> = HashMap::new();
        for (idx, style) in styled.iter().enumerate() {
            let id = u16::try_from(idx).expect("style index fits u16");
            index.entry(style.clone()).or_insert(id);
        }
        Ok(Self {
            styles: styled,
            index,
            epoch: 0,
        })
    }

    /// Intern a style, returning its stable u16 ID.
    pub fn intern(&mut self, style: Style) -> u16 {
        if let Some(&id) = self.index.get(&style) {
            return id;
        }
        let id = u16::try_from(self.styles.len()).expect("style table overflow u16");
        self.index.insert(style.clone(), id);
        self.styles.push(style);
        id
    }

    pub fn get(&self, id: u16) -> Option<&Style> {
        self.styles.get(usize::from(id))
    }

    pub fn as_slice(&self) -> &[Style] {
        &self.styles
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    pub(crate) fn stage_remapped(&self, remap: &StyleRemap) -> Result<Self, TerminalError> {
        remap.validate_for(&self.styles)?;
        let mut staged = Self::from_exact(&remap.new_styles)?;
        staged.epoch = self.epoch.wrapping_add(1);
        Ok(staged)
    }

    pub fn len(&self) -> usize {
        self.styles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.styles.is_empty()
    }
}

/// Grapheme/combining cluster table: rare clusters live here, not per-cell heap.
/// Stable u32 IDs are allocated for each distinct cluster; Cell flags indicate presence.
#[derive(Clone, Debug, Default)]
pub struct GraphemeTable {
    entries: Vec<Vec<u32>>,
    index: HashMap<Vec<u32>, u32>,
}

impl GraphemeTable {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            index: HashMap::new(),
        }
    }

    pub fn intern(&mut self, cluster: Vec<u32>) -> u32 {
        if cluster.is_empty() {
            return u32::MAX;
        }
        if let Some(&id) = self.index.get(&cluster) {
            return id;
        }
        let id = u32::try_from(self.entries.len()).expect("grapheme table overflow");
        self.index.insert(cluster.clone(), id);
        self.entries.push(cluster);
        id
    }

    pub fn get(&self, id: u32) -> Option<&[u32]> {
        self.entries
            .get(usize::try_from(id).ok()?)
            .map(|v| v.as_slice())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// OSC 8 hyperlink identity interned to stable u32.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HyperlinkIdentity {
    pub id: String,
    pub uri: String,
}

#[derive(Clone, Debug, Default)]
pub struct HyperlinkTable {
    entries: Vec<HyperlinkIdentity>,
    index: HashMap<HyperlinkIdentity, u32>,
}

impl HyperlinkTable {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            index: HashMap::new(),
        }
    }

    pub fn intern(&mut self, id: impl Into<String>, uri: impl Into<String>) -> u32 {
        let key = HyperlinkIdentity {
            id: id.into(),
            uri: uri.into(),
        };
        if let Some(&existing) = self.index.get(&key) {
            return existing;
        }
        let new_id = u32::try_from(self.entries.len()).expect("hyperlink table overflow");
        self.index.insert(key.clone(), new_id);
        self.entries.push(key);
        new_id
    }

    pub fn get(&self, id: u32) -> Option<&HyperlinkIdentity> {
        self.entries.get(usize::try_from(id).ok()?)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Semantic prompt/output region with stable logical offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SemanticKind {
    Prompt,
    Output,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LogicalOffset(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticRegion {
    pub start: LogicalOffset,
    pub end: LogicalOffset,
    pub kind: SemanticKind,
}

#[derive(Clone, Debug, Default)]
pub struct SemanticTable {
    regions: Vec<SemanticRegion>,
}

impl SemanticTable {
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
        }
    }

    pub fn push(&mut self, region: SemanticRegion) -> usize {
        let idx = self.regions.len();
        self.regions.push(region);
        idx
    }

    pub fn get(&self, idx: usize) -> Option<&SemanticRegion> {
        self.regions.get(idx)
    }

    pub fn as_slice(&self) -> &[SemanticRegion] {
        &self.regions
    }

    pub fn len(&self) -> usize {
        self.regions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// Remap logical offsets across reflow/scroll; keeps selection anchors stable.
    pub fn remap<F>(&mut self, mut f: F)
    where
        F: FnMut(LogicalOffset) -> LogicalOffset,
    {
        for r in &mut self.regions {
            r.start = f(r.start);
            r.end = f(r.end);
        }
    }
}

/// Stable selection anchor stored as logical offsets, remapped on reflow/scroll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionAnchor {
    pub offset: LogicalOffset,
    pub col: u16,
}

impl SelectionAnchor {
    pub fn new(offset: u64, col: u16) -> Self {
        Self {
            offset: LogicalOffset(offset),
            col,
        }
    }

    pub fn remap<F>(&mut self, f: F)
    where
        F: FnOnce(LogicalOffset) -> LogicalOffset,
    {
        self.offset = f(self.offset);
    }
}

#[cfg(test)]
mod style_table_exact_tests {
    use super::*;
    use crate::{NamedColorValue, NormalizedColor};

    fn red_style() -> Style {
        Style {
            foreground: NormalizedColor::Named(NamedColorValue::Red),
            background: NormalizedColor::Named(NamedColorValue::Background),
            underline: None,
        }
    }

    #[test]
    fn from_exact_retains_duplicates_and_first_id_wins() {
        let def = Style::default();
        let red = red_style();
        let input = vec![def.clone(), red.clone(), red.clone()];
        let mut table = StyleTable::from_exact(&input).expect("from_exact should succeed");
        assert_eq!(table.len(), 3);
        assert_eq!(table.get(0), Some(&def));
        assert_eq!(table.get(1), Some(&red));
        assert_eq!(table.get(2), Some(&red));
        // intern of duplicate value resolves to first ID (1)
        assert_eq!(table.intern(red.clone()), 1);
        // ensure underlying vec retains order/duplicates
        assert_eq!(table.as_slice(), input.as_slice());
    }

    #[test]
    fn from_exact_accepts_65536_defaults() {
        let def = Style::default();
        let input = vec![def; 65536];
        let table = StyleTable::from_exact(&input).expect("65536 should be accepted");
        assert_eq!(table.len(), 65536);
        // converting index 65,535 succeeds - get max u16
        assert!(table.get(u16::MAX).is_some());
    }

    #[test]
    fn from_exact_rejects_65537() {
        let def = Style::default();
        let input = vec![def; 65537];
        let err = StyleTable::from_exact(&input).unwrap_err();
        assert_eq!(err, TerminalError::RestoreStyleTable);
    }

    #[test]
    fn from_exact_rejects_nondefault_index0() {
        let red = red_style();
        let input = vec![red];
        let err = StyleTable::from_exact(&input).unwrap_err();
        assert_eq!(err, TerminalError::RestoreStyleTable);
        // also empty rejected
        let empty: Vec<Style> = vec![];
        assert_eq!(
            StyleTable::from_exact(&empty).unwrap_err(),
            TerminalError::RestoreStyleTable
        );
    }
}
