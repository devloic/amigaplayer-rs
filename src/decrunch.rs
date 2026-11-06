//! Decrunching of packed modules. DeliTracker did this through
//! powerpacker.library / xpkmaster.library; here PowerPacker (PP20) data is
//! unpacked natively, and so are the single-file depackers of libxmp 4.7.4
//! (Pack-Ice, XPK SQSH, StoneCracker S404 and MMCMP, ported in the
//! submodules with their licence notices). Anything else, and anything that
//! only starts with a magic by chance, is passed through unchanged.

mod ice;
mod mmcmp;
mod s404;
mod sqsh;
#[cfg(test)]
mod depack_tests;

/// libxmp's `LIBXMP_DEPACK_LIMIT`.
pub(crate) const DEPACK_LIMIT: usize = 512 << 20;

/// Zeroed buffer that fails instead of aborting when it cannot be allocated.
pub(crate) fn alloc_zeroed(n: usize) -> Option<Vec<u8>> {
    Vec::<u8>::new().try_reserve_exact(n).ok()?;
    // calloc: pages are only touched when written.
    Some(vec![0u8; n])
}

/// libxmp ignores files shorter than this when looking for a depacker.
const MIN_PACKED: usize = 64;

/// The libxmp depackers, in its order (sqsh, mmcmp, ice1, ice2, s404).
fn libxmp_depack(data: &[u8]) -> Option<(&'static str, Option<Vec<u8>>)> {
    if data.len() < MIN_PACKED {
        return None;
    }
    if sqsh::test(data) {
        return Some(("XPK SQSH", sqsh::depack(data)));
    }
    if mmcmp::test(data) {
        return Some(("MMCMP", mmcmp::depack(data)));
    }
    if ice::test1(data) {
        return Some(("Pack-Ice", ice::unpack1(data)));
    }
    if ice::test2(data) {
        return Some(("Pack-Ice", ice::unpack2(data)));
    }
    if s404::test(data) {
        return Some(("StoneCracker S404", s404::depack(data)));
    }
    None
}

/// The packers this module undoes, for the library's `unpack` module.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    PowerPacker,
    XpkSqsh,
    Mmcmp,
    PackIce,
    StoneCracker,
}

/// Which packer made `data`, by its header alone (nothing is unpacked).
pub(crate) fn identify(data: &[u8]) -> Option<Kind> {
    if data.len() > 12 && &data[..4] == b"PP20" {
        return Some(Kind::PowerPacker);
    }
    if data.len() < MIN_PACKED {
        return None;
    }
    if sqsh::test(data) {
        Some(Kind::XpkSqsh)
    } else if mmcmp::test(data) {
        Some(Kind::Mmcmp)
    } else if ice::test1(data) || ice::test2(data) {
        Some(Kind::PackIce)
    } else if s404::test(data) {
        Some(Kind::StoneCracker)
    } else {
        None
    }
}

/// Unpacks `data` as the packer it identifies as: `None` when it is not packed, `Some(None)`
/// when it is but the data is corrupt, `Some(Some(bytes))` otherwise.
pub(crate) fn unpack_identified(data: &[u8]) -> Option<(Kind, Option<Vec<u8>>)> {
    let kind = identify(data)?;
    let out = match kind {
        Kind::PowerPacker => pp20(data),
        Kind::XpkSqsh => sqsh::depack(data),
        Kind::Mmcmp => mmcmp::depack(data),
        Kind::PackIce => {
            if ice::test1(data) {
                ice::unpack1(data)
            } else {
                ice::unpack2(data)
            }
        }
        Kind::StoneCracker => s404::depack(data),
    };
    Some((kind, out.filter(|d| !d.is_empty())))
}

pub fn decrunch(data: Vec<u8>) -> Vec<u8> {
    if data.len() > 12 && &data[..4] == b"PP20" {
        match pp20(&data) {
            Some(d) => {
                log::info!("PowerPacker: {} -> {} bytes", data.len(), d.len());
                return d;
            }
            None => log::warn!("PowerPacker data is corrupt; using it as is"),
        }
    }
    match libxmp_depack(&data) {
        Some((name, Some(d))) if !d.is_empty() => {
            log::info!("{}: {} -> {} bytes", name, data.len(), d.len());
            return d;
        }
        Some((name, _)) => log::warn!("{} data is corrupt; using it as is", name),
        None => {}
    }
    data
}

struct Bits<'a> {
    src: &'a [u8],
    pos: usize, // next longword is src[pos-4..pos]
    buf: u32,
    left: u32,
}

impl Bits<'_> {
    fn bit(&mut self) -> Option<u32> {
        if self.left == 0 {
            if self.pos < 4 {
                return None;
            }
            self.pos -= 4;
            let s = &self.src[self.pos..self.pos + 4];
            self.buf = u32::from_be_bytes([s[0], s[1], s[2], s[3]]);
            self.left = 32;
        }
        let b = self.buf & 1;
        self.buf >>= 1;
        self.left -= 1;
        Some(b)
    }
    fn bits(&mut self, n: u32) -> Option<u32> {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | self.bit()?;
        }
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    /// Needs a PP20 file and its unpacked original:
    /// DELI_PP_TEST=/path/file.pp (expects /path/file.raw next to it)
    #[test]
    #[ignore]
    fn pp20_matches_reference() {
        let pp = std::path::PathBuf::from(std::env::var("DELI_PP_TEST").unwrap());
        let packed = std::fs::read(&pp).unwrap();
        let raw = std::fs::read(pp.with_extension("raw")).unwrap();
        assert_eq!(super::pp20(&packed).expect("decrunch failed"), raw);
    }
}

fn pp20(src: &[u8]) -> Option<Vec<u8>> {
    let eff = &src[4..8];
    let t = src.len() - 4;
    let out_len = ((src[t] as usize) << 16) | ((src[t + 1] as usize) << 8) | src[t + 2] as usize;
    if out_len == 0 || out_len > 16 << 20 {
        return None;
    }
    let packed = &src[8..t];
    if packed.len() % 4 != 0 {
        return None;
    }
    let mut b = Bits { src: packed, pos: packed.len(), buf: 0, left: 0 };
    b.bits(src[t + 3] as u32)?;
    let mut out = vec![0u8; out_len];
    let mut dst = out_len;
    while dst > 0 {
        if b.bit()? == 0 {
            let mut n = 1;
            loop {
                let x = b.bits(2)?;
                n += x as usize;
                if x != 3 {
                    break;
                }
            }
            for _ in 0..n {
                if dst == 0 {
                    return None;
                }
                dst -= 1;
                out[dst] = b.bits(8)? as u8;
            }
            if dst == 0 {
                break;
            }
        }
        let x = b.bits(2)? as usize;
        let mut n = x + 2;
        let offset = if x == 3 {
            let ob = if b.bit()? == 0 { 7 } else { eff[3] as u32 };
            let off = b.bits(ob)? as usize;
            loop {
                let y = b.bits(3)? as usize;
                n += y;
                if y != 7 {
                    break;
                }
            }
            off
        } else {
            b.bits(eff[x] as u32)? as usize
        };
        for _ in 0..n {
            if dst == 0 || dst + offset >= out_len {
                return None;
            }
            out[dst - 1] = out[dst + offset];
            dst -= 1;
        }
    }
    Some(out)
}
