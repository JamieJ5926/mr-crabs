//! Image loading: chunk accumulation, transports, decompression, PNG decode.
//!
//! Faithful port of `src/terminal/kitty/graphics_image.zig` (Ghostty
//! `d2c70a8c7b9b6893c13640c02d7b6f9a1624f3f0`). All byte paths are bounded
//! by `max_size`; file and shared-memory payloads are validated before any
//! read; temporary files are deleted after a successful load.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::image::{
    Compression, Image, ImageData, ImageError, ImageFormat, Medium, decode_png_to_rgba,
    zlib_decompress,
};
use crate::kitty::command::{Command, Quiet, Transmission};

/// Transport limits for image loading (`graphics_image.zig` `Limits`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    pub file: bool,
    pub temporary_file: TempFileLimit,
    pub shared_memory: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TempFileLimit {
    Enabled { directory: PathBuf },
    Disabled,
}

impl Limits {
    /// Only the direct medium is allowed (the oracle's default).
    pub fn direct() -> Self {
        Self {
            file: false,
            temporary_file: TempFileLimit::Disabled,
            shared_memory: false,
        }
    }

    /// Enable every filesystem-related medium; `path` is the directory to
    /// expect temporary files in (`allWithTempDir`).
    pub fn all_with_temp_dir(path: impl Into<PathBuf>) -> Self {
        Self {
            file: true,
            temporary_file: TempFileLimit::Enabled {
                directory: path.into(),
            },
            shared_memory: true,
        }
    }
}

/// An image still being loaded. `init` on the first chunk, `add_data` per
/// subsequent chunk, `complete` to finalize.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadingImage {
    /// The in-progress image; the first chunk carries all the metadata.
    pub image: Image,
    data: Vec<u8>,
    /// Transmit-and-display deferred display request.
    pub display: Option<crate::kitty::command::Display>,
    /// Quiet setting of the initial load command (inherited by chunks).
    pub quiet: Quiet,
    temporary_directory: Option<PathBuf>,
    max_size: usize,
    max_dimension: u32,
}

impl LoadingImage {
    /// Initialize from the first transmission chunk. Non-direct media are
    /// validated against `limits` and their payloads read (and for
    /// temporary files, deleted) immediately.
    pub fn init(
        cmd: &Command,
        limits: &Limits,
        max_size: usize,
        max_dimension: u32,
    ) -> Result<Self, ImageError> {
        let t = cmd.transmission().ok_or(ImageError::InvalidData)?;
        let mut result = Self {
            image: Image {
                id: t.image_id,
                number: t.image_number,
                width: t.width,
                height: t.height,
                format: t.format,
                compression: t.compression,
                data: ImageData::Pending(0),
                transient: t.transient,
                implicit_id: false,
                placement_count: 0,
                generation: 0,
            },
            data: Vec::new(),
            display: cmd.display(),
            quiet: cmd.quiet,
            temporary_directory: match &limits.temporary_file {
                TempFileLimit::Enabled { directory } => Some(directory.clone()),
                TempFileLimit::Disabled => None,
            },
            max_size,
            max_dimension,
        };

        // Special case: the direct medium just accumulates the chunk.
        if t.medium == Medium::Direct {
            result.add_data(&cmd.data)?;
            return Ok(result);
        }

        // Verify the medium is allowed.
        let allowed = match t.medium {
            Medium::Direct => unreachable!("handled above"),
            Medium::File => limits.file,
            Medium::TemporaryFile => limits.temporary_file != TempFileLimit::Disabled,
            Medium::SharedMemory => limits.shared_memory,
        };
        if !allowed {
            return Err(ImageError::UnsupportedMedium);
        }

        // Otherwise the payload is a path.
        if cmd.data.contains(&0) {
            // POSIX paths cannot contain internal NULs.
            return Err(ImageError::InvalidData);
        }

        match t.medium {
            Medium::Direct => unreachable!(),
            Medium::File => result.read_file(t, &cmd.data, false)?,
            Medium::TemporaryFile => result.read_file(t, &cmd.data, true)?,
            Medium::SharedMemory => result.read_shared_memory(t, &cmd.data)?,
        }

        Ok(result)
    }

    /// Append a chunk of data (the `m` parameter continues a transmission).
    pub fn add_data(&mut self, data: &[u8]) -> Result<(), ImageError> {
        if data.is_empty() {
            return Ok(());
        }
        if self.data.len().saturating_add(data.len()) > self.max_size {
            return Err(ImageError::InvalidData);
        }
        self.data.extend_from_slice(data);
        Ok(())
    }

    /// Complete the chunked image, returning a fully decoded image.
    pub fn complete(mut self) -> Result<Image, ImageError> {
        // Inflate compressed payloads in place.
        if self.image.compression == Compression::ZlibDeflate {
            self.data = zlib_decompress(&self.data, self.max_size)?;
            self.image.compression = Compression::None;
        }

        // Decode PNG payloads to RGBA, updating dimensions and format.
        if self.image.format == ImageFormat::Png {
            let decoded = decode_png_to_rgba(&self.data, self.max_size, self.max_dimension)?;
            self.data = decoded.rgba;
            self.image.width = decoded.width;
            self.image.height = decoded.height;
            self.image.format = ImageFormat::Rgba;
        }

        // Validate dimensions.
        if self.image.width == 0 || self.image.height == 0 {
            return Err(ImageError::DimensionsRequired);
        }
        if self.image.width > self.max_dimension || self.image.height > self.max_dimension {
            return Err(ImageError::DimensionsTooLarge);
        }

        // Data length must be exactly width*height*bpp.
        let bpp = self.image.format.bpp();
        let expected = (self.image.width as usize)
            .checked_mul(self.image.height as usize)
            .and_then(|n| n.checked_mul(bpp as usize))
            .ok_or(ImageError::InvalidData)?;
        if self.data.len() != expected {
            return Err(ImageError::InvalidData);
        }

        let mut image = self.image;
        image.data = ImageData::Complete(self.data);
        Ok(image)
    }

    /// Read a file payload. `temporary` requires the canonical path to live
    /// inside the temporary directory, to be named `tty-graphics-protocol*`,
    /// and deletes the file after reading (mirroring the oracle's defer).
    fn read_file(
        &mut self,
        t: Transmission,
        path: &[u8],
        temporary: bool,
    ) -> Result<(), ImageError> {
        let path = Path::new(std::str::from_utf8(path).map_err(|_| ImageError::InvalidData)?);

        // Open first, before validation, to avoid TOCTOU issues.
        let mut file = fs::File::open(path).map_err(|_| ImageError::InvalidData)?;

        // Derive the canonical path from the open handle so the file we
        // validate is the exact file we read.
        let abs_path = fs::canonicalize(path).map_err(|_| ImageError::InvalidData)?;

        // The oracle's blocklist: /proc, /sys, and /dev (except /dev/shm).
        let p = abs_path.to_string_lossy();
        let blocked = p.starts_with("/proc/")
            || p.starts_with("/sys/")
            || (p.starts_with("/dev/") && !p.starts_with("/dev/shm/"));
        if blocked {
            return Err(ImageError::InvalidData);
        }

        // Temporary-file checks (before any cleanup is armed).
        if temporary {
            let dir = self
                .temporary_directory
                .as_ref()
                .ok_or(ImageError::TemporaryFileNotInTempDir)?;
            if !is_path_in_temp_dir(dir, &abs_path) {
                return Err(ImageError::TemporaryFileNotInTempDir);
            }
            if !p.contains("tty-graphics-protocol") {
                return Err(ImageError::TemporaryFileNotNamedCorrectly);
            }
        }

        // The file must be a regular file.
        if !file
            .metadata()
            .map_err(|_| ImageError::InvalidData)?
            .is_file()
        {
            return Err(ImageError::InvalidData);
        }

        // Seek and read with an explicit bound.
        if t.offset > 0 {
            file.seek(SeekFrom::Start(t.offset as u64))
                .map_err(|_| ImageError::InvalidData)?;
        }
        let limit: u64 = if t.size > 0 {
            (t.size as usize).min(self.max_size) as u64
        } else {
            self.max_size as u64
        };
        let mut buf = Vec::new();
        file.by_ref()
            .take(limit)
            .read_to_end(&mut buf)
            .map_err(|_| ImageError::InvalidData)?;

        self.data = buf;

        // Temporary files are consumed by the read.
        if temporary {
            let _ = fs::remove_file(&abs_path);
        }
        Ok(())
    }

    /// Read a POSIX shared-memory payload: open read-only, stat, validate
    /// the requested range, map it, and copy the validated range, then
    /// unlink the object. Only compiled when the `shm` feature (and thus
    /// the `libc` dependency) is enabled; builds without it report
    /// `UnsupportedMedium` exactly like the oracle's non-POSIX builds.
    ///
    /// The oracle maps the object with `mmap` rather than reading from the
    /// fd: macOS/BSD shared-memory objects return `ENXIO` for read/write.
    #[cfg(all(unix, feature = "shm"))]
    fn read_shared_memory(&mut self, t: Transmission, name: &[u8]) -> Result<(), ImageError> {
        use std::os::unix::io::{AsRawFd, FromRawFd};

        let cname = std::ffi::CString::new(name).map_err(|_| ImageError::InvalidData)?;
        let fd = unsafe { libc::shm_open(cname.as_ptr(), libc::O_RDONLY, 0) };
        if fd < 0 {
            return Err(ImageError::InvalidData);
        }
        // The object name is consumed by the open (the oracle unlinks with
        // a defer right after open); the fd remains valid for stat/mmap.
        unsafe {
            libc::shm_unlink(cname.as_ptr());
        }
        // Transfer fd ownership to a std File (closes on drop).
        let file = unsafe { fs::File::from_raw_fd(fd) };

        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd, &mut st) } != 0 {
            return Err(ImageError::InvalidData);
        }
        let stat_size = st.st_size;
        if stat_size <= 0 {
            return Err(ImageError::InvalidData);
        }
        let stat_size = stat_size as usize;

        let range = self.shared_memory_range(t, stat_size)?;
        let len = range.end - range.start;

        // Map the whole object (the stat size may exceed the requested
        // range; shared memory is page-aligned) and copy the validated
        // range, exactly like the oracle's `readSharedMemory`.
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                stat_size,
                libc::PROT_READ,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if map == libc::MAP_FAILED {
            return Err(ImageError::InvalidData);
        }
        let slice = unsafe { std::slice::from_raw_parts(map as *const u8, stat_size) };
        let mut buf = Vec::with_capacity(len);
        buf.extend_from_slice(&slice[range.start..range.end]);
        unsafe { libc::munmap(map, stat_size) };

        self.data = buf;
        Ok(())
    }

    /// Non-POSIX targets and builds without the `shm` feature reject the
    /// shared-memory medium outright (oracle non-POSIX behavior).
    #[cfg(not(all(unix, feature = "shm")))]
    fn read_shared_memory(&mut self, t: Transmission, name: &[u8]) -> Result<(), ImageError> {
        let _ = (t, name);
        Err(ImageError::UnsupportedMedium)
    }

    /// The byte range to copy from a shared-memory object, validating the
    /// protocol's offset/size/dimension fields (`sharedMemoryRange`).
    fn shared_memory_range(
        &self,
        t: Transmission,
        stat_size: usize,
    ) -> Result<std::ops::Range<usize>, ImageError> {
        let expected_size: Option<usize> = match self.image.format {
            // PNG dimensions come from the decoded data.
            ImageFormat::Png => None,
            _ => {
                if self.image.width > self.max_dimension || self.image.height > self.max_dimension {
                    return Err(ImageError::DimensionsTooLarge);
                }
                let bpp = self.image.format.bpp() as usize;
                Some(self.image.width as usize * self.image.height as usize * bpp)
            }
        };

        let start = t.offset as usize;
        if start > stat_size {
            return Err(ImageError::InvalidData);
        }

        let available = stat_size - start;
        let data_size: usize = if t.size > 0 {
            t.size as usize
        } else if self.image.compression == Compression::None && expected_size.is_some() {
            expected_size.unwrap()
        } else {
            available
        };
        if data_size > self.max_size || data_size > available {
            return Err(ImageError::InvalidData);
        }

        Ok(start..start + data_size)
    }
}

/// Returns true if `path` is `dir` or contained within it, requiring a
/// path-separator boundary so similarly prefixed directories do not match
/// (`isPathInDir`, `graphics_image.zig:705-717`).
fn is_path_in_dir(dir: &Path, path: &Path) -> bool {
    let dir_str = dir.to_string_lossy();
    let path_str = path.to_string_lossy();
    if dir_str.is_empty() || !path_str.starts_with(dir_str.as_ref()) {
        return false;
    }
    let dir_len = dir_str.len();
    path_str.len() == dir_len
        || dir_str.ends_with('/')
        || path_str.as_bytes().get(dir_len).copied() == Some(b'/')
}

/// Returns true if `path` appears to be in a temporary directory. Copies the
/// oracle's logic: `/tmp`, `/dev/shm`, the configured dir, or the realpath
/// of the configured dir (macOS `/tmp` resolves through `/private/var`).
fn is_path_in_temp_dir(dir: &Path, path: &Path) -> bool {
    if is_path_in_dir(Path::new("/tmp"), path) {
        return true;
    }
    if is_path_in_dir(Path::new("/dev/shm"), path) {
        return true;
    }
    if is_path_in_dir(dir, path) {
        return true;
    }
    if let Ok(real_dir) = fs::canonicalize(dir) {
        if is_path_in_dir(&real_dir, path) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::{MAX_DIMENSION, MAX_SIZE};
    use crate::kitty::command::{Command, Control};

    const RGB_20X15: &[u8] = include_bytes!(
        "../../../../verification/graphics-corpus/fixtures/image-rgb-none-20x15-2147483647-raw.data"
    );

    fn rgb_cmd(id: u32, medium: Medium, more: bool) -> Command {
        Command {
            control: Control::Transmit(Transmission {
                format: ImageFormat::Rgb,
                medium,
                width: 20,
                height: 15,
                image_id: id,
                more_chunks: more,
                ..Transmission::default()
            }),
            quiet: Quiet::No,
            data: RGB_20X15.to_vec(),
        }
    }

    #[test]
    fn direct_load_completes() {
        let cmd = rgb_cmd(31, Medium::Direct, false);
        let loading = LoadingImage::init(&cmd, &Limits::direct(), MAX_SIZE, MAX_DIMENSION).unwrap();
        let img = loading.complete().unwrap();
        assert_eq!(img.id, 31);
        assert_eq!((img.width, img.height), (20, 15));
        assert_eq!(img.format, ImageFormat::Rgb);
        assert_eq!(img.data.bytes().unwrap().len(), 900);
    }

    #[test]
    fn direct_load_chunked() {
        // The first chunk of a chunked transmission carries only part of
        // the payload; the rest arrives via add_data.
        let mut cmd = rgb_cmd(31, Medium::Direct, true);
        cmd.data = RGB_20X15[..300].to_vec();
        let mut loading =
            LoadingImage::init(&cmd, &Limits::direct(), MAX_SIZE, MAX_DIMENSION).unwrap();
        loading.add_data(&RGB_20X15[300..600]).unwrap();
        loading.add_data(&RGB_20X15[600..]).unwrap();
        let img = loading.complete().unwrap();
        assert_eq!(img.data.bytes().unwrap().len(), 900);
    }

    #[test]
    fn data_length_mismatch_rejected() {
        let mut cmd = rgb_cmd(31, Medium::Direct, false);
        cmd.data = RGB_20X15[..100].to_vec();
        let loading = LoadingImage::init(&cmd, &Limits::direct(), MAX_SIZE, MAX_DIMENSION).unwrap();
        assert_eq!(loading.complete(), Err(ImageError::InvalidData));
    }

    #[test]
    fn dimensions_required() {
        // No width/height on the wire: completion must reject with
        // DimensionsRequired (checked before the length check, oracle
        // order).
        let cmd = Command {
            control: Control::Transmit(Transmission {
                format: ImageFormat::Rgb,
                medium: Medium::Direct,
                image_id: 31,
                ..Transmission::default()
            }),
            quiet: Quiet::No,
            data: Vec::new(),
        };
        let loading = LoadingImage::init(&cmd, &Limits::direct(), MAX_SIZE, MAX_DIMENSION).unwrap();
        assert_eq!(loading.complete(), Err(ImageError::DimensionsRequired));
    }

    #[test]
    fn dimensions_too_large() {
        let cmd = Command {
            control: Control::Transmit(Transmission {
                format: ImageFormat::Rgb,
                medium: Medium::Direct,
                width: 10_001,
                height: 1,
                image_id: 31,
                ..Transmission::default()
            }),
            quiet: Quiet::No,
            data: vec![0u8; 30_003],
        };
        let loading = LoadingImage::init(&cmd, &Limits::direct(), MAX_SIZE, MAX_DIMENSION).unwrap();
        assert_eq!(loading.complete(), Err(ImageError::DimensionsTooLarge));
    }

    #[test]
    fn add_data_respects_max_size() {
        // First chunk must fit the bound; the second chunk exceeds it.
        let mut cmd = rgb_cmd(31, Medium::Direct, true);
        cmd.data = RGB_20X15[..60].to_vec();
        let mut loading = LoadingImage::init(&cmd, &Limits::direct(), 64, MAX_DIMENSION).unwrap();
        assert_eq!(loading.add_data(&[0u8; 100]), Err(ImageError::InvalidData));
    }

    #[test]
    fn file_medium_reads_bounded() {
        let dir = tempfile_dir();
        let path = dir.join("image.data");
        fs::write(&path, RGB_20X15).unwrap();
        let cmd = Command {
            control: Control::Transmit(Transmission {
                format: ImageFormat::Rgb,
                medium: Medium::File,
                width: 20,
                height: 15,
                image_id: 31,
                ..Transmission::default()
            }),
            quiet: Quiet::No,
            data: path.to_string_lossy().as_bytes().to_vec(),
        };
        let loading = LoadingImage::init(
            &cmd,
            &Limits::all_with_temp_dir(dir.clone()),
            MAX_SIZE,
            MAX_DIMENSION,
        )
        .unwrap();
        let img = loading.complete().unwrap();
        assert_eq!(img.data.bytes().unwrap().len(), 900);
        // A plain `file` medium must NOT delete the file.
        assert!(path.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_medium_rejects_blocklisted_paths() {
        // /proc and /sys are rejected outright; /dev is rejected unless the
        // path is under /dev/shm.
        let cases: &[&str] = &["/proc/self/maps", "/sys/kernel/version", "/dev/zero"];
        for p in cases {
            let cmd = Command {
                control: Control::Transmit(Transmission {
                    format: ImageFormat::Rgb,
                    medium: Medium::File,
                    width: 1,
                    height: 1,
                    image_id: 31,
                    ..Transmission::default()
                }),
                quiet: Quiet::No,
                data: p.as_bytes().to_vec(),
            };
            let result = LoadingImage::init(
                &cmd,
                &Limits::all_with_temp_dir(std::env::temp_dir()),
                MAX_SIZE,
                MAX_DIMENSION,
            );
            assert!(result.is_err(), "expected rejection for {p}");
        }
    }

    #[test]
    fn temporary_file_rejected_outside_temp_dir() {
        let dir = tempfile_dir();
        let outside = dir.join("tty-graphics-protocol-image.data");
        fs::write(&outside, RGB_20X15).unwrap();
        let trusted = tempfile_dir();

        let cmd = Command {
            control: Control::Transmit(Transmission {
                format: ImageFormat::Rgb,
                medium: Medium::TemporaryFile,
                width: 20,
                height: 15,
                image_id: 31,
                ..Transmission::default()
            }),
            quiet: Quiet::No,
            data: outside.to_string_lossy().as_bytes().to_vec(),
        };
        let result = LoadingImage::init(
            &cmd,
            &Limits::all_with_temp_dir(trusted.clone()),
            MAX_SIZE,
            MAX_DIMENSION,
        );
        assert_eq!(result, Err(ImageError::TemporaryFileNotInTempDir));
        // Rejection happens before cleanup is armed: file still exists.
        assert!(outside.exists());
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&trusted);
    }

    #[test]
    fn temporary_file_rejected_wrong_name() {
        let dir = tempfile_dir();
        let path = dir.join("image.data");
        fs::write(&path, RGB_20X15).unwrap();
        let cmd = Command {
            control: Control::Transmit(Transmission {
                format: ImageFormat::Rgb,
                medium: Medium::TemporaryFile,
                width: 20,
                height: 15,
                image_id: 31,
                ..Transmission::default()
            }),
            quiet: Quiet::No,
            data: path.to_string_lossy().as_bytes().to_vec(),
        };
        let result = LoadingImage::init(
            &cmd,
            &Limits::all_with_temp_dir(dir.clone()),
            MAX_SIZE,
            MAX_DIMENSION,
        );
        assert_eq!(result, Err(ImageError::TemporaryFileNotNamedCorrectly));
        assert!(path.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn temporary_file_loaded_and_deleted() {
        let dir = tempfile_dir();
        let path = dir.join("tty-graphics-protocol-image.data");
        fs::write(&path, RGB_20X15).unwrap();
        let cmd = Command {
            control: Control::Transmit(Transmission {
                format: ImageFormat::Rgb,
                medium: Medium::TemporaryFile,
                width: 20,
                height: 15,
                image_id: 31,
                ..Transmission::default()
            }),
            quiet: Quiet::No,
            data: path.to_string_lossy().as_bytes().to_vec(),
        };
        let loading = LoadingImage::init(
            &cmd,
            &Limits::all_with_temp_dir(dir.clone()),
            MAX_SIZE,
            MAX_DIMENSION,
        )
        .unwrap();
        let img = loading.complete().unwrap();
        assert_eq!(img.data.bytes().unwrap().len(), 900);
        // The temporary file must have been consumed by the load.
        assert!(!path.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn temporary_file_with_offset_and_size() {
        let dir = tempfile_dir();
        let path = dir.join("tty-graphics-protocol-image.data");
        fs::write(&path, RGB_20X15).unwrap();
        let cmd = Command {
            control: Control::Transmit(Transmission {
                format: ImageFormat::Rgb,
                medium: Medium::TemporaryFile,
                width: 20,
                height: 15,
                offset: 3,
                size: 900,
                image_id: 31,
                ..Transmission::default()
            }),
            quiet: Quiet::No,
            data: path.to_string_lossy().as_bytes().to_vec(),
        };
        let loading = LoadingImage::init(
            &cmd,
            &Limits::all_with_temp_dir(dir.clone()),
            MAX_SIZE,
            MAX_DIMENSION,
        )
        .unwrap();
        // Offset 3 + size 900 exceeds the file: only 897 bytes are read and
        // the length check rejects the transmission.
        assert_eq!(loading.complete(), Err(ImageError::InvalidData));
        assert!(!path.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn shared_memory_range_semantics() {
        let loading = LoadingImage {
            image: Image {
                id: 0,
                number: 0,
                width: 1,
                height: 1,
                format: ImageFormat::Rgb,
                compression: Compression::None,
                data: ImageData::Pending(0),
                transient: false,
                implicit_id: false,
                placement_count: 0,
                generation: 0,
            },
            data: Vec::new(),
            display: None,
            quiet: Quiet::No,
            temporary_directory: None,
            max_size: MAX_SIZE,
            max_dimension: MAX_DIMENSION,
        };
        let t = Transmission {
            offset: 2,
            size: 3,
            ..Transmission::default()
        };
        assert_eq!(loading.shared_memory_range(t, 5).unwrap(), 2..5);
        let t = Transmission {
            offset: 2,
            ..Transmission::default()
        };
        // rgb 1x1 expected = 3 bytes; no explicit size -> expected size.
        assert_eq!(loading.shared_memory_range(t, 5).unwrap(), 2..5);
        // Out of bounds offset.
        let t = Transmission {
            offset: 4,
            ..Transmission::default()
        };
        assert!(loading.shared_memory_range(t, 3).is_err());
        // Dimensions validated before multiplication.
        let big = LoadingImage {
            image: Image {
                width: 20_000,
                height: 20_000,
                ..loading.image.clone()
            },
            ..loading
        };
        assert_eq!(
            big.shared_memory_range(Transmission::default(), 1),
            Err(ImageError::DimensionsTooLarge)
        );
    }

    #[cfg(all(unix, feature = "shm"))]
    #[test]
    fn shared_memory_transport_reads_and_unlinks() {
        let name = format!(
            "/mr-crabs-s7-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        );
        let cname = std::ffi::CString::new(name.clone()).unwrap();

        // Create and fill the object. macOS/BSD shared-memory objects do
        // not support write(); the payload is placed via mmap, mirroring
        // the transport under test.
        let fd = unsafe { libc::shm_open(cname.as_ptr(), libc::O_CREAT | libc::O_RDWR, 0o600) };
        assert!(fd >= 0, "shm_open failed");
        assert_eq!(unsafe { libc::ftruncate(fd, RGB_20X15.len() as i64) }, 0);
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                RGB_20X15.len(),
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        assert!(map != libc::MAP_FAILED, "mmap failed");
        unsafe {
            std::ptr::copy_nonoverlapping(RGB_20X15.as_ptr(), map as *mut u8, RGB_20X15.len());
            libc::munmap(map, RGB_20X15.len());
            libc::close(fd);
        }

        let cmd = Command {
            control: Control::Transmit(Transmission {
                format: ImageFormat::Rgb,
                medium: Medium::SharedMemory,
                width: 20,
                height: 15,
                image_id: 31,
                ..Transmission::default()
            }),
            quiet: Quiet::No,
            data: name.as_bytes().to_vec(),
        };
        let loading = LoadingImage::init(
            &cmd,
            &Limits::all_with_temp_dir(std::env::temp_dir()),
            MAX_SIZE,
            MAX_DIMENSION,
        )
        .unwrap();
        let img = loading.complete().unwrap();
        assert_eq!(img.data.bytes().unwrap().len(), 900);

        // The object must have been unlinked by the transport.
        let fd2 = unsafe { libc::shm_open(cname.as_ptr(), libc::O_RDONLY, 0) };
        assert!(fd2 < 0, "shared memory object was not unlinked");
    }

    #[test]
    fn png_load_decodes_to_rgba() {
        let png: &[u8] = include_bytes!(
            "../../../../verification/graphics-corpus/fixtures/image-png-none-50x76-2147483647-raw.data"
        );
        let cmd = Command {
            control: Control::Transmit(Transmission {
                format: ImageFormat::Png,
                medium: Medium::Direct,
                image_id: 7,
                ..Transmission::default()
            }),
            quiet: Quiet::No,
            data: png.to_vec(),
        };
        let loading = LoadingImage::init(&cmd, &Limits::direct(), MAX_SIZE, MAX_DIMENSION).unwrap();
        let img = loading.complete().unwrap();
        assert_eq!((img.width, img.height), (50, 76));
        assert_eq!(img.format, ImageFormat::Rgba);
        assert_eq!(img.data.bytes().unwrap().len(), 50 * 76 * 4);
    }

    #[test]
    fn zlib_load_decompresses() {
        let z: &[u8] = include_bytes!(
            "../../../../verification/graphics-corpus/fixtures/image-rgb-zlib_deflate-128x96-2147483647-raw.data"
        );
        let cmd = Command {
            control: Control::Transmit(Transmission {
                format: ImageFormat::Rgb,
                medium: Medium::Direct,
                compression: Compression::ZlibDeflate,
                width: 128,
                height: 96,
                image_id: 31,
                ..Transmission::default()
            }),
            quiet: Quiet::No,
            data: z.to_vec(),
        };
        let loading = LoadingImage::init(&cmd, &Limits::direct(), MAX_SIZE, MAX_DIMENSION).unwrap();
        let img = loading.complete().unwrap();
        assert_eq!(img.compression, Compression::None);
        assert_eq!(img.data.bytes().unwrap().len(), 128 * 96 * 3);
    }

    /// Create a unique temporary directory for transport tests.
    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mr-crabs-graphics-s7-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
