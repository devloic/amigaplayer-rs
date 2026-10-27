//! Rocket Ranger (1988): the "KBlocks.0" / "KBlocks.1" files on each floppy.
//!
//! They are not real OFS files: the AmigaDOS header only reserves a run of contiguous sectors (no valid
//! OFS data blocks) which the game reads raw. Layout of a run:
//!
//! ```text
//! run + 0x000   N names, 4 ASCII bytes each, NUL-padded when short ("V1\0\0"), then mastering junk
//! run + 0x800   N+1 big-endian u32: ABSOLUTE disk byte offsets; entry i = [pos[i], pos[i+1])
//! ```
//!
//! N is not stored: it is the count of leading name-like words, reduced until the offset table is
//! non-decreasing and inside the run. Check: pos[N] - pos[0] equals the header's byte size.
//! The game looks names up per volume, volume = 2 * disk + run (0..=3); a few names exist on several
//! volumes (TSAN, TSBK, FDSL).

use crate::adf::{Adf, BS};
use crate::be32;

const DIR_OFFSETS: usize = 0x800;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KEntry {
    pub name: String,
    /// 2 * disk + run (0..=3).
    pub volume: u32,
    /// Absolute byte offset in the ADF image of disk `volume / 2`.
    pub disk_offset: u32,
    pub size: u32,
}

impl KEntry {
    /// The entry's bytes, given the image of its disk.
    pub fn data<'a>(&self, disk_img: &'a [u8]) -> &'a [u8] {
        &disk_img[self.disk_offset as usize..(self.disk_offset + self.size) as usize]
    }
}

/// One KBlocks.N file of one disk.
#[derive(Clone, Debug)]
pub struct Run {
    /// 0 or 1 (from the file name).
    pub run: u32,
    pub first_block: u32,
    pub nblocks: u32,
    /// Byte size written in the AmigaDOS header.
    pub header_size: u32,
    pub entries: Vec<KEntry>,
}

impl Run {
    /// Sum of entry sizes (should equal `header_size`).
    pub fn total(&self) -> u32 {
        self.entries.iter().map(|e| e.size).sum()
    }
}

/// Printable ASCII, NUL-padded on the right ("TX-1", "$AFA", "V1\0\0").
fn is_name(b: &[u8]) -> bool {
    let t = match b.iter().position(|&c| c == 0) {
        Some(p) => {
            if b[p..].iter().any(|&c| c != 0) {
                return false;
            }
            &b[..p]
        }
        None => b,
    };
    !t.is_empty() && t.iter().all(|&c| (0x21..=0x7E).contains(&c))
}

/// Directory of the run whose AmigaDOS file is `KBlocks.<run>` on disk `disk` (0 = Disk.1).
pub fn run(adf: &Adf, disk: u32, run: u32) -> Result<Run, String> {
    let fname = format!("KBlocks.{run}");
    let hdr = adf.find(&fname)?;
    let blocks = adf.data_blocks(hdr)?;
    let first = *blocks.first().ok_or_else(|| format!("{fname}: no sectors"))?;
    if blocks.iter().enumerate().any(|(i, &b)| b != first + i as u32) {
        return Err(format!("{fname}: sectors are not contiguous"));
    }
    let (start, end) = (first as usize * BS, (first as usize + blocks.len()) * BS);
    let raw = adf.img.get(start..end).ok_or_else(|| format!("{fname}: run outside the disk"))?;
    if raw.len() < DIR_OFFSETS + 8 {
        return Err(format!("{fname}: run too short for a directory"));
    }
    let mut n = 0;
    while 4 * n < DIR_OFFSETS && is_name(&raw[4 * n..4 * n + 4]) {
        n += 1;
    }
    let pos = loop {
        if n == 0 {
            return Err(format!("{fname}: no consistent offset table"));
        }
        if DIR_OFFSETS + 4 * (n + 1) <= raw.len() {
            let pos: Vec<usize> = (0..=n).map(|i| be32(raw, DIR_OFFSETS + 4 * i) as usize).collect();
            if pos.windows(2).all(|w| start <= w[0] && w[0] <= w[1] && w[1] <= end) {
                break pos;
            }
        }
        n -= 1;
    };
    let volume = 2 * disk + run;
    let entries = (0..n)
        .map(|i| {
            let nm = &raw[4 * i..4 * i + 4];
            let nm = &nm[..nm.iter().position(|&c| c == 0).unwrap_or(4)];
            KEntry {
                name: nm.iter().map(|&c| c as char).collect(),
                volume,
                disk_offset: pos[i] as u32,
                size: (pos[i + 1] - pos[i]) as u32,
            }
        })
        .collect();
    Ok(Run { run, first_block: first, nblocks: blocks.len() as u32, header_size: adf.file_size(hdr)?, entries })
}

/// All KBlocks runs present on disk `disk` (KBlocks.0, KBlocks.1; missing ones are skipped).
pub fn runs(adf: &Adf, disk: u32) -> Result<Vec<Run>, String> {
    (0..2).filter(|r| adf.find(&format!("KBlocks.{r}")).is_ok()).map(|r| run(adf, disk, r)).collect()
}

/// Every KBlocks entry of the given disk images (index in `disks` = disk number), with its bytes:
/// (name, volume, data).
pub fn entries(disks: &[&[u8]]) -> Result<Vec<(String, u32, Vec<u8>)>, String> {
    let mut out = Vec::new();
    for (d, img) in disks.iter().enumerate() {
        let adf = Adf::new(img)?;
        for r in runs(&adf, d as u32)? {
            out.extend(r.entries.into_iter().map(|e| {
                let data = e.data(img).to_vec();
                (e.name, e.volume, data)
            }));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_words() {
        assert!(is_name(b"TSBK") && is_name(b"V1\0\0") && is_name(b"$AFA"));
        assert!(!is_name(b"\0\0\0\0") && !is_name(b"A\0B\0") && !is_name(b"AB \x01"));
    }
}
