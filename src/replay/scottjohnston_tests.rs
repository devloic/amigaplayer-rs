use super::*;
use crate::engine::Engine;
use m68k::CpuType;
use std::path::{Path, PathBuf};

/// A module: the first slot names table entry 1 (`acoustic`), the others repeat it; `rows` are
/// 12-byte rows.
fn module(timer: u16, mask: u8, rows: &[[u8; 12]]) -> Vec<u8> {
    let mut m = MAGIC.to_vec();
    m.extend_from_slice(b"TEST\0\0\0\0\0\0\0\0");
    let mut slots = [0u8; 16];
    slots[0] = 1;
    m.extend_from_slice(&slots);
    m.extend_from_slice(&[if timer == 0xFFFF { 0xFF } else { timer as u8 }, (timer >> 8) as u8, mask]);
    for r in rows {
        m.extend_from_slice(r);
    }
    m
}

/// An 8SVX file: VHDR at 12 (one-shot 8, repeat 4 bytes), BODY at 40 with 16 bytes of data.
fn svx(one: u32, rep: u32) -> Vec<u8> {
    let mut f = vec![0u8; 12];
    f[..4].copy_from_slice(b"FORM");
    f.extend_from_slice(b"VHDR");
    f.extend_from_slice(&20u32.to_be_bytes());
    f.extend_from_slice(&one.to_be_bytes());
    f.extend_from_slice(&rep.to_be_bytes());
    f.resize(40, 0);
    f.extend_from_slice(b"BODY");
    f.extend_from_slice(&16u32.to_be_bytes());
    f.extend_from_slice(&[1; 16]);
    f
}

fn cell(note: u8, ins: u8, cmd: u8) -> [u8; 3] {
    [note, ins, cmd]
}

fn row(c: [[u8; 3]; 4]) -> [u8; 12] {
    let mut r = [0u8; 12];
    for (i, x) in c.iter().enumerate() {
        r[3 * i..3 * i + 3].copy_from_slice(x);
    }
    r
}

fn play(d: Vec<u8>, with_file: bool, ticks: usize) -> (Option<String>, Vec<(u32, u16, u64)>) {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    if with_file {
        e.m.os.mem_files.insert(PathBuf::from("/test/Instruments/acoustic"), svx(16, 8));
    }
    let ok = e.play_data(Path::new("/test/SJ.test"), d).is_ok();
    let format = e.playing.as_ref().map(|p| p.format.clone()).filter(|_| ok);
    let mut out = vec![0f32; 2 * 1000 * ticks];
    if format.is_some() {
        e.render(&mut out);
    }
    (format, e.m.bus.write_log.take().unwrap())
}

fn reg(log: &[(u32, u16, u64)], a: u32) -> Vec<u16> {
    log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
}

#[test]
fn detect_rules() {
    let d = module(1000, 0xF, &[row([[0; 3]; 4])]);
    assert!(detect(&d));
    assert!(!detect(&d[..0x2E]));
    let mut e = d.clone();
    e[15] = b'2';
    assert!(!detect(&e));
    assert!(!detect(&[]));
}

#[test]
fn names_table_shape() {
    assert_eq!(NAMES.len(), 95 * 12);
    assert_eq!(&NAMES[..8], b"acoustic");
    assert_eq!(&NAMES[12..18], b"blocks");
}

#[test]
fn record_from_8svx() {
    let r = record_of(&svx(16, 8), 0x1000);
    assert_eq!(r, Inst { start: 0x1030, len: 8, loop_at: 0x1040, loop_len: 4 });
    // a negative low half of the one-shot size moves the loop back
    let r = record_of(&svx(0xFFFE, 0), 0x1000);
    assert_eq!(r.loop_at, 0x1030u32.wrapping_sub(2));
    // no VHDR or BODY: an empty record; a short file never panics
    assert_eq!(record_of(&[0; 100], 0), Inst::default());
    assert_eq!(record_of(&[], 0), Inst::default());
}

#[test]
fn missing_file_or_bad_slot_is_not_played() {
    let d = module(1000, 0xF, &[row([[0; 3]; 4])]);
    assert_eq!(play(d.clone(), false, 1).0, None);
    assert_eq!(play(d.clone(), true, 1).0.as_deref(), Some("Scott Johnston"));
    let mut e = d.clone();
    e[0x1C] = 0; // first slot empty
    assert_eq!(play(e, true, 1).0, None);
    let mut e = d.clone();
    e[0x1D] = 96; // past the 95 names
    assert_eq!(play(e, true, 1).0, None);
}

#[test]
fn init_group_and_first_row() {
    // note 16 (period 428) on voice 0 with a volume command, then a song loop on every voice
    let d = module(
        2000,
        0xF,
        &[
            row([cell(16, 1, 3 + 40), cell(0, 0, 0), cell(0, 0, 0), cell(0, 0, 0)]),
            row([cell(0, 0, 0xDE); 4]),
        ],
    );
    let (f, log) = play(d, true, 400);
    assert_eq!(f.as_deref(), Some("Scott Johnston"));
    let first: Vec<(u32, u16)> = log.iter().take(4).map(|w| (w.0, w.1)).collect();
    assert_eq!(first, vec![(0xDFF096, 1), (0xDFF096, 2), (0xDFF096, 4), (0xDFF096, 8)]);
    // DMA on after the refresh of the four voices
    assert_eq!(log[16].0, 0xDFF096);
    assert_eq!(log[16].1, 0x800F);
    // the volume command writes 40, the note starts with period 428 and DMA on for voice 0
    assert!(reg(&log, 0xDFF0A8).contains(&40));
    assert_eq!(reg(&log, 0xDFF0A6)[0], 428);
    assert!(log.iter().any(|w| w.0 == 0xDFF096 && w.1 == 0x8001));
    // the instrument's length (8 words) is written for the note, the loop (4 words) after it
    let lens = reg(&log, 0xDFF0A4);
    assert!(lens.contains(&8) && lens.contains(&4));
}

#[test]
fn ticks_have_the_timer_period_and_rows_come_every_six() {
    let d = module(1000, 0xF, &[row([cell(16, 1, 0), cell(0, 0, 0), cell(0, 0, 0), cell(0, 0, 0)]), row([cell(0, 0, 0xDE); 4])]);
    let r = ScottJohnston::build(&d, [Inst::default(); 39], 0x100).unwrap();
    assert_eq!(r.tick_clocks(), 5 * 1001);
    let d = module(0xFFFF, 0xF, &[row([[0; 3]; 4])]);
    let r = ScottJohnston::build(&d, [Inst::default(); 39], 0x100).unwrap();
    assert_eq!(r.tick_clocks(), 327_680);
}

fn run_tick(r: &mut ScottJohnston, m: &mut Machine, k: u64) {
    m.bus.clock = 1_000_000 + 100_000 * k;
    let mut hw = Hw { bus: &mut m.bus };
    r.tick(&mut hw);
}

fn started(d: &[u8], ticks: u64) -> (ScottJohnston, Machine) {
    let mut insts = [Inst::default(); 39];
    for i in 1..17 {
        insts[i] = record_of(&svx(16, 8), 0x1000);
    }
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    let mut r = ScottJohnston::build(d, insts, 0x100).unwrap();
    r.counter = 5;
    r.start_song();
    for k in 0..ticks {
        run_tick(&mut r, &mut m, k);
    }
    (r, m)
}

#[test]
fn row_delay_and_loop_commands() {
    // delay 3 on voice 0 row 0: the track stays two more rows; loop start 2 / loop end
    let rows = [
        row([cell(0, 0, 0x56 + 3), cell(0, 0, 0xD3), cell(0, 0, 0xDD), cell(0, 0, 0)]),
        row([cell(0, 0, 0), cell(0, 0, 0xDC), cell(0, 0, 0), cell(0, 0, 0)]),
    ];
    let d = module(1000, 0xF, &rows);
    // five ticks to the first row
    let (r, _) = started(&d, 5);
    assert_eq!(r.v[0].track.delay, 3);
    assert_eq!(r.v[1].track.loop_count, 2);
    assert_eq!(r.v[1].track.loop_save, 0);
    // voice 2 is held on row 0 by the stop marker: the row pointer steps back after each read
    let (r, _) = started(&d, 5 + 6 * 4);
    assert_eq!(r.v[2].track.row, -1);
    // voice 0 got past its delay (read the second row after 3 row ticks)
    assert_eq!(r.v[0].track.delay, 0);
    // voice 1's loop ran its two repeats and finished
    assert_eq!(r.v[1].track.loop_count, 0xFF);
}

#[test]
fn volume_fade_steps_and_stops() {
    let rows = [row([cell(0, 0, 3 + 2), cell(0, 0, 0), cell(0, 0, 0), cell(0, 0, 0)]), row([cell(0, 0, 0xB0 + 1), cell(0, 0, 0), cell(0, 0, 0), cell(0, 0, 0xDD)])];
    let d = module(1000, 0xF, &rows);
    let (r, m) = started(&d, 5 + 6 + 2 * 6 + 1);
    // volume 2 fades down by one per tick to 0, then the fade stops
    assert_eq!(r.v[0].volume, 0);
    assert_eq!(r.v[0].fade_speed, 0);
    let vols: Vec<u16> = m.bus.write_log.as_ref().unwrap().iter().filter(|w| w.0 == 0xDFF0A8).map(|w| w.1).collect();
    assert_eq!(vols, vec![2, 1, 0]);
}

#[test]
fn dma_off_command_on_voice_zero_stops_voice_one() {
    let d = module(1000, 0xF, &[row([cell(0, 0, 0x43), cell(0, 0, 0x43), cell(0, 0, 0), cell(0, 0, 0)])]);
    let (_, m) = started(&d, 5);
    let dma: Vec<u16> = m.bus.write_log.as_ref().unwrap().iter().filter(|w| w.0 == 0xDFF096).map(|w| w.1).collect();
    assert_eq!(dma, vec![0x0002, 0x0002]);
}

#[test]
fn hold_skips_the_loop_refresh() {
    let d = module(1000, 0xF, &[row([cell(0, 0, 0xCF), cell(0, 0, 0), cell(0, 0, 0), cell(0, 0, 0)]), row([cell(0, 0, 0xDD); 4])]);
    let (_, m) = started(&d, 5);
    let n5 = m.bus.write_log.as_ref().unwrap().iter().filter(|w| w.0 == 0xDFF0A4).count();
    assert_eq!(n5, 5);
    let (_, m) = started(&d, 6);
    let n6 = m.bus.write_log.as_ref().unwrap().iter().filter(|w| w.0 == 0xDFF0A4).count();
    // voice 0 is on hold from the sixth tick: no sixth refresh of its length
    assert_eq!(n6, 5);
}

#[test]
fn deterministic_and_malformed_input_is_safe() {
    let rows = [row([cell(16, 1, 0x43), cell(250, 200, 0xFF), cell(39, 0, 0xDC), cell(1, 99, 0xD2)])];
    let d = module(300, 0xF, &rows);
    let (_, a) = started(&d, 300);
    let (_, b) = started(&d, 300);
    assert_eq!(a.bus.write_log, b.bus.write_log);
    // a module cut short, with no loop or stop command: runs on zero cells
    let mut t = module(300, 0xF, &rows);
    t.truncate(0x30);
    let (_, _) = started(&t, 200);
    // every command value on every voice
    for k in 0..=255u8 {
        let d = module(300, 0x3, &[row([cell(1, 1, k), cell(2, 2, k), cell(3, 0, k), cell(4, 3, k)])]);
        let (_, _) = started(&d, 20);
    }
}
