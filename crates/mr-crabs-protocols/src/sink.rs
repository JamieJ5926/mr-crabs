//! The protocol sink: where parsed protocol events are delivered.
//!
//! The terminal engine implements the S6-owned *state* effects (title, pwd,
//! hyperlinks, semantic prompts) itself; the sink receives notifications for
//! the app layer and supplies the runtime values reports need (device
//! attributes, sizes, palette lookups, the XTVERSION string). Every method
//! has a safe default, so an embedder can opt in to only what it needs.

use crate::apc::Command as ApcCommand;
use crate::color::{ColorTarget, KittyColorKey, KittyColorRequest, Rgb};
use crate::osc::ProgressState;
use crate::reports::{DeviceAttributes, Size};
use crate::semantic_prompt::SemanticPrompt;
use crate::tmux::Notification;

/// An event the sink must forward to the clipboard controller (S5 owns the
/// permission checks; the terminal only parses and forwards).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClipboardEvent {
    pub kind: u8,
    pub data: Vec<u8>,
}

/// A color-related side effect the sink must apply (theme/palette), when the
/// terminal engine cannot apply it itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorEvent {
    /// A dynamic color (foreground/background/cursor) was set.
    DynamicSet { target: ColorTarget, color: Rgb },
    /// A dynamic color was reset.
    DynamicReset { target: ColorTarget },
    /// The special color table was reset.
    SpecialReset,
}

/// Receives parsed protocol events and supplies report runtime values.
pub trait ProtocolSink: Send {
    /// Write bytes back to the PTY (reports, DECRQSS replies, XTGETTCAP).
    fn write_pty(&mut self, _bytes: &[u8]) {}
    /// The window title changed (after Ghostty's 1024-byte truncation).
    fn title_changed(&mut self, _title: &str) {}
    /// The working directory changed (OSC 7 / ConEmu 9;9).
    fn pwd_changed(&mut self, _url: &str) {}
    /// BEL received.
    fn bell(&mut self) {}
    /// Desktop notification requested (OSC 9 / 777).
    fn notification(&mut self, _title: &str, _body: &str) {}
    /// OSC 8 hyperlink started/ended.
    fn hyperlink(&mut self, _id: Option<&str>, _uri: &str) {}
    /// OSC 133 semantic prompt command.
    fn semantic_prompt(&mut self, _cmd: &SemanticPrompt) {}
    /// OSC 52 clipboard request (base64 payload still encoded).
    fn clipboard(&mut self, _event: &ClipboardEvent) {}
    /// OSC 21 kitty color requests.
    fn kitty_color(&mut self, _requests: &[KittyColorRequest]) {}
    /// tmux control-mode notification.
    fn tmux(&mut self, _notification: &Notification) {}
    /// APC command (kitty graphics payloads are handed to the graphics
    /// slice through this hook).
    fn apc(&mut self, _command: &ApcCommand) {}
    /// ConEmu progress report (OSC 9;4).
    fn progress(&mut self, _state: ProgressState, _progress: Option<u8>) {}
    /// Mouse shape request (OSC 22).
    fn mouse_shape(&mut self, _shape: &str) {}
    /// ENQ response; empty suppresses the reply.
    fn enquiry(&mut self) -> String {
        String::new()
    }
    /// Text area size in cells/pixels for CSI 14/16/18 t; `None` suppresses
    /// the reply.
    fn text_area_size(&mut self) -> Option<Size> {
        None
    }
    /// Device attributes reported for CSI c / > c / = c.
    fn device_attributes(&mut self) -> DeviceAttributes {
        DeviceAttributes::default()
    }
    /// Palette color for an OSC color query; `None` suppresses the reply.
    fn color_for(&mut self, _target: ColorTarget) -> Option<Rgb> {
        None
    }
    /// Kitty palette color for an OSC 21 query.
    fn kitty_color_for(&mut self, _key: KittyColorKey) -> Option<Rgb> {
        None
    }
    /// XTVERSION string; empty reports `libghostty`.
    fn xtversion(&mut self) -> String {
        String::new()
    }
    /// The terminfo name reported for XTGETTCAP `TN`.
    fn terminfo_name(&mut self) -> String {
        String::new()
    }
}

/// A sink that drops everything and provides no runtime values.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopSink;

impl ProtocolSink for NoopSink {}

/// A recording sink for tests and embedders that want the event log.
#[derive(Clone, Debug, Default)]
pub struct RecordingSink {
    pub pty_writes: Vec<Vec<u8>>,
    pub titles: Vec<String>,
    pub pwds: Vec<String>,
    pub bells: usize,
    pub notifications: Vec<(String, String)>,
    pub hyperlinks: Vec<(Option<String>, String)>,
    pub semantic_prompts: Vec<SemanticPrompt>,
    pub clipboards: Vec<ClipboardEvent>,
    pub kitty_colors: Vec<Vec<KittyColorRequest>>,
    pub tmux: Vec<Notification>,
    pub apcs: Vec<ApcCommand>,
    pub progress: Vec<(ProgressState, Option<u8>)>,
    pub mouse_shapes: Vec<String>,
}

impl RecordingSink {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ProtocolSink for RecordingSink {
    fn write_pty(&mut self, bytes: &[u8]) {
        self.pty_writes.push(bytes.to_vec());
    }
    fn title_changed(&mut self, title: &str) {
        self.titles.push(title.to_owned());
    }
    fn pwd_changed(&mut self, url: &str) {
        self.pwds.push(url.to_owned());
    }
    fn bell(&mut self) {
        self.bells += 1;
    }
    fn notification(&mut self, title: &str, body: &str) {
        self.notifications.push((title.to_owned(), body.to_owned()));
    }
    fn hyperlink(&mut self, id: Option<&str>, uri: &str) {
        self.hyperlinks
            .push((id.map(str::to_owned), uri.to_owned()));
    }
    fn semantic_prompt(&mut self, cmd: &SemanticPrompt) {
        self.semantic_prompts.push(cmd.clone());
    }
    fn clipboard(&mut self, event: &ClipboardEvent) {
        self.clipboards.push(event.clone());
    }
    fn kitty_color(&mut self, requests: &[KittyColorRequest]) {
        self.kitty_colors.push(requests.to_vec());
    }
    fn tmux(&mut self, notification: &Notification) {
        self.tmux.push(notification.clone());
    }
    fn apc(&mut self, command: &ApcCommand) {
        self.apcs.push(command.clone());
    }
    fn progress(&mut self, state: ProgressState, progress: Option<u8>) {
        self.progress.push((state, progress));
    }
    fn mouse_shape(&mut self, shape: &str) {
        self.mouse_shapes.push(shape.to_owned());
    }
}
