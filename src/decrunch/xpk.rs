//! The XPK container (`XPKF`), dispatching each chunk to its method. Ported from LibAncient's
//! `XpkMain.cs` (NostalgicPlayer, Thomas Neumann, MIT), itself a port of ancient's
//! `XPKMain.cpp` (Teemu Suutari, BSD-2-Clause), following the C++ where the two differ; see
//! LICENSE-ancient.
//!
//! SQSH is not handled here: it keeps its libxmp port (`sqsh.rs`), which `decrunch` tries
//! first. The methods are those LibAncient has; others (FAST, NUKE's relatives, encrypted
//! ones...) are not recognised, so such files pass through as they are. Password protected
//! files are recognised and then refused, as ancient does.

mod blzw;
mod bzp2;
pub(crate) mod lhlb;
mod mash;
mod nuke;
mod rake;
mod shri;
mod smpl;

use super::alloc_zeroed;
use super::ancient::{be16, be32, MAX_RAW};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Method {
    Bzp2,
    Blzw,
    Lhlb,
    Mash,
    Nuke,
    Duke,
    Rake,
    Shri,
    Smpl,
}

impl Method {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Method::Bzp2 => "XPK BZP2",
            Method::Blzw => "XPK BLZW",
            Method::Lhlb => "XPK LHLB",
            Method::Mash => "XPK MASH",
            Method::Nuke => "XPK NUKE",
            Method::Duke => "XPK DUKE",
            Method::Rake => "XPK RAKE",
            Method::Shri => "XPK SHRI",
            Method::Smpl => "XPK SMPL",
        }
    }

    fn from_id(id: &[u8]) -> Option<Method> {
        Some(match id {
            b"BZP2" => Method::Bzp2,
            b"BLZW" => Method::Blzw,
            b"LHLB" => Method::Lhlb,
            b"MASH" => Method::Mash,
            b"NUKE" => Method::Nuke,
            b"DUKE" => Method::Duke,
            b"RAKE" => Method::Rake,
            b"SHRI" => Method::Shri,
            b"SMPL" => Method::Smpl,
            _ => return None,
        })
    }
}

struct Header {
    method: Method,
    packed: usize,
    raw: usize,
    header_size: usize,
    long_headers: bool,
    password: bool,
}

/// The constructor's checks of ancient's `XPKMain`, without the verification pass.
fn header(d: &[u8]) -> Option<Header> {
    if d.len() < 44 || &d[..4] != b"XPKF" {
        return None;
    }
    let packed = be32(d, 4)? as usize;
    let method = Method::from_id(&d[8..12])?;
    let raw = be32(d, 12)? as usize;
    if raw == 0 || packed == 0 || raw > MAX_RAW || packed > 0x800_0000 {
        return None;
    }
    let flags = d[32];
    let header_size = if flags & 4 != 0 { 38 + be16(d, 36)? as usize } else { 36 };
    if packed + 8 > d.len() {
        return None;
    }
    Some(Header { method, packed, raw, header_size, long_headers: flags & 1 != 0, password: flags & 2 != 0 })
}

/// The method of an XPK file this module unpacks, by its header.
pub(crate) fn test(d: &[u8]) -> Option<Method> {
    header(d).map(|h| h.method)
}

/// One chunk: its header bytes, its data, its unpacked size and its type.
struct Chunk<'a> {
    header: &'a [u8],
    data: &'a [u8],
    raw: usize,
    kind: u8,
}

/// ancient's `forEachChunk`; `f` returns `Some(false)` to stop early, `None` on an error.
fn for_each_chunk<'a>(d: &'a [u8], h: &Header, mut f: impl FnMut(Chunk<'a>) -> Option<bool>) -> Option<()> {
    let mut cur = 0usize;
    let mut last = false;
    let hlen = if h.long_headers { 12 } else { 8 };
    let dual = |cur: usize, short: usize, long: usize| -> Option<usize> {
        if h.long_headers {
            be32(d, cur.checked_add(long)?).map(|v| v as usize)
        } else {
            be16(d, cur.checked_add(short)?).map(|v| v as usize)
        }
    };
    while cur < h.packed + 8 && !last {
        if cur == 0 {
            cur = h.header_size;
        } else {
            // The previous chunk's packed size, padded to 4 bytes (ancient checks the sum
            // against the packed size without the 8 header bytes).
            let tmp = (dual(cur, 4, 4)?.checked_add(3)? & !3) as u64;
            if tmp + cur as u64 + hlen as u64 > h.packed as u64 {
                return None;
            }
            cur += hlen + tmp as usize;
        }
        let packed = dual(cur, 4, 4)?;
        let raw = dual(cur, 6, 8)?;
        let header = d.get(cur..cur.checked_add(hlen)?)?;
        let start = cur + hlen;
        let data = d.get(start..start.checked_add(packed)?)?;
        let kind = *d.get(cur)?;
        if !f(Chunk { header, data, raw, kind })? {
            return Some(());
        }
        if kind == 15 {
            last = true;
        }
    }
    last.then_some(())
}

fn header_checksum(b: &[u8]) -> bool {
    !b.is_empty() && b.iter().fold(0u8, |a, &x| a ^ x) == 0
}

fn chunk_checksum(b: &[u8], check: u32) -> bool {
    let mut t = [0u8; 2];
    for (i, &x) in b.iter().enumerate() {
        t[i & 1] ^= x;
    }
    !b.is_empty() && u32::from(t[0]) == check >> 8 && u32::from(t[1]) == check & 0xff
}

/// The method state carried between chunks (only SHRI has one).
type State = Option<Box<shri::State>>;

/// The sub-decompressor constructors' checks.
fn create(m: Method, chunk: &[u8], state: &mut State) -> Option<()> {
    match m {
        Method::Bzp2 => bzp2::check(chunk).map(|_| ()),
        Method::Blzw => blzw::check(chunk).map(|_| ()),
        Method::Rake => rake::check(chunk).map(|_| ()),
        Method::Smpl => smpl::check(chunk),
        Method::Shri => shri::check(chunk, state),
        Method::Lhlb | Method::Mash | Method::Nuke | Method::Duke => Some(()),
    }
}

pub(crate) fn depack(d: &[u8]) -> Option<Vec<u8>> {
    let h = header(d)?;
    // The verification pass of the constructor.
    if !header_checksum(&d[..36]) {
        return None;
    }
    let mut state: State = None;
    for_each_chunk(d, &h, |c| {
        if !header_checksum(c.header) {
            return None;
        }
        let check = be16(c.header, 2)?;
        if !c.data.is_empty() && !chunk_checksum(c.data, check) {
            return None;
        }
        match c.kind {
            1 => create(h.method, c.data, &mut state)?,
            0 | 15 => {}
            _ => return None,
        }
        Some(true)
    })?;

    if h.password {
        return None;
    }
    let mut out = alloc_zeroed(h.raw)?;
    let mut dest = 0usize;
    let mut state: State = None;
    for_each_chunk(d, &h, |c| {
        let end = dest.checked_add(c.raw)?;
        if end > h.raw {
            return None;
        }
        if c.raw == 0 {
            return Some(true);
        }
        match c.kind {
            0 => {
                if c.raw != c.data.len() {
                    return None;
                }
                out[dest..end].copy_from_slice(c.data);
            }
            1 => {
                create(h.method, c.data, &mut state)?;
                if h.method == Method::Shri {
                    // Its copies may reach into the earlier chunks.
                    shri::decompress(c.data, &mut out, dest, end, &mut state)?;
                } else {
                    let o = &mut out[dest..end];
                    match h.method {
                        Method::Bzp2 => bzp2::decompress(c.data, o)?,
                        Method::Blzw => blzw::decompress(c.data, o)?,
                        Method::Lhlb => lhlb::decompress(c.data, o)?,
                        Method::Mash => mash::decompress(c.data, o)?,
                        Method::Nuke => nuke::decompress(c.data, o, false)?,
                        Method::Duke => nuke::decompress(c.data, o, true)?,
                        Method::Rake => rake::decompress(c.data, o)?,
                        Method::Smpl => smpl::decompress(c.data, o)?,
                        Method::Shri => return None, // handled above
                    }
                }
            }
            15 => {}
            _ => return Some(false),
        }
        dest = end;
        Some(true)
    })?;
    if dest != h.raw {
        return None;
    }
    // The header holds the first 16 unpacked bytes.
    let n = h.raw.min(16);
    if d[16..16 + n] != out[..n] {
        return None;
    }
    Some(out)
}
