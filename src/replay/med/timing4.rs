//! Start-up time of a MED 2.10 ("MED\x04") file: the original converts the file to an in-memory MMD0
//! first, and that conversion's cost is a sum of unit costs times counts of things in the file
//! (nibbles fetched from the packed blocks, lines, cells, samples' bytes ...), in colour clocks.
//! The counts are those of the `to_mmd0` conversion; the unit costs were measured on the original's
//! runs. A few terms the model does not capture are added as an empirical correction
//! (see `CORRECTION`), so the result is good to well under 1 % but not exact for every file.

fn u16_(d: &[u8], o: usize) -> Option<usize> {
    Some((*d.get(o)? as usize) << 8 | *d.get(o + 1)? as usize)
}
fn u32_(d: &[u8], o: usize) -> Option<usize> {
    Some(u16_(d, o)? << 16 | u16_(d, o + 2)?)
}

#[derive(Default)]
struct F {
    mask_bytes: f64,
    fb4: f64,
    fb10: f64,
    fb20: f64,
    fld_1: f64,
    fld_2: f64,
    fld_30: f64,
    fld_40: f64,
    allocs: f64,
    blocks: f64,
    ch_medv: f64,
    chunks: f64,
    ch_anno: f64,
    ch_hldc: f64,
    anno_bytes: f64,
    samples: f64,
    synth: f64,
    bytes: f64,
    vlen_sum: f64,
    wlen_sum: f64,
    nwf4: f64,
    synth_wf_bytes: f64,
    n: f64,
    odd_blocks: f64,
    wcalls: f64,
    witers: f64,
    hlen_sum: f64,
    mask_load: f64,
    bflag_bytes: f64,
    lm_all: f64,
    lm_read: f64,
    em_all: f64,
    em_read: f64,
    lines: f64,
    lm_trk_iter: f64,
    lm_cells: f64,
    em_trk_iter: f64,
    em_cells: f64,
    l32: f64,
    songlen: f64,
    tempo_le10: f64,
    tempo_gt10: f64,
    // for the correction
    ln_none: f64,
    ln_note: f64,
    ln_eff: f64,
    ln_both: f64,
}

/// Counts what the conversion does for the file `d`.
fn count(d: &[u8]) -> Option<F> {
    let mut f = F::default();
    let mut p = 4usize;
    let mut mm = *d.get(p)?;
    p += 1;
    let (mut mask, mut n) = (0u64, 0u32);
    for _ in 0..8 {
        if mm & 0x80 != 0 {
            mask = mask << 8 | *d.get(p)? as u64;
            p += 1;
            n += 1;
        }
        mm <<= 1;
    }
    f.mask_bytes = n as f64;
    mask = mask.checked_shl(64 - n * 8).unwrap_or(0);
    let mut named = [false; 63];
    for nm in &mut named {
        if mask & 1 << 63 != 0 {
            let flag = *d.get(p)?;
            let len = *d.get(p + 1)? as usize;
            *nm = d.get(p + 2..p + 2 + len)?.iter().any(|&c| c != 0);
            p += 2 + len;
            f.fb4 += 1.0;
            if flag & 1 == 0 {
                p += 2;
                f.fld_1 += 1.0;
            }
            if flag & 2 == 0 {
                p += 2;
                f.fld_2 += 1.0;
            }
            if flag & 4 == 0 {
                p += 1;
            }
            if flag & 8 == 0 {
                p += 1;
            }
            if flag & 0x30 == 0 {
                p += 1;
                f.fld_30 += 1.0;
            }
            if flag & 0x40 == 0 {
                p += 1;
                f.fld_40 += 1.0;
            }
            f.fb10 += (flag & 0x10 != 0) as u8 as f64;
            f.fb20 += (flag & 0x20 != 0) as u8 as f64;
        }
        mask <<= 1;
    }
    let nb = u16_(d, p)?;
    p += 2;
    let sl = u16_(d, p)?;
    p += 2;
    f.blocks = nb as f64;
    f.songlen = sl as f64;
    p += sl;
    let tempo = u16_(d, p)?;
    p += 2 + 2 + 2 + 20 + 16 + 1;
    f.tempo_le10 = (tempo <= 10) as u8 as f64;
    f.tempo_gt10 = (tempo > 10) as u8 as f64;
    let (mut singles, mut wi, mut oddb) = (0usize, 0usize, 0usize);
    let mut wc = 0usize;
    let (mut hdrn, mut lmr, mut emr, mut bfb, mut ml) = (0usize, 0usize, 0usize, 0usize, 0usize);
    for _ in 0..nb {
        let hlen = *d.get(p)? as usize;
        p += 1;
        let hs = p;
        let tracks = *d.get(p)? as usize;
        let rows = *d.get(p + 1)? as usize;
        let dlen = u16_(d, p + 2)?;
        p += 4;
        f.hlen_sum += hlen as f64;
        let mut bfl = 0u32;
        let mut i = rows as i32 + 1;
        let mut nbf = 0;
        while i > 0 {
            bfl = bfl << 8 | *d.get(p)? as u32;
            p += 1;
            i -= 64;
            nbf += 1;
        }
        bfb += nbf;
        bfl = bfl.checked_shl(32 - ((rows as u32 + 64) / 64) * 8).unwrap_or(0);
        let (mut lmask, mut emask) = ([0u32; 8], [0u32; 8]);
        let mut j = rows as i32 + 1;
        let mut k = 0;
        while j > 0 {
            ml += 1;
            lmask[k] = if bfl & 0x8000_0000 != 0 {
                f.lm_all += 1.0;
                !0
            } else if bfl & 0x4000_0000 != 0 {
                0
            } else {
                lmr += 1;
                u32_(d, p)? as u32
            };
            if bfl & 0x8000_0000 == 0 && bfl & 0x4000_0000 == 0 {
                p += 4;
            }
            emask[k] = if bfl & 0x2000_0000 != 0 {
                f.em_all += 1.0;
                !0
            } else if bfl & 0x1000_0000 != 0 {
                0
            } else {
                emr += 1;
                u32_(d, p)? as u32
            };
            if bfl & 0x2000_0000 == 0 && bfl & 0x1000_0000 == 0 {
                p += 4;
            }
            bfl <<= 4;
            j -= 32;
            k += 1;
        }
        p = hs + hlen;
        let data = d.get(p..p + dlen)?;
        p += dlen;
        hdrn += 8;
        let mut nib = 0usize;
        let mut get = |cnt: usize| -> Option<usize> {
            let mut v = 0usize;
            for _ in 0..cnt {
                let b = *data.get(nib / 2)?;
                v = v << 4 | if nib & 1 != 0 { b & 15 } else { b >> 4 } as usize;
                nib += 1;
            }
            Some(v)
        };
        let (mut lm, mut em) = (0u32, 0u32);
        for i in 0..=rows {
            f.lines += 1.0;
            if i % 32 == 0 {
                lm = lmask[i / 32];
                em = emask[i / 32];
            }
            if i % 32 == 31 {
                f.l32 += 1.0;
            }
            let mut had = 0;
            for kind in 0..2 {
                let m = if kind == 0 { lm } else { em };
                if m & 0x8000_0000 == 0 {
                    continue;
                }
                had |= 1 << kind;
                let kk = tracks / 4;
                let mut cm = get(kk)?;
                if kk == 1 {
                    singles += 1;
                } else {
                    wc += 1;
                    wi += kk;
                }
                cm = (cm << 16usize.saturating_sub(tracks)) & 0xffff;
                if kind == 0 {
                    f.lm_trk_iter += tracks as f64;
                } else {
                    f.em_trk_iter += tracks as f64;
                }
                for _ in 0..tracks {
                    if cm & 0x8000 != 0 {
                        if kind == 0 {
                            f.lm_cells += 1.0;
                            get(2)?;
                            get(1)?;
                        } else {
                            f.em_cells += 1.0;
                            get(1)?;
                            get(2)?;
                        }
                        wc += 1;
                        wi += 2;
                        singles += 1;
                    }
                    cm <<= 1;
                }
            }
            match had {
                0 => f.ln_none += 1.0,
                1 => f.ln_note += 1.0,
                2 => f.ln_eff += 1.0,
                _ => f.ln_both += 1.0,
            }
            lm <<= 1;
            em <<= 1;
        }
        if nib & 1 != 0 {
            oddb += 1;
        }
    }
    let hdr_nibbles = hdrn + 2 * bfb + 8 * (lmr + emr);
    f.n = (singles + wi + hdr_nibbles) as f64;
    f.wcalls = (wc + singles) as f64;
    f.witers = wi as f64;
    f.odd_blocks = oddb as f64;
    f.mask_load = ml as f64;
    f.bflag_bytes = bfb as f64;
    f.lm_read = lmr as f64;
    f.em_read = emr as f64;
    // the samples and synth sounds
    let nsamples = named.iter().rposition(|&x| x).map_or(0, |i| i + 1);
    let _ = nsamples;
    let mut smask = (u32_(d, p)? as u64) << 32 | u32_(d, p + 4)? as u64;
    p += 8;
    for _ in 0..63 {
        smask <<= 1;
        if smask & 1 << 63 == 0 {
            continue;
        }
        let ln = u32_(d, p)?;
        let typ = u16_(d, p + 4)?;
        if typ == 0xffff || typ == 0xfffe {
            f.synth += 1.0;
            let start = p + 6;
            let vlen = u16_(d, start + 14)?;
            let wlen = u16_(d, start + 16)?;
            let nw = u16_(d, start + 20)?;
            f.vlen_sum += vlen as f64;
            f.wlen_sum += wlen as f64;
            f.nwf4 += nw as f64;
            let mut end = start + 22 + vlen + wlen + 4 * nw;
            for k in 0..nw {
                let o = u32_(d, start + 22 + vlen + wlen + 4 * k)?;
                let mut r = start.checked_add(o)?;
                if k == 0 && typ == 0xfffe {
                    let l = u32_(d, r)?;
                    r += 6;
                    f.synth_wf_bytes += l as f64;
                    r += l;
                } else {
                    let l = u16_(d, r)?;
                    r += 2;
                    f.synth_wf_bytes += 2.0 * l as f64;
                    r += 2 * l;
                }
                end = end.max(r);
            }
            p = end;
        } else {
            f.samples += 1.0;
            f.bytes += ln as f64;
            p += 6 + ln;
        }
    }
    f.allocs = 1.0 + nb as f64 + f.samples + f.synth;
    // the trailing chunks
    let tail = d.len().saturating_sub(1024);
    if let Some(i) = d[tail..].windows(4).position(|w| w == b"MEDV") {
        let mut q = tail + i;
        while q + 8 <= d.len() {
            let id = &d[q..q + 4];
            let len = u32_(d, q + 4)?;
            q += 8 + len;
            f.chunks += 1.0;
            match id {
                b"MEDV" => f.ch_medv = 1.0,
                b"ANNO" => {
                    f.ch_anno = 1.0;
                    f.anno_bytes += len as f64;
                }
                b"HLDC" => f.ch_hldc = 1.0,
                _ => {}
            }
        }
    }
    Some(f)
}

/// Empirical corrections: clocks per count the ledger does not account for (fitted on the
/// 21 reference files; the largest remaining error is about 8000 clocks of up to 3.8 million).
const CORRECTION: [(f64, fn(&F) -> f64); 7] = [
    (2.69, |f| f.ln_none),
    (-13.88, |f| f.ln_note),
    (-37.08, |f| f.ln_eff),
    (5.78, |f| f.ln_both),
    (1024.85, |f| f.synth),
    (302.64, |f| f.lm_read),
    (196.80, |f| f.em_read),
];

/// The clocks from the start of the first call to the write that starts the CIA timer.
pub fn init_clocks(d: &[u8]) -> Option<f64> {
    let f = count(d)?;
    let f = &f;
    // call 0.0
    let c0 = (1.0) * 203.5
        + (8.0) * 24.5
        + (f.mask_bytes) * 4.0
        + (63.0) * 24.0
        + (f.fb4) * 69.0
        + (f.fld_1) * 4.0
        + (f.fld_2) * 4.0
        + (-1.0 * f.fb10 + f.fb4) * 9.5
        + (f.fld_30) * 4.0
        + (f.fld_40) * 4.0
        + (7.0) * 4.0;
    // call 1.0
    let c1 = (1.0) * 1146.5
        + (f.allocs) * 137.5
        + (4774.0) * 11.0
        + (f.blocks) * 375.5
        + (-1.0 * f.ch_medv + 1.0) * 16.0
        + (f.ch_medv) * 48.0
        + (f.chunks) * 49.0
        + (8.0 * f.chunks) * 11.0
        + (f.ch_anno + f.ch_hldc) * 11.5
        + (f.ch_hldc) * 24.0
        + (64.0 * f.ch_hldc) * 9.5
        + (63.0 * f.ch_hldc) * 21.0
        + (f.ch_anno) * 81.5
        + (f.anno_bytes) * 11.0
        + (8.0) * 35.5
        + (63.0) * 61.5
        + (f.samples + f.synth) * 20.5
        + (6.0 * f.samples + 6.0 * f.synth) * 11.0
        + (f.synth) * 216.5
        + (f.samples) * 62.5
        + (f.bytes) * 14.5
        + (18.0 * f.synth) * 11.0
        + (f.vlen_sum) * 11.0
        + (f.wlen_sum) * 11.0
        + (f.nwf4) * 11.0
        + (0.25 * f.nwf4) * 57.5
        + (f.synth_wf_bytes) * 11.0
        + (0.25 * f.nwf4 + f.samples) * 71.0
        + (f.blocks + f.synth) * 66.0
        + (f.n) * 57.5
        + (0.5 * f.n + 0.5 * f.odd_blocks) * 18.0
        + (0.5 * f.n - 0.5 * f.odd_blocks) * 10.0
        + (f.wcalls) * 18.0
        + (f.witers) * 23.5
        + (f.hlen_sum) * 11.0
        + (f.mask_load) * 72.0
        + (2.0 * f.bflag_bytes - 1.0 * f.mask_load) * 17.0
        + (-1.0 * f.lm_all + f.mask_load) * 11.5
        + (f.lm_read) * 49.0
        + (-1.0 * f.em_all + f.mask_load) * 11.5
        + (f.em_read) * 49.0
        + (f.lines) * 138.0
        + (0.25 * f.lm_trk_iter) * 55.0
        + (f.lm_trk_iter) * 22.5
        + (f.lm_cells) * 41.0
        + (0.25 * f.em_trk_iter) * 55.0
        + (f.em_trk_iter) * 22.5
        + (f.em_cells) * 40.0
        + (f.l32) * 8.0
        + (f.mask_bytes) * 4.0
        + (f.fb4) * 78.0
        + (f.fld_1) * 16.0
        + (f.fld_2) * 16.0
        + (f.fb10) * 13.0
        + (-1.0 * f.fb10 + f.fb4) * 9.5
        + (f.fb20) * 13.0
        + (f.fld_30) * 8.0
        + (f.fld_40) * 8.0
        + (-1.0 * f.fb4 + 63.0) * 20.0
        + (7.0) * 4.0
        + (f.songlen) * 11.0
        + (17.0) * 11.0
        + (4.0) * 19.0;
    // call 2.0
    let c2 = (1.0) * 107.0
        + (400.0) * 11.0;
    // call 3.0
    let c3 = (1.0) * 300.5
        + (63.0) * 15.5
        + (f.samples + f.synth) * 17.5
        + (f.samples) * 7.0
        + (f.synth) * 2.0
        + (f.ch_anno) * 20.5;
    // call 4.0
    let c4 = (1.0) * 26.0;
    // call 5.0
    let c5 = (1.0) * 505.0
        + (16.0) * 57.0
        + (4.0) * 31.0
        + (63.0) * 145.5
        + (f.fld_1) * 10.0;
    // call 6.0
    let c6 = (1.0) * 712.0
        + (f.tempo_le10) * 16.0
        + (f.tempo_gt10) * 70.0
        + (16.0) * 57.0
        + (4.0) * 31.0
        + (63.0) * 145.5
        + (f.fld_1) * 10.0;
    let corr: f64 = CORRECTION.iter().map(|(k, g)| k * g(f)).sum();
    Some(c0 + c1 + c2 + c3 + c4 + c5 + c6 - 14.0 - 4.1 - 0.04 * f.allocs - 8.6 + corr)
}
