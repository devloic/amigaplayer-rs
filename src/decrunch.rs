//! Decrunching of packed modules. DeliTracker did this through
//! powerpacker.library / xpkmaster.library; here PowerPacker (PP20) data is
//! unpacked natively and anything else is passed through unchanged.

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
