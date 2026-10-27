//! AmigaOS hunk executable parser (the format of DeliPlayers and genies).

use anyhow::{bail, Context, Result};

const HUNK_CODE: u32 = 0x3E9;
const HUNK_DATA: u32 = 0x3EA;
const HUNK_BSS: u32 = 0x3EB;
const HUNK_RELOC32: u32 = 0x3EC;
const HUNK_SYMBOL: u32 = 0x3F0;
const HUNK_DEBUG: u32 = 0x3F1;
const HUNK_END: u32 = 0x3F2;
const HUNK_HEADER: u32 = 0x3F3;
const HUNK_DREL32: u32 = 0x3F7;
const HUNK_RELOC32SHORT: u32 = 0x3FC;

pub struct Hunk {
    /// Memory image (code/data followed by zero fill up to the allocated size).
    pub data: Vec<u8>,
    /// (offset inside this hunk, target hunk index)
    pub relocs: Vec<(u32, u32)>,
}

pub struct HunkFile {
    pub hunks: Vec<Hunk>,
}

struct Reader<'a> {
    d: &'a [u8],
    p: usize,
}

impl Reader<'_> {
    fn long(&mut self) -> Result<u32> {
        let b = self.d.get(self.p..self.p + 4).context("truncated hunk file")?;
        self.p += 4;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn word(&mut self) -> Result<u16> {
        let b = self.d.get(self.p..self.p + 2).context("truncated hunk file")?;
        self.p += 2;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn bytes(&mut self, n: usize) -> Result<&[u8]> {
        let b = self.d.get(self.p..self.p + n).context("truncated hunk file")?;
        self.p += n;
        Ok(b)
    }
}

pub fn is_hunk_file(d: &[u8]) -> bool {
    d.len() >= 4 && u32::from_be_bytes([d[0], d[1], d[2], d[3]]) == HUNK_HEADER
}

pub fn parse(d: &[u8]) -> Result<HunkFile> {
    let mut r = Reader { d, p: 0 };
    if r.long()? != HUNK_HEADER {
        bail!("not an AmigaOS executable");
    }
    // resident library names (obsolete), terminated by 0
    loop {
        let n = r.long()?;
        if n == 0 {
            break;
        }
        r.bytes(n as usize * 4)?;
    }
    let _table = r.long()?;
    let first = r.long()?;
    let last = r.long()?;
    if last < first || last - first > 256 {
        bail!("bad hunk table");
    }
    let mut sizes = Vec::new();
    for _ in first..=last {
        let s = r.long()?;
        if s & 0xC000_0000 == 0xC000_0000 {
            r.long()?; // extended memory attributes
        }
        sizes.push(((s & 0x3FFF_FFFF) * 4) as usize);
    }
    let mut hunks: Vec<Hunk> = Vec::new();
    while r.p + 4 <= d.len() {
        let t = r.long()? & 0x3FFF_FFFF;
        match t {
            HUNK_CODE | HUNK_DATA | HUNK_BSS => {
                let idx = hunks.len();
                let alloc = *sizes.get(idx).context("more hunks than declared")?;
                let n = r.long()? as usize * 4;
                let mut data = vec![0u8; alloc.max(n)];
                if t != HUNK_BSS {
                    data[..n].copy_from_slice(r.bytes(n)?);
                }
                hunks.push(Hunk { data, relocs: Vec::new() });
            }
            HUNK_RELOC32 => {
                let h = hunks.last_mut().context("reloc before hunk")?;
                loop {
                    let n = r.long()?;
                    if n == 0 {
                        break;
                    }
                    let target = r.long()?;
                    for _ in 0..n {
                        h.relocs.push((r.long()?, target));
                    }
                }
            }
            HUNK_RELOC32SHORT | HUNK_DREL32 => {
                let h = hunks.last_mut().context("reloc before hunk")?;
                let start = r.p;
                loop {
                    let n = r.word()?;
                    if n == 0 {
                        break;
                    }
                    let target = r.word()? as u32;
                    for _ in 0..n {
                        h.relocs.push((r.word()? as u32, target));
                    }
                }
                if (r.p - start) & 2 != 0 {
                    r.word()?;
                }
            }
            HUNK_SYMBOL => loop {
                let n = r.long()?;
                if n == 0 {
                    break;
                }
                r.bytes(n as usize * 4 + 4)?;
            },
            HUNK_DEBUG => {
                let n = r.long()? as usize;
                r.bytes(n * 4)?;
            }
            HUNK_END => {}
            _ => bail!("unsupported hunk type ${t:x}"),
        }
    }
    if hunks.len() != sizes.len() {
        bail!("hunk count mismatch ({} of {})", hunks.len(), sizes.len());
    }
    Ok(HunkFile { hunks })
}
