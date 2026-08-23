//! Compact live-grid and scrollback: one `CompactRow` / `CompactPage`
//! authority shared by the visible screen and history.

pub mod flags;
pub mod row;
pub mod state;
pub mod width;

mod engine;

pub use engine::CompactEngine;
#[cfg(test)]
pub(crate) use engine::EngineCensusDiag;
pub(crate) use engine::EngineStyleRemap;
#[cfg(test)]
pub use row::{CompactPage, CompactRow, RowExtras};

#[cfg(test)]
mod tests;
