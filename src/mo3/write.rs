//! An MO3 module put back into its original format (IT, XM, S3M, MOD, MTM), from what
//! libopenmpt 0.8.7 makes of the MO3 file (`Load_mo3.cpp`): the values are written so that the
//! format's loader gets back what libopenmpt has after loading the MO3 file (the inverses of
//! the format loaders' conversions, as libopenmpt's own loaders and savers do them). Ported
//! from libopenmpt 0.8.7 (BSD-3-Clause; see LICENSE-openmpt), the ProTracker period table from
//! `Tables.cpp`.

use super::{Cell, Env, Kind, Smp, Song, hflag, sflag};

fn put16(o: &mut Vec<u8>, v: u16) {
    o.extend_from_slice(&v.to_le_bytes());
}
fn put32(o: &mut Vec<u8>, v: u32) {
    o.extend_from_slice(&v.to_le_bytes());
}
fn set16(o: &mut [u8], at: usize, v: u16) {
    o[at..at + 2].copy_from_slice(&v.to_le_bytes());
}
fn set32(o: &mut [u8], at: usize, v: u32) {
    o[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
/// `src` in a field of `n` bytes, NUL-padded.
fn text(o: &mut Vec<u8>, src: &[u8], n: usize) {
    let k = src.len().min(n);
    o.extend_from_slice(&src[..k]);
    o.resize(o.len() + n - k, 0);
}

/// The sample's frames (0 when it has no data).
fn frames(s: &Smp) -> usize {
    if s.data.is_empty() || s.channels == 0 { 0 } else { s.data.len() / s.channels }
}

/// The sample's data in mono (stereo mixed down, as the format cannot hold it).
fn mono(s: &Smp) -> Vec<i16> {
    if s.channels == 2 {
        s.data.chunks_exact(2).map(|p| ((p[0] as i32 + p[1] as i32) >> 1) as i16).collect()
    } else {
        s.data.clone()
    }
}

/// The sample's data as 8-bit values.
fn mono8(s: &Smp) -> Vec<i8> {
    mono(s).into_iter().map(|v| if s.bits16 { (v >> 8) as i8 } else { v as i8 }).collect()
}

/// `(loop on, ping-pong, start, end)` clamped to `len`.
fn sample_loop(s: &Smp, len: usize) -> (bool, bool, u32, u32) {
    let on = s.flags & sflag::LOOP != 0;
    let end = (s.loop_end as usize).min(len) as u32;
    let start = s.loop_start.min(end);
    (on && end > start, s.flags & sflag::PINGPONG != 0, start, end)
}

/// The sample pre-amp libopenmpt sets from the header's sample volume.
fn preamp(song: &Song) -> u32 {
    if let Some(v) = song.mix_volume {
        return v;
    }
    if song.sample_volume < 0 {
        (song.sample_volume as i32 + 52) as u32
    } else {
        (song.sample_volume as f64 * 3.1 / 20.0).exp() as u32 + 51
    }
}

pub(crate) fn write(song: &Song) -> Option<Vec<u8>> {
    match song.kind {
        Kind::Xm => xm(song),
        Kind::It => it(song),
        Kind::S3m => s3m(song),
        Kind::Mod => modf(song),
        Kind::Mtm => mtm(song),
    }
}

fn speed(song: &Song) -> u8 {
    if song.speed == 0 { 6 } else { song.speed }
}
fn tempo(song: &Song) -> u8 {
    if song.tempo == 0 { 125 } else { song.tempo }
}

// ---------------------------------------------------------------------------------------------
// XM

fn xm(song: &Song) -> Option<Vec<u8>> {
    let mut o = b"Extended Module: ".to_vec();
    text(&mut o, &song.title, 20);
    o.push(0x1A);
    let tracker: Vec<u8> = if song.tracker.is_empty() {
        b"FastTracker v2.00   ".to_vec()
    } else {
        // (space-padded in the chunk)
        let t = &song.tracker;
        let n = t.iter().rposition(|&c| c != b' ' && c != 0).map_or(0, |n| n + 1);
        t[..n].to_vec()
    };
    let mut tr = tracker.clone();
    tr.resize(20, b' ');
    o.extend_from_slice(&tr[..20]);
    put16(&mut o, 0x0104);
    put32(&mut o, 276);
    let mut orders: Vec<u8> = song.orders.iter().copied().take(256).collect();
    if orders.is_empty() {
        orders.push(0);
    }
    let npat = song.patterns.len().clamp(1, 256);
    let nins = song.instruments.len().min(128);
    put16(&mut o, orders.len() as u16);
    put16(&mut o, song.restart);
    put16(&mut o, song.channels as u16);
    put16(&mut o, npat as u16);
    put16(&mut o, nins as u16);
    put16(&mut o, (song.flags & hflag::LINEAR != 0) as u16);
    put16(&mut o, speed(song) as u16);
    put16(&mut o, tempo(song) as u16);
    let mut ord = [0u8; 256];
    ord[..orders.len()].copy_from_slice(&orders);
    o.extend_from_slice(&ord);

    for p in 0..npat {
        let grid = song.patterns.get(p).and_then(|g| g.as_ref());
        let rows = grid.map_or(64, |g| g.len().min(256));
        let mut d = Vec::new();
        for r in 0..rows {
            for ch in 0..song.channels {
                let c = grid.map_or(Cell::default(), |g| g[r][ch]);
                let fx = c.fx.filter(|&(t, p)| t != 0 || p != 0);
                let mut mask = 0x80u8;
                if c.note.is_some() {
                    mask |= 1;
                }
                if c.ins != 0 {
                    mask |= 2;
                }
                if c.vol.is_some() {
                    mask |= 4;
                }
                if let Some((t, p)) = fx {
                    if t != 0 {
                        mask |= 8;
                    }
                    if p != 0 {
                        mask |= 16;
                    }
                }
                d.push(mask);
                if let Some(n) = c.note {
                    d.push(n);
                }
                if c.ins != 0 {
                    d.push(c.ins);
                }
                if let Some(v) = c.vol {
                    d.push(v);
                }
                if let Some((t, p)) = fx {
                    if t != 0 {
                        d.push(t);
                    }
                    if p != 0 {
                        d.push(p);
                    }
                }
            }
        }
        if d.len() > 0xFFFF {
            return None;
        }
        put32(&mut o, 9);
        o.push(0);
        put16(&mut o, rows as u16);
        put16(&mut o, d.len() as u16);
        o.extend_from_slice(&d);
    }

    let nsmp = song.samples.len();
    for ins in &song.instruments[..nins] {
        // the samples of the instrument: those of its note map
        let mut list: Vec<usize> = ins.map[..96].iter().map(|e| e.1 as usize).filter(|&s| s < nsmp).collect();
        list.sort_unstable();
        list.dedup();
        list.truncate(32);
        let size = if list.is_empty() { 29 } else { 263 };
        put32(&mut o, size);
        text(&mut o, &ins.name, 22);
        o.push(0);
        put16(&mut o, list.len() as u16);
        if list.is_empty() {
            continue;
        }
        put32(&mut o, 40);
        for e in &ins.map[..96] {
            o.push(list.iter().position(|&s| s == e.1 as usize).unwrap_or(0) as u8);
        }
        let env = |o: &mut Vec<u8>, e: &Env| {
            let n = e.nodes(0);
            for k in 0..12 {
                let (t, v) = n.get(k).copied().unwrap_or((0, 0));
                put16(o, t);
                put16(o, v as u16);
            }
        };
        env(&mut o, &ins.vol);
        env(&mut o, &ins.pan);
        o.push(ins.vol.nodes(0).len().min(12) as u8);
        o.push(ins.pan.nodes(0).len().min(12) as u8);
        o.extend_from_slice(&[ins.vol.sus_start, ins.vol.loop_start, ins.vol.loop_end]);
        o.extend_from_slice(&[ins.pan.sus_start, ins.pan.loop_start, ins.pan.loop_end]);
        o.push(ins.vol.flags & 7);
        o.push(ins.pan.flags & 7);
        o.extend_from_slice(&ins.vib);
        put16(&mut o, ins.fadeout);
        o.resize(o.len() + 22, 0);
        let mut datas = Vec::new();
        for &si in &list {
            let s = &song.samples[si];
            let len = frames(s);
            let bps = if s.bits16 { 2 } else { 1 };
            let (on, ping, ls, le) = sample_loop(s, len);
            put32(&mut o, (len * bps) as u32);
            put32(&mut o, if on { ls * bps as u32 } else { 0 });
            put32(&mut o, if on { (le - ls) * bps as u32 } else { 0 });
            o.push(s.volume.min(64));
            o.push(s.finetune(Kind::Xm) as u8);
            let mut typ = if !on {
                0
            } else if ping {
                2
            } else {
                1
            };
            if s.bits16 {
                typ |= 0x10;
            }
            o.push(typ);
            o.push(if s.panning <= 256 { s.panning.min(255) as u8 } else { 128 });
            o.push(s.transpose as u8);
            o.push(0);
            text(&mut o, &s.name, 22);
            // delta-coded
            let m = mono(s);
            let mut d = Vec::with_capacity(len * bps);
            if s.bits16 {
                let mut prev = 0i16;
                for &v in &m {
                    d.extend_from_slice(&v.wrapping_sub(prev).to_le_bytes());
                    prev = v;
                }
            } else {
                let mut prev = 0i8;
                for &v in &m {
                    let v = v as i8;
                    d.push(v.wrapping_sub(prev) as u8);
                    prev = v;
                }
            }
            datas.push(d);
        }
        for d in datas {
            o.extend_from_slice(&d);
        }
    }
    Some(o)
}

// ---------------------------------------------------------------------------------------------
// IT

/// The MIDI macros libopenmpt makes from the header when it has any and there is no MIDI
/// chunk: 9 global, 16 parametered, 128 fixed macros of 32 bytes.
fn it_macros(song: &Song) -> Option<Vec<u8>> {
    if let Some(m) = &song.midi {
        return Some(m.clone());
    }
    let any = song.sfx_macros.iter().any(|&v| v != 0) || song.fixed_macros.iter().any(|m| m[1] != 0);
    if !any {
        return None;
    }
    // (libopenmpt's default global macros)
    let global: [&[u8]; 9] = [b"FF", b"FC", b"", b"", b"", b"9c n v", b"9c n 0", b"", b"Cc p"];
    let mut o = Vec::with_capacity(4896);
    let put = |o: &mut Vec<u8>, s: &[u8]| text(o, s, 32);
    for g in global {
        put(&mut o, g);
    }
    for &v in &song.sfx_macros {
        let s = if v != 0 { format!("F0F0{:02X}z", v - 1) } else { String::new() };
        put(&mut o, s.as_bytes());
    }
    for m in &song.fixed_macros {
        let s = if m[1] != 0 { format!("F0F0{:02X}{:02X}", m[1] - 1, m[0]) } else { String::new() };
        put(&mut o, s.as_bytes());
    }
    Some(o)
}

fn it_env(o: &mut Vec<u8>, e: &Env, shift: u32, signed: bool) {
    let n = e.nodes(shift);
    let mut flg = 0u8;
    if e.flags & 0x01 != 0 {
        flg |= 1;
    }
    if e.flags & 0x04 != 0 {
        flg |= 2;
    }
    if e.flags & 0x02 != 0 {
        flg |= 4;
    }
    if e.flags & 0x20 != 0 {
        flg |= 8;
    }
    if e.flags & 0x10 != 0 {
        flg |= 0x80;
    }
    o.extend_from_slice(&[flg, n.len() as u8, e.loop_start, e.loop_end, e.sus_start, e.sus_end]);
    for k in 0..25 {
        let (t, v) = n.get(k).copied().unwrap_or((0, if signed { 32 } else { 0 }));
        o.push(if signed { (v as i8).wrapping_sub(32) as u8 } else { v });
        put16(o, t);
    }
    o.push(0);
}

fn it(song: &Song) -> Option<Vec<u8>> {
    let nins = song.instruments.len().min(255);
    let nsmp = song.samples.len().min(255);
    let npat = song.patterns.len().min(255);
    let mut orders: Vec<u8> = song.orders.iter().copied().take(255).collect();
    if orders.last() != Some(&0xFF) {
        orders.push(0xFF);
    }
    let macros = it_macros(song);
    let mut o = b"IMPM".to_vec();
    text(&mut o, &song.title, 26);
    let (rpb, rpm) = song.rows_per_beat.unwrap_or((0, 0));
    o.extend_from_slice(&[rpb, rpm]);
    put16(&mut o, orders.len() as u16);
    put16(&mut o, nins as u16);
    put16(&mut o, nsmp as u16);
    put16(&mut o, npat as u16);
    let cwt = if song.cwtv != 0 { song.cwtv } else { 0x0214 };
    let mut cmwt = if song.cmwt != 0 { song.cmwt } else { 0x0214 };
    if nins > 0 && cmwt < 0x200 {
        cmwt = 0x200;
    }
    put16(&mut o, cwt);
    put16(&mut o, cmwt);
    let mut flags = 1u16;
    if !song.sample_mode {
        flags |= 4;
    }
    if song.flags & hflag::LINEAR != 0 {
        flags |= 8;
    }
    if song.flags & hflag::IT_OLD_FX == 0 {
        flags |= 0x10;
    }
    if song.flags & hflag::IT_COMPAT_GXX == 0 {
        flags |= 0x20;
    }
    let mut special = 0u16;
    if macros.is_some() {
        flags |= 0x80;
        special |= 8;
    }
    if song.rows_per_beat.is_some() {
        special |= 4;
    }
    put16(&mut o, flags);
    put16(&mut o, special);
    o.push(song.global_vol.min(128));
    o.push(preamp(song).min(128) as u8);
    o.push(speed(song));
    o.push(tempo(song));
    o.push(song.pan_separation.min(128));
    o.push(0);
    put16(&mut o, 0);
    put32(&mut o, 0);
    put32(&mut o, 0);
    for i in 0..64 {
        o.push(if i >= song.channels {
            0x80 | 32
        } else {
            match song.chn_pan[i] {
                127 => 100,
                255 => 64,
                p => p / 4,
            }
        });
    }
    for i in 0..64 {
        o.push(song.chn_volume[i].min(64));
    }
    o.extend_from_slice(&orders);
    let ptrs = o.len();
    o.resize(o.len() + 4 * (nins + nsmp + npat), 0);
    if let Some(m) = &macros {
        o.extend_from_slice(m);
    }

    for (i, ins) in song.instruments[..nins].iter().enumerate() {
        let at = o.len() as u32;
        set32(&mut o, ptrs + 4 * i, at);
        o.extend_from_slice(b"IMPI");
        text(&mut o, &ins.filename, 12);
        o.push(0);
        o.extend_from_slice(&[ins.nna, ins.dct, ins.dca]);
        put16(&mut o, (ins.fadeout >> 5).min(1024));
        o.push(ins.pps);
        o.push(ins.ppc);
        o.push(ins.global_vol.min(128));
        o.push(if ins.panning <= 256 { (ins.panning / 4) as u8 } else { 0x80 | 32 });
        o.push(ins.vol_swing.min(100) as u8);
        o.push((ins.pan_swing.min(256) / 4) as u8);
        put16(&mut o, cwt);
        let mut used: Vec<u16> = ins.map.iter().map(|e| e.1).filter(|&s| (s as usize) < nsmp).collect();
        used.sort_unstable();
        used.dedup();
        o.push(used.len() as u8);
        o.push(0);
        text(&mut o, &ins.name, 26);
        o.push(ins.cutoff);
        o.push(ins.resonance);
        o.extend_from_slice(&[0, 0xFF]);
        put16(&mut o, 0xFFFF);
        for e in &ins.map {
            o.push(e.0.min(119) as u8);
            o.push(if (e.1 as usize) < nsmp { (e.1 + 1) as u8 } else { 0 });
        }
        it_env(&mut o, &ins.vol, 0, false);
        it_env(&mut o, &ins.pan, 0, true);
        it_env(&mut o, &ins.pitch, 5, true);
        o.resize(at as usize + 554, 0);
    }

    let mut heads = Vec::with_capacity(nsmp);
    for (i, s) in song.samples[..nsmp].iter().enumerate() {
        let at = o.len();
        set32(&mut o, ptrs + 4 * (nins + i), at as u32);
        heads.push(at);
        let len = frames(s);
        o.extend_from_slice(b"IMPS");
        text(&mut o, &s.filename, 12);
        o.push(0);
        o.push(s.global_vol.min(64));
        let (on, ping, ls, le) = sample_loop(s, len);
        let sus_end = (s.sus_end as usize).min(len) as u32;
        let sus_start = s.sus_start.min(sus_end);
        let sus = s.flags & sflag::SUSTAIN != 0 && sus_end > sus_start;
        let mut flg = 0u8;
        if len > 0 {
            flg |= 1;
        }
        if s.bits16 {
            flg |= 2;
        }
        if s.channels == 2 {
            flg |= 4;
        }
        if on {
            flg |= 0x10;
        }
        if sus {
            flg |= 0x20;
        }
        if on && ping {
            flg |= 0x40;
        }
        if sus && s.flags & sflag::SUSTAIN_PINGPONG != 0 {
            flg |= 0x80;
        }
        o.push(flg);
        o.push(s.volume.min(64));
        text(&mut o, &s.name, 26);
        o.push(1);
        o.push(if s.panning <= 256 { 0x80 | (s.panning / 4) as u8 } else { 32 });
        put32(&mut o, len as u32);
        put32(&mut o, if on { ls } else { 0 });
        put32(&mut o, if on { le } else { 0 });
        put32(&mut o, s.c5speed(song.hertz));
        put32(&mut o, if sus { sus_start } else { 0 });
        put32(&mut o, if sus { sus_end } else { 0 });
        put32(&mut o, 0);
        o.extend_from_slice(&[s.vib_rate, s.vib_depth, s.vib_sweep, s.vib_type]);
    }

    for p in 0..npat {
        let Some(grid) = &song.patterns[p] else { continue };
        let at = o.len();
        set32(&mut o, ptrs + 4 * (nins + nsmp + p), at as u32);
        let mut d = Vec::new();
        for row in grid {
            for (ch, c) in row.iter().enumerate().take(64) {
                let fx = c.fx.filter(|&(t, p)| t != 0 || p != 0);
                let mut mask = 0u8;
                if c.note.is_some() {
                    mask |= 1;
                }
                if c.ins != 0 {
                    mask |= 2;
                }
                if c.vol.is_some() {
                    mask |= 4;
                }
                if fx.is_some() {
                    mask |= 8;
                }
                if mask == 0 {
                    continue;
                }
                d.push((ch as u8 + 1) | 0x80);
                d.push(mask);
                if let Some(n) = c.note {
                    d.push(n);
                }
                if c.ins != 0 {
                    d.push(c.ins);
                }
                if let Some(v) = c.vol {
                    d.push(v);
                }
                if let Some((t, p)) = fx {
                    d.extend_from_slice(&[t, p]);
                }
            }
            d.push(0);
        }
        if d.len() > 0xFFFF {
            return None;
        }
        put16(&mut o, d.len() as u16);
        put16(&mut o, grid.len() as u16);
        put32(&mut o, 0);
        o.extend_from_slice(&d);
    }

    for (i, s) in song.samples[..nsmp].iter().enumerate() {
        let len = frames(s);
        if len == 0 {
            continue;
        }
        let at = o.len() as u32;
        set32(&mut o, heads[i] + 72, at);
        // non-interleaved: all of the left channel, then all of the right
        for ch in 0..s.channels {
            for k in 0..len {
                let v = s.data[k * s.channels + ch];
                if s.bits16 {
                    o.extend_from_slice(&v.to_le_bytes());
                } else {
                    o.push(v as i8 as u8);
                }
            }
        }
    }
    Some(o)
}

// ---------------------------------------------------------------------------------------------
// S3M

fn s3m(song: &Song) -> Option<Vec<u8>> {
    let nsmp = song.samples.len().min(255);
    let npat = song.patterns.len().min(255);
    let mut orders: Vec<u8> = song.orders.iter().copied().take(254).collect();
    if orders.last() != Some(&0xFF) {
        orders.push(0xFF);
    }
    let mut o = Vec::new();
    text(&mut o, &song.title, 28);
    o.extend_from_slice(&[0x1A, 16, 0, 0]);
    put16(&mut o, orders.len() as u16);
    put16(&mut o, nsmp as u16);
    put16(&mut o, npat as u16);
    let mut flags = 0u16;
    if song.flags & hflag::S3M_AMIGA_LIMITS != 0 {
        flags |= 0x10;
    }
    if song.flags & hflag::S3M_FAST_SLIDES != 0 {
        flags |= 0x40;
    }
    put16(&mut o, flags);
    put16(&mut o, if song.cwtv != 0 { song.cwtv } else { 0x1320 });
    put16(&mut o, 2);
    o.extend_from_slice(b"SCRM");
    o.push(song.global_vol.min(64));
    o.push(speed(song));
    o.push(tempo(song));
    o.push(0x80 | preamp(song).min(127) as u8);
    o.push(0);
    o.push(0xFC);
    o.resize(o.len() + 10, 0);
    let pan = |i: usize| -> u8 {
        match song.chn_pan[i] {
            127 => 8,
            p => p >> 4,
        }
    };
    for i in 0..32 {
        o.push(if i < song.channels.min(32) { (i % 8) as u8 + if pan(i) >= 8 { 8 } else { 0 } } else { 0xFF });
    }
    o.extend_from_slice(&orders);
    let pp_ins = o.len();
    o.resize(o.len() + 2 * (nsmp + npat), 0);
    for i in 0..32 {
        o.push(if i < song.channels.min(32) { 0x20 | pan(i) } else { 0 });
    }
    let align = |o: &mut Vec<u8>| {
        let n = o.len().div_ceil(16) * 16;
        o.resize(n, 0);
    };
    let mut heads = Vec::with_capacity(nsmp);
    for (i, s) in song.samples[..nsmp].iter().enumerate() {
        align(&mut o);
        let at = o.len();
        set16(&mut o, pp_ins + 2 * i, (at / 16) as u16);
        heads.push(at);
        let len = frames(s);
        o.push(1);
        text(&mut o, &s.filename, 12);
        o.extend_from_slice(&[0, 0, 0]);
        let (on, _, ls, le) = sample_loop(s, len);
        put32(&mut o, len as u32);
        put32(&mut o, if on { ls } else { 0 });
        put32(&mut o, if on { le } else { 0 });
        o.push(s.volume.min(64));
        o.push(0);
        o.push(0);
        o.push(on as u8 | if s.bits16 { 4 } else { 0 });
        put32(&mut o, s.c5speed(song.hertz));
        o.resize(o.len() + 12, 0);
        text(&mut o, &s.name, 28);
        o.extend_from_slice(b"SCRS");
    }
    for p in 0..npat {
        let Some(grid) = &song.patterns[p] else { continue };
        align(&mut o);
        let at = o.len();
        set16(&mut o, pp_ins + 2 * (nsmp + p), (at / 16) as u16);
        let mut d = vec![0u8, 0];
        for r in 0..64 {
            if let Some(row) = grid.get(r) {
                for (ch, c) in row.iter().enumerate().take(32) {
                    let fx = c.fx.filter(|&(t, p)| t != 0 || p != 0);
                    let mut what = ch as u8;
                    if c.note.is_some() || c.ins != 0 {
                        what |= 0x20;
                    }
                    if c.vol.is_some() {
                        what |= 0x40;
                    }
                    if fx.is_some() {
                        what |= 0x80;
                    }
                    if what & 0xE0 == 0 {
                        continue;
                    }
                    d.push(what);
                    if what & 0x20 != 0 {
                        d.push(c.note.unwrap_or(255));
                        d.push(c.ins);
                    }
                    if let Some(v) = c.vol {
                        d.push(v);
                    }
                    if let Some((t, p)) = fx {
                        d.extend_from_slice(&[t, p]);
                    }
                }
            }
            d.push(0);
        }
        let n = d.len() as u16;
        set16(&mut d, 0, n);
        o.extend_from_slice(&d);
    }
    for (i, s) in song.samples[..nsmp].iter().enumerate() {
        if frames(s) == 0 {
            continue;
        }
        align(&mut o);
        let at = o.len() / 16;
        o[heads[i] + 13] = (at >> 16) as u8;
        set16(&mut o, heads[i] + 14, at as u16);
        for v in mono(s) {
            if s.bits16 {
                o.extend_from_slice(&((v as u16) ^ 0x8000).to_le_bytes());
            } else {
                o.push((v as u8) ^ 0x80);
            }
        }
    }
    Some(o)
}

// ---------------------------------------------------------------------------------------------
// MOD

/// libopenmpt's `ProTrackerPeriodTable` (7 octaves from C-0).
const PERIODS: [u16; 84] = [
    3424, 3232, 3048, 2880, 2712, 2560, 2416, 2280, 2152, 2032, 1920, 1812, //
    1712, 1616, 1524, 1440, 1356, 1280, 1208, 1140, 1076, 1016, 960, 907, //
    856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, //
    428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, //
    214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113, //
    107, 101, 95, 90, 85, 80, 75, 71, 67, 63, 60, 56, //
    53, 50, 47, 45, 42, 40, 37, 35, 33, 31, 30, 28,
];

fn modf(song: &Song) -> Option<Vec<u8>> {
    if song.channels > 32 {
        return None;
    }
    let mut o = Vec::new();
    text(&mut o, &song.title, 20);
    let datas: Vec<Vec<i8>> = song.samples.iter().take(31).map(|s| {
        let mut d = mono8(s);
        d.truncate(0x1FFFE);
        d
    }).collect();
    for i in 0..31 {
        let Some(s) = song.samples.get(i) else {
            o.resize(o.len() + 30, 0);
            put16_at_end(&mut o);
            continue;
        };
        let len = datas[i].len();
        text(&mut o, &s.name, 22);
        o.extend_from_slice(&((len / 2) as u16).to_be_bytes());
        o.push((s.finetune(Kind::Mod) >> 4) as u8 & 0x0F);
        o.push(s.volume.min(64));
        let (on, _, ls, le) = sample_loop(s, len);
        if on {
            o.extend_from_slice(&((ls / 2) as u16).to_be_bytes());
            o.extend_from_slice(&(((le - ls) / 2).max(1) as u16).to_be_bytes());
        } else {
            o.extend_from_slice(&[0, 0, 0, 1]);
        }
    }
    let orders: Vec<u8> = song.orders.iter().copied().filter(|&p| p < 0x80).take(128).collect();
    let orders = if orders.is_empty() { vec![0] } else { orders };
    o.push(orders.len() as u8);
    o.push(if (song.restart as usize) < orders.len() { song.restart as u8 } else { 0x7F });
    let mut ord = [0u8; 128];
    ord[..orders.len()].copy_from_slice(&orders);
    o.extend_from_slice(&ord);
    let magic = match song.channels {
        4 => "M.K.".to_string(),
        n @ 1..=9 => format!("{n}CHN"),
        n => format!("{n:02}CH"),
    };
    o.extend_from_slice(magic.as_bytes());
    let npat = *orders.iter().max().unwrap_or(&0) as usize + 1;
    for p in 0..npat {
        let grid = song.patterns.get(p).and_then(|g| g.as_ref());
        for r in 0..64 {
            for ch in 0..song.channels {
                let c = grid.and_then(|g| g.get(r)).map_or(Cell::default(), |row| row[ch]);
                let period = match c.note {
                    Some(n @ 1..=84) => PERIODS[n as usize - 1],
                    _ => 0,
                };
                let (t, p) = c.fx.unwrap_or((0, 0));
                o.push((c.ins & 0xF0) | (period >> 8) as u8);
                o.push(period as u8);
                o.push((c.ins << 4) | (t & 0x0F));
                o.push(p);
            }
        }
    }
    for d in &datas {
        o.extend(d.iter().map(|&v| v as u8));
        if d.len() % 2 == 1 {
            o.push(0);
        }
    }
    Some(o)
}

/// An empty MOD sample header ends with a loop length of one word.
fn put16_at_end(o: &mut Vec<u8>) {
    let n = o.len();
    o[n - 1] = 1;
}

// ---------------------------------------------------------------------------------------------
// MTM

fn mtm(song: &Song) -> Option<Vec<u8>> {
    if song.channels > 32 {
        return None;
    }
    let nsmp = song.samples.len().min(63);
    let orders: Vec<u8> = song.orders.iter().copied().take(128).collect();
    let orders = if orders.is_empty() { vec![0] } else { orders };
    let npat = song.patterns.len().clamp(1, 256);
    // tracks: one per distinct column of 64 rows
    let mut tracks: Vec<[u8; 192]> = Vec::new();
    let mut seq: Vec<[u16; 32]> = Vec::with_capacity(npat);
    for p in 0..npat {
        let grid = song.patterns.get(p).and_then(|g| g.as_ref());
        let mut idx = [0u16; 32];
        for (ch, slot) in idx.iter_mut().enumerate().take(song.channels) {
            let mut t = [0u8; 192];
            for r in 0..64 {
                let c = grid.and_then(|g| g.get(r)).map_or(Cell::default(), |row| row[ch]);
                let note = c.note.unwrap_or(0) & 0x3F;
                let (fx, prm) = c.fx.unwrap_or((0, 0));
                let ins = c.ins & 0x3F;
                t[r * 3] = (note << 2) | (ins >> 4);
                t[r * 3 + 1] = ((ins & 0x0F) << 4) | (fx & 0x0F);
                t[r * 3 + 2] = prm;
            }
            if t.iter().all(|&b| b == 0) {
                continue;
            }
            *slot = match tracks.iter().position(|x| *x == t) {
                Some(k) => k as u16 + 1,
                None => {
                    tracks.push(t);
                    tracks.len() as u16
                }
            };
        }
        seq.push(idx);
    }
    let mut o = b"MTM".to_vec();
    o.push(song.mtm_version.unwrap_or(0x10));
    text(&mut o, &song.title, 20);
    put16(&mut o, tracks.len() as u16);
    o.push((npat - 1) as u8);
    o.push((orders.len() - 1) as u8);
    put16(&mut o, 0);
    o.push(nsmp as u8);
    o.push(0);
    o.push(64);
    o.push(song.channels as u8);
    for i in 0..32 {
        o.push(song.chn_pan[i] >> 4);
    }
    let datas: Vec<Vec<i8>> = song.samples[..nsmp].iter().map(mono8).collect();
    for (s, d) in song.samples[..nsmp].iter().zip(&datas) {
        text(&mut o, &s.name, 22);
        let (on, _, ls, le) = sample_loop(s, d.len());
        put32(&mut o, d.len() as u32);
        put32(&mut o, if on { ls } else { 0 });
        // (libopenmpt's MTM loader takes one off the loop end)
        put32(&mut o, if on { le + 1 } else { 0 });
        o.push(s.finetune(Kind::Mtm) as u8);
        o.push(s.volume.min(64));
        o.push(0);
    }
    let mut ord = [0u8; 128];
    ord[..orders.len()].copy_from_slice(&orders);
    o.extend_from_slice(&ord);
    for t in &tracks {
        o.extend_from_slice(t);
    }
    for idx in &seq {
        for &t in idx {
            put16(&mut o, t);
        }
    }
    for d in &datas {
        o.extend(d.iter().map(|&v| (v as u8) ^ 0x80));
    }
    Some(o)
}
