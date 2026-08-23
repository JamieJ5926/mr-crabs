//! Model module: windows, tabs, recursive splits, panes, and the app model.

pub mod app_model;
pub mod geometry;
pub mod input_dock;
pub mod launch_bytes;
pub mod pane;
pub mod pane_sink;
pub mod presentation;
pub mod shell_integration;
pub mod split;
pub mod tab;
pub mod window;

pub use geometry::{PaddingPx, SurfaceGeometry};
pub use pane_sink::{PaneProtocolSink, PaneSinkEvent};
