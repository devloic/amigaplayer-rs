use super::*;
use crate::machine::Machine;

/// Appends words; labels are positions in the byte vector.
struct Asm {
    d: Vec<u8>,
}

impl Asm {
    fn w(&mut self, v: u16) -> usize {
        self.d.extend_from_slice(&v.to_be_bytes());
        self.d.len() - 2
    }
    fn pos(&self) -> usize {
        self.d.len()
    }
    /// A word holding the displacement from its own position to `target` (set later by `fix`).
    fn pcrel(&mut self) -> usize {
        self.w(0)
    }
    fn fix(&mut self, at: usize, target: usize) {
        let v = (target as isize - at as isize) as i16 as u16;
        self.d[at..at + 2].copy_from_slice(&v.to_be_bytes());
    }
}

/// A small module in the shape the player expects (section 2.4 of the specification), built in
/// code: four entries, a start entry that stores its subsong number into the register buffer, a
/// tick entry with the status-register words, the position-independent pair, the end word, three
/// song-end sites (command codes 8, 10 and 9), the hardware test, the register site, then the
/// register buffer, the subsong marker and `nsub` times four subsong words. The register buffer
/// holds tone A with P = 16, tone B with P = 512, tone C off, all levels 15 and `mix` as mixer.
fn module(nsub: usize, mix: u8, sites: bool) -> Vec<u8> {
    let mut a = Asm { d: Vec::new() };
    let mut entries = Vec::new();
    for _ in 0..4 {
        a.w(0x6000);
        entries.push(a.pcrel());
    }
    a.w(0x49FA);
    a.w(0);
    while a.pos() < 0x28 {
        a.w(0x4E71);
    }
    // the start entry
    let init = a.pos();
    a.w(0x48E7);
    a.w(0xFFFE);
    a.w(0x4DFA);
    let lea_vars = a.pcrel();
    a.w(0x51EE);
    a.w(0);
    a.w(0x6100);
    let bsr = a.pcrel();
    a.w(0x4CDF);
    a.w(0x7FFF);
    a.w(0x4E75);
    let doinit = a.pos();
    a.w(0x1C80);
    a.w(0x4E75);
    // the tick entry
    let play = a.pos();
    a.w(0x48E7);
    a.w(0xFFFE);
    a.w(0x40C2);
    a.w(0x46FC);
    a.w(0x2700);
    a.w(0x41FA);
    let lea_regs = a.pcrel();
    a.w(0x43FA);
    let lea_frames = a.pcrel();
    a.w(0x43E9);
    let dlen = a.w(0);
    let mut chs = Vec::new();
    for _ in 0..3 {
        a.w(0x49FA);
        chs.push(a.pcrel());
        if sites {
            a.w(0x206C);
            a.w(0x0032);
        } else {
            a.w(0x4E71);
            a.w(0x4E71);
        }
    }
    a.w(0x4A2E);
    a.w(0);
    a.w(0x6712);
    a.w(0x45F8);
    a.w(0x8800);
    a.w(0x4E71);
    a.w(0x0838);
    a.w(0x0007);
    a.w(0xFA01);
    a.w(0x6604);
    a.w(0x14BC);
    a.w(0x0007);
    a.w(0x4BFA);
    let lea_a5 = a.pcrel();
    a.w(0x40C1);
    a.w(0x46FC);
    a.w(0x2700);
    for _ in 0..3 {
        a.w(0x4E71);
    }
    a.w(0x4CDF);
    a.w(0x7FFF);
    a.w(0x4E75);
    // data
    let mut chpos = Vec::new();
    for code in [8u8, 10, 9] {
        chpos.push(a.pos());
        let mut blk = [0u8; 0x40];
        blk[0x3E] = code;
        a.d.extend_from_slice(&blk);
    }
    let regs = a.pos();
    // R1, R0, R3, R2, R5, R4, R6, R7, R8, R9, R10 and a pad byte
    a.d.extend_from_slice(&[0, 16, 2, 0, 0, 0, 5, mix, 15, 15, 15, 0]);
    a.d.extend_from_slice(&0x56u32.to_be_bytes());
    for _ in 0..4 * nsub {
        a.w(0x46);
    }
    let end = a.pos();
    for e in entries.clone() {
        let t = if e == entries[1] { play } else { init };
        a.fix(e, t);
    }
    a.fix(lea_vars, regs);
    a.fix(bsr, doinit);
    a.fix(lea_regs, regs);
    a.fix(lea_frames, regs);
    a.fix(lea_a5, regs);
    let n = ((end - regs) / 2) as u16;
    a.d[dlen..dlen + 2].copy_from_slice(&n.to_be_bytes());
    for (c, p) in chs.into_iter().zip(chpos) {
        a.fix(c, p);
    }
    a.d
}

fn start_mode(d: &[u8], mode: u8) -> (Machine, Box<dyn Replayer>, u32) {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    let base = m.alloc(d.len() as u32);
    m.bus.write_bytes(base, d);
    let r = build(d, base, &mut m, mode).expect("recognised");
    (m, r, base)
}

fn tick(m: &mut Machine, r: &mut Box<dyn Replayer>) {
    m.bus.clock += TICK;
    r.tick(&mut Hw { bus: &mut m.bus });
}

/// The audio register writes (address, value) of the log after the first `skip` entries.
fn audio(m: &Machine, skip: usize) -> Vec<(u32, u16)> {
    m.bus.write_log.as_ref().unwrap()[skip..].iter().filter(|e| (0xDFF096..=0xDFF0DF).contains(&e.0)).map(|e| (e.0, e.1)).collect()
}

#[test]
fn the_noise_buffers_are_those_of_the_specification() {
    // values taken from the specification's generator output: the first 16 and the last 8 bytes
    // and the byte sum of each buffer
    let a = noise(0x4849_5050, 1024);
    assert_eq!(a.len(), 1024);
    assert_eq!(a[..16], [0x7b, 0x20, 0x0e, 0x4b, 0xe3, 0x0a, 0x07, 0x8a, 0x6e, 0x0a, 0x8d, 0x00, 0xfe, 0x33, 0x22, 0xe1]);
    assert_eq!(a[1016..], [0x85, 0xee, 0x55, 0x78, 0x94, 0x2b, 0x01, 0x24]);
    assert_eq!(a.iter().map(|&b| b as u32).sum::<u32>(), 131992);
    let b = noise(0, 16384);
    assert_eq!(b.len(), 16384);
    assert_eq!(b[..16], [0xe8, 0x0f, 0x86, 0x46, 0x40, 0xbd, 0x86, 0x80, 0xaa, 0xaf, 0x82, 0xa1, 0x09, 0xfc, 0xfe, 0xdd]);
    assert_eq!(b[16376..], [0x60, 0xb4, 0xea, 0x38, 0x3f, 0x13, 0x7e, 0x40]);
    assert_eq!(b.iter().map(|&b| b as u32).sum::<u32>(), 2092416);
}

#[test]
fn detect_accepts_the_shape_and_rejects_what_the_check_rejects() {
    let d = module(3, 0x38, true);
    assert!(detect(&d));
    for o in [0usize, 4, 8, 12, 16] {
        let mut bad = d.clone();
        bad[o] ^= 0x01;
        assert!(!detect(&bad), "offset {o}");
    }
    // a displacement that is zero, odd or negative
    for v in [0u16, 0x0003, 0x8002] {
        let mut bad = d.clone();
        bad[6..8].copy_from_slice(&v.to_be_bytes());
        assert!(!detect(&bad), "displacement {v:#x}");
    }
    // the start-entry shape at the target of the entry at offset 8
    for o in [0x28usize, 0x2C, 0x30, 0x34] {
        let mut bad = d.clone();
        bad[o] ^= 0x10;
        assert!(!detect(&bad), "shape {o:#x}");
    }
    assert!(!detect(&[]));
    assert!(!detect(&d[..20]));
    assert!(!detect(&[0; 4000]));
    assert!(!detect(&[0xFF; 600]));
}

#[test]
fn the_registry_gives_such_a_file_to_this_replayer_only() {
    let d = module(3, 0x38, true);
    let hits: Vec<&str> = crate::replay::REPLAYERS.iter().filter(|o| (o.detect)(&d)).map(|o| o.name).collect();
    assert_eq!(hits, vec!["Quartet PSG"]);
}

#[test]
fn the_patch_walk_applies_the_patches_and_counts_the_subsongs() {
    let d = module(3, 0x38, true);
    let p = plan(&d, 0x1000, 0x1008).expect("plan");
    assert_eq!(p.subs, 3);
    assert_eq!(p.img.len(), d.len());
    // the status register words are no-operations
    let at = d.windows(2).position(|w| w == [0x40, 0xC2]).unwrap();
    assert!(at % 2 == 0);
    assert_eq!(&p.img[at..at + 6], &[0x4E, 0x71, 0x4E, 0x71, 0x4E, 0x71]);
    // the three song-end sites became relative calls, the first jump sits at offset 8
    assert_eq!(w16(&p.img, 8), Some(0x4EF9));
    assert_eq!(w32(&p.img, 10), Some(0x1008));
    assert_eq!(w16(&p.img, 16), Some(0x4EF9));
    assert_eq!(w16(&p.img, 24), Some(0x4EF9));
    assert_eq!(p.img.windows(4).filter(|w| w == &[0x61, 0x00, 0x00, 0x00]).count(), 0);
    let calls = (0..p.img.len() - 1).step_by(2).filter(|&o| w16(&p.img, o) == Some(0x6100)).count();
    assert!(calls >= 3);
    // the register site became an absolute call
    let q = (0..p.img.len() - 5).step_by(2).find(|&o| w16(&p.img, o) == Some(0x4EB9) && w32(&p.img, o + 2) == Some(0x1000)).expect("register call");
    assert_eq!(w16(&p.img, q + 6), Some(0x4E71));
    // the branch before the hardware test is unconditional
    let hw = (0..p.img.len() - 3).step_by(2).find(|&o| w32(&p.img, o) == Some(0x0838_0007)).unwrap();
    assert_eq!(p.img[hw - 8], 0x60);
}

#[test]
fn the_walk_cost_follows_the_ledger() {
    // the same module without sites, and with a site whose next position is not tested
    let a = plan(&module(1, 0x38, false), 0x1000, 0x1008).unwrap();
    let b = plan(&module(1, 0x38, true), 0x1000, 0x1008).unwrap();
    // a site position costs 101 and the position after it, whose site test is skipped, 75; the
    // same three positions without a site cost 104 each
    assert_eq!(a.cost - b.cost, 3 * (3 * 104 - 101 - 75));
    assert_eq!(a.subs, 1);
}

#[test]
fn malformed_modules_are_not_recognised_by_new() {
    let d = module(3, 0x38, true);
    // no subsong marker: the marker long overwritten
    let mut bad = d.clone();
    let m = bad.windows(4).position(|w| w == [0, 0, 0, 0x56]).unwrap();
    bad[m + 3] = 0x57;
    assert!(plan(&bad, 0x1000, 0x1008).is_none());
    // an end past the file
    let mut bad = d.clone();
    let e = bad.windows(2).position(|w| w == [0x43, 0xE9]).unwrap();
    let n = u16::from_be_bytes([bad[e + 2], bad[e + 3]]) + 100;
    bad[e + 2..e + 4].copy_from_slice(&n.to_be_bytes());
    assert!(plan(&bad, 0x1000, 0x1008).is_none());
    // a marker at or past the end (the end moved before it)
    let mut bad = d.clone();
    let n = u16::from_be_bytes([bad[e + 2], bad[e + 3]]) - 20;
    bad[e + 2..e + 4].copy_from_slice(&n.to_be_bytes());
    assert!(plan(&bad, 0x1000, 0x1008).is_none());
    // no status register word
    let mut bad = d.clone();
    let at = bad.windows(2).position(|w| w == [0x40, 0xC2]).unwrap();
    bad[at] = 0x4E;
    bad[at + 1] = 0x71;
    assert!(plan(&bad, 0x1000, 0x1008).is_none());
}

#[test]
fn single_byte_damage_never_panics_or_hangs() {
    let d = module(2, 0x38, true);
    for o in (0..d.len()).step_by(7) {
        for x in [0xFFu8, 0x01] {
            let mut bad = d.clone();
            bad[o] ^= x;
            let mut m = Machine::new(CpuType::M68000);
            let base = m.alloc(bad.len() as u32);
            m.bus.write_bytes(base, &bad);
            if !detect(&bad) {
                continue;
            }
            for mode in [0u8, 1] {
                if let Some(mut r) = build(&bad, base, &mut m, mode) {
                    for _ in 0..2 {
                        r.tick(&mut Hw { bus: &mut m.bus });
                        m.bus.clock += TICK;
                    }
                }
            }
        }
    }
}

#[test]
fn the_configuration_file_chooses_the_mode() {
    assert_eq!(cfg_mode(b"WT\0\x01"), 1);
    assert_eq!(cfg_mode(b"WT\0\x00"), 0);
    assert_eq!(cfg_mode(b"WT\0\x02"), 0);
    assert_eq!(cfg_mode(b"XT\0\x01"), 0);
    assert_eq!(cfg_mode(b"WT\0"), 0);
    assert_eq!(cfg_mode(b""), 0);
}

#[test]
fn mode_1_starts_the_voices_and_converts_a_tick() {
    let d = module(3, 0x38, true);
    let cost = plan(&d, 0x1000, 0x1008).unwrap().cost;
    let (mut m, mut r, _) = start_mode(&d, 1);
    // the first logged write is DMACON 0x000F, 689 colour clocks after the start of InitSound
    let i = S0 + (1345 + cost + 161 + 44) / 2;
    let log = m.bus.write_log.clone().unwrap();
    assert_eq!((log[0].0, log[0].1, log[0].2), (0xDFF096, 0x000F, i + 689));
    // four voices' set-up: LC (two words), LEN, PER = 1, VOL and DAT
    let up = audio(&m, 0);
    assert_eq!(up.len(), 1 + 4 * 6);
    assert_eq!(up[4], (0xDFF0A6, 1));
    let before = m.bus.write_log.as_ref().unwrap().len();
    tick(&mut m, &mut r);
    let w = audio(&m, before);
    let addrs: Vec<u32> = w.iter().map(|e| e.0).collect();
    // tone A (P = 16) and tone B (P = 512) get periods, tone C (P = 0) none; noise voice last
    assert_eq!(addrs, vec![0xDFF0A6, 0xDFF0A8, 0xDFF0B6, 0xDFF0B8, 0xDFF0C8, 0xDFF0D6, 0xDFF0D8, 0xDFF096, 0xDFF096]);
    assert_eq!(w[0].1, 16 * 1419 / 100);
    assert_eq!(w[1].1, 64);
    assert_eq!(w[2].1, (512 * 1419 / 100) as u16);
    assert_eq!(w[4].1, 64);
    assert_eq!(w[5].1, 0x80 + 0x20 * 5);
    assert_eq!(w[6].1, 0);
    // DMA on for voices 0 and 1 only, then the other bits off
    assert_eq!(w[7].1, 0x8003);
    assert_eq!(w[8].1, 0x000C);
    // the tick is on its grid: the first write is 266 + the driver's cycles + the conversion
    // start after the tick's start
    let t = m.bus.write_log.as_ref().unwrap()[before].2;
    assert!(t > m.bus.write_log.as_ref().unwrap()[before - 1].2 + TICK - 100);
}

#[test]
fn mode_0_restarts_a_voice_when_its_status_changes() {
    // tones A and B on, tone C and all noise off
    let d = module(3, 0x3C, true);
    let (mut m, mut r, _) = start_mode(&d, 0);
    // InitSound writes nothing in mode 0
    assert!(audio(&m, 0).is_empty());
    tick(&mut m, &mut r);
    let w = audio(&m, 0);
    // voice 0 (tone A) is restarted first: PER = 1 ... then its volume and period
    assert_eq!(w[0], (0xDFF0A6, 1));
    let per_a = ((16u32 * 14540) >> 10) as u16 + 1;
    assert!(w.contains(&(0xDFF0A8, 15)) || w.contains(&(0xDFF0A8, VT0[15])));
    assert!(w.iter().any(|e| *e == (0xDFF0A6, per_a)));
    // voice 3 (tone B) and voice 2 (tone C off: restart with the empty waveform every tick)
    assert!(w.iter().any(|e| e.0 == 0xDFF0D6));
    assert!(w.iter().any(|e| e.0 == 0xDFF0C6));
    // a second tick with the same registers: voices A and B keep playing, only C is restarted
    let n = m.bus.write_log.as_ref().unwrap().len();
    tick(&mut m, &mut r);
    let w2 = audio(&m, n);
    assert!(!w2.iter().any(|e| e.0 == 0xDFF0A0 || e.0 == 0xDFF0D0));
    assert!(w2.iter().any(|e| e.0 == 0xDFF0C0));
    assert!(w2.iter().any(|e| *e == (0xDFF0A6, per_a)));
}

#[test]
fn the_song_ends_when_the_driver_has_shown_all_three_codes() {
    for mode in [0u8, 1] {
        let d = module(3, 0x38, true);
        let (mut m, mut r, _) = start_mode(&d, mode);
        assert!(!r.songend());
        tick(&mut m, &mut r);
        assert!(r.songend(), "mode {mode}");
    }
    // without a site the song never ends
    let d = module(3, 0x38, false);
    let (mut m, mut r, _) = start_mode(&d, 1);
    for _ in 0..3 {
        tick(&mut m, &mut r);
    }
    assert!(!r.songend());
}

#[test]
fn subsongs_run_the_end_sound_and_the_start_again() {
    let d = module(3, 0x38, true);
    let (mut m, mut r, _) = start_mode(&d, 1);
    assert_eq!(r.subsongs(), (0, 2, 0));
    let n = m.bus.write_log.as_ref().unwrap().len();
    let end0 = m.bus.clock;
    r.set_subsong(1, &mut Hw { bus: &mut m.bus });
    let w = audio(&m, n);
    // EndSound: DMACON and the four volumes, then the start (DMACON, 4 x 6 writes)
    assert_eq!(&w[..5], &[(0xDFF096, 0x000F), (0xDFF0A8, 0), (0xDFF0B8, 0), (0xDFF0C8, 0), (0xDFF0D8, 0)]);
    assert_eq!(w[5], (0xDFF096, 0x000F));
    let log = m.bus.write_log.as_ref().unwrap();
    assert_eq!(log[n].2, end0 + 14);
    assert_eq!(log[n + 5].2, end0 + 48 + 236 + 689);
    assert_eq!(r.subsong_tick_at(), Some(m.bus.clock + TICK));
    let n = m.bus.write_log.as_ref().unwrap().len();
    tick(&mut m, &mut r);
    // the start entry stored the subsong number as the coarse byte of tone A: P = 256 + 16
    assert_eq!(audio(&m, n)[0], (0xDFF0A6, (272 * 1419 / 100) as u16));
    // out of range: ignored
    r.set_subsong(3, &mut Hw { bus: &mut m.bus });
    assert_eq!(r.subsongs().2, 1);
}

#[test]
fn the_runs_are_deterministic() {
    let d = module(3, 0x00, true);
    let run = |mode| {
        let (mut m, mut r, _) = start_mode(&d, mode);
        for _ in 0..4 {
            tick(&mut m, &mut r);
        }
        m.bus.write_log.clone().unwrap()
    };
    for mode in [0u8, 1] {
        assert_eq!(run(mode), run(mode));
    }
}
