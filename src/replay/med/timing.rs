//! The original's start-up time, from counters of the module file (colour clocks, 2 CPU cycles each).
//!
//! Behaviour model fitted to the original's runs: the start-up is a chain of calls, each costing a
//! constant plus unit costs times counts of things in the file (pointers relocated, instruments,
//! extension entries ...). A relocated pointer costs 37.5 clocks if non-zero and 31.5 if zero.

fn u8_(d: &[u8], o: usize) -> u32 {
    d.get(o).copied().unwrap_or(0) as u32
}
fn u16_(d: &[u8], o: usize) -> u32 {
    u8_(d, o) << 8 | u8_(d, o + 1)
}
fn u32_(d: &[u8], o: usize) -> u32 {
    u16_(d, o) << 16 | u16_(d, o + 2)
}

/// Counters of the file that the start-up cost depends on.
#[derive(Default, Debug)]
pub struct Counts {
    songs: f64,
    nz: f64,
    z: f64,
    s_it: f64,
    s_nzp: f64,
    s_syn: f64,
    s_wf: f64,
    b_it: f64,
    m1: f64,
    bi_it: f64,
    bi_bi: f64,
    bi_pages: f64,
    m2: f64,
    m2_pseq: f64,
    noexp: bool,
    // module info
    smp: f64,
    syn: f64,
    zp: f64,
    sections: f64,
    songname: bool,
    alen2: bool,
    aptr: bool,
    extra: bool,
    // init sound
    ntv: f64,
    mmd2: bool,
    ext: bool,
    classes: [f64; 5],
    srep_lo: f64,
    ent_raw: u32,
    srep63: f64,
    ptr_nz_ent0: bool,
    hasexp: bool,
    bpm: bool,
    tempo: u32,
    filter: bool,
}

impl Counts {
    fn rel(&mut self, v: u32) {
        if v != 0 {
            self.nz += 1.0;
        } else {
            self.z += 1.0;
        }
    }
}

/// Pointer slots of the expansion data that the original relocates (offsets in the structure).
const EXP_SLOTS: [usize; 15] = [0x0, 0x4, 0xc, 0x14, 0x1c, 0x20, 0x24, 0x28, 0x2c, 0x34, 0x38, 0x3c, 0x40, 0x44, 0x48];

fn walk_header(d: &[u8], hdr: usize, first: bool, c: &mut Counts) {
    let typ = u8_(d, hdr + 3);
    c.songs += 1.0;
    let song = u32_(d, hdr + 8) as usize;
    let blockarr = u32_(d, hdr + 0x10) as usize;
    let smplarr = u32_(d, hdr + 0x18) as usize;
    let exp = u32_(d, hdr + 0x20) as usize;
    for v in [song as u32, blockarr as u32, smplarr as u32, exp as u32] {
        c.rel(v);
    }
    if first {
        let mut n = u8_(d, song + 0x313) as usize;
        if n == 0 {
            n = 256;
        }
        c.s_it += n as f64;
        c.ntv = 16.0;
        for i in 0..n {
            let p = u32_(d, smplarr + 4 * i) as usize;
            c.rel(p as u32);
            if p != 0 {
                c.s_nzp += 1.0;
                if i < 63 || true {
                    let t = u16_(d, p + 4) as u16 as i16;
                    if t < 0 {
                        c.s_syn += 1.0;
                        let mut w = u16_(d, p + 0x14) as f64;
                        if w == 0.0 {
                            w = 65536.0;
                        }
                        c.s_wf += w;
                        c.syn += 1.0;
                    } else {
                        c.smp += 1.0;
                    }
                }
            } else {
                c.zp += 1.0;
            }
        }
    }
    let nb = u16_(d, song + 0x1f8) as usize;
    c.b_it += nb as f64;
    for b in 0..nb {
        c.rel(u32_(d, blockarr + 4 * b));
    }
    if typ >= b'1' as u32 {
        c.m1 += 1.0;
        c.bi_it += nb as f64;
        for b in 0..nb {
            let blk = u32_(d, blockarr + 4 * b) as usize;
            let info = u32_(d, blk + 4);
            c.rel(info);
            if info != 0 {
                let info = info as usize;
                c.bi_bi += 1.0;
                c.rel(u32_(d, info));
                c.rel(u32_(d, info + 4));
                let pt = u32_(d, info + 0xc);
                c.rel(pt);
                if pt != 0 {
                    let mut np = u16_(d, pt as usize) as usize;
                    if np == 0 {
                        np = 65536;
                    }
                    c.bi_pages += np as f64;
                    for k in 1..=np.min(4096) {
                        c.rel(u32_(d, pt as usize + 4 * k));
                    }
                }
            }
        }
    }
    if typ >= b'2' as u32 {
        c.m2 += 1.0;
        let np = u16_(d, song + 0x20a) as usize;
        c.m2_pseq += np as f64;
        let pst = u32_(d, song + 0x1fc);
        for v in [pst, u32_(d, song + 0x200), u32_(d, song + 0x204), u32_(d, song + 0x20c)] {
            c.rel(v);
        }
        for i in 0..np {
            c.rel(u32_(d, pst as usize + 4 * i));
        }
    }
    if exp != 0 {
        for s in EXP_SLOTS {
            c.rel(u32_(d, exp + s));
        }
    }
}

fn counts(d: &[u8]) -> Counts {
    let mut c = Counts::default();
    let exp0 = u32_(d, 0x20) as usize;
    c.noexp = exp0 == 0;
    c.hasexp = exp0 != 0;
    walk_header(d, 0, true, &mut c);
    // extra songs: the continuation modules the expansion data chains
    let extra = u8_(d, 0x33);
    c.extra = extra != 0;
    let mut hdr = 0usize;
    for _ in 0..extra {
        let exp = u32_(d, hdr + 0x20) as usize;
        if exp == 0 {
            break;
        }
        let next = u32_(d, exp) as usize;
        if next == 0 || next + 0x34 > d.len() {
            break;
        }
        hdr = next;
        walk_header(d, hdr, false, &mut c);
    }
    let typ = u8_(d, 3);
    let song = u32_(d, 8) as usize;
    c.mmd2 = typ >= b'2' as u32;
    if c.mmd2 {
        c.sections = u16_(d, song + 0x1fa) as f64;
        c.ntv = u16_(d, song + 0x208) as f64;
    }
    if exp0 != 0 {
        c.songname = u32_(d, exp0 + 0x2c) != 0;
        let alen = u32_(d, exp0 + 0x10);
        c.alen2 = alen >= 2;
        c.aptr = u32_(d, exp0 + 0xc) != 0;
        c.ent_raw = u16_(d, exp0 + 8);
        c.ptr_nz_ent0 = u32_(d, exp0 + 4) != 0;
        c.ext = c.ptr_nz_ent0 && c.ent_raw != 0;
        let size = u16_(d, exp0 + 0xa);
        let n = c.ent_raw.min(63) as usize;
        let cls = match size {
            0..=3 => 0,
            4..=5 => 1,
            6..=8 => 2,
            9..=17 => 3,
            _ => 4,
        };
        c.classes[cls] = n as f64;
        if size < 6 {
            for i in 0..n {
                if u16_(d, song + 8 * i + 2) > 1 {
                    c.srep_lo += 1.0;
                }
            }
        }
    }
    for i in 0..63 {
        if u16_(d, song + 8 * i + 2) > 1 {
            c.srep63 += 1.0;
        }
    }
    let flags2 = u8_(d, song + 0x300);
    c.bpm = flags2 & 0x20 != 0;
    c.tempo = u16_(d, song + 0x2fc);
    c.filter = u8_(d, song + 0x2ff) & 1 != 0;
    c
}

fn c2(c: &Counts) -> f64 {
    (if c.noexp { -192.0 } else { 0.0 })
        + 53220.5
        + (331.0 - 126.0) * c.songs
        + 37.5 * c.nz
        + 31.5 * c.z
        + 17.5 * c.s_it
        + 12.5 * c.s_nzp
        + 12.0 * c.s_syn
        + 15.0 * c.s_wf
        + 5.0 * c.b_it
        + 29.0 * c.bi_it
        + 26.5 * c.bi_bi
        + 21.0 * c.bi_pages
        + 14.5 * c.m1
        + 49.0 * c.m2
        + 5.0 * c.m2_pseq
}

fn c4(c: &Counts) -> f64 {
    (if c.hasexp { 0.0 } else { -31.0 })
        + 308.0
        + 40.0 * c.smp
        + 35.0 * c.syn
        + 15.5 * c.zp
        + 56.5 * c.sections
        + if c.mmd2 { 9.0 } else { 0.0 }
        + if c.songname { 8.0 } else { 0.0 }
        + if c.alen2 { 10.3 } else { 0.0 }
        + if c.alen2 && c.aptr { 10.3 } else { 0.0 }
        + if c.extra { 20.0 } else { 0.0 }
}

const ENTRY_COST: [f64; 5] = [135.5, 145.5, 148.0, 158.0, 151.0];

fn c5(c: &Counts) -> f64 {
    let mut v = 629.0 + 57.0 * c.ntv + if c.mmd2 { 78.0 } else { 0.0 };
    if c.ext {
        for k in 0..5 {
            v += ENTRY_COST[k] * c.classes[k];
        }
        v += 10.0 * c.srep_lo;
        if c.ent_raw > 63 {
            v += 3.5;
        }
    } else {
        v += 5783.0 + 9.5 * c.srep63;
        if c.ent_raw == 0 && c.hasexp && c.ptr_nz_ent0 {
            v += 14.0;
        }
    }
    v
}

fn c6(c: &Counts) -> f64 {
    let delta = if c.bpm {
        312.0
    } else if c.tempo <= 10 {
        223.0
    } else {
        277.0
    };
    c5(c) + delta - if c.filter { 5.0 } else { 0.0 }
}

/// The clocks from the start of the first call (the module check) to the write that starts the
/// CIA timer, for an MMD0/1/2 file.
pub fn init_clocks(d: &[u8]) -> f64 {
    let c = counts(d);
    127.0 + c2(&c).floor() + 4541.0 + c4(&c).round() + 26.0 + c5(&c).floor() + c6(&c).floor() - 14.0
}

/// The same for a MED 2.10 file.
pub fn med4_init_clocks(d: &[u8]) -> Option<f64> {
    super::timing4::init_clocks(d)
}
