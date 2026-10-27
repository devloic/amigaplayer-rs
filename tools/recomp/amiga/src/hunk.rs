//! AmigaDOS "hunk" executables (LoadSeg format): sections, relocations, symbols.
//!
//! ```text
//! let h = amiga::hunk::parse(&bytes)?;
//! let images = h.relocate(&[0x10000, 0x20000, 0x30000]); // one base address per section
//! ```

use crate::be32;

pub const HUNK_NAME: u32 = 0x3E8;
pub const HUNK_CODE: u32 = 0x3E9;
pub const HUNK_DATA: u32 = 0x3EA;
pub const HUNK_BSS: u32 = 0x3EB;
pub const HUNK_RELOC32: u32 = 0x3EC;
pub const HUNK_EXT: u32 = 0x3EF;
pub const HUNK_SYMBOL: u32 = 0x3F0;
pub const HUNK_DEBUG: u32 = 0x3F1;
pub const HUNK_END: u32 = 0x3F2;
pub const HUNK_HEADER: u32 = 0x3F3;
/// Officially HUNK_DREL32, but LoadSeg treats it as RELOC32SHORT in executables.
pub const HUNK_DREL32: u32 = 0x3F7;
pub const HUNK_RELOC32SHORT: u32 = 0x3FC;
pub const HUNK_OVERLAY: u32 = 0x3F5;
pub const HUNK_BREAK: u32 = 0x3F6;

/// Overlay information of an overlaid executable: the root node's hunks, then HUNK_OVERLAY with
/// the overlay table, then the overlay nodes, each a HUNK_HEADER (with global hunk numbers)
/// followed by its hunks and a HUNK_BREAK.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overlay {
    /// The overlay table as stored in the file: the TableSize + 1 longs after the size long
    /// (amitools reads the same `(size + 1) * 4` bytes).
    pub table: Vec<u32>,
    /// File offset of the HUNK_OVERLAY long.
    pub table_file_offset: usize,
    /// One entry per overlay node: (file offset of its HUNK_HEADER, first hunk, last hunk).
    pub nodes: Vec<(usize, u32, u32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Code,
    Data,
    Bss,
}

/// Memory requirement from the two high bits of a hunk size (bit 30 = chip, bit 31 = fast).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mem {
    Any,
    Chip,
    Fast,
}

#[derive(Clone, Debug)]
pub struct Section {
    pub index: usize,
    pub kind: Kind,
    /// Bytes reserved in memory (from HUNK_HEADER), BSS tail included.
    pub alloc: u32,
    pub mem: Mem,
    /// Initialised bytes (empty for BSS).
    pub data: Vec<u8>,
    /// (target section, offsets in this section of the longs to which the target's base is added),
    /// one entry per target in file order (a target may appear more than once).
    pub relocs: Vec<(u32, Vec<u32>)>,
    /// (name, offset in this section).
    pub symbols: Vec<(String, u32)>,
}

impl Section {
    /// Total number of relocated longs in this section.
    pub fn reloc_count(&self) -> usize {
        self.relocs.iter().map(|(_, o)| o.len()).sum()
    }
}

#[derive(Clone, Debug)]
pub struct Hunk {
    pub sections: Vec<Section>,
    /// Number of root (always resident) sections; `sections.len()` without overlays.
    pub root: usize,
    pub overlay: Option<Overlay>,
}

impl Hunk {
    /// Image of every section, padded with zeroes to `alloc`, each relocated long += bases[target].
    pub fn relocate(&self, bases: &[u32]) -> Vec<Vec<u8>> {
        self.sections
            .iter()
            .map(|s| {
                let mut b = s.data.clone();
                b.resize((s.alloc as usize).max(b.len()), 0);
                for (tgt, offs) in &s.relocs {
                    let base = bases[*tgt as usize];
                    for &o in offs {
                        let o = o as usize;
                        let v = be32(&b, o).wrapping_add(base);
                        b[o..o + 4].copy_from_slice(&v.to_be_bytes());
                    }
                }
                b
            })
            .collect()
    }

    /// Every relocated long after relocation: (absolute address of the long, value it now holds,
    /// target section), sorted by address.
    pub fn reloc_targets(&self, bases: &[u32]) -> Vec<(u32, u32, usize)> {
        let imgs = self.relocate(bases);
        let mut out = Vec::new();
        for (s, img) in self.sections.iter().zip(&imgs) {
            for (tgt, offs) in &s.relocs {
                for &o in offs {
                    out.push((bases[s.index].wrapping_add(o), be32(img, o as usize), *tgt as usize));
                }
            }
        }
        out.sort_unstable();
        out
    }
}

/// Bounds-checked big-endian reader over the file.
struct Rd<'a> {
    b: &'a [u8],
    p: usize,
}

impl Rd<'_> {
    fn l(&mut self) -> Result<u32, String> {
        if self.p + 4 > self.b.len() {
            return Err(format!("unexpected end of file at {:#x}", self.p));
        }
        let v = be32(self.b, self.p);
        self.p += 4;
        Ok(v)
    }
    fn w(&mut self) -> Result<u32, String> {
        if self.p + 2 > self.b.len() {
            return Err(format!("unexpected end of file at {:#x}", self.p));
        }
        let v = u16::from_be_bytes([self.b[self.p], self.b[self.p + 1]]) as u32;
        self.p += 2;
        Ok(v)
    }
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        if self.p + n > self.b.len() {
            return Err(format!("block of {n} bytes at {:#x} runs past end of file", self.p));
        }
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    fn skip_longs(&mut self, n: u32) -> Result<(), String> {
        self.take(4 * n as usize).map(|_| ())
    }
}

fn mem_of(size: u32) -> Mem {
    match size >> 30 {
        1 => Mem::Chip,
        2 => Mem::Fast,
        _ => Mem::Any,
    }
}

/// Parse an AmigaDOS executable. Unknown hunk types are an error.
pub fn parse(b: &[u8]) -> Result<Hunk, String> {
    let mut r = Rd { b, p: 0 };
    if r.l()? != HUNK_HEADER {
        return Err("no HUNK_HEADER".into());
    }
    // Resident library names (always empty in practice).
    loop {
        let n = r.l()?;
        if n == 0 {
            break;
        }
        r.skip_longs(n)?;
    }
    let (_table, first, last) = (r.l()?, r.l()?, r.l()?);
    if last < first || last - first > 0xFFFF {
        return Err(format!("bad hunk range {first}..={last}"));
    }
    let mut sizes = Vec::new();
    for _ in first..=last {
        let s = r.l()?;
        if s >> 30 == 3 {
            r.l()?; // extended memory attributes long; treat as "any"
        }
        sizes.push(s);
    }

    let mut overlay: Option<Overlay> = None;
    let mut root: Option<usize> = None;
    let mut secs: Vec<Section> = Vec::new();
    let mut cur: Option<usize> = None; // section receiving relocs/symbols
    let need = |cur: Option<usize>, what: &str| cur.ok_or_else(|| format!("{what} outside a section"));
    while r.p < b.len() {
        let t = r.l()? & 0x3FFF_FFFF;
        match t {
            HUNK_CODE | HUNK_DATA | HUNK_BSS => {
                let n = r.l()? & 0x3FFF_FFFF;
                let i = secs.len();
                let size = *sizes.get(i).ok_or_else(|| format!("more hunks than the header's {}", sizes.len()))?;
                let kind = match t {
                    HUNK_CODE => Kind::Code,
                    HUNK_DATA => Kind::Data,
                    _ => Kind::Bss,
                };
                let data = if t == HUNK_BSS { Vec::new() } else { r.take(4 * n as usize)?.to_vec() };
                let alloc = ((size & 0x3FFF_FFFF) * 4).max(data.len() as u32);
                secs.push(Section { index: i, kind, alloc, mem: mem_of(size), data, relocs: vec![], symbols: vec![] });
                cur = Some(i);
            }
            HUNK_RELOC32 => {
                let i = need(cur, "HUNK_RELOC32")?;
                loop {
                    let n = r.l()?;
                    if n == 0 {
                        break;
                    }
                    let tgt = r.l()?;
                    let offs = (0..n).map(|_| r.l()).collect::<Result<Vec<_>, _>>()?;
                    secs[i].relocs.push((tgt, offs));
                }
            }
            HUNK_RELOC32SHORT | HUNK_DREL32 => {
                let i = need(cur, "HUNK_RELOC32SHORT")?;
                loop {
                    let n = r.w()?;
                    if n == 0 {
                        break;
                    }
                    let tgt = r.w()?;
                    let offs = (0..n).map(|_| r.w()).collect::<Result<Vec<_>, _>>()?;
                    secs[i].relocs.push((tgt, offs));
                }
                r.p = (r.p + 3) & !3; // back to long alignment
            }
            HUNK_SYMBOL => {
                let i = need(cur, "HUNK_SYMBOL")?;
                loop {
                    let n = r.l()?;
                    if n == 0 {
                        break;
                    }
                    let raw = r.take(4 * n as usize)?;
                    let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
                    let name = raw[..end].iter().map(|&c| c as char).collect(); // latin-1
                    let off = r.l()?;
                    secs[i].symbols.push((name, off));
                }
            }
            HUNK_DEBUG | HUNK_NAME => {
                let n = r.l()?;
                r.skip_longs(n)?;
            }
            HUNK_EXT => loop {
                // Skipped: executables have no unresolved externals worth keeping.
                let h = r.l()?;
                if h == 0 {
                    break;
                }
                let (ty, len) = (h >> 24, h & 0x00FF_FFFF);
                r.skip_longs(len)?; // symbol name
                if ty < 128 {
                    r.l()?; // definition: value
                } else {
                    if ty == 130 {
                        r.l()?; // EXT_COMMON: common block size
                    }
                    let n = r.l()?;
                    r.skip_longs(n)?; // reference offsets
                }
            },
            HUNK_END => cur = None,
            HUNK_OVERLAY | HUNK_BREAK => {
                if t == HUNK_OVERLAY {
                    if overlay.is_some() {
                        return Err(format!("second HUNK_OVERLAY at {:#x}", r.p - 4));
                    }
                    let at = r.p - 4;
                    let n = r.l()?;
                    let table = (0..=n).map(|_| r.l()).collect::<Result<Vec<_>, _>>()?;
                    root = Some(secs.len());
                    overlay = Some(Overlay { table, table_file_offset: at, nodes: vec![] });
                } else if overlay.is_none() {
                    return Err(format!("HUNK_BREAK without HUNK_OVERLAY at {:#x}", r.p - 4));
                } else if r.p >= b.len() {
                    break; // the last node's HUNK_BREAK ends the file
                }
                // The next overlay node: its own HUNK_HEADER, numbered after the hunks so far.
                let at = r.p;
                if r.l()? != HUNK_HEADER {
                    return Err(format!("overlay node at {at:#x} has no HUNK_HEADER"));
                }
                loop {
                    let n = r.l()?;
                    if n == 0 {
                        break;
                    }
                    r.skip_longs(n)?;
                }
                let (_t, f, l) = (r.l()?, r.l()?, r.l()?);
                if l < f || f as usize != secs.len() || l - f > 0xFFFF {
                    return Err(format!("overlay node at {at:#x}: hunks {f}..={l}, expected to start at {}", secs.len()));
                }
                sizes.truncate(f as usize);
                for _ in f..=l {
                    let s = r.l()?;
                    if s >> 30 == 3 {
                        r.l()?;
                    }
                    sizes.push(s);
                }
                overlay.as_mut().unwrap().nodes.push((at, f, l));
                cur = None;
            }
            _ => return Err(format!("unknown hunk type {t:#x} at {:#x}", r.p - 4)),
        }
    }
    let root = root.unwrap_or(secs.len());
    Ok(Hunk { sections: secs, root, overlay })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn longs(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_be_bytes()).collect()
    }

    /// Synthetic 3-section executable: code (chip) with RELOC32 + symbols, data with RELOC32SHORT, BSS.
    fn sample() -> Vec<u8> {
        let mut f = longs(&[HUNK_HEADER, 0, 3, 0, 2, 2 | 0x4000_0000, 2, 4]);
        // Section 0: code, 2 longs; long at 4 points 0x10 into section 1... (value 0x10 is relative)
        f.extend(longs(&[HUNK_CODE, 2, 0x4E75_4E71, 0x0000_0010]));
        f.extend(longs(&[HUNK_RELOC32, 1, 1, 4, 0]));
        f.extend(longs(&[HUNK_SYMBOL, 2]));
        f.extend(b"_main\0\0\0");
        f.extend(longs(&[0, 0, HUNK_END]));
        // Section 1: data, 2 longs pointing into section 2, via RELOC32SHORT (5 words -> 2 pad bytes).
        f.extend(longs(&[HUNK_DATA, 2, 0x0000_0008, 0x0000_000C, HUNK_RELOC32SHORT]));
        f.extend([0, 2, 0, 2, 0, 0, 0, 4, 0, 0, 0, 0]); // n=2 tgt=2 offs 0,4, terminator, pad
        f.extend(longs(&[HUNK_END]));
        // Section 2: BSS, 4 longs.
        f.extend(longs(&[HUNK_NAME, 1]));
        f.extend(b"bss\0");
        f.extend(longs(&[HUNK_BSS, 4, HUNK_END]));
        f
    }

    #[test]
    fn parse_and_relocate() {
        let h = parse(&sample()).unwrap();
        assert_eq!(h.sections.len(), 3);
        let s0 = &h.sections[0];
        assert_eq!((s0.kind, s0.mem, s0.alloc, s0.data.len()), (Kind::Code, Mem::Chip, 8, 8));
        assert_eq!(s0.relocs, vec![(1, vec![4])]);
        assert_eq!(s0.symbols, vec![("_main".to_string(), 0)]);
        assert_eq!(h.sections[1].relocs, vec![(2, vec![0, 4])]);
        assert_eq!((h.sections[2].kind, h.sections[2].alloc, h.sections[2].data.len()), (Kind::Bss, 16, 0));

        let bases = [0x1000, 0x2000, 0x3000];
        let img = h.relocate(&bases);
        assert_eq!(img[0][4..8], 0x2010u32.to_be_bytes());
        assert_eq!(img[1], longs(&[0x3008, 0x300C]));
        assert_eq!(img[2], vec![0; 16]);
        assert_eq!(h.reloc_targets(&bases), vec![(0x1004, 0x2010, 1), (0x2000, 0x3008, 2), (0x2004, 0x300C, 2)]);
    }

    /// Overlaid executable: root (code + data), HUNK_OVERLAY with a 3-long table, two nodes of
    /// one code hunk each (global numbers 2 and 3), each ending with HUNK_BREAK.
    #[test]
    fn overlay_nodes() {
        let mut f = longs(&[HUNK_HEADER, 0, 4, 0, 1, 1, 1]);
        f.extend(longs(&[HUNK_CODE, 1, 0x4E75_4E71, HUNK_END]));
        f.extend(longs(&[HUNK_DATA, 1, 0x1234_5678, HUNK_END]));
        let tab = f.len();
        f.extend(longs(&[HUNK_OVERLAY, 2, 2, 0x100, 0x200]));
        let n1 = f.len();
        f.extend(longs(&[HUNK_HEADER, 0, 4, 2, 2, 2, HUNK_CODE, 2, 0x4E71_4E71, 0x4E75_0000, HUNK_END, HUNK_BREAK]));
        let n2 = f.len();
        f.extend(longs(&[HUNK_HEADER, 0, 4, 3, 3, 1, HUNK_CODE, 1, 0x7001_4E75]));
        f.extend(longs(&[HUNK_RELOC32, 1, 1, 0, 0, HUNK_END, HUNK_BREAK]));
        let h = parse(&f).unwrap();
        assert_eq!(h.sections.len(), 4);
        assert_eq!(h.root, 2);
        let ov = h.overlay.as_ref().unwrap();
        assert_eq!(ov.table, vec![2, 0x100, 0x200]);
        assert_eq!(ov.table_file_offset, tab);
        assert_eq!(ov.nodes, vec![(n1, 2, 2), (n2, 3, 3)]);
        assert_eq!((h.sections[2].index, h.sections[2].alloc), (2, 8));
        assert_eq!(h.sections[3].relocs, vec![(1, vec![0])]);
        // without overlays, root == number of sections
        assert_eq!(parse(&sample()).unwrap().root, 3);
        // a HUNK_BREAK with no HUNK_OVERLAY is an error
        let mut g = sample();
        g.extend(longs(&[HUNK_BREAK]));
        assert!(parse(&g).is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse(&longs(&[0x3E7])).is_err());
        let mut f = sample();
        f.extend(longs(&[0x3FF]));
        assert!(parse(&f).unwrap_err().contains("unknown hunk type"));
        assert!(parse(&sample()[..44]).is_err());
    }
}
