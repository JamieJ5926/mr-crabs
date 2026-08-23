//! GPUI bridge: shell actions as GPUI actions, menu/keybinding conversion,
//! and the window view that renders `TerminalElement`s.

pub mod actions;
pub mod input_dock;
pub mod input_surface;
pub mod menus;
pub mod shell;
pub mod wake;
pub mod workspace;

pub use shell::AppShell;
pub use wake::{drain_scheduled, install_wake, new_output_wake, spawn_wake_task};
pub use workspace::WindowView;
