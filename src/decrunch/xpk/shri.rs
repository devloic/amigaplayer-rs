//! XPK-SHRI (LZ77 with an adaptive range coder; version 2 chunks continue the model of the
//! previous chunk). Ported from LibAncient's `ShrXDecompressor.cs` (NostalgicPlayer, Thomas
//! Neumann, MIT), a port of ancient's `SHRXDecompressor.cpp` (Teemu Suutari, BSD-2-Clause);
//! see LICENSE-ancient. SHR3, which ancient also handles, is not in LibAncient and not here.

use crate::decrunch::ancient::{be16, be32, ByteSource, Fwd, FwdOut};

/// The model carried from chunk to chunk.
pub(crate) struct State {
    vlen: u32,
    vnext: u32,
    shift: u32,
    ar: [u32; 999],
}

struct Header {
    ver: u8,
    raw: usize,
    start: usize,
}

fn header(chunk: &[u8]) -> Option<Header> {
    if chunk.len() < 6 {
        return None;
    }
    let ver = chunk[0];
    if ver == 0 || ver > 2 {
        return None;
    }
    // The second byte defines something that does not seem to be terribly important.
    let (raw, start) = if chunk[2] & 0x80 != 0 {
        ((!be32(chunk, 2)?).wrapping_add(1) as usize, 6)
    } else {
        (be16(chunk, 2)? as usize, 4)
    };
    Some(Header { ver, raw, start })
}

/// The constructor checks: a version 2 chunk needs the state of an earlier chunk; a version 1
/// chunk creates the state when there is none.
pub(super) fn check(chunk: &[u8], state: &mut Option<Box<State>>) -> Option<()> {
    let h = header(chunk)?;
    if state.is_none() {
        if h.ver == 2 {
            return None;
        }
        *state = Some(Box::new(State { vlen: 0, vnext: 0, shift: 0, ar: [0; 999] }));
    }
    Some(())
}

const UPDATES1: [u32; 6] = [358, 359, 386, 387, 414, 415];
const UPDATES2: [u32; 4] = [442, 456, 470, 484];

/// Decodes `buf[start..end]`; copies may reach into the earlier chunks before `start`.
pub(super) fn decompress(
    chunk: &[u8],
    buf: &mut [u8],
    start: usize,
    end: usize,
    state: &mut Option<Box<State>>,
) -> Option<()> {
    check(chunk, state)?;
    let h = header(chunk)?;
    let st = state.as_mut()?;
    if end - start != h.raw {
        return None;
    }
    let mut input = Fwd::new(chunk, h.start, chunk.len())?;
    let mut o = FwdOut::new(buf, start, end)?;
    let mut m = Model { ar: [0; 999] };

    let (mut vlen, mut vnext, mut shift);
    if h.ver == 1 {
        m.init();
        m.update(498, 1);
        vlen = 0;
        vnext = 0;
        shift = 0x8000_0000u32;
    } else {
        vlen = st.vlen;
        vnext = st.vnext;
        shift = st.shift;
        m.ar = st.ar;
    }
    let mut stream = input.be32()?;

    macro_rules! refill {
        () => {
            while shift < 0x100_0000 {
                stream = (stream << 8) | u32::from(input.byte()?);
                shift <<= 8;
            }
        };
    }

    while !o.eof() {
        while vlen >= vnext {
            m.upgrade(&mut vlen, &mut vnext);
        }
        // getSymbol
        let code = {
            if shift >> 16 == 0 {
                return None;
            }
            let ar1 = m.ar[1];
            let vvalue = (stream / (shift >> 16)) & 0xffff;
            let threshold = ar1.wrapping_mul(vvalue) >> 16;
            let mut ar_index = 1usize;
            let mut result = 0u32;
            loop {
                ar_index <<= 1;
                let tmp = m.ar[ar_index].wrapping_add(result);
                if threshold >= tmp {
                    result = tmp;
                    ar_index += 1;
                }
                if ar_index >= 499 {
                    break;
                }
            }
            let mut new_value = scale(result, ar1, shift)?;
            // Legitimate data needs at most a lap of the leaves; the cap only stops corrupt
            // data from spinning here (the C++ code would hang).
            let mut guard = 0u32;
            if new_value > stream {
                while new_value > stream {
                    ar_index -= 1;
                    if ar_index < 499 {
                        ar_index += 499;
                    }
                    result = result.wrapping_sub(m.ar[ar_index]);
                    new_value = scale(result, ar1, shift)?;
                    guard += 1;
                    if guard > 4096 {
                        return None;
                    }
                }
            } else {
                result = result.wrapping_add(m.ar[ar_index]);
                while result < ar1 {
                    let compare = scale(result, ar1, shift)?;
                    if stream < compare {
                        break;
                    }
                    ar_index += 1;
                    if ar_index >= 998 {
                        ar_index -= 499;
                    }
                    result = result.wrapping_add(m.ar[ar_index]);
                    new_value = compare;
                    guard += 1;
                    if guard > 4096 {
                        return None;
                    }
                }
            }
            stream = stream.wrapping_sub(new_value);
            shift = scale(m.ar[ar_index], ar1, shift)?;
            let addition = (m.ar[1] >> 10) + 3;
            let sym = (ar_index - 499) as u32;
            m.update(sym, addition);
            refill!();
            sym
        };
        macro_rules! get_code {
            ($size:expr) => {{
                let mut ret = 0u32;
                for _ in 0..$size {
                    ret = ret.wrapping_shl(1);
                    shift >>= 1;
                    if stream >= shift {
                        ret = ret.wrapping_add(1);
                        stream = stream.wrapping_sub(shift);
                    }
                    refill!();
                }
                ret
            }};
        }
        if code < 256 {
            o.write(code as u8)?;
            vlen = vlen.wrapping_add(1);
        } else {
            let add = |i: u32| (1u32.wrapping_shl(i + 2).wrapping_sub(1)) & !3;
            let (count, distance);
            if code < 304 {
                count = 2;
                distance = code - 255;
            } else if code < 332 {
                let tmp = code - 304;
                let extra = get_code!(tmp >> 2);
                distance = ((extra << 2) | (tmp & 3)).wrapping_add(add(tmp >> 2)).wrapping_add(1);
                count = 3;
            } else if code < 442 {
                let (base, c) = match code {
                    332..=357 => (332, 4),
                    358..=385 => (358, 5),
                    386..=413 => (386, 6),
                    _ => (414, 7),
                };
                let tmp = code - base;
                let extra = get_code!((tmp >> 1) + 1);
                distance = ((extra << 1) | (tmp & 1)).wrapping_add(add(tmp >> 1)).wrapping_add(1);
                count = c;
            } else if code < 498 {
                let tmp = code - 442;
                let (d, md) = (tmp / 14, tmp % 14);
                count = get_code!(d + 2).wrapping_add(add(d)).wrapping_add(8);
                distance = get_code!(md + 2).wrapping_add(add(md)).wrapping_add(1);
            } else {
                count = get_code!(16);
                distance = get_code!(16);
            }
            vlen = vlen.wrapping_add(count);
            if count == 0 {
                return None;
            }
            o.copy_prev(distance as usize, count as usize)?;
        }
    }
    st.vlen = vlen;
    st.vnext = vnext;
    st.shift = shift;
    st.ar = m.ar;
    Some(())
}

fn scale(a: u32, b: u32, mult: u32) -> Option<u32> {
    if b == 0 {
        return None;
    }
    let tmp = (a << 16) / b;
    let tmp2 = (((a << 16) % b) << 16) / b;
    Some(
        ((mult & 0xffff).wrapping_mul(tmp) >> 16)
            .wrapping_add((mult >> 16).wrapping_mul(tmp2) >> 16)
            .wrapping_add((mult >> 16).wrapping_mul(tmp)),
    )
}

struct Model {
    ar: [u32; 999],
}

impl Model {
    fn resum(&mut self) {
        for i in (1..=498).rev() {
            self.ar[i] = self.ar[i * 2].wrapping_add(self.ar[i * 2 + 1]);
        }
    }
    fn init(&mut self) {
        self.ar = [0; 999];
        for i in 0..256 {
            self.ar[i + 499] = if !(32..=126).contains(&i) { 1 } else { 3 };
        }
        self.resum();
    }
    fn update(&mut self, index: u32, inc: u32) {
        if index >= 499 {
            return;
        }
        let mut i = index as usize + 499;
        while i != 0 {
            self.ar[i] = self.ar[i].wrapping_add(inc);
            i >>= 1;
        }
        if self.ar[1] >= 0x2000 {
            for i in 499..998 {
                if self.ar[i] != 0 {
                    self.ar[i] = (self.ar[i] >> 1) + 1;
                }
            }
            self.resum();
        }
    }
    fn upgrade(&mut self, vlen: &mut u32, vnext: &mut u32) {
        if *vnext >= 65532 {
            *vnext = !0;
        } else if *vlen == 0 {
            *vnext = 1;
        } else {
            let mut vvalue = vnext.wrapping_sub(1);
            if vvalue < 48 {
                self.update(vvalue + 256, 1);
            }
            let (mut bits, mut compare) = (0u32, 4u32);
            while vvalue >= compare {
                vvalue -= compare;
                compare = compare.wrapping_shl(1);
                bits += 1;
            }
            if bits >= 14 {
                *vnext = !0;
            } else {
                if vvalue == 0 {
                    if bits < 7 {
                        for i in 304..=307 {
                            self.update((bits << 2) + i, 1);
                        }
                    }
                    if bits < 13 {
                        for i in 332..=333 {
                            self.update((bits << 1) + i, 1);
                        }
                    }
                    for it in UPDATES1 {
                        self.update((bits << 1) + it, 1);
                    }
                    for it in UPDATES2 {
                        self.update(bits + it, 1);
                    }
                }
                if *vnext < 49 {
                    *vnext += 1;
                } else if *vnext == 49 {
                    *vnext = 61;
                } else {
                    *vnext = (*vnext << 1).wrapping_add(3);
                }
            }
        }
    }
}
