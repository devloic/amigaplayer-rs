use super::*;
use crate::machine::Machine;

/// Hunk offsets of the synthetic module's routines and data.
const INIT: usize = 0x200;
const PLAY: usize = 0x220;
const DATA: usize = 0x300;
const NAME: usize = 0x100;
/// Size of the hunk in longs.
const LONGS: usize = 0x100;

fn put32(h: &mut [u8], o: usize, v: u32) {
    h[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

fn words(h: &mut [u8], o: usize, w: &[u16]) {
    for (i, v) in w.iter().enumerate() {
        h[o + 2 * i..o + 2 * i + 2].copy_from_slice(&v.to_be_bytes());
    }
}

/// A module of the Wanted Team's packaging whose routines are tiny: the start routine stores D0
/// in voice 0's volume, the tick routine reads a word through a relocated pointer into voice
/// 0's period. `tag` is the 8 or 12 bytes at hunk offset 4.
fn module(tag: &[u8], spl: bool, subs: u32, timer: u32) -> Vec<u8> {
    let mut h = vec![0u8; 4 * LONGS];
    h[0..4].copy_from_slice(&[0x70, 0xFF, 0x4E, 0x75]);
    h[4..4 + tag.len()].copy_from_slice(tag);
    let (play_at, init_at, subs_at, name_at) = if spl { (0x0C, 0x10, 0x14, 0x18) } else { (0x10, 0x14, 0x18, 0x24) };
    put32(&mut h, play_at, PLAY as u32);
    put32(&mut h, init_at, INIT as u32);
    put32(&mut h, subs_at, subs);
    put32(&mut h, name_at, NAME as u32);
    put32(&mut h, if spl { 0x34 } else { 0x48 }, timer);
    h[NAME..NAME + 5].copy_from_slice(b"Test\0");
    // init: move.w d0,$DFF0A8.l ; rts
    words(&mut h, INIT, &[0x33C0, 0x00DF, 0xF0A8, 0x4E75]);
    // play: lea DATA.l,a0 (relocated) ; move.w (a0),$DFF0A6.l ; rts
    words(&mut h, PLAY, &[0x41F9, 0, 0, 0x33D0, 0x00DF, 0xF0A6, 0x4E75]);
    put32(&mut h, PLAY + 2, DATA as u32);
    words(&mut h, DATA, &[0x0123]);
    let mut f = Vec::new();
    for v in [0x3F3u32, 0, 1, 0, 0, 0x4000_0000 | LONGS as u32, 0x3E9, LONGS as u32] {
        f.extend(v.to_be_bytes());
    }
    f.extend(&h);
    // relocation of the long in the tick routine, then the end
    for v in [0x3ECu32, 1, 0, PLAY as u32 + 2, 0, 0x3F2] {
        f.extend(v.to_be_bytes());
    }
    f
}

fn spl(subs: u32, timer: u32) -> Vec<u8> {
    module(b"!SOPROL!", true, subs, timer)
}

fn bss(tag: &[u8; 4], subs: u32, timer: u32) -> Vec<u8> {
    module(&[b"BEATHOVE".as_slice(), tag].concat(), false, subs, timer)
}

fn start_at(d: &[u8], base: Option<u32>) -> Option<(Machine, Box<dyn Replayer>)> {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    let base = base.unwrap_or_else(|| m.alloc(d.len() as u32));
    m.bus.write_bytes(base, d);
    let r = new(d, base, &mut m)?;
    Some((m, r))
}

fn start(d: &[u8]) -> (Machine, Box<dyn Replayer>) {
    start_at(d, None).expect("recognised")
}

fn writes_to(m: &mut Machine, reg: u32) -> Vec<u16> {
    m.bus.write_log.as_ref().unwrap().iter().filter(|e| e.0 == reg).map(|e| e.1).collect()
}

#[test]
fn detect_takes_both_formats_and_their_generations() {
    assert!(detect(&spl(3, 14188)));
    for g in [b"N100", b"N109", b"NNEW"] {
        assert!(detect(&bss(g, 1, 0)), "{g:?}");
    }
    assert_eq!(kind(&spl(1, 0)).map(|k| k.0), Some(Kind::Spl));
    assert_eq!(kind(&bss(b"NNEW", 1, 0)).map(|k| k.0), Some(Kind::Bss { new: true }));
    assert_eq!(kind(&bss(b"N109", 1, 0)).map(|k| k.0), Some(Kind::Bss { new: false }));
}

#[test]
fn detect_rejects_other_files() {
    let d = spl(1, 0);
    for (o, v) in [(3usize, 0u8), (0x23, 0), (0x24, b'?'), (0x2B, b'?'), (0x13, 1), (0x1B, 0xEA)] {
        let mut bad = d.clone();
        bad[o] = v;
        assert!(!detect(&bad), "offset {o:#x}");
    }
    let mut bad = bss(b"N109", 1, 0);
    bad[0x2C + 3] = b'7';
    assert!(!detect(&bad));
    // truncated: the size long names more than the file holds
    assert!(!detect(&d[..0x200]));
    assert!(!detect(&d[..0x40]));
    assert!(!detect(&[]));
}

#[test]
fn spl_start_routine_takes_the_subsong_from_one() {
    let (mut m, mut r) = start(&spl(3, 14188));
    assert_eq!(r.format(), "Sound Programming Language");
    assert_eq!(r.title(), "Test");
    assert_eq!(r.subsongs(), (0, 2, 0));
    assert_eq!(writes_to(&mut m, 0xDFF0A8), [1]);
    r.set_subsong(2, &mut Hw { bus: &mut m.bus });
    assert_eq!(writes_to(&mut m, 0xDFF0A8), [1, 3]);
    assert_eq!(r.subsongs(), (0, 2, 2));
    // a subsong the header does not have is refused
    r.set_subsong(3, &mut Hw { bus: &mut m.bus });
    assert_eq!(writes_to(&mut m, 0xDFF0A8), [1, 3]);
}

#[test]
fn bss_start_routine_takes_the_subsong_from_zero() {
    let (mut m, mut r) = start(&bss(b"N109", 2, 0));
    assert_eq!(r.format(), "Beathoven Synthesizer");
    assert_eq!(r.subsongs(), (0, 1, 0));
    assert_eq!(writes_to(&mut m, 0xDFF0A8), [0]);
    r.set_subsong(1, &mut Hw { bus: &mut m.bus });
    assert_eq!(writes_to(&mut m, 0xDFF0A8), [0, 1]);
}

#[test]
fn the_timer_gives_the_tick_period() {
    // SPL: always the long at 0x34
    assert_eq!(start(&spl(1, 14188)).1.tick_clocks(), 14189 * 5);
    // Beathoven: the long at 0x48, or the host's 50 Hz when it is 0, and not in the NEW generation
    assert_eq!(start(&bss(b"N100", 1, 14188)).1.tick_clocks(), 14189 * 5);
    assert_eq!(start(&bss(b"N109", 1, 0)).1.tick_clocks(), DT_TICK);
    assert_eq!(start(&bss(b"NNEW", 1, 14188)).1.tick_clocks(), DT_TICK);
}

#[test]
fn a_tick_calls_the_tick_routine_through_the_relocated_hunk() {
    for base in [None, Some(0x4_0000)] {
        let (mut m, mut r) = start_at(&spl(1, 14188), base).unwrap();
        assert!(writes_to(&mut m, 0xDFF0A6).is_empty());
        r.tick(&mut Hw { bus: &mut m.bus });
        assert_eq!(writes_to(&mut m, 0xDFF0A6), [0x0123]);
        r.tick(&mut Hw { bus: &mut m.bus });
        assert_eq!(writes_to(&mut m, 0xDFF0A6), [0x0123, 0x0123]);
        assert!(!r.songend());
    }
}

#[test]
fn the_routines_cost_the_cycles_of_their_instructions() {
    let (mut m, mut r) = start(&spl(1, 14188));
    m.bus.clock = 1_000_000;
    r.tick(&mut Hw { bus: &mut m.bus });
    let e = *m.bus.write_log.as_ref().unwrap().iter().filter(|e| e.0 == 0xDFF0A6).last().unwrap();
    // lea abs.l,a0 (12 cycles) and move.w (a0),abs.l (20) execute before the store: the 68000's
    // bus cycles of the move fall in its last 8, so the store is at least 32 cycles in
    assert!(e.2 >= 1_000_000 + 16 && e.2 <= 1_000_000 + 40, "{}", e.2);
    assert!(m.bus.clock >= e.2);
}

#[test]
fn entries_outside_the_hunk_or_odd_are_not_taken() {
    let mut m = Machine::new(CpuType::M68000);
    for (o, v) in [(0x0Cusize, 0x4000u32), (0x10, 0x4000), (0x0C, 0x201), (0x10, 0xFFFF_FFFF)] {
        let mut d = spl(1, 0);
        put32(&mut d, 0x20 + o, v);
        assert!(new(&d, 0, &mut m).is_none(), "{o:#x} {v:#x}");
    }
    let mut d = bss(b"N109", 1, 0);
    put32(&mut d, 0x20 + 0x14, 0x7FFF_FFFF);
    assert!(new(&d, 0, &mut m).is_none());
    // a hunk that would not fit the machine
    assert!(start_at_checked(&spl(1, 0), 0x00FF_FFF0).is_none());
}

fn start_at_checked(d: &[u8], base: u32) -> Option<()> {
    let mut m = Machine::new(CpuType::M68000);
    new(d, base, &mut m).map(|_| ())
}

#[test]
fn a_routine_that_never_returns_ends_the_song() {
    let mut d = spl(1, 0);
    // play: bra.s *
    put32(&mut d, 0x20 + 0x0C, 0x240);
    d[0x20 + 0x240..0x20 + 0x242].copy_from_slice(&[0x60, 0xFE]);
    let (mut m, mut r) = start(&d);
    assert!(!r.songend());
    r.tick(&mut Hw { bus: &mut m.bus });
    assert!(r.songend());
    // and it is not called again: no more time passes
    let t = m.bus.clock;
    r.tick(&mut Hw { bus: &mut m.bus });
    assert_eq!(m.bus.clock, t);
    r.set_subsong(0, &mut Hw { bus: &mut m.bus });
    assert_eq!(m.bus.clock, t);
}

#[test]
fn a_routine_the_core_cannot_run_ends_the_song() {
    let mut d = bss(b"N109", 1, 0);
    // start routine: an illegal instruction
    d[0x20 + INIT..0x20 + INIT + 2].copy_from_slice(&[0x4A, 0xFC]);
    let (_, r) = start(&d);
    assert!(r.songend());
}

#[test]
fn the_ports_come_before_the_fallback() {
    let pos = |name: &str| REPLAYERS_NAMES().iter().position(|&n| n == name);
    let e = pos("Embedded 68000 replayer").unwrap();
    assert!(pos("Sound Programming Language").unwrap() < e);
    assert!(pos("Beathoven Synthesizer").unwrap() < e);
}

#[allow(non_snake_case)]
fn REPLAYERS_NAMES() -> Vec<&'static str> {
    crate::replay::REPLAYERS.iter().map(|o| o.name).collect()
}

#[test]
fn only_the_fallback_takes_a_synthetic_module() {
    // a module whose tables the ports do not know: the ports' checks refuse it, the fallback's takes it
    for d in [spl(1, 14188), bss(b"N109", 1, 0)] {
        let takers: Vec<&str> = crate::replay::REPLAYERS.iter().filter(|o| (o.detect)(&d)).map(|o| o.name).collect();
        assert_eq!(takers, ["Embedded 68000 replayer"]);
    }
}
