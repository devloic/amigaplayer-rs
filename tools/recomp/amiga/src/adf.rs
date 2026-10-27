//! AmigaDOS floppy images (.adf, raw 880 KB DD or 1.76 MB HD), OFS and FFS, read-only.
//!
//! The boot block is ignored on purpose: Rocket Ranger's Disk.2 has "DOS\0" repeated over the whole
//! block. Only the root block (middle of the disk, 880 on a DD disk) matters. OFS vs FFS is taken from
//! the boot block's DOS type when it is sane, else detected from the first data block.

use crate::be32;

pub const BS: usize = 512;
const T_HEADER: u32 = 2;
const T_DATA: u32 = 8;
const T_LIST: u32 = 16;
const ST_ROOT: i32 = 1;
const ST_USERDIR: i32 = 2;
const ST_FILE: i32 = -3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Path from the root, '/'-separated; directories end with '/'.
    pub path: String,
    pub is_dir: bool,
    /// File byte size from the header (0 for directories).
    pub size: u32,
    /// Header block number.
    pub block: u32,
}

pub struct Adf<'a> {
    pub img: &'a [u8],
    pub nblocks: u32,
    pub root: u32,
    pub ffs: bool,
    pub label: String,
}

impl<'a> Adf<'a> {
    pub fn new(img: &'a [u8]) -> Result<Self, String> {
        if img.len() != 901_120 && img.len() != 1_802_240 {
            return Err(format!("size {}: neither DD (901120) nor HD (1802240)", img.len()));
        }
        let nblocks = (img.len() / BS) as u32;
        let mut a = Adf { img, nblocks, root: nblocks / 2, ffs: false, label: String::new() };
        let r = a.block(a.root)?;
        if be32(r, 0) != T_HEADER || be32(r, BS - 4) as i32 != ST_ROOT {
            return Err(format!("block {}: not a root block", a.root));
        }
        a.label = name(r);
        a.ffs = &img[..3] == b"DOS" && img[3] & 1 == 1;
        Ok(a)
    }

    pub fn block(&self, n: u32) -> Result<&'a [u8], String> {
        if n >= self.nblocks {
            return Err(format!("block {n} is outside the disk"));
        }
        Ok(&self.img[n as usize * BS..(n as usize + 1) * BS])
    }

    /// (block number, block) of every entry hashed in a directory block (root or user dir).
    fn children(&self, dir: u32) -> Result<Vec<(u32, &'a [u8])>, String> {
        let b = self.block(dir)?;
        let mut out = Vec::new();
        for i in 0..72 {
            let mut n = be32(b, 24 + 4 * i);
            while n != 0 && !out.iter().any(|&(m, _)| m == n) {
                let h = self.block(n)?;
                out.push((n, h));
                n = be32(h, 0x1F0); // next in hash chain
            }
        }
        Ok(out)
    }

    /// Recursive listing, sorted case-insensitively within each directory.
    pub fn walk(&self) -> Result<Vec<Entry>, String> {
        let mut out = Vec::new();
        self.walk_into(self.root, "", &mut out, 0)?;
        Ok(out)
    }

    fn walk_into(&self, dir: u32, prefix: &str, out: &mut Vec<Entry>, depth: u32) -> Result<(), String> {
        if depth > 32 {
            return Err("directory tree too deep (loop?)".into());
        }
        let mut kids = self.children(dir)?;
        kids.sort_by_key(|(_, h)| name(h).to_lowercase());
        for (n, h) in kids {
            let path = format!("{prefix}{}", name(h));
            match be32(h, BS - 4) as i32 {
                ST_USERDIR => {
                    out.push(Entry { path: format!("{path}/"), is_dir: true, size: 0, block: n });
                    self.walk_into(n, &format!("{path}/"), out, depth + 1)?;
                }
                ST_FILE => out.push(Entry { path, is_dir: false, size: be32(h, 0x144), block: n }),
                _ => {}
            }
        }
        Ok(())
    }

    /// Header block of `path` (case-insensitive, as AmigaDOS).
    pub fn find(&self, path: &str) -> Result<u32, String> {
        let mut cur = self.root;
        for part in path.split('/').filter(|p| !p.is_empty()) {
            cur = self
                .children(cur)?
                .into_iter()
                .find(|(_, h)| name(h).eq_ignore_ascii_case(part))
                .map(|(n, _)| n)
                .ok_or_else(|| format!("{path}: not found"))?;
        }
        Ok(cur)
    }

    /// Byte size recorded in a file header.
    pub fn file_size(&self, header: u32) -> Result<u32, String> {
        Ok(be32(self.block(header)?, 0x144))
    }

    /// Data block numbers of a file, in file order (header table is stored backwards, 72 per block,
    /// then continued in extension blocks).
    pub fn data_blocks(&self, header: u32) -> Result<Vec<u32>, String> {
        let mut h = self.block(header)?;
        if be32(h, BS - 4) as i32 != ST_FILE {
            return Err(format!("block {header}: not a file header"));
        }
        let mut out = Vec::new();
        let mut seen = vec![header];
        loop {
            let cnt = be32(h, 8).min(72) as usize;
            out.extend((0..cnt).map(|i| be32(h, 24 + 4 * (71 - i))));
            let ext = be32(h, 0x1F8);
            if ext == 0 || seen.contains(&ext) {
                return Ok(out);
            }
            seen.push(ext);
            h = self.block(ext)?;
            if be32(h, 0) != T_LIST {
                return Err(format!("block {ext}: extension block expected"));
            }
        }
    }

    /// Contents of the file at `path`.
    pub fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        self.read_block(self.find(path)?)
    }

    /// Contents of the file whose header is block `header`.
    pub fn read_block(&self, header: u32) -> Result<Vec<u8>, String> {
        let size = self.file_size(header)? as usize;
        let blocks = self.data_blocks(header)?;
        // OFS data blocks carry a 24-byte header with type T_DATA; FFS blocks are raw. The boot block's
        // DOS type is unreliable on some masters, so trust the first data block when it disagrees.
        let ffs = match blocks.first() {
            Some(&d) => {
                let b = self.block(d)?;
                !(be32(b, 0) == T_DATA && be32(b, 4) == header)
            }
            None => self.ffs,
        };
        let mut out = Vec::with_capacity(size);
        for d in blocks {
            let b = self.block(d)?;
            if ffs {
                out.extend_from_slice(b);
            } else {
                let n = (be32(b, 12) as usize).min(BS - 24);
                out.extend_from_slice(&b[24..24 + n]);
            }
        }
        if out.len() < size {
            return Err(format!("block {header}: read {} of {size} bytes", out.len()));
        }
        out.truncate(size);
        Ok(out)
    }
}

/// BCPL name of a header/root block (length byte at 0x1B0).
fn name(b: &[u8]) -> String {
    let n = (b[0x1B0] as usize).min(30);
    b[0x1B1..0x1B1 + n].iter().map(|&c| c as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(img: &mut [u8], blk: usize, off: usize, v: u32) {
        img[blk * BS + off..blk * BS + off + 4].copy_from_slice(&v.to_be_bytes());
    }
    fn set_name(img: &mut [u8], blk: usize, s: &str) {
        img[blk * BS + 0x1B0] = s.len() as u8;
        img[blk * BS + 0x1B1..blk * BS + 0x1B1 + s.len()].copy_from_slice(s.as_bytes());
    }
    /// AmigaDOS name hash (international variant is irrelevant for ASCII test names).
    fn hash(s: &str) -> usize {
        let mut h = s.len() as u32;
        for c in s.bytes() {
            h = (h * 13 + c.to_ascii_uppercase() as u32) & 0x7FF;
        }
        (h % 72) as usize
    }

    /// Tiny synthetic OFS disk: root -> dir "s" -> file "startup-sequence" (600 bytes, 2 data blocks).
    #[test]
    fn ofs_dir_and_file() {
        let mut img = vec![0u8; 901_120];
        img[..4].copy_from_slice(b"DOS\0");
        let (root, dir, file, d1, d2) = (880, 881, 882, 883, 884);
        put(&mut img, root, 0, T_HEADER);
        put(&mut img, root, BS - 4, ST_ROOT as u32);
        set_name(&mut img, root, "Test");
        put(&mut img, root, 24 + 4 * hash("s"), dir as u32);
        put(&mut img, dir, 0, T_HEADER);
        put(&mut img, dir, BS - 4, ST_USERDIR as u32);
        set_name(&mut img, dir, "s");
        put(&mut img, dir, 24 + 4 * hash("startup-sequence"), file as u32);
        put(&mut img, file, 0, T_HEADER);
        put(&mut img, file, 8, 2);
        put(&mut img, file, 24 + 4 * 71, d1 as u32);
        put(&mut img, file, 24 + 4 * 70, d2 as u32);
        put(&mut img, file, 0x144, 600);
        put(&mut img, file, BS - 4, ST_FILE as u32);
        set_name(&mut img, file, "startup-sequence");
        for (blk, len, fill) in [(d1, 488u32, b'a'), (d2, 112, b'b')] {
            put(&mut img, blk, 0, T_DATA);
            put(&mut img, blk, 4, file as u32);
            put(&mut img, blk, 12, len);
            img[blk * BS + 24..blk * BS + 24 + len as usize].fill(fill);
        }
        let a = Adf::new(&img).unwrap();
        assert_eq!(a.label, "Test");
        let w = a.walk().unwrap();
        assert_eq!(w.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["s/", "s/startup-sequence"]);
        let data = a.read("S/Startup-Sequence").unwrap();
        assert_eq!(data.len(), 600);
        assert!(data[..488].iter().all(|&c| c == b'a') && data[488..].iter().all(|&c| c == b'b'));
        assert!(a.read("s/nope").is_err());
    }
}
