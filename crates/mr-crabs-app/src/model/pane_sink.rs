//! Pane-owned protocol sink.
//!
//! Each [`PaneModel`](super::pane::PaneModel) installs exactly one shared
//! [`PaneProtocolSink`] before the first feed. Replies, title/pwd/semantic
//! events, and clipboard requests stay on owner-local bounded queues — there
//! is no global active-pane router. Report values come from the last
//! committed [`SurfaceGeometry`] plus the Mr Crabs / xterm-ghostty identity.

use std::collections::VecDeque;
use std::sync::Arc;

use parking_lot::Mutex;

use mr_crabs_protocols::reports::{DeviceAttributes, Size};
use mr_crabs_protocols::semantic_prompt::SemanticPrompt;
use mr_crabs_protocols::sink::{ClipboardEvent, ProtocolSink};

use super::geometry::SurfaceGeometry;

/// Bound applied independently to each owner-local queue.
const QUEUE_CAP: usize = 64;

/// XTVERSION payload reported for `CSI > q`.
pub const XTVERSION: &str = "Mr Crabs";

/// XTGETTCAP `TN` payload.
pub const TERMINFO_NAME: &str = "xterm-ghostty";

/// Title, working-directory, and semantic-prompt notifications for this pane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PaneSinkEvent {
    Title(String),
    Pwd(String),
    Semantic(SemanticPrompt),
}

#[derive(Debug, Default)]
struct PaneSinkState {
    replies: VecDeque<Vec<u8>>,
    events: VecDeque<PaneSinkEvent>,
    clipboards: VecDeque<ClipboardEvent>,
    text_area: Option<Size>,
    terminfo_name: String,
}

/// Shared, pane-owned [`ProtocolSink`]. Cloning shares the same queues.
#[derive(Clone, Debug)]
pub struct PaneProtocolSink {
    state: Arc<Mutex<PaneSinkState>>,
}

impl PaneProtocolSink {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(PaneSinkState {
                terminfo_name: TERMINFO_NAME.to_owned(),
                ..PaneSinkState::default()
            })),
        }
    }

    /// Publish the committed surface so CSI 14/16/18 t reports use it.
    pub fn set_geometry(&self, geometry: SurfaceGeometry) {
        self.state.lock().text_area = Some(Size {
            rows: geometry.grid.rows,
            columns: geometry.grid.cols,
            cell_width: u32::from(geometry.cell_px.0),
            cell_height: u32::from(geometry.cell_px.1),
        });
    }

    pub fn set_terminfo_name(&self, name: impl Into<String>) {
        self.state.lock().terminfo_name = name.into();
    }

    pub fn text_area_report(&self) -> Option<Size> {
        self.state.lock().text_area
    }

    pub fn drain_pty_replies(&self) -> Vec<Vec<u8>> {
        self.state.lock().replies.drain(..).collect()
    }

    pub fn requeue_pty_replies(&self, replies: Vec<Vec<u8>>) {
        let mut state = self.state.lock();
        for reply in replies.into_iter().rev() {
            if state.replies.len() < QUEUE_CAP {
                state.replies.push_front(reply);
            }
        }
    }

    pub fn drain_events(&self) -> Vec<PaneSinkEvent> {
        self.state.lock().events.drain(..).collect()
    }

    pub fn drain_clipboard(&self) -> Vec<ClipboardEvent> {
        self.state.lock().clipboards.drain(..).collect()
    }
}

impl Default for PaneProtocolSink {
    fn default() -> Self {
        Self::new()
    }
}

fn push_bounded<T>(queue: &mut VecDeque<T>, item: T) {
    if queue.len() >= QUEUE_CAP {
        queue.pop_front();
    }
    queue.push_back(item);
}

impl ProtocolSink for PaneProtocolSink {
    fn write_pty(&mut self, bytes: &[u8]) {
        push_bounded(&mut self.state.lock().replies, bytes.to_vec());
    }

    fn title_changed(&mut self, title: &str) {
        push_bounded(
            &mut self.state.lock().events,
            PaneSinkEvent::Title(title.to_owned()),
        );
    }

    fn pwd_changed(&mut self, url: &str) {
        push_bounded(
            &mut self.state.lock().events,
            PaneSinkEvent::Pwd(url.to_owned()),
        );
    }

    fn semantic_prompt(&mut self, cmd: &SemanticPrompt) {
        push_bounded(
            &mut self.state.lock().events,
            PaneSinkEvent::Semantic(cmd.clone()),
        );
    }

    fn clipboard(&mut self, event: &ClipboardEvent) {
        push_bounded(&mut self.state.lock().clipboards, event.clone());
    }

    fn text_area_size(&mut self) -> Option<Size> {
        self.state.lock().text_area
    }

    fn device_attributes(&mut self) -> DeviceAttributes {
        DeviceAttributes::default()
    }

    fn xtversion(&mut self) -> String {
        XTVERSION.to_owned()
    }

    fn terminfo_name(&mut self) -> String {
        self.state.lock().terminfo_name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;

    use mr_crabs_element::{CellMetrics, PixelExtent};
    use mr_crabs_pty::WriteError;
    use mr_crabs_terminal::GridSize;

    use crate::model::geometry::PaddingPx;
    use crate::model::pane::{PaneModel, PaneSession};
    use crate::model::split::PaneId;

    fn pane_with_writer(
        id: u64,
        size: GridSize,
    ) -> (PaneModel, std::sync::mpsc::Receiver<Vec<u8>>) {
        let (writer_tx, writer_rx) = sync_channel(16);
        let mut pane = PaneModel::detached(PaneId::new(id), size).expect("detached pane");
        pane.session = PaneSession::from_receivers_with_writer(size, None, None, Some(writer_tx));
        (pane, writer_rx)
    }

    fn take_writes(rx: &std::sync::mpsc::Receiver<Vec<u8>>) -> Vec<u8> {
        let mut out = Vec::new();
        while let Ok(chunk) = rx.try_recv() {
            out.extend_from_slice(&chunk);
        }
        out
    }

    fn contains_seq(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    #[test]
    fn protocol_replies_stay_owner_local() {
        let size = GridSize::new(80, 24);
        let (mut left, left_rx) = pane_with_writer(1, size);
        let (mut right, right_rx) = pane_with_writer(2, size);

        left.feed_test_output(b"\x1b[c\x1bP+q544E\x1b\\")
            .expect("pane_sink fixture feed should succeed");
        left.pump(64);
        right
            .feed_test_output(b"\x1b[5n")
            .expect("pane_sink fixture feed should succeed");
        right.pump(64);

        let left_bytes = take_writes(&left_rx);
        let right_bytes = take_writes(&right_rx);
        assert!(
            contains_seq(&left_bytes, b"\x1b[?62;22c"),
            "left DA reply: {left_bytes:?}"
        );
        assert!(
            contains_seq(
                &left_bytes,
                b"\x1bP1+r544E=787465726D2D67686F73747479\x1b\\"
            ),
            "left XTGETTCAP TN: {left_bytes:?}"
        );
        assert!(
            !contains_seq(&left_bytes, b"\x1b[0n"),
            "right DSR must not land on left"
        );
        assert_eq!(right_bytes, b"\x1b[0n");
        assert!(
            !contains_seq(&right_bytes, b"\x1b[?62;22c"),
            "left DA must not land on right"
        );
        assert!(
            !contains_seq(&right_bytes, b"\x1bP1+r544E="),
            "left XTGETTCAP must not land on right"
        );
        assert!(matches!(
            left.session.write(b"ok"),
            Ok(()) | Err(WriteError::Full)
        ));
    }

    #[test]
    fn geometry_report_uses_committed_surface() {
        let (mut pane, rx) = pane_with_writer(1, GridSize::new(80, 24));
        let geometry = SurfaceGeometry::from_viewport(
            PixelExtent {
                width: 1000.0,
                height: 600.0,
            },
            CellMetrics::new(10.0, 20.0).expect("metrics"),
            PaddingPx::default(),
        )
        .expect("geometry");
        assert_eq!(geometry.grid, GridSize::new(100, 30));
        assert_eq!(geometry.cell_px, (10, 20));
        assert!(pane.commit_geometry(geometry, None).expect("commit"));
        assert_eq!(
            pane.protocol_sink().text_area_report(),
            Some(Size {
                rows: 30,
                columns: 100,
                cell_width: 10,
                cell_height: 20,
            })
        );

        pane.feed_test_output(b"\x1b[18t\x1b[")
            .expect("pane_sink fixture feed should succeed");
        pane.feed_test_output(b"16")
            .expect("pane_sink fixture feed should succeed");
        pane.feed_test_output(b"t\x1b[14t")
            .expect("pane_sink fixture feed should succeed");
        pane.pump(64);
        let bytes = take_writes(&rx);
        assert!(
            contains_seq(&bytes, b"\x1b[8;30;100t"),
            "CSI 18 t: {bytes:?}"
        );
        assert!(
            contains_seq(&bytes, b"\x1b[6;20;10t"),
            "CSI 16 t: {bytes:?}"
        );
        assert!(
            contains_seq(&bytes, b"\x1b[4;600;1000t"),
            "CSI 14 t: {bytes:?}"
        );
    }
}
