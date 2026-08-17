use std::collections::HashMap;

use crate::Style;

/// Interned style table: dedup via Vec<Style> + HashMap. ID 0 is always Style::default().
/// IDs are limited to u16 for the 8-byte Cell invariant.
#[derive(Clone, Debug)]
pub struct StyleTable {
    styles: Vec<Style>,
    index: HashMap<Style, u16>,
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
        }
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
