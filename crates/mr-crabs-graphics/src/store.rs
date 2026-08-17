//! Kitty graphics image storage: byte/count budgets, deterministic LRU
//! eviction, generation stamps, placements, and command execution.
//!
//! Faithful port of `src/terminal/kitty/graphics_storage.zig` and
//! `graphics_exec.zig` (Ghostty `d2c70a8c7b9b6893c13640c02d7b6f9a1624f3f0`)
//! adapted to a terminal-agnostic host model:
//!
//! - placements anchor to absolute scrollback rows (`Location::Pin { row,
//!   col }`); scrolling the viewport is a host-side change of
//!   `TerminalContext::viewport_first_row` and never mutates the store;
//! - `prune_history` removes placements whose pin row fell out of retained
//!   history (the oracle's garbage-pin sweep);
//! - every mutation bumps a process-global generation stamp and calls
//!   `GraphicsHost::storage_changed`.
//!
//! Bounds (all configurable, oracle defaults):
//! - `total_limit` bytes of decoded image data (default 320 MB; 0 disables
//!   the protocol entirely);
//! - per-image `max_image_size` (default 400 MB, the protocol limit) and
//!   `max_dimension` (default 10 000 px);
//! - `max_placements` (default 2^18) bounds placement memory — a deliberate
//!   count bound the oracle lacks, required by the S7 memory contract;
//! - eviction is deterministic: (transient+unused) < (unused) < (transient+
//!   used) < (used), then oldest generation, then lowest image id.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::host::GraphicsHost;
use crate::image::{Image, ImageData, ImageError, MAX_DIMENSION, MAX_SIZE};
use crate::kitty::command::{Command, Control, CursorMovement, Delete, Quiet, Response};
use crate::kitty::load::{Limits, LoadingImage};
use crate::placement::{Location, Placement, PlacementId, PlacementKey, Point, TerminalContext};

/// Process-global generation counter backing all store stamps. Unique
/// across every store in the process (the oracle's `nextGeneration`).
static GENERATION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Return the next generation stamp: strictly monotonic process-wide,
/// starting at 1 (0 means "never stamped").
pub fn next_generation() -> u64 {
    GENERATION_COUNTER.fetch_add(1, Ordering::Relaxed) + 1
}

/// Storage configuration. All bounds are explicit; the defaults match the
/// oracle (`total_limit = 320 MB`, protocol `max_size`/`max_dimension`).
#[derive(Clone, Debug)]
pub struct StoreConfig {
    /// Total decoded image bytes; zero disables the kitty protocol.
    pub total_limit: usize,
    /// Per-image payload bound (protocol `max_size`).
    pub max_image_size: usize,
    /// Per-axis dimension bound (protocol `max_dimension`).
    pub max_dimension: u32,
    /// Maximum number of placements (count bound on placement memory).
    pub max_placements: usize,
    /// Allowed transmission mediums.
    pub limits: Limits,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            total_limit: 320 * 1000 * 1000,
            max_image_size: MAX_SIZE,
            max_dimension: MAX_DIMENSION,
            max_placements: 1 << 18,
            limits: Limits::direct(),
        }
    }
}

/// One exact pending image transmission. The generation is assigned when
/// the pending image is inserted, so a later replacement of the same ID
/// cannot consume stale payload bytes (oracle `PendingImage`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingImage {
    pub id: u32,
    pub generation: u64,
}

impl PendingImage {
    /// Attach owned decoded bytes if this exact pending transmission is
    /// still resident. On `true` the store owns `data`; on `false` the
    /// caller retains ownership.
    pub fn complete(self, store: &mut ImageStore, data: Vec<u8>) -> bool {
        let Some(img) = store.images.get_mut(&self.id) else {
            return false;
        };
        if img.generation != self.generation {
            return false;
        }
        let expected = match &img.data {
            ImageData::Complete(_) => return false,
            ImageData::Pending(len) => *len,
        };
        if data.len() != expected {
            return false;
        }
        img.data = ImageData::Complete(data);
        store.mark_mutated();
        true
    }
}

/// Image storage associated with one terminal screen.
pub struct ImageStore {
    config: StoreConfig,
    /// Dirty is informational for the renderer: set on every content
    /// mutation and on prune; the renderer clears it if it cares.
    pub dirty: bool,
    /// Generation stamp of the last content mutation. Geometry-only events
    /// (viewport scroll) never change it.
    pub generation: u64,
    next_image_id: u32,
    next_internal_placement_id: u32,
    images: HashMap<u32, Image>,
    placements: HashMap<PlacementKey, Placement>,
    /// In-progress chunked transmission.
    loading: Option<LoadingImage>,
    total_bytes: usize,
}

impl Default for ImageStore {
    fn default() -> Self {
        Self::new(StoreConfig::default())
    }
}

impl ImageStore {
    pub fn new(config: StoreConfig) -> Self {
        Self {
            config,
            dirty: false,
            generation: 0,
            next_image_id: 2147483647,
            next_internal_placement_id: 0,
            images: HashMap::new(),
            placements: HashMap::new(),
            loading: None,
            total_bytes: 0,
        }
    }

    /// The kitty protocol is enabled iff the byte budget is non-zero.
    pub fn enabled(&self) -> bool {
        self.config.total_limit != 0
    }

    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    pub fn total_limit(&self) -> usize {
        self.config.total_limit
    }

    pub fn image_count(&self) -> usize {
        self.images.len()
    }

    pub fn placement_count(&self) -> usize {
        self.placements.len()
    }

    /// All resident images, for renderer texture synchronization. The
    /// iterator borrows the store; callers must not mutate while iterating.
    pub fn images(&self) -> impl Iterator<Item = (&u32, &Image)> {
        self.images.iter()
    }

    /// All placements, for renderer enumeration (paint order is the
    /// caller's z-sort). The iterator borrows the store.
    pub fn placements(&self) -> impl Iterator<Item = (&PlacementKey, &Placement)> {
        self.placements.iter()
    }

    pub fn config(&self) -> &StoreConfig {
        &self.config
    }

    /// Set the byte budget; lowering evicts oldest images first, and zero
    /// clears the storage entirely (disabling the protocol until re-enabled).
    pub fn set_limit(&mut self, limit: usize) {
        if limit == 0 {
            let limits = self.config.limits.clone();
            self.images.clear();
            self.placements.clear();
            self.loading = None;
            self.total_bytes = 0;
            self.next_image_id = 2147483647;
            self.next_internal_placement_id = 0;
            self.config.limits = limits;
            self.mark_mutated();
            self.config.total_limit = 0;
            return;
        }

        if limit < self.total_bytes {
            let req = self.total_bytes - limit;
            let _ = self.evict_image(req);
        }
        self.config.total_limit = limit;
    }

    fn mark_mutated(&mut self) {
        self.dirty = true;
        self.generation = next_generation();
    }

    /// Remove placements whose pin row is below `boundary_row` (history
    /// trimmed by the host). Virtual placements are retained. Images become
    /// eligible for eviction when their last placement is pruned.
    pub fn prune_history(&mut self, boundary_row: u64) {
        let mut removed = Vec::new();
        for (key, p) in &self.placements {
            if let Location::Pin { row, .. } = p.location {
                if row < boundary_row {
                    removed.push(*key);
                }
            }
        }
        if removed.is_empty() {
            return;
        }
        for key in removed {
            self.remove_placement_by_key(&key);
        }
        self.mark_mutated();
    }

    /// Add a fully decoded image, replacing any image with the same ID (and
    /// deleting that image's placements, per the kitty protocol).
    pub fn add_image(&mut self, img: Image) -> Result<(), ImageError> {
        let new_len = img.data_len();
        if new_len > self.config.total_limit {
            return Err(ImageError::OutOfMemory);
        }
        let old_len = self
            .images
            .get(&img.id)
            .map(|old| old.data_len())
            .unwrap_or(0);
        debug_assert!(old_len <= self.total_bytes);

        // Reserve map capacity before evicting so failure cannot leave the
        // storage partially evicted.
        if !self.images.contains_key(&img.id) {
            self.images
                .try_reserve(1)
                .map_err(|_| ImageError::OutOfMemory)?;
        }

        // If this would put us over the limit, evict (never the image being
        // replaced: its old reservation was already credited).
        let total = self.total_bytes - old_len + new_len;
        if total > self.config.total_limit {
            let req = total - self.config.total_limit;
            if !self.evict_image_except(req, Some(img.id)) {
                return Err(ImageError::OutOfMemory);
            }
        }

        if let Some(old) = self.images.remove(&img.id) {
            self.remove_placements_by_image_id(img.id);
            self.total_bytes -= old.data_len();
        }

        let mut img = img;
        img.placement_count = 0;
        self.total_bytes += new_len;
        self.mark_mutated();
        img.generation = self.generation;
        self.images.insert(img.id, img);
        Ok(())
    }

    /// Add an image whose decoded payload has not arrived yet; returns the
    /// token required to complete this exact transmission.
    pub fn add_pending_image(&mut self, mut img: Image) -> Result<PendingImage, ImageError> {
        debug_assert!(img.data.is_pending());
        let expected = img.data_len();
        img.data = ImageData::Pending(expected);
        let id = img.id;
        self.add_image(img)?;
        let stored = self.images.get(&id).unwrap();
        Ok(PendingImage {
            id: stored.id,
            generation: stored.generation,
        })
    }

    pub fn image_by_id(&self, image_id: u32) -> Option<&Image> {
        self.images.get(&image_id)
    }

    /// Newest image with the given image number (oracle `imageByNumber`).
    pub fn image_by_number(&self, image_number: u32) -> Option<&Image> {
        self.images
            .values()
            .filter(|img| img.number == image_number)
            .max_by_key(|img| img.generation)
    }

    pub fn image_by_id_cloned(&self, image_id: u32) -> Option<Image> {
        self.images.get(&image_id).cloned()
    }

    /// Add a placement for a given image. `placement_id == 0` becomes an
    /// internal id (multiple anonymous placements per image are valid);
    /// non-zero ids are external and replace any existing placement with
    /// the same (image id, placement id) pair.
    pub fn add_placement(
        &mut self,
        image_id: u32,
        placement_id: u32,
        p: Placement,
    ) -> Result<(), ImageError> {
        if !self.images.contains_key(&image_id) {
            return Err(ImageError::InvalidData);
        }
        if self.placements.len() >= self.config.max_placements {
            return Err(ImageError::OutOfMemory);
        }
        self.placements
            .try_reserve(1)
            .map_err(|_| ImageError::OutOfMemory)?;

        let key = PlacementKey {
            image_id,
            placement_id: if placement_id == 0 {
                let id = self.next_internal_placement_id;
                self.next_internal_placement_id = self.next_internal_placement_id.wrapping_add(1);
                PlacementId::Internal(id)
            } else {
                PlacementId::External(placement_id)
            },
        };

        if let Some(_old) = self.placements.remove(&key) {
            // Replacing an existing placement keeps the image's placement
            // count unchanged (oracle `found_existing` path).
        } else {
            let img = self.images.get_mut(&image_id).unwrap();
            img.placement_count = img
                .placement_count
                .checked_add(1)
                .ok_or(ImageError::OutOfMemory)?;
        }
        self.placements.insert(key, p);
        self.mark_mutated();
        Ok(())
    }

    fn decrement_placement_count(&mut self, image_id: u32) {
        if let Some(img) = self.images.get_mut(&image_id) {
            debug_assert!(img.placement_count > 0);
            img.placement_count = img.placement_count.saturating_sub(1);
        }
    }

    fn remove_placement_by_key(&mut self, key: &PlacementKey) {
        self.decrement_placement_count(key.image_id);
        self.placements.remove(key);
    }

    fn remove_placements_by_image_id(&mut self, image_id: u32) {
        let keys: Vec<PlacementKey> = self
            .placements
            .keys()
            .filter(|k| k.image_id == image_id)
            .copied()
            .collect();
        for key in keys {
            self.remove_placement_by_key(&key);
        }
    }

    /// Delete an image if nothing references it.
    fn delete_if_unused(&mut self, image_id: u32) {
        let Some(img) = self.images.get(&image_id) else {
            return;
        };
        if img.placement_count > 0 {
            return;
        }
        self.total_bytes -= img.data_len();
        self.images.remove(&image_id);
    }

    /// Evict the oldest images to free `req` bytes, prioritizing transient
    /// and unused images (deterministic: priority, then generation, then id).
    fn evict_image(&mut self, req: usize) -> bool {
        self.evict_image_except(req, None)
    }

    fn evict_image_except(&mut self, req: usize, exclude_id: Option<u32>) -> bool {
        debug_assert!(req <= self.config.total_limit);
        let images_before = self.images.len();
        let mut evicted: usize = 0;
        while evicted < req {
            // Deterministic candidate selection.
            let best = self
                .images
                .iter()
                .filter(|(id, _)| exclude_id != Some(**id))
                .min_by_key(|(id, img)| {
                    let priority = (if img.transient { 0u8 } else { 1u8 })
                        + if img.placement_count > 0 { 2u8 } else { 0u8 };
                    (priority, img.generation, **id)
                })
                .map(|(id, _)| *id);
            let Some(id) = best else {
                return false;
            };

            self.remove_placements_by_image_id(id);
            let Some(img) = self.images.remove(&id) else {
                return false;
            };
            evicted += img.data_len();
            self.total_bytes -= img.data_len();
        }
        if self.images.len() != images_before {
            self.mark_mutated();
        }
        true
    }

    /// Execute one kitty graphics command (`graphics_exec.zig` `execute`).
    /// Never fails; the returned response may carry an error message and the
    /// store may be unchanged.
    pub fn execute(
        &mut self,
        ctx: &TerminalContext,
        cmd: &Command,
        host: &mut impl GraphicsHost,
    ) -> Option<Response> {
        // A disabled storage disables the whole protocol, including queries.
        if !self.enabled() {
            return None;
        }

        let generation_before = self.generation;
        let mut quiet = cmd.quiet;
        let resp = match cmd.control {
            Control::Query(_) => self.query(cmd),
            Control::Display(_) => self.display(ctx, cmd, host),
            Control::Delete(_) => {
                self.delete(ctx, cmd);
                Response::default()
            }
            Control::Transmit(_) | Control::TransmitAndDisplay { .. } => {
                // Chunked transmissions inherit the starting command's quiet
                // setting unless a later chunk raises it.
                if let Some(loading) = &mut self.loading {
                    quiet = match cmd.quiet {
                        Quiet::No => loading.quiet,
                        q => q,
                    };
                    loading.quiet = quiet;
                }
                self.transmit(ctx, cmd, host)
            }
            Control::TransmitAnimationFrame(_)
            | Control::ControlAnimation(_)
            | Control::ComposeAnimation(_) => Response {
                message: "ERROR: unimplemented action",
                ..Response::default()
            },
        };
        if self.generation != generation_before {
            host.storage_changed();
        }

        let final_resp = match quiet {
            Quiet::No => {
                if resp.empty() {
                    None
                } else {
                    Some(resp)
                }
            }
            Quiet::Ok => {
                if resp.ok() {
                    None
                } else {
                    Some(resp)
                }
            }
            Quiet::Failures => None,
        };
        if let Some(resp) = final_resp {
            let mut buf = Vec::new();
            resp.encode(&mut buf);
            host.write_response(&buf);
        }
        final_resp
    }

    /// Query: validate a transmission without persisting anything.
    fn query(&self, cmd: &Command) -> Response {
        let t = cmd.transmission().unwrap();
        let mut result = Response {
            id: t.image_id,
            image_number: t.image_number,
            placement_id: t.placement_id,
            ..Response::default()
        };
        if t.image_id == 0 {
            result.message = "EINVAL: image ID required";
            return result;
        }
        if let Err(err) = LoadingImage::init(
            cmd,
            &self.config.limits,
            self.config.max_image_size,
            self.config.max_dimension,
        ) {
            result.message = err.message();
        }
        result
    }

    /// Transmit image data (and optionally display it).
    fn transmit(
        &mut self,
        ctx: &TerminalContext,
        cmd: &Command,
        host: &mut impl GraphicsHost,
    ) -> Response {
        let t = cmd.transmission().unwrap();
        let mut result = Response {
            id: t.image_id,
            image_number: t.image_number,
            placement_id: t.placement_id,
            ..Response::default()
        };
        if t.image_id > 0 && t.image_number > 0 {
            result.message = "EINVAL: image ID and number are mutually exclusive";
            return result;
        }

        let loaded = match self.load_and_add_image(cmd) {
            Ok(loaded) => loaded,
            Err(err) => {
                result.message = err.message();
                return result;
            }
        };

        // Transmit-and-display displays after a successful load.
        if let Some(mut d) = loaded.display {
            debug_assert!(!loaded.more);
            d.image_id = loaded.image_id;
            // The image is already in the store; the display path looks it
            // up there (identical width/height, no extra clone needed).
            result = self.display_placement(ctx, d, host, None);
        }

        // Chunked transmissions never respond (the final chunk completes
        // the image and its display, but the response for implicit ids is
        // suppressed below).
        if loaded.more {
            return Response::default();
        }
        // Images assigned IDs implicitly (no id and no number) never
        // receive a response.
        if loaded.implicit_id {
            return Response::default();
        }
        result.id = loaded.image_id;
        result
    }

    /// Load image data, honoring chunking; returns the stored image, whether
    /// more chunks are expected, and the deferred display request.
    fn load_and_add_image(&mut self, cmd: &Command) -> Result<LoadedImage, ImageError> {
        let t = cmd.transmission().unwrap();

        let mut loading = if let Some(mut loading) = self.loading.take() {
            loading.add_data(&cmd.data)?;
            if t.more_chunks {
                let image_id = loading.image.id;
                let implicit_id = loading.image.implicit_id;
                self.loading = Some(loading);
                return Ok(LoadedImage {
                    image_id,
                    implicit_id,
                    more: true,
                    display: None,
                });
            }
            loading
        } else {
            LoadingImage::init(
                cmd,
                &self.config.limits,
                self.config.max_image_size,
                self.config.max_dimension,
            )?
        };

        // Assign an automatic ID if none was given.
        if loading.image.id == 0 {
            loading.image.id = self.next_image_id;
            self.next_image_id = self.next_image_id.wrapping_add(1);
            if loading.image.number == 0 {
                loading.image.implicit_id = true;
            }
        }

        // Beginning of a new chunked transmission.
        if t.more_chunks {
            let image_id = loading.image.id;
            let implicit_id = loading.image.implicit_id;
            self.loading = Some(loading);
            return Ok(LoadedImage {
                image_id,
                implicit_id,
                more: true,
                display: None,
            });
        }

        // Validate, decode, and store the image.
        let display = loading.display;
        let image = loading.complete()?;
        let image_id = image.id;
        let implicit_id = image.implicit_id;
        self.add_image(image)?;

        Ok(LoadedImage {
            image_id,
            implicit_id,
            more: false,
            display,
        })
    }

    /// Display a previously transmitted image.
    fn display(
        &mut self,
        ctx: &TerminalContext,
        cmd: &Command,
        host: &mut impl GraphicsHost,
    ) -> Response {
        self.display_placement(ctx, cmd.display().unwrap(), host, None)
    }

    fn display_placement(
        &mut self,
        ctx: &TerminalContext,
        d: crate::kitty::command::Display,
        host: &mut impl GraphicsHost,
        known_image: Option<&Image>,
    ) -> Response {
        let mut result = Response {
            id: d.image_id,
            image_number: d.image_number,
            placement_id: d.placement_id,
            ..Response::default()
        };

        if d.image_id == 0 && d.image_number == 0 {
            result.message = "EINVAL: image ID or number required";
            return result;
        }

        // Look up the image (by id, or by number -> newest).
        let img = match known_image {
            Some(img) => img.clone(),
            None => {
                let found = if d.image_id != 0 {
                    self.image_by_id_cloned(d.image_id)
                } else {
                    self.image_by_number(d.image_number).cloned()
                };
                match found {
                    Some(img) => img,
                    None => {
                        result.message = "ENOENT: image not found";
                        return result;
                    }
                }
            }
        };
        result.id = img.id;

        // Placement location: virtual placements are untracked; pinned
        // placements anchor at the current cursor position.
        let location = if d.virtual_placement {
            if d.parent_id > 0 {
                result.message = "EINVAL: virtual placement cannot refer to a parent";
                return result;
            }
            Location::Virtual
        } else {
            Location::Pin {
                row: ctx.viewport_first_row.saturating_add(ctx.cursor.y as u64),
                col: ctx.cursor.x,
            }
        };

        let p = Placement {
            location,
            x_offset: d.x_offset,
            y_offset: d.y_offset,
            source_x: d.x,
            source_y: d.y,
            source_width: d.width,
            source_height: d.height,
            columns: d.columns,
            rows: d.rows,
            z: d.z,
            ..Placement::default()
        };
        if let Err(err) = self.add_placement(img.id, d.placement_id, p) {
            result.message = err.message();
            return result;
        }

        // Cursor movement (C=0, the default): index down `rows` then set the
        // column. Bounded so untrusted row counts cannot spin (oracle bounds
        // by the terminal height).
        if let Location::Pin { row: _, col } = p.location {
            if d.cursor_movement == CursorMovement::After {
                let gs = p.grid_size(&img, ctx);
                let rows_to_move = gs.height.min(ctx.rows);
                let new_col = col.saturating_add(gs.width).saturating_add(1);
                host.cursor_after_placement(rows_to_move, new_col);
            }
        }

        result
    }

    /// Delete placements and images (`graphics_storage.zig` `delete`).
    /// Delete never responds on success.
    pub fn delete(&mut self, ctx: &TerminalContext, cmd: &Command) {
        let Control::Delete(delete) = cmd.control else {
            return;
        };
        let placements_before = self.placements.len();
        let images_before = self.images.len();

        match delete {
            Delete::All { delete_images } => {
                let keys: Vec<PlacementKey> = self
                    .placements
                    .keys()
                    .filter(|k| {
                        matches!(
                            self.placements.get(*k).map(|p| p.location),
                            Some(Location::Pin { .. })
                        )
                    })
                    .copied()
                    .collect();
                for key in keys {
                    self.remove_placement_by_key(&key);
                    if delete_images {
                        self.delete_if_unused(key.image_id);
                    }
                }
                if delete_images {
                    let ids: Vec<u32> = self.images.keys().copied().collect();
                    for id in ids {
                        self.delete_if_unused(id);
                    }
                }
            }
            Delete::Id {
                delete,
                image_id,
                placement_id,
            } => self.delete_by_id(image_id, placement_id, delete),
            Delete::Newest {
                delete,
                image_number,
                placement_id,
            } => {
                if let Some(img) = self.image_by_number(image_number) {
                    self.delete_by_id(img.id, placement_id, delete);
                }
            }
            Delete::IntersectCursor { delete_images } => {
                self.delete_intersecting(ctx, ctx.cursor, delete_images, None);
            }
            Delete::IntersectCell { delete, x, y } => {
                if x == 0 || y == 0 {
                    return;
                }
                self.delete_intersecting(ctx, Point { x: x - 1, y: y - 1 }, delete, None);
            }
            Delete::IntersectCellZ { delete, x, y, z } => {
                if x == 0 || y == 0 {
                    return;
                }
                self.delete_intersecting(ctx, Point { x: x - 1, y: y - 1 }, delete, Some(z));
            }
            Delete::Column { delete, x } => {
                if x == 0 {
                    return;
                }
                let x0 = x - 1;
                let keys: Vec<PlacementKey> = self
                    .placements
                    .keys()
                    .filter(|key| {
                        let key = *key;
                        let Some(p) = self.placements.get(key) else {
                            return false;
                        };
                        let Some(img) = self.images.get(&key.image_id) else {
                            return false;
                        };
                        p.rect(img, ctx)
                            .map(|r| r.top_left.1 <= x0 && r.bottom_right.1 >= x0)
                            .unwrap_or(false)
                    })
                    .copied()
                    .collect();
                for key in keys {
                    self.remove_placement_by_key(&key);
                    if delete {
                        self.delete_if_unused(key.image_id);
                    }
                }
            }
            Delete::Row { delete, y } => {
                if y == 0 {
                    return;
                }
                let target_row = ctx.viewport_first_row.saturating_add((y - 1) as u64);
                let keys: Vec<PlacementKey> = self
                    .placements
                    .keys()
                    .filter(|key| {
                        let key = *key;
                        let Some(p) = self.placements.get(key) else {
                            return false;
                        };
                        let Some(img) = self.images.get(&key.image_id) else {
                            return false;
                        };
                        p.rect(img, ctx)
                            .map(|r| r.top_left.0 <= target_row && r.bottom_right.0 >= target_row)
                            .unwrap_or(false)
                    })
                    .copied()
                    .collect();
                for key in keys {
                    self.remove_placement_by_key(&key);
                    if delete {
                        self.delete_if_unused(key.image_id);
                    }
                }
            }
            Delete::Z { delete, z } => {
                let keys: Vec<PlacementKey> = self
                    .placements
                    .keys()
                    .filter(|key| {
                        let key = *key;
                        let Some(p) = self.placements.get(key) else {
                            return false;
                        };
                        matches!(p.location, Location::Pin { .. }) && p.z == z
                    })
                    .copied()
                    .collect();
                for key in keys {
                    self.remove_placement_by_key(&key);
                    if delete {
                        self.delete_if_unused(key.image_id);
                    }
                }
            }
            Delete::Range {
                delete,
                first,
                last,
            } => {
                if first == 0 || last == 0 {
                    return;
                }
                let keys: Vec<PlacementKey> = self
                    .placements
                    .keys()
                    .filter(|k| k.image_id >= first && k.image_id <= last)
                    .copied()
                    .collect();
                for key in keys {
                    self.remove_placement_by_key(&key);
                    if delete {
                        self.delete_if_unused(key.image_id);
                    }
                }
            }
            // We don't support animation frames, so they are successfully
            // deleted!
            Delete::AnimationFrames => {}
        }

        if self.placements.len() != placements_before || self.images.len() != images_before {
            self.mark_mutated();
        }
    }

    fn delete_by_id(&mut self, image_id: u32, placement_id: u32, delete_unused: bool) {
        if placement_id == 0 {
            self.remove_placements_by_image_id(image_id);
        } else {
            let key = PlacementKey {
                image_id,
                placement_id: PlacementId::External(placement_id),
            };
            if self.placements.remove(&key).is_some() {
                self.decrement_placement_count(image_id);
            }
        }
        if delete_unused {
            self.delete_if_unused(image_id);
        }
    }

    fn delete_intersecting(
        &mut self,
        ctx: &TerminalContext,
        point: Point,
        delete_unused: bool,
        z_filter: Option<i32>,
    ) {
        let target = (
            ctx.viewport_first_row.saturating_add(point.y as u64),
            point.x,
        );
        let keys: Vec<PlacementKey> = self
            .placements
            .keys()
            .filter(|key| {
                let key = *key;
                let Some(p) = self.placements.get(key) else {
                    return false;
                };
                let Some(img) = self.images.get(&key.image_id) else {
                    return false;
                };
                let Some(rect) = p.rect(img, ctx) else {
                    return false;
                };
                if let Some(z) = z_filter {
                    if p.z != z {
                        return false;
                    }
                }
                rect.contains(target.0, target.1)
            })
            .copied()
            .collect();
        for key in keys {
            self.remove_placement_by_key(&key);
            if delete_unused {
                self.delete_if_unused(key.image_id);
            }
        }
    }
}

/// Result of loading image data. The image itself is stored in the store;
/// only the identity needed for response/display handling is carried.
struct LoadedImage {
    image_id: u32,
    implicit_id: bool,
    more: bool,
    display: Option<crate::kitty::command::Display>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::RecordingHost;
    use crate::image::{Compression, ImageFormat};
    use crate::kitty::command::parse_string;
    use base64::Engine;

    const RGB_20X15: &[u8] = include_bytes!(
        "../../../verification/graphics-corpus/fixtures/image-rgb-none-20x15-2147483647-raw.data"
    );

    fn ctx() -> TerminalContext {
        TerminalContext {
            cols: 80,
            rows: 24,
            width_px: 800,
            height_px: 600,
            ..TerminalContext::default()
        }
    }

    fn store() -> (ImageStore, RecordingHost) {
        (
            ImageStore::new(StoreConfig {
                limits: Limits::direct(),
                ..StoreConfig::default()
            }),
            RecordingHost::default(),
        )
    }

    fn transmit_cmd(id: u32, format: ImageFormat, more: bool, payload: &[u8]) -> Command {
        let f = match format {
            ImageFormat::Rgb => 24,
            ImageFormat::Rgba => 32,
            ImageFormat::Png => 100,
            _ => 24,
        };
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(payload);
        let more_s = if more { ",m=1" } else { "" };
        let input = format!("a=t,t=d,f={f},s=20,v=15,i={id}{more_s};{b64}");
        parse_string(input.as_bytes()).unwrap()
    }

    fn display_cmd(id: u32, placement_id: u32) -> Command {
        parse_string(format!("a=p,i={id},p={placement_id},C=1").as_bytes()).unwrap()
    }

    #[test]
    fn transmit_then_display_and_response_bytes() {
        let (mut s, mut host) = store();
        let cmd = transmit_cmd(1, ImageFormat::Rgb, false, RGB_20X15);
        let resp = s.execute(&ctx(), &cmd, &mut host).unwrap();
        assert!(resp.ok());
        assert_eq!(resp.id, 1);
        assert_eq!(host.responses.last().unwrap(), b"\x1b_Gi=1;OK\x1b\\");
        assert_eq!(s.image_count(), 1);
        assert_eq!(s.image_by_id(1).unwrap().width, 20);

        // Display with C=1 (no cursor movement).
        host.responses.clear();
        let cmd = display_cmd(1, 0);
        let resp = s.execute(&ctx(), &cmd, &mut host).unwrap();
        assert!(resp.ok());
        assert_eq!(s.placement_count(), 1);
        assert!(host.cursor_moves.is_empty());
    }

    #[test]
    fn transmit_and_display_moves_cursor() {
        let (mut s, mut host) = store();
        let input = format!(
            "a=T,t=d,f=24,s=20,v=15,i=1,c=2,r=3;{}",
            base64::engine::general_purpose::STANDARD.encode(RGB_20X15)
        );
        let cmd = parse_string(input.as_bytes()).unwrap();
        let resp = s.execute(&ctx(), &cmd, &mut host).unwrap();
        assert!(resp.ok());
        // Cursor movement: rows=min(3, 24)=3, col = 0 + 2 + 1 = 3.
        assert_eq!(host.cursor_moves, vec![(3, 3)]);
        assert_eq!(s.placement_count(), 1);
    }

    #[test]
    fn implicit_id_gets_no_response() {
        let (mut s, mut host) = store();
        let cmd = transmit_cmd(0, ImageFormat::Rgb, false, RGB_20X15);
        let resp = s.execute(&ctx(), &cmd, &mut host);
        assert!(resp.is_none());
        assert!(host.responses.is_empty());
        assert_eq!(s.image_count(), 1);
    }

    #[test]
    fn display_missing_image_is_enoent() {
        let (mut s, mut host) = store();
        let cmd = parse_string("a=p,i=4294967295".as_bytes()).unwrap();
        let resp = s.execute(&ctx(), &cmd, &mut host).unwrap();
        assert!(!resp.ok());
        assert_eq!(resp.message, "ENOENT: image not found");
        assert_eq!(
            host.response_bytes(),
            b"\x1b_Gi=4294967295;ENOENT: image not found\x1b\\"
        );
    }

    #[test]
    fn transmit_requires_not_both_id_and_number() {
        let (mut s, mut host) = store();
        let cmd = parse_string(b"a=t,t=d,f=24,i=1,I=2;////").unwrap();
        let resp = s.execute(&ctx(), &cmd, &mut host).unwrap();
        assert_eq!(
            resp.message,
            "EINVAL: image ID and number are mutually exclusive"
        );
    }

    #[test]
    fn chunked_transmission_quiet_inheritance() {
        let (mut s, mut host) = store();
        // First chunk: q=1 (respond only on error) and m=1.
        let cmd = parse_string(
            format!(
                "a=t,t=d,f=24,i=1,s=20,v=15,m=1,q=1;{}",
                base64::engine::general_purpose::STANDARD.encode(&RGB_20X15[..300])
            )
            .as_bytes(),
        )
        .unwrap();
        assert!(s.execute(&ctx(), &cmd, &mut host).is_none());
        assert!(host.responses.is_empty());

        // Final chunk: no q -> inherits q=1 from the start, so an OK
        // response is suppressed.
        let cmd = parse_string(
            format!(
                "m=0;{}",
                base64::engine::general_purpose::STANDARD.encode(&RGB_20X15[300..])
            )
            .as_bytes(),
        )
        .unwrap();
        assert!(s.execute(&ctx(), &cmd, &mut host).is_none());
        assert_eq!(s.image_count(), 1);

        // q=0 chunked transmission DOES respond on the final chunk.
        let (mut s2, mut host2) = store();
        let cmd = parse_string(
            format!(
                "a=t,t=d,f=24,i=2,s=20,v=15,m=1,q=0;{}",
                base64::engine::general_purpose::STANDARD.encode(&RGB_20X15[..300])
            )
            .as_bytes(),
        )
        .unwrap();
        assert!(s2.execute(&ctx(), &cmd, &mut host2).is_none());
        let cmd = parse_string(
            format!(
                "m=0;{}",
                base64::engine::general_purpose::STANDARD.encode(&RGB_20X15[300..])
            )
            .as_bytes(),
        )
        .unwrap();
        let resp = s2.execute(&ctx(), &cmd, &mut host2).unwrap();
        assert!(resp.ok());
        assert_eq!(resp.id, 2);
    }

    #[test]
    fn retransmit_same_id_bumps_generation_and_clears_placements() {
        let (mut s, mut host) = store();
        let cmd = transmit_cmd(1, ImageFormat::Rgb, false, RGB_20X15);
        s.execute(&ctx(), &cmd, &mut host);
        let gen1 = s.image_by_id(1).unwrap().generation;
        let d = display_cmd(1, 0);
        s.execute(&ctx(), &d, &mut host);
        assert_eq!(s.placement_count(), 1);

        let cmd = transmit_cmd(1, ImageFormat::Rgb, false, RGB_20X15);
        s.execute(&ctx(), &cmd, &mut host);
        let gen2 = s.image_by_id(1).unwrap().generation;
        assert!(gen2 > gen1);
        // Retransmission removes every old placement.
        assert_eq!(s.placement_count(), 0);
    }

    #[test]
    fn delete_all_and_clear_screen_semantics() {
        let (mut s, mut host) = store();
        let cmd = transmit_cmd(1, ImageFormat::Rgb, false, RGB_20X15);
        s.execute(&ctx(), &cmd, &mut host);
        s.execute(&ctx(), &display_cmd(1, 0), &mut host);
        let generation = s.generation;

        // d=a removes placements but keeps images; no response.
        let cmd = parse_string(b"a=d,d=a").unwrap();
        assert!(s.execute(&ctx(), &cmd, &mut host).is_none());
        assert_eq!(s.placement_count(), 0);
        assert_eq!(s.image_count(), 1);
        assert!(s.generation > generation);

        // d=A also removes the image.
        s.execute(&ctx(), &display_cmd(1, 0), &mut host);
        let cmd = parse_string(b"a=d,d=A").unwrap();
        assert!(s.execute(&ctx(), &cmd, &mut host).is_none());
        assert_eq!(s.image_count(), 0);
    }

    #[test]
    fn delete_by_id_placement_and_image() {
        let (mut s, mut host) = store();
        s.execute(
            &ctx(),
            &transmit_cmd(1, ImageFormat::Rgb, false, RGB_20X15),
            &mut host,
        );
        s.execute(&ctx(), &display_cmd(1, 7), &mut host);
        // d=i,p=7 removes only the named placement.
        let cmd = parse_string(b"a=d,d=i,i=1,p=7").unwrap();
        s.execute(&ctx(), &cmd, &mut host);
        assert_eq!(s.placement_count(), 0);
        assert_eq!(s.image_count(), 1);
        // d=I removes the placement and the image (now unused).
        s.execute(&ctx(), &display_cmd(1, 0), &mut host);
        let cmd = parse_string(b"a=d,d=I,i=1").unwrap();
        s.execute(&ctx(), &cmd, &mut host);
        assert_eq!(s.image_count(), 0);
    }

    #[test]
    fn delete_intersect_cell_and_z() {
        let (mut s, mut host) = store();
        // Place at cursor (0,0) with 2x3 grid.
        let cmd = parse_string(
            format!(
                "a=T,t=d,f=24,i=1,s=20,v=15,c=2,r=3,C=1;{}",
                base64::engine::general_purpose::STANDARD.encode(RGB_20X15)
            )
            .as_bytes(),
        )
        .unwrap();
        s.execute(&ctx(), &cmd, &mut host);

        // Intersect cell (1,1) hits the placement (0-based 0,0).
        let cmd = parse_string(b"a=d,d=p,x=1,y=1").unwrap();
        s.execute(&ctx(), &cmd, &mut host);
        assert_eq!(s.placement_count(), 0);

        // z-filtered delete does not match different z.
        s.execute(&ctx(), &display_cmd(1, 0), &mut host);
        let cmd = parse_string(b"a=d,d=Q,x=1,y=1,z=5").unwrap();
        s.execute(&ctx(), &cmd, &mut host);
        assert_eq!(s.placement_count(), 1);
        let cmd = parse_string(b"a=d,d=Q,x=1,y=1,z=0").unwrap();
        s.execute(&ctx(), &cmd, &mut host);
        assert_eq!(s.placement_count(), 0);
    }

    #[test]
    fn delete_column_row_range() {
        let (mut s, mut host) = store();
        let cmd = parse_string(
            format!(
                "a=T,t=d,f=24,i=1,s=20,v=15,c=2,r=3,C=1;{}",
                base64::engine::general_purpose::STANDARD.encode(RGB_20X15)
            )
            .as_bytes(),
        )
        .unwrap();
        s.execute(&ctx(), &cmd, &mut host);

        // Column 1 intersects (0-based col 0).
        let cmd = parse_string(b"a=d,d=x,x=1").unwrap();
        s.execute(&ctx(), &cmd, &mut host);
        assert_eq!(s.placement_count(), 0);

        s.execute(&ctx(), &display_cmd(1, 0), &mut host);
        let cmd = parse_string(b"a=d,d=y,y=1").unwrap();
        s.execute(&ctx(), &cmd, &mut host);
        assert_eq!(s.placement_count(), 0);

        s.execute(&ctx(), &display_cmd(1, 0), &mut host);
        let cmd = parse_string(b"a=d,d=r,x=1,y=5").unwrap();
        s.execute(&ctx(), &cmd, &mut host);
        assert_eq!(s.placement_count(), 0);
    }

    #[test]
    fn eviction_is_deterministic_and_byte_bounded() {
        let (mut s, mut host) = store();
        s.set_limit(2000);
        // Two 900-byte images fit in the 2000-byte budget.
        s.execute(
            &ctx(),
            &transmit_cmd(1, ImageFormat::Rgb, false, RGB_20X15),
            &mut host,
        );
        s.execute(
            &ctx(),
            &transmit_cmd(2, ImageFormat::Rgb, false, RGB_20X15),
            &mut host,
        );
        // Mark image 1 used; image 2 stays unused.
        s.execute(&ctx(), &display_cmd(1, 0), &mut host);
        // A third image needs 700 bytes: the unused image 2 evicts before
        // the used image 1 (priority order), even though 1 is older.
        s.execute(
            &ctx(),
            &transmit_cmd(3, ImageFormat::Rgb, false, RGB_20X15),
            &mut host,
        );
        assert!(s.image_by_id(1).is_some(), "used image must survive");
        assert!(s.image_by_id(2).is_none(), "unused image evicts first");
        assert!(s.image_by_id(3).is_some());
        assert!(s.total_bytes() <= s.total_limit());
        // Eviction is deterministic: repeat on a fresh store yields the same
        // survivor set (generations differ across stores; presence does not).
        let (mut s2, mut host2) = store();
        s2.set_limit(2000);
        s2.execute(
            &ctx(),
            &transmit_cmd(1, ImageFormat::Rgb, false, RGB_20X15),
            &mut host2,
        );
        s2.execute(
            &ctx(),
            &transmit_cmd(2, ImageFormat::Rgb, false, RGB_20X15),
            &mut host2,
        );
        s2.execute(&ctx(), &display_cmd(1, 0), &mut host2);
        s2.execute(
            &ctx(),
            &transmit_cmd(3, ImageFormat::Rgb, false, RGB_20X15),
            &mut host2,
        );
        assert_eq!(s2.image_by_id(1).is_some(), s.image_by_id(1).is_some());
        assert_eq!(s2.image_by_id(2).is_some(), s.image_by_id(2).is_some());
        assert_eq!(s2.image_by_id(3).is_some(), s.image_by_id(3).is_some());
        assert!(s2.image_by_id(2).is_none());
        assert!(s2.image_by_id(3).is_some());
    }

    #[test]
    fn eviction_removes_placements() {
        let (mut s, mut host) = store();
        s.set_limit(1000);
        s.execute(
            &ctx(),
            &transmit_cmd(1, ImageFormat::Rgb, false, RGB_20X15),
            &mut host,
        );
        s.execute(&ctx(), &display_cmd(1, 0), &mut host);
        s.execute(
            &ctx(),
            &transmit_cmd(2, ImageFormat::Rgb, false, RGB_20X15),
            &mut host,
        );
        // Evicting image 1 must take its placement with it.
        assert_eq!(s.placement_count(), 0);
    }

    #[test]
    fn set_limit_zero_disables_protocol() {
        let (mut s, mut host) = store();
        s.execute(
            &ctx(),
            &transmit_cmd(1, ImageFormat::Rgb, false, RGB_20X15),
            &mut host,
        );
        s.set_limit(0);
        assert_eq!(s.image_count(), 0);
        assert!(!s.enabled());
        // Even queries are refused when disabled.
        let cmd = parse_string(b"a=q,i=1").unwrap();
        assert!(s.execute(&ctx(), &cmd, &mut host).is_none());
    }

    #[test]
    fn animation_commands_respond_only_under_quiet_ok() {
        let (mut s, mut host) = store();
        let cmd = parse_string(b"a=f,c=1").unwrap();
        // q=0: the response has no id/image number, so nothing is written.
        assert!(s.execute(&ctx(), &cmd, &mut host).is_none());
        let cmd = parse_string(b"a=f,c=1,q=1").unwrap();
        let resp = s.execute(&ctx(), &cmd, &mut host).unwrap();
        assert_eq!(resp.message, "ERROR: unimplemented action");
    }

    #[test]
    fn generation_is_process_global_and_monotonic() {
        let g1 = next_generation();
        let g2 = next_generation();
        assert!(g2 > g1);
        let (mut s, mut host) = store();
        s.execute(
            &ctx(),
            &transmit_cmd(1, ImageFormat::Rgb, false, RGB_20X15),
            &mut host,
        );
        assert!(s.generation > g2);
        let (mut s2, mut host2) = store();
        s2.execute(
            &ctx(),
            &transmit_cmd(1, ImageFormat::Rgb, false, RGB_20X15),
            &mut host2,
        );
        assert!(s2.generation > s.generation);
    }

    #[test]
    fn prune_history_removes_scrolled_out_placements() {
        let (mut s, mut host) = store();
        let mut c = ctx();
        c.viewport_first_row = 100;
        c.cursor = crate::placement::Point { x: 1, y: 2 };
        s.execute(
            &c,
            &transmit_cmd(1, ImageFormat::Rgb, false, RGB_20X15),
            &mut host,
        );
        s.execute(&c, &display_cmd(1, 0), &mut host);
        assert_eq!(s.placement_count(), 1);

        // Prune everything below row 150 -> placement at row 102 goes.
        s.prune_history(150);
        assert_eq!(s.placement_count(), 0);
        // Images stay until evicted or explicitly deleted.
        assert_eq!(s.image_count(), 1);
    }

    #[test]
    fn placement_count_budget_is_enforced() {
        let (mut s, mut host) = store();
        s.config.max_placements = 2;
        s.execute(
            &ctx(),
            &transmit_cmd(1, ImageFormat::Rgb, false, RGB_20X15),
            &mut host,
        );
        s.execute(&ctx(), &display_cmd(1, 0), &mut host);
        s.execute(&ctx(), &display_cmd(1, 0), &mut host);
        // Third placement over the count budget -> error response.
        let cmd = display_cmd(1, 0);
        let resp = s.execute(&ctx(), &cmd, &mut host).unwrap();
        assert_eq!(resp.message, "ENOMEM: out of memory");
        assert_eq!(s.placement_count(), 2);
    }

    #[test]
    fn pending_image_completion_token() {
        let (mut s, _host) = store();
        let img = Image {
            id: 9,
            number: 0,
            width: 20,
            height: 15,
            format: ImageFormat::Rgb,
            compression: Compression::None,
            data: ImageData::Pending(900),
            transient: false,
            implicit_id: false,
            placement_count: 0,
            generation: 0,
        };
        let token = s.add_pending_image(img).unwrap();
        assert!(s.image_by_id(9).unwrap().data.is_pending());

        // Stale token (generation mismatch) is rejected.
        let stale = PendingImage {
            id: 9,
            generation: 0,
        };
        assert!(!stale.complete(&mut s, vec![0u8; 900]));

        // Correct token completes; wrong length is rejected.
        let wrong = PendingImage {
            id: 9,
            generation: token.generation,
        };
        assert!(!wrong.complete(&mut s, vec![0u8; 100]));
        assert!(token.complete(&mut s, vec![0u8; 900]));
        assert!(!s.image_by_id(9).unwrap().data.is_pending());
    }
}
