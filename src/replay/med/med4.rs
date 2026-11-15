//! MED 2.10 files (MED\x04) as MMD0 modules: NostalgicPlayer's Med4Format
//! converter (Copyright (c) 2023 Thomas Neumann, MIT licence: see
//! LICENSE-NostalgicPlayer). Samples and synth sounds must be in the file (no
//! external instruments).

struct Reader<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.d.get(self.p)?;
        self.p += 1;
        Some(v)
    }
    fn u16(&mut self) -> Option<u16> {
        Some((self.u8()? as u16) << 8 | self.u8()? as u16)
    }
    fn u32(&mut self) -> Option<u32> {
        Some((self.u16()? as u32) << 16 | self.u16()? as u32)
    }
    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.d.get(self.p..self.p.checked_add(n)?)?;
        self.p += n;
        Some(s)
    }
}

fn put16(o: &mut Vec<u8>, v: u16) {
    o.extend_from_slice(&v.to_be_bytes());
}

fn put32(o: &mut Vec<u8>, v: u32) {
    o.extend_from_slice(&v.to_be_bytes());
}

fn set32(o: &mut [u8], at: usize, v: u32) {
    o[at..at + 4].copy_from_slice(&v.to_be_bytes());
}

/// A table of up to 128 bytes, zero-padded.
fn table(o: &mut Vec<u8>, r: &mut Reader, len: usize) -> Option<()> {
    let mut t = [0u8; 128];
    let v = r.bytes(len)?;
    let n = len.min(128);
    t[..n].copy_from_slice(&v[..n]);
    o.extend_from_slice(&t);
    Some(())
}

pub fn to_mmd0(d: &[u8]) -> Option<Vec<u8>> {
    let mut r = Reader { d, p: 4 };
    let mut o = Vec::with_capacity(d.len() * 2);
    o.extend_from_slice(b"MMD0");
    o.resize(0x34, 0);
    o[0x2e] = 0xff; // the play sequence number: -1
    o[0x2f] = 0xff;

    // the song: the instruments that are there, by a mask of mask bytes
    let song = o.len();
    set32(&mut o, 8, song as u32);
    let mut mm = r.u8()?;
    let (mut mask, mut n) = (0u64, 0u32);
    for _ in 0..8 {
        if mm & 0x80 != 0 {
            mask = mask << 8 | r.u8()? as u64;
            n += 1;
        }
        mm <<= 1;
    }
    mask = mask.checked_shl(64 - n * 8).unwrap_or(0);
    let mut named = [false; 63];
    for nm in &mut named {
        let (mut rep, mut replen, mut midich, mut preset, mut vol, mut tr) =
            (0u16, 0u16, 0u8, 0u8, 64u8, 0u8);
        if mask & 1 << 63 != 0 {
            let flag = r.u8()?;
            let len = r.u8()? as usize;
            // the converter takes a name that is there as an instrument
            *nm = r.bytes(len)?.iter().any(|&c| c != 0);
            if flag & 1 == 0 {
                rep = r.u16()?;
            }
            if flag & 2 == 0 {
                replen = r.u16()?;
            }
            if flag & 4 == 0 {
                midich = r.u8()?;
            }
            if flag & 8 == 0 {
                preset = r.u8()?;
            }
            if flag & 0x30 == 0 {
                vol = r.u8()?;
            }
            if flag & 0x40 == 0 {
                tr = r.u8()?;
            }
            if replen <= 2 {
                rep = 0;
                replen = 0;
            }
        }
        put16(&mut o, rep);
        put16(&mut o, replen);
        o.extend_from_slice(&[midich, preset, vol, tr]);
        mask <<= 1;
    }
    let mut nsamples = named.iter().rposition(|&x| x).map_or(0, |i| i + 1);
    let numblocks = r.u16()?;
    put16(&mut o, numblocks);
    let songlen = r.u16()?;
    put16(&mut o, songlen);
    let mut seq = [0u8; 256];
    let s = r.bytes(songlen as usize)?;
    let n = s.len().min(256);
    seq[..n].copy_from_slice(&s[..n]);
    o.extend_from_slice(&seq);
    put16(&mut o, r.u16()?); // tempo
    let transp = r.u8()?;
    let flags = r.u8()?;
    o.extend_from_slice(&[transp, flags, 0]);
    let tempo2_at = o.len();
    o.push(r.u16()? as u8); // tempo2
    r.bytes(20)?; // jump mask and colours
    o.extend_from_slice(r.bytes(16)?); // track volumes
    o.push(r.u8()?); // master volume
    o.push(nsamples as u8);

    // the blocks: packed by line and command masks, unpacked to MMD0's 3 bytes
    let blockarr = o.len();
    set32(&mut o, 0x10, blockarr as u32);
    o.resize(blockarr + 4 * numblocks as usize, 0);
    for b in 0..numblocks as usize {
        let at = o.len() as u32;
        set32(&mut o, blockarr + 4 * b, at);
        block(&mut o, &mut r)?;
    }

    // NostalgicPlayer counts the instruments by their names; the original
    // plays every sample the file holds, named or not
    if flags & 0x08 != 0 {
        let m = Reader { d, p: r.p }
            .bytes(8)
            .map(|b| u64::from_be_bytes(b.try_into().unwrap()));
        if let Some(m) = m {
            let top = (0..63).rev().find(|&i| m & 1 << (62 - i) != 0);
            nsamples = nsamples.max(top.map_or(0, |i| i + 1));
        }
        o[song + 0x313] = nsamples as u8;
    }

    // the IFF-like chunks at the end: hold and decay
    let tail = d.len().saturating_sub(1024);
    let mut hldc: Option<&[u8]> = None;
    let end = d.len() - tail;
    let medv = d[tail..]
        .windows(4)
        .take(end.saturating_sub(8))
        .any(|w| w == b"MEDV");
    if !medv {
        // NostalgicPlayer takes the file's tempo2; the original plays a file
        // without a version chunk (an older MED's) at 6 ticks a row
        o[tempo2_at] = 6;
    }
    if let Some(i) = d[tail..]
        .windows(4)
        .take(end.saturating_sub(8))
        .position(|w| w == b"MEDV")
    {
        let mut c = Reader { d, p: tail + i };
        loop {
            let (Some(id), Some(len)) = (c.bytes(4).map(|x| [x[0], x[1], x[2], x[3]]), c.u32())
            else {
                break;
            };
            let Some(body) = c.bytes(len as usize) else {
                break;
            };
            if &id == b"HLDC" {
                hldc = Some(body);
            }
        }
    }
    let (mut iext, mut iext_n) = (0u32, 0u16);
    if let Some(h) = hldc {
        iext = o.len() as u32;
        iext_n = (nsamples as u16).min(h.len() as u16 / 2);
        for i in 0..iext_n as usize {
            o.extend_from_slice(&[h[2 * i], h[2 * i + 1], 0, 0]);
        }
    }
    let exp = o.len();
    set32(&mut o, 0x20, exp as u32);
    put32(&mut o, 0);
    put32(&mut o, iext);
    put16(&mut o, iext_n);
    put16(&mut o, if hldc.is_some() { 4 } else { 0 });
    o.resize(exp + 0x50, 0);

    // the samples and synth sounds
    let smplarr = o.len();
    set32(&mut o, 0x18, smplarr as u32);
    o.resize(smplarr + 4 * nsamples, 0);
    if flags & 0x08 == 0 {
        return None;
    }
    let mut smask = (r.u32()? as u64) << 32;
    smask |= r.u32()? as u64;
    for i in 0..nsamples {
        smask <<= 1; // there is no instrument 0
        if smask & 1 << 63 == 0 {
            continue;
        }
        // each in a block of its own, as memory allocated for it would be
        o.resize((o.len() + 7) & !7, 0);
        let at = o.len();
        set32(&mut o, smplarr + 4 * i, at as u32);
        let len = r.u32()?;
        let typ = r.u16()?;
        if typ == 0xffff || typ == 0xfffe {
            // the waveforms' offsets count from the sound's own header, after
            // its length and type
            let start = r.p;
            r.bytes(6)?;
            synth(&mut o, &mut r, at, start, typ)?;
        } else {
            put32(&mut o, len);
            // NostalgicPlayer writes type 0; the original keeps multi-octave types
            put16(&mut o, typ);
            // a truncated file ends inside its last sample: the rest reads as silence
            let have = r.d.len().saturating_sub(r.p).min(len as usize);
            o.extend_from_slice(r.bytes(have)?);
            o.resize(o.len() + len as usize - have, 0);
        }
    }
    let n = o.len() as u32;
    set32(&mut o, 4, n);
    Some(o)
}

fn block(o: &mut Vec<u8>, r: &mut Reader) -> Option<()> {
    let hlen = r.u8()? as usize;
    let hstart = r.p;
    let tracks = r.u8()? as usize;
    let rows = r.u8()? as usize;
    let dlen = r.u16()? as usize;
    let mut bflag = 0u32;
    let mut i = rows as i32 + 1;
    while i > 0 {
        bflag = bflag << 8 | r.u8()? as u32;
        i -= 64;
    }
    bflag = bflag
        .checked_shl(32 - ((rows as u32 + 64) / 64) * 8)
        .unwrap_or(0);
    let (mut lmask, mut emask) = ([0u32; 8], [0u32; 8]);
    let mut j = rows as i32 + 1;
    let mut k = 0;
    while j > 0 {
        lmask[k] = if bflag & 0x8000_0000 != 0 {
            !0
        } else if bflag & 0x4000_0000 != 0 {
            0
        } else {
            r.u32()?
        };
        emask[k] = if bflag & 0x2000_0000 != 0 {
            !0
        } else if bflag & 0x1000_0000 != 0 {
            0
        } else {
            r.u32()?
        };
        bflag <<= 4;
        j -= 32;
        k += 1;
    }
    r.p = hstart + hlen;
    let data = r.bytes(dlen)?;
    let mut nib = 0usize;
    let mut get = |cnt: usize| -> Option<u16> {
        let mut v = 0u16;
        for _ in 0..cnt {
            let b = *data.get(nib / 2)?;
            v = v << 4 | if nib & 1 != 0 { b & 15 } else { b >> 4 } as u16;
            nib += 1;
        }
        Some(v)
    };
    o.push(tracks as u8);
    o.push(rows as u8);
    let w = tracks * 3;
    let mut blk = vec![0u8; w * (rows + 1)];
    let (mut lm, mut em) = (0u32, 0u32);
    for i in 0..=rows {
        if i % 32 == 0 {
            lm = lmask[i / 32];
            em = emask[i / 32];
        }
        if lm & 0x8000_0000 != 0 {
            let mut cm = get(tracks / 4)?
                .checked_shl(16u32.saturating_sub(tracks as u32))
                .unwrap_or(0);
            for t in 0..tracks {
                if cm & 0x8000 != 0 {
                    blk[i * w + t * 3] = get(2)? as u8;
                    blk[i * w + t * 3 + 1] = (get(1)? << 4) as u8;
                }
                cm <<= 1;
            }
        }
        if em & 0x8000_0000 != 0 {
            let mut cm = get(tracks / 4)?
                .checked_shl(16u32.saturating_sub(tracks as u32))
                .unwrap_or(0);
            for t in 0..tracks {
                if cm & 0x8000 != 0 {
                    blk[i * w + t * 3 + 1] |= get(1)? as u8;
                    blk[i * w + t * 3 + 2] = get(2)? as u8;
                }
                cm <<= 1;
            }
        }
        lm <<= 1;
        em <<= 1;
    }
    o.extend_from_slice(&blk);
    Some(())
}

/// A synth or hybrid sound: its header with full-size tables, its waveforms.
fn synth(o: &mut Vec<u8>, r: &mut Reader, at: usize, start: usize, typ: u16) -> Option<()> {
    put32(o, 0x110);
    put16(o, typ);
    o.push(r.u8()?); // decay
    r.bytes(3)?;
    o.extend_from_slice(&[0, 0, 0]);
    put16(o, r.u16()?); // loop start
    put16(o, r.u16()?); // loop length
    let vlen = r.u16()? as usize;
    let wlen = r.u16()? as usize;
    put16(o, 128);
    put16(o, 128);
    o.push(r.u8()?); // volume speed
    o.push(r.u8()?); // waveform speed
    let nwf = r.u16()? as usize;
    put16(o, nwf as u16);
    table(o, r, vlen)?;
    table(o, r, wlen)?;
    let ptrs = o.len();
    let mut offs = Vec::with_capacity(nwf);
    for _ in 0..nwf {
        offs.push(r.u32()?);
        put32(o, 0);
    }
    let mut end = r.p;
    for (k, &off) in offs.iter().enumerate() {
        let rel = (o.len() - at) as u32;
        set32(o, ptrs + 4 * k, rel);
        r.p = start.checked_add(off as usize)?;
        if k == 0 && typ == 0xfffe {
            let l = r.u32()?;
            r.u16()?;
            put32(o, l);
            put16(o, 0);
            o.extend_from_slice(r.bytes(l as usize)?);
        } else {
            let l = r.u16()?;
            put16(o, l);
            o.extend_from_slice(r.bytes(2 * l as usize)?);
        }
        end = end.max(r.p);
    }
    r.p = end;
    Some(())
}
