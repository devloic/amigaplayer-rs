use super::*;
use crate::machine::Machine;

/// Builds a one-hunk module in code: header, an init routine that stores a volume through a
/// hardware address written in the driver's stripped form, tick routine B with three voices'
/// worth of the replaced shapes (TRIPLE for voice 0, RESTART for voices 0 to 2), a marker long and
/// a mask word. `mask` is the value of the driver's mask word.
fn module(mask: u16) -> Vec<u8> {
    let mut h: Vec<u8> = vec![0; 0x60];
    let mut relocs: Vec<u32> = Vec::new();
    let w = |h: &mut Vec<u8>, v: u16| h.extend(v.to_be_bytes());
    let l = |h: &mut Vec<u8>, v: u32| h.extend(v.to_be_bytes());
    h[0..4].copy_from_slice(&[0x70, 0xFF, 0x4E, 0x75]);
    h[4..12].copy_from_slice(b"RIFFRAFF");
    // names and the sample table are filled in below; init at 0x60
    // init: store 0x40 to voice 0's volume (address word pair 00DF 00A8 before the fix-up), return
    for v in [0x33FCu16, 0x0040, 0x00DF, 0x00A8, 0x4E75] {
        w(&mut h, v);
    }
    let b = h.len();
    // B: copy the mask word, point a0 at the record and a1 at voice 0's chip base, TRIPLE
    w(&mut h, 0x33F9);
    relocs.push(h.len() as u32);
    l(&mut h, 0); // mask pointer, patched below
    relocs.push(h.len() as u32);
    l(&mut h, 0); // destination, patched below
    w(&mut h, 0x41F9);
    relocs.push(h.len() as u32);
    l(&mut h, 0); // record pointer, patched below
    for chip in [0x0000u16, 0x0010, 0x0020] {
        for v in [0x43F9, 0x00DF, chip] {
            w(&mut h, v);
        }
        if chip == 0 {
            for v in [0x2368u16, 0x0002, 0x00A0, 0x3368, 0x0006, 0x00A4, 0x3368, 0x0008, 0x00A6] {
                w(&mut h, v);
            }
        }
        for v in [0x2168u16, 0x002A, 0x001C] {
            w(&mut h, v);
        }
    }
    w(&mut h, 0x4E75);
    // the marker long ends the region
    l(&mut h, MARKER);
    let mask_at = h.len();
    w(&mut h, mask);
    w(&mut h, 0);
    let rec = h.len();
    // record: volume 0, location 0x1234, length at +6, period at +8, loop pointer at +0x2A
    let mut r = vec![0u8; 0x40];
    r[2..6].copy_from_slice(&0x0001_2345u32.to_be_bytes());
    r[6..8].copy_from_slice(&0x0020u16.to_be_bytes());
    r[8..10].copy_from_slice(&0x01ACu16.to_be_bytes());
    r[0x2A..0x2E].copy_from_slice(&0x0002_0000u32.to_be_bytes());
    h.extend(&r);
    let name = h.len();
    h.extend(b"Song\0\0\0\0");
    // sample address table: one entry, non-zero (no fill)
    let tab = h.len();
    l(&mut h, 0x0001_0000);
    let tab_end = h.len();
    w(&mut h, 0);
    let mut put = |h: &mut Vec<u8>, o: usize, v: u32| h[o..o + 4].copy_from_slice(&v.to_be_bytes());
    put(&mut h, 0x0C, b as u32);
    put(&mut h, 0x10, 0x20 + 0x40);
    put(&mut h, 0x14, 1);
    put(&mut h, 0x18, 0);
    put(&mut h, 0x1C, name as u32);
    put(&mut h, 0x20, name as u32);
    put(&mut h, 0x24, name as u32);
    put(&mut h, 0x28, tab as u32);
    put(&mut h, 0x2C, tab_end as u32);
    // the init routine sits at 0x60, where the header ends
    put(&mut h, 0x10, 0x60);
    // pointers inside B
    put(&mut h, b + 2, mask_at as u32);
    put(&mut h, b + 6, mask_at as u32 + 2);
    put(&mut h, b + 12, rec as u32);
    relocs.extend([0x0C, 0x10, 0x1C, 0x20, 0x24, 0x28, 0x2C]);
    while h.len() % 4 != 0 {
        h.push(0);
    }
    let mut f = Vec::new();
    for v in [0x3F3u32, 0, 1, 0, 0, 0x4000_0000 | (h.len() as u32 / 4), 0x3E9, h.len() as u32 / 4] {
        f.extend(v.to_be_bytes());
    }
    f.extend(&h);
    f.extend(0x3ECu32.to_be_bytes());
    f.extend((relocs.len() as u32).to_be_bytes());
    f.extend(0u32.to_be_bytes());
    for o in &relocs {
        f.extend(o.to_be_bytes());
    }
    f.extend(0u32.to_be_bytes());
    f.extend(0x3F2u32.to_be_bytes());
    f
}

fn start(d: &[u8]) -> (Machine, Box<dyn Replayer>) {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    let base = m.alloc(d.len() as u32);
    m.bus.write_bytes(base, d);
    let r = new(d, base, &mut m).expect("recognised");
    (m, r)
}

#[test]
fn detect_checks_the_header() {
    let d = module(0xF);
    assert!(detect(&d));
    for (o, v) in [(3u32, 0u8), (0x14, 0), (0x28, b'X'), (0x24, b'X'), (0x2F, 0), (0x33, 0), (0x37, 0)] {
        let mut bad = d.clone();
        bad[o as usize] = v;
        if o == 0x2F || o == 0x33 || o == 0x37 {
            bad[o as usize - 3..=o as usize].fill(0);
        }
        assert!(!detect(&bad), "offset {o:#x}");
    }
    assert!(!detect(&d[..0x40]));
}

#[test]
fn malformed_files_are_not_recognised_or_harmless() {
    let d = module(0xF);
    let mut m = Machine::new(CpuType::M68000);
    // truncated hunk: fewer bytes than the size long says
    assert!(new(&d[..0x80], 0, &mut m).is_none());
    // a routine pointer past the end of the hunk
    let mut bad = d.clone();
    bad[0x20 + 0x0C..0x20 + 0x10].copy_from_slice(&0x00FF_0000u32.to_be_bytes());
    assert!(new(&bad, 0, &mut m).is_none());
    // a sample table end far past the hunk is cut, not looped over
    let mut bad = d.clone();
    bad[0x20 + 0x2C..0x20 + 0x30].copy_from_slice(&0xFFFF_FFF0u32.to_be_bytes());
    let _ = new(&bad, 0, &mut m);
}

#[test]
fn site_keys_parse_into_ten_keys() {
    let k = keys();
    assert_eq!(k.iter().map(|k| k.len()).collect::<Vec<_>>(), vec![4, 4, 4, 3, 9, 8, 3, 12, 5, 4]);
}

#[test]
fn start_up_host_volumes_and_init_write() {
    let d = module(0xF);
    let (mut m, r) = start(&d);
    assert_eq!(r.title(), "Song");
    assert_eq!(r.subsongs(), (0, 0, 0));
    let log = m.bus.write_log.take().unwrap();
    // eight host volume writes of 0 at the spacing of section 4.1, then the init's own store
    let vols: Vec<_> = log.iter().take(8).collect();
    assert_eq!(vols.iter().map(|e| e.0).collect::<Vec<_>>(), [0xDFF0A8, 0xDFF0B8, 0xDFF0C8, 0xDFF0D8, 0xDFF0A8, 0xDFF0B8, 0xDFF0C8, 0xDFF0D8]);
    assert!(vols.iter().all(|e| e.1 == 0));
    let v0 = vols[0].2;
    assert_eq!(vols.iter().map(|e| e.2 - v0).collect::<Vec<_>>(), [0, 132, 276, 415, 664, 796, 940, 1079]);
    // the init stores 0x40 through the fixed-up hardware address
    let init = &log[8];
    assert_eq!((init.0, init.1), (0xDFF0A8, 0x40));
    // the init call starts 1108 colour clocks after V0; its first instruction begins after 1140
    // cycles and the store comes 20 cycles later
    assert_eq!(init.2, v0 + 1108 + (1140 + 20) / 2);
    // the first tick is due one tick period after the end of the call
    assert!(m.bus.clock > init.2);
}

#[test]
fn tick_runs_the_triple_site_and_ends_the_song() {
    let d = module(0x0007);
    let (mut m, mut r) = start(&d);
    m.bus.write_log = Some(Vec::new());
    assert!(!r.songend());
    r.tick(&mut Hw { bus: &mut m.bus });
    let log = m.bus.write_log.take().unwrap();
    // the location (a long: two word writes at one time), the length and the period, 20 cycles apart each
    let loc: Vec<_> = log.iter().filter(|e| e.0 == 0xDFF0A0 || e.0 == 0xDFF0A2).collect();
    assert_eq!(loc.len(), 2);
    assert_eq!((loc[0].1, loc[1].1), (1, 0x2345));
    assert_eq!(loc[0].2, loc[1].2);
    let len = log.iter().find(|e| e.0 == 0xDFF0A4).unwrap();
    let per = log.iter().find(|e| e.0 == 0xDFF0A6).unwrap();
    assert_eq!(len.1, 0x20);
    assert_eq!(per.1, 0x1AC);
    assert_eq!(len.2 - loc[0].2, 10);
    assert_eq!(per.2 - len.2, 10);
    // voices 0, 1 and 2 restarted: a 3-voice song (the mask word has bit 3 clear) ends
    assert!(r.songend());
}

#[test]
fn a_fourth_voice_keeps_the_song_going() {
    let d = module(0x000F);
    let (mut m, mut r) = start(&d);
    r.tick(&mut Hw { bus: &mut m.bus });
    assert!(!r.songend(), "voice 3 never restarts: no song end");
}

#[test]
fn ticks_are_deterministic() {
    let run = || {
        let d = module(0x000F);
        let (mut m, mut r) = start(&d);
        m.bus.write_log = Some(Vec::new());
        for _ in 0..3 {
            r.tick(&mut Hw { bus: &mut m.bus });
            m.bus.clock += TICK;
        }
        m.bus.write_log.take().unwrap()
    };
    assert_eq!(run(), run());
}

#[test]
fn site_search_finds_each_kind_once_and_not_after_the_marker() {
    let d = module(0xF);
    let hunk = &d[0x20..];
    // the hunk's bytes are the file's after the header words; the sites are searched in the fixed-up image
    let mut img = hunk.to_vec();
    let (start, end) = region(&img, 0x60);
    // apply the address fix-up as new() does
    let mut p = start;
    while p + 4 <= end {
        if be16(&img, p) == 0x00DF {
            let v = be16(&img, p + 2) | 0xF000;
            img[p + 2..p + 4].copy_from_slice(&v.to_be_bytes());
            p += 4;
        } else {
            p += 2;
        }
    }
    let s = Sites::find(&img, start, end);
    let found: Vec<_> = s.at.iter().flatten().map(|(k, _)| *k).collect();
    assert_eq!(found.iter().filter(|&&k| k == Kind::Triple).count(), 1);
    assert_eq!(found.iter().filter(|&&k| k == Kind::Restart).count(), 3);
    // a search that ends before the sites sees none
    let s = Sites::find(&img, start, start + 8);
    assert!(s.at.iter().all(|x| x.is_none()));
}
