//! Model module: windows, tabs, recursive splits, panes, and the app model.

pub mod app_model;
pub mod fetch_animation;
pub mod geometry;
pub mod pane;
pub mod pane_sink;
pub mod split;
pub mod tab;
pub mod window;

pub use geometry::{PaddingPx, SurfaceGeometry};
pub use pane_sink::{PaneProtocolSink, PaneSinkEvent};
