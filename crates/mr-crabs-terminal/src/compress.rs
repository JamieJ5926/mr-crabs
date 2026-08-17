//! LZ4 block compression for paged scrollback cells.
//!
//! Cells are 8-byte `#[repr(C)]` values; compression operates on their raw
//! byte representation to guarantee byte-identical roundtrips. We use
//! `lz4_flex::block` (raw block, not frame) so the caller supplies the
//! exact output buffer size on decompression.

use crate::Cell;

/// Compress a slice of `Cell` into an LZ4 block.
///
/// The input is reinterpreted as `len * 8` bytes. The output is a
/// self-contained LZ4 block (no prepended size); decompression requires
/// the caller to know the uncompressed size (provided via `out.len()`).
pub fn compress_page(cells: &[Cell]) -> Vec<u8> {
    let bytes = unsafe {
        // SAFETY: Cell is #[repr(C)] 8 bytes, no padding; as_ptr is valid for cells.len()*8 bytes,
        // lifetime tied to `cells`, alignment of u8 is 1 so reinterpretation is safe for byte view.
        std::slice::from_raw_parts(cells.as_ptr().cast::<u8>(), std::mem::size_of_val(cells))
    };
    lz4_flex::block::compress(bytes)
}

/// Decompress an LZ4 block into the provided `Cell` buffer.
///
/// `compressed` must be a block produced by [`compress_page`] and `out` must
/// have the original length. Returns the underlying LZ4 error on corruption.
pub fn decompress_page(
    compressed: &[u8],
    out: &mut [Cell],
) -> Result<(), lz4_flex::block::DecompressError> {
    let out_bytes = unsafe {
        // SAFETY: Cell is #[repr(C)] 8 bytes; out is valid for len*8 bytes; alignment of u8 is less
        // restrictive than Cell, so casting Cell pointer to u8 is allowed; bounds are exact by size_of_val.
        std::slice::from_raw_parts_mut(out.as_mut_ptr().cast::<u8>(), std::mem::size_of_val(out))
    };
    // lz4_flex 0.11 block API: decompress_into returns written bytes.
    let written = lz4_flex::block::decompress_into(compressed, out_bytes)?;
    debug_assert_eq!(written, out_bytes.len(), "lz4 decompressed size mismatch");
    Ok(())
}

/// Compress raw bytes (used by the background worker when the job payload is
/// already serialized as `Vec<u8>`).
pub fn compress_bytes(input: &[u8]) -> Vec<u8> {
    lz4_flex::block::compress(input)
}

/// Compress raw bytes into `scratch` (reusing its allocation across calls)
/// and return the compressed block as an owned `Vec`.
///
/// The scratch is sized once to the LZ4 worst-case bound; the returned
/// `Vec` holds only the actual compressed bytes, so per-job allocation is
/// the small compressed size instead of a worst-case page-sized buffer.
pub fn compress_bytes_reuse(input: &[u8], scratch: &mut Vec<u8>) -> Vec<u8> {
    let bound = lz4_flex::block::get_maximum_output_size(input.len());
    if scratch.capacity() < bound {
        scratch.reserve(bound - scratch.capacity());
    }
    scratch.resize(bound, 0);
    match lz4_flex::block::compress_into(input, scratch) {
        Ok(written) => {
            scratch.truncate(written);
            scratch.clone()
        }
        Err(_) => {
            // `CompressError` is only `OutputTooSmall`, which the
            // worst-case bound excludes; fall back to the self-contained
            // path rather than fabricate output.
            scratch.clear();
            lz4_flex::block::compress(input)
        }
    }
}

/// Decompress raw bytes into a caller-supplied buffer.
pub fn decompress_bytes(
    compressed: &[u8],
    out: &mut [u8],
) -> Result<usize, lz4_flex::block::DecompressError> {
    lz4_flex::block::decompress_into(compressed, out)
}
