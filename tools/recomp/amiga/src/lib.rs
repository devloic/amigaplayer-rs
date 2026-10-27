//! Amiga file formats needed by the recompiler: hunk executables, ADF disk images and
//! Rocket Ranger's KBlocks virtual directory. Pure Rust, no dependencies.

pub mod adf;
pub mod hunk;
pub mod kblocks;

/// Big-endian u32 at `off` (panics if out of range; callers bounds-check first).
#[inline]
pub(crate) fn be32(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
