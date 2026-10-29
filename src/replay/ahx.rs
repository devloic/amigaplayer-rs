//! AHX (Abyss' Highest eXperience, the "THX Sound System", 1.x and 2.x modules),
//! ported from 8bitbubsy's ahx2play (Copyright (c) 2021-2024, Olav Sørensen,
//! BSD 3-Clause licence: see LICENSE-ahx2play), a port of AHX 2.3d-sp3's
//! replayer.
//!
//! The Eagleplayer ("THX Soundsystem Eagleplayer Replayer module V2.1",
//! Martin Wodok and Buggs of DEFECT) runs AHX's external replayer, the
//! "THX-Sound_System_BinaryPlayer_2.00", which differs from the tracker's
//! replayer that ahx2play ports; where they differ this follows the
//! Eagleplayer. It plays from the module in memory, as the original does:
//! tracks are `TrackLength` rows apart and read from the module (track 0 of a
//! module that leaves it out reads 64 KB past the tracks for a position
//! jump's parameter), instruments and their play lists are the module's (a
//! jump past an instrument's list reads the next one), and nothing is clamped
//! or range-checked. Its waveforms (triangles, sawtooths, squares, noise and
//! their 31 low- and high-pass filtered copies) are built in guest memory in
//! the original's layout, and each voice's 0x280-byte buffer in chip memory is
//! rewritten from them when its waveform changes, as the original's SetAudio
//! does. Every tick starts with SetAudio: a new period goes out with a volume
//! of 0 after it (the Eagleplayer's poke), then the buffer is copied, then the
//! volume.
//!
//! Not in this replayer (they are in the tracker's): effects E1, E2 (fine
//! slides), E4 (vibrato control), EA and EB (fine volume). Its new instrument
//! clears the portamento's speed, not the play list's slide speed; a play
//! list filter command that takes the 4xx override's position rebuilds the
//! waveform too; a voice starts with its square limits and sign at 0; the
//! volume's product is taken in 16 bits, with the envelope's high byte signed;
//! revision 0 modules ignore the filter commands (track effect 4, play list
//! command 0 and the parameter of command 4) instead of having them cleared at
//! load time; the noise generator's seed carries over from subsong to subsong.
//! Effect 0 sets the jump's high position to its parameter's low nibble, and
//! B back to the current position or before it and F00, like the wrap at the
//! song's last position, signal the song's end. A note
//! below 0 reads the original's code (see `before_periods`). The tick rate is
//! the module's (CIA timer 14209, 7104, 4736 or 3552), set once at load.

use super::{be16, Hw, Open, Replayer};
use crate::bus::Bus;
use crate::machine::Machine;
use crate::paula::LINE;

pub const OPEN: Open = Open { name: "AHX", detect, new };

/// The original's variables-and-waveforms block (its layout): the voices'
/// structures (their square buffers matter here), the waveform pointers, and
/// the waveforms.
const BLOCK_SIZE: u32 = 0x649F0;
const VOICE0: u32 = 0x8;
const VOICE_SIZE: u32 = 0xE8;
const VOICE_SQUARE: u32 = 0x68;
const WAVE_TAB: u32 = 0x556;
const LOW_PASSES: u32 = 0x568;
const TRIANGLES: u32 = 0x31AF0;
const SAWTOOTHS: u32 = 0x31BEC;
const SQUARES: u32 = 0x31CE8;
const NOISE: u32 = 0x32CE8;
const HIGH_PASSES: u32 = 0x33468;
/// One filter's copy of all 45 waveforms.
const FILTER_LEN: i32 = 0x1978;
/// Each voice's buffer in chip memory.
const BUFFER: u32 = 0x280;

/// CIA periods of the header's four tempos.
const TIMERS: [u16; 4] = [14209, 7104, 4736, 3552];

// Colour clocks of the original's code, fitted to the Eagleplayer's logs.
/// InitSound's four volume pokes, apart.
const QUIET_GAP: u64 = 19;
/// From InitSound's last poke to the timer's start.
const INIT_TO_TIMER: u64 = 10793;
/// From EndSound's first VOL0 write to InitSound's first.
const END_TO_INIT: u64 = 10895 - 4 * QUIET_GAP;
/// From the interrupt to the first SetAudio (its first write comes TO_PERIOD
/// or TO_VOLUME later).
const ENTRY: u64 = 74;
/// The first tick's setup pokes, from the interrupt.
const SETUP_DMA_OFF: u64 = 113;
const SETUP_LOC: u64 = 33;
const SETUP_LOC_NEXT: u64 = 38;
const SETUP_REG: [u64; 3] = [23, 21, 19];
const SETUP_DMA_ON: u64 = 34;
/// From the end of the DMA wait to the first voice's volume.
const SETUP_TO_VOICE: u64 = 156;
/// SetAudio: from the previous write to the period, to its volume 0, and from
/// there to the volume; or to the volume with no period.
const TO_PERIOD: u64 = 138;
const PERIOD_TO_ZERO: u64 = 19;
const ZERO_TO_VOLUME: u64 = 100;
const TO_VOLUME: u64 = 148;
/// The buffer's copy: a waveform's, with a cost per pass over it (5 passes
/// for 128 bytes, 160 for 4), or the noise's.
const COPY: u64 = 2565;
const COPY_PASS: u64 = 32;
const COPY_NOISE: u64 = 2122;

// The Eagleplayer's start: before InitSound's first write the host runs the
// player's InitPlayer, which builds the waveforms (the same time for every
// module) and the module info, whose song-length scan and loops over the
// instruments and their names take a time that depends on the module. Its
// 68000 cycles (two a colour clock), counted per code path and checked
// against the first write of all 483 reference logs.
/// The cycles that do not depend on the module, with the host's own.
const START_CYCLES: u64 = 71_475_758 + 9594;
/// Per instrument (its two loops), per byte of the title and of the names,
/// with subsongs, with a title, with track 0 in the module.
const START_INSTRUMENT: u64 = 134;
const START_BYTE: u64 = 17;
const START_SUBSONGS: u64 = 36;
const START_TITLE: u64 = 16;
const START_TRACK0: u64 = 8;
/// The song-length scan: its set-up, a position, a voice's track, with track 0
/// left out of the module, a row (its speed loop a pass, the rest), a voice's
/// call and move on, the end of a row and the next, after the last, the end.
const SCAN_SETUP: u64 = 3571;
const SCAN_POSITION: u64 = 54;
const SCAN_TRACK: u64 = 209;
const SCAN_NULL_SUB: u64 = 13;
const SCAN_NULL_ZERO: u64 = 4;
const SCAN_ROWS: u64 = 411;
const SCAN_SPEED: u64 = 27;
const SCAN_ROW: u64 = 36;
const SCAN_CALL: u64 = 99;
const SCAN_NEXT: u64 = 38;
const SCAN_ROW_END: u64 = 64;
const SCAN_ROW_NEXT: u64 = 14;
const SCAN_LAST: u64 = 33;
const SCAN_EXIT: u64 = 465;
/// The effect of a row: 0, B (it ends the scan; more with a position other
/// than the first), D, F (0 ends it), E (with a D0 command), the others.
const SCAN_E0: u64 = 149;
const SCAN_EB: u64 = 409;
const SCAN_EB_MORE: u64 = 95;
const SCAN_ED: u64 = 241;
const SCAN_EF: u64 = 139;
const SCAN_EF_END: u64 = 125;
const SCAN_EE: u64 = 156;
const SCAN_EE_D0: u64 = 150;
const SCAN_EOTHER: u64 = 119;

/// The colour clocks from the host's start to InitSound's first write, for
/// a module's bytes (what lies past them reads as 0).
fn start_clocks(d: &[u8]) -> u64 {
    let b = |a: i64| -> i64 {
        if a >= 0 && (a as usize) < d.len() {
            d[a as usize] as i64
        } else {
            0
        }
    };
    let fl = ((d[6] as i64) << 8) | d[7] as i64;
    let (ns, len_nr, tl, ni) = (d[13] as i64, fl & 0x3FF, d[10] as i64, d[12] as i64);
    let null = fl & 0x8000 != 0;
    let (pos, tracks) = (14 + 2 * ns, 14 + 2 * ns + 8 * len_nr);
    let mut cyc = START_CYCLES + SCAN_SETUP;
    if len_nr != 0 {
        let last = pos + 8 * (len_nr - 1);
        let (mut a0, mut speed, mut start_row) = (pos - 8, 6i64, 0i64);
        'scan: loop {
            a0 += 8;
            cyc += SCAN_POSITION;
            let mut ptr = [0i64; 4];
            for (v, p) in ptr.iter_mut().enumerate() {
                let mut t = b(a0 + 2 * v as i64);
                cyc += SCAN_TRACK;
                if null {
                    cyc += SCAN_NULL_SUB;
                    t -= 1;
                    if t < 0 {
                        t = 0;
                        cyc += SCAN_NULL_ZERO;
                    }
                }
                *p = tracks + ((((t * tl) & 0xFFFF) + start_row) & 0xFFFF) * 3;
            }
            cyc += SCAN_ROWS;
            if last < a0 {
                break;
            }
            let mut rows = ((tl - 1 - start_row) & 0xFFFF) + 1;
            start_row = 0;
            let mut brk = false;
            loop {
                cyc += SCAN_SPEED * speed.max(1) as u64 + SCAN_ROW;
                for p in ptr.iter_mut() {
                    let (e, param) = (b(*p + 1) & 0xF, b(*p + 2));
                    let mut end = false;
                    cyc += match e {
                        0 => SCAN_E0,
                        0xB => {
                            end = true;
                            SCAN_EB + if pos != a0 { SCAN_EB_MORE } else { 0 }
                        }
                        0xD => {
                            start_row = ((param >> 4) * 10 + (param & 0xF)) & 0xFF;
                            brk = true;
                            SCAN_ED
                        }
                        0xF if param == 0 => {
                            end = true;
                            SCAN_EF_END
                        }
                        0xF => {
                            speed = param;
                            SCAN_EF
                        }
                        0xE => SCAN_EE + if param & 0xF0 == 0xD0 { SCAN_EE_D0 } else { 0 },
                        _ => SCAN_EOTHER,
                    };
                    cyc += SCAN_CALL;
                    if end {
                        break 'scan;
                    }
                    cyc += SCAN_NEXT;
                    *p += 3;
                }
                cyc += SCAN_ROW_END;
                if brk {
                    break;
                }
                cyc += SCAN_ROW_NEXT;
                rows -= 1;
                if rows == 0 {
                    cyc += SCAN_LAST;
                    if last > a0 {
                        break;
                    }
                    break 'scan;
                }
            }
        }
    }
    cyc += SCAN_EXIT;
    // the module info: the instruments (past the tracks, as InitModule's
    // adda.w takes their size), then the title and the names
    let sz = ((tl * 3 * (d[11] as i64 + !null as i64)) & 0xFFFF) as u16 as i16 as i64;
    let mut a = 14 + 2 * ns + 8 * len_nr + sz;
    for _ in 0..ni {
        a += 22 + 4 * b(a + 0x15);
    }
    let mut t = a;
    while b(t) != 0 {
        t += 1;
    }
    cyc += START_BYTE * (t - a + 1) as u64 + START_INSTRUMENT * ni as u64;
    if b(a) != 0 {
        cyc += START_TITLE;
    }
    let mut p = t + 1;
    for _ in 0..ni {
        let q = p;
        while b(p) != 0 {
            p += 1;
        }
        cyc += START_BYTE * (p - q + 1) as u64;
        p += 1;
    }
    if ns != 0 {
        cyc += START_SUBSONGS;
    }
    if !null {
        cyc += START_TRACK0;
    }
    cyc / 2
}

fn detect(d: &[u8]) -> bool {
    // the Eagleplayer's check: "THX" and a revision of 0 or 1
    d.len() >= 14 && d.starts_with(b"THX") && d[3] <= 1
}

/// One voice, as the original's structure has it.
#[derive(Default, Clone)]
struct Voice {
    track: u8,
    transpose: i8,
    next_track: u8,
    adsr: i16,
    a_frames: u8,
    d_frames: u8,
    s_frames: u8,
    r_frames: u8,
    a_delta: i16,
    d_delta: i16,
    r_delta: i16,
    /// Guest address of the instrument (0: none).
    instrument: u32,
    waveform: u8,
    wavelength: u8,
    instr_period: i16,
    track_period: i16,
    vibrato_period: i16,
    note_max_volume: u8,
    perf_sub_volume: u8,
    track_master_volume: u8,
    new_waveform: bool,
    plant_square: bool,
    ignore_square: bool,
    plant_period: bool,
    fixed_note: bool,
    volume_slide_up: u8,
    volume_slide_down: u8,
    hard_cut: u8,
    hard_cut_release: bool,
    hard_cut_release_f: u8,
    period_slide_speed: i16,
    period_slide_period: i16,
    period_slide_limit: i16,
    period_slide_on: bool,
    period_slide_with_limit: bool,
    period_perf_slide_speed: i16,
    period_perf_slide_period: i16,
    period_perf_slide_on: bool,
    vibrato_delay: u8,
    vibrato_current: u8,
    vibrato_depth: u8,
    vibrato_speed: u8,
    square_on: bool,
    square_init: bool,
    square_wait: u8,
    square_lower: u8,
    square_upper: u8,
    square_pos: u8,
    square_signum: u8,
    square_sliding_in: bool,
    filter_on: bool,
    filter_init: bool,
    filter_wait: u8,
    filter_lower: u8,
    filter_upper: u8,
    filter_pos: u8,
    filter_signum: u8,
    filter_speed: u8,
    filter_sliding_in: bool,
    ignore_filter: u8,
    perf_current: u8,
    perf_speed: u8,
    perf_wait: u8,
    perf_list: u32,
    note_delay_wait: u8,
    note_delay_on: bool,
    note_cut_wait: u8,
    note_cut_on: bool,
    audio_source: u32,
    audio_period: u16,
    audio_volume: u16,
}

pub struct Ahx {
    title: String,
    revision: u8,
    null_empty: bool,
    timer: u16,
    len_nr: u16,
    res_nr: u16,
    track_length: u16,
    subsongs: u8,
    sub_table: u32,
    pos_table: u32,
    tracks: u32,
    instruments: [u32; 63],
    block: u32,
    chip: u32,
    voices: [Voice; 4],
    first_tick: bool,
    step_wait: u16,
    get_new_position: bool,
    tempo: u16,
    pattern_break: bool,
    pos_jump: u16,
    pos_jump_note: u16,
    wn_random: u32,
    note_nr: u16,
    pos_nr: u16,
    sub: u16,
    songend: bool,
}

/// `lsr.b` by a register: counts of 8 or more (mod 64) clear the byte.
fn lsr_b(v: u8, n: u8) -> u8 {
    let n = n & 63;
    if n >= 8 {
        0
    } else {
        v >> n
    }
}

fn lsl_b(v: u8, n: u8) -> u8 {
    let n = n & 63;
    if n >= 8 {
        0
    } else {
        v << n
    }
}

/// `divs.w`: the quotient, or the dividend's low word on overflow (a divisor
/// of 0 traps in the original; it is taken as an overflow here).
fn divs(n: i32, d: i16) -> i16 {
    if d == 0 {
        return n as i16;
    }
    let q = n / d as i32;
    if q < i16::MIN as i32 || q > i16::MAX as i32 {
        n as i16
    } else {
        q as i16
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let flags = be16(d, 6);
    let null_empty = flags & 0x8000 != 0;
    let len_nr = flags & 0x3FF;
    let track_length = d[10] as u16;
    let highest = d[11] as u16;
    let num_ins = (d[12] as usize).min(63);
    let subsongs = d[13];
    // the original's InitModule: the tables follow the header, the tracks of
    // TrackLength rows (adda.w: their size taken as a signed word), the
    // instruments, each 22 bytes and its play list
    let sub_table = base + 14;
    let pos_table = sub_table + 2 * subsongs as u32;
    let tracks = pos_table + 8 * len_nr as u32;
    let ntracks = highest + !null_empty as u16;
    let size = (track_length as u32 * 3 * ntracks as u32) as u16 as i16;
    let mut a = tracks.wrapping_add_signed(size as i32);
    let mut instruments = [0u32; 63];
    for ins in instruments.iter_mut().take(num_ins) {
        *ins = a;
        a = a.wrapping_add(22 + 4 * m.bus.r8(a + 0x15) as u32);
    }
    let title = m.bus.read_cstr(a);
    // its revision goes to a variable and the module's byte to 0
    let revision = d[3];
    m.bus.w8(base + 3, 0);

    // the block goes where the reference's lies, 0x400 bytes past the module
    // in amigaplayer-rs's heap: an empty track's parameter, read 64 KB past the
    // tracks, can land in it. The voice buffers are in chip memory, at the
    // heap's top.
    let top = std::mem::replace(&mut m.alloc_top, false);
    let block = m.alloc(BLOCK_SIZE);
    m.alloc_top = true;
    let chip = m.alloc(4 * BUFFER);
    m.alloc_top = top;
    if block == 0 || chip == 0 {
        return None;
    }
    build_waves(&mut m.bus, block);

    let mut p = Ahx {
        title,
        revision,
        null_empty,
        timer: TIMERS[(flags >> 13) as usize & 3],
        len_nr,
        res_nr: be16(d, 8),
        track_length,
        subsongs,
        sub_table,
        pos_table,
        tracks,
        instruments,
        block,
        chip,
        voices: Default::default(),
        first_tick: false,
        step_wait: 0,
        get_new_position: false,
        tempo: 6,
        pattern_break: false,
        pos_jump: 0,
        pos_jump_note: 0,
        wn_random: 0,
        note_nr: 0,
        pos_nr: 0,
        sub: 0,
        songend: false,
    };
    let mut hw = Hw { bus: &mut m.bus };
    hw.wait(start_clocks(d));
    p.init_sub_song(0, &mut hw);
    hw.wait(INIT_TO_TIMER);
    // the init routine sets the LED bit of CIA-A's port: the filter is off
    m.bus.ciaa.pra |= 2;
    Some(Box::new(p))
}

/// The waveforms, as the original's InitWaves builds them (ahx2play's
/// generators), at their offsets in `block`, and the waveform pointers.
fn build_waves(bus: &mut Bus, block: u32) {
    let mut w = vec![0u8; (HIGH_PASSES - TRIANGLES) as usize];
    // six triangles of 4 to 128 bytes
    let mut o = 0;
    for i in 0..6 {
        let full = 4usize << i;
        let len = full >> 2;
        let delta = (128 / len) as i16;
        let start = o;
        let mut data: i16 = 0;
        for _ in 0..len {
            w[o] = data as u8;
            o += 1;
            data += delta;
        }
        w[o] = 127;
        o += 1;
        data = 128;
        for _ in 0..len - 1 {
            data -= delta;
            w[o] = data as u8;
            o += 1;
        }
        // the second half: the first one negated (127 as -128)
        let mut src = o - full / 2;
        for _ in 0..len * 2 {
            let s = w[src] as i8;
            src += 1;
            w[o] = if s == 127 { 0x80 } else { s.wrapping_neg() as u8 };
            o += 1;
        }
        debug_assert_eq!(o - start, full);
    }
    // six sawtooths
    for i in 0..6 {
        let len = 4usize << i;
        let delta = (256 / (len - 1)) as i8;
        let mut data: i8 = -128;
        for _ in 0..len {
            w[o] = data as u8;
            o += 1;
            data = data.wrapping_add(delta);
        }
    }
    // 32 squares of 128 bytes, the first with 2 high bytes, the last with 64
    for i in 1..=32 {
        for j in 0..64 {
            let v = if j < 64 - i { 0x80 } else { 0x7F };
            w[o] = v;
            w[o + 1] = v;
            o += 2;
        }
    }
    // white noise
    let mut seed: u32 = 0x4159_5321;
    for _ in 0..0x780 {
        w[o] = if seed & 0x100 == 0 {
            seed as u8
        } else if seed & 0x8000 != 0 {
            0x80
        } else {
            0x7F
        };
        o += 1;
        seed = seed.rotate_right(5) ^ 0x9A;
        let mut t = seed as u16;
        seed = seed.rotate_left(2);
        t = t.wrapping_add(seed as u16);
        seed ^= t as u32;
        seed = seed.rotate_right(3);
    }
    debug_assert_eq!(o, (HIGH_PASSES - TRIANGLES) as usize);

    // the filtered copies: 31 low passes (before the waveforms) and 31 high
    // passes (after them), each over all 45 waveforms in turn
    let n = FILTER_LEN as usize * 31;
    let (mut lo, mut hi) = (vec![0u8; n], vec![0u8; n]);
    let clip = |x: i32| -> i32 {
        let i = (x >> 16) as i16;
        if i > 127 {
            127 << 16
        } else if i < -128 {
            -128 << 16
        } else {
            x
        }
    };
    let mut d5: i32 = ((((8 << 16) * 125) / 100) / 100) >> 8;
    let mut k = 0;
    for _ in 0..31 {
        let mut src = 0;
        for &len in FILTER_LENGTHS.iter() {
            let (mut d2, mut d3) = (0i32, 0i32);
            // three passes settle the filter (the original's table of their
            // results keeps 8 bits less)
            for pass in 0..4 {
                if pass == 3 {
                    d2 &= !0xFF;
                    d3 &= !0xFF;
                }
                for s in &w[src..src + len] {
                    let d0 = (*s as i8 as i32) << 16;
                    let d1 = clip(d0 - d2 - d3);
                    d2 = clip(d2 + (d1 >> 8) * d5);
                    d3 = clip(d3 + (d2 >> 8) * d5);
                    if pass == 3 {
                        hi[k] = (d1 >> 16) as u8;
                        lo[k] = (d3 >> 16) as u8;
                        k += 1;
                    }
                }
            }
            src += len;
        }
        d5 += ((((3 << 16) * 125) / 100) / 100) >> 8;
    }
    bus.write_bytes(block + LOW_PASSES, &lo);
    bus.write_bytes(block + TRIANGLES, &w);
    bus.write_bytes(block + HIGH_PASSES, &hi);
    for (i, o) in [TRIANGLES, SAWTOOTHS, 0, NOISE].into_iter().enumerate() {
        bus.w32(block + WAVE_TAB + 4 * i as u32, if o == 0 { 0 } else { block + o });
    }
    // the filter loop's counter, which a waveform past the four reads
    bus.w16(block + WAVE_TAB + 16, 31);
}

impl Replayer for Ahx {
    fn format(&self) -> &'static str {
        "AHX"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn tick_clocks(&self) -> u64 {
        (self.timer as u64 + 1) * 5
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.subsongs as u16, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        // a subsong change is EndSound, which silences the voices as
        // InitSound does, then InitSound
        for v in 0..4 {
            hw.vol(v, 0);
            hw.wait(QUIET_GAP);
        }
        hw.wait(END_TO_INIT);
        self.init_sub_song(n, hw);
    }

    fn tick(&mut self, hw: &mut Hw) {
        if self.first_tick {
            // the first interrupt after InitSound sets the voices up
            self.first_tick = false;
            hw.wait(SETUP_DMA_OFF);
            hw.dmacon(0x000F);
            for v in 0..4 {
                hw.wait(if v == 0 { SETUP_LOC } else { SETUP_LOC_NEXT });
                hw.loc(v, self.chip + BUFFER * v as u32);
                hw.wait(SETUP_REG[0]);
                hw.len(v, 0x140);
                hw.wait(SETUP_REG[1]);
                hw.per(v, 0x88);
                hw.wait(SETUP_REG[2]);
                hw.vol(v, 0);
            }
            hw.wait(SETUP_DMA_ON);
            hw.dmacon(0x800F);
            let now = hw.bus.clock;
            hw.wait((now / LINE + 10) * LINE - now);
            hw.wait(SETUP_TO_VOICE - TO_VOLUME);
        } else {
            hw.wait(ENTRY);
        }
        for v in 0..4 {
            self.set_audio(v, hw);
        }
        let bus = &mut *hw.bus;
        if self.step_wait == 0 {
            if self.get_new_position {
                let mut next = self.pos_nr.wrapping_add(1);
                if next == self.len_nr {
                    next = 0;
                }
                let at = |p: u16| self.pos_table.wrapping_add_signed((p << 3) as i16 as i32);
                let (cur, nxt) = (at(self.pos_nr), at(next));
                for (i, v) in self.voices.iter_mut().enumerate() {
                    v.track = bus.r8(cur + 2 * i as u32);
                    v.transpose = bus.r8(cur + 2 * i as u32 + 1) as i8;
                    v.next_track = bus.r8(nxt + 2 * i as u32);
                }
                self.get_new_position = false;
            }
            for v in 0..4 {
                self.process_step(v, bus);
            }
            self.step_wait = self.tempo;
        }
        for v in 0..4 {
            self.process_frame(v, bus);
        }
        self.step_wait = self.step_wait.wrapping_sub(1);
        if self.step_wait != 0 {
            return;
        }
        if !self.pattern_break {
            self.note_nr = self.note_nr.wrapping_add(1);
            if self.note_nr != self.track_length {
                return;
            }
            self.pos_jump = self.pos_nr.wrapping_add(1);
        }
        self.pattern_break = false;
        self.note_nr = self.pos_jump_note;
        self.pos_jump_note = 0;
        self.pos_nr = self.pos_jump;
        self.pos_jump = 0;
        if self.pos_nr == self.len_nr {
            self.songend = true;
            self.pos_nr = self.res_nr;
        }
        self.get_new_position = true;
    }
}

impl Ahx {
    /// InitSound's InitSubSong: subsong 0 plays from position 0, subsong n
    /// from the n-th entry of the module's table.
    fn init_sub_song(&mut self, n: u16, hw: &mut Hw) {
        self.sub = n;
        self.pos_nr = if n == 0 { 0 } else { hw.bus.r16(self.sub_table + 2 * (n as u32 - 1)) };
        self.pos_jump = 0;
        self.tempo = 6;
        self.step_wait = 0;
        self.get_new_position = true;
        self.note_nr = 0;
        for v in 0..4 {
            if v > 0 {
                hw.wait(QUIET_GAP);
            }
            hw.vol(v, 0);
        }
        for v in self.voices.iter_mut() {
            *v = Voice { track_master_volume: 0x40, ..Default::default() };
        }
        self.first_tick = true;
    }

    /// The word of a track's row and the address of its parameter byte, read
    /// as the original does: a track left out of the module (track 0 with the
    /// header's flag) reads as an empty row, its parameter 64 KB on.
    fn row(&self, bus: &Bus, track: u8, note: u16) -> Option<(u16, u8)> {
        let mut t = track as u16;
        let off = if self.null_empty {
            t = t.wrapping_sub(1);
            if (t as i16) < 0 {
                return None;
            }
            (((t as u32 * self.track_length as u32) as u16).wrapping_add(note) as u32) * 3
        } else {
            (((t as u32 * self.track_length as u32) as u16).wrapping_add(note) as u32) * 3
        };
        let a = self.tracks.wrapping_add(off);
        Some((bus.r16(a), bus.r8(a + 2)))
    }

    fn set_audio(&mut self, v: usize, hw: &mut Hw) {
        let ch = &mut self.voices[v];
        if ch.plant_period {
            hw.wait(TO_PERIOD);
            hw.per(v, ch.audio_period);
            hw.wait(PERIOD_TO_ZERO);
            hw.vol(v, 0);
            ch.plant_period = false;
            hw.wait(ZERO_TO_VOLUME);
        } else {
            hw.wait(TO_VOLUME);
        }
        if ch.new_waveform {
            let dst = self.chip + BUFFER * v as u32;
            let src = ch.audio_source;
            let bus = &mut *hw.bus;
            if ch.waveform == 3 {
                for i in 0..BUFFER {
                    bus.w8(dst + i, bus.r8(src.wrapping_add(i)));
                }
                hw.wait(COPY_NOISE);
            } else if ch.wavelength <= 5 {
                let len = 4u32 << ch.wavelength;
                let passes = 5u32 << (5 - ch.wavelength);
                for p in 0..passes {
                    for i in 0..len {
                        bus.w8(dst + p * len + i, bus.r8(src.wrapping_add(i)));
                    }
                }
                hw.wait(COPY + COPY_PASS * passes as u64);
            }
            // a wavelength past 5 makes the original copy 65536 passes over
            // memory past the buffer; not done here
            ch.new_waveform = false;
        }
        hw.vol(v, ch.audio_volume);
    }

    fn process_step(&mut self, v: usize, bus: &mut Bus) {
        let tempo = self.tempo as u8;
        let ch = &mut self.voices[v];
        ch.volume_slide_up = 0;
        ch.volume_slide_down = 0;
        let (word, param) = match {
            let ch = &self.voices[v];
            self.row(bus, ch.track, self.note_nr)
        } {
            Some(r) => r,
            // the left-out track: an empty row, the parameter read 64 KB on
            None => (0, bus.r8(self.tracks.wrapping_add(0xFFFF + 2))),
        };
        let ch = &mut self.voices[v];
        let cmd = word & 0xF;
        if cmd == 0xE {
            let (e, p) = (param >> 4, param & 0xF);
            if e == 0xC && (p as i8) < tempo as i8 {
                ch.note_cut_wait = p;
                if p != 0 {
                    ch.note_cut_on = true;
                    ch.hard_cut_release = false;
                }
            }
            if e == 0xD {
                if ch.note_delay_on {
                    ch.note_delay_on = false;
                } else if (p as i8) < tempo as i8 {
                    ch.note_delay_wait = p;
                    if p != 0 {
                        ch.note_delay_on = true;
                        return;
                    }
                }
            }
        }
        if cmd == 0 && param != 0 && param & 0xF <= 9 {
            self.pos_jump = (param & 0xF) as u16;
        }
        if cmd == 0xD {
            self.pos_jump = self.pos_nr.wrapping_add(1);
            let n = (param & 0xF).wrapping_add((param >> 4) * 10);
            self.pos_jump_note = (self.pos_jump_note & 0xFF00) | n as u16;
            if self.track_length as i16 <= self.pos_jump_note as i16 {
                self.pos_jump_note = 0;
            }
            self.pattern_break = true;
        }
        if cmd == 0xB {
            let j = (self.pos_jump as u32 * 100) as u16;
            let j = j.wrapping_add((param & 0xF) as u16).wrapping_add((param >> 4) as u16 * 10);
            self.pos_jump = j;
            if j as i16 <= self.pos_nr as i16 {
                self.songend = true;
            }
            self.pattern_break = true;
        }
        if cmd == 0xF {
            self.tempo = param as u16;
            if param == 0 {
                self.songend = true;
            }
        }
        if cmd == 5 || cmd == 0xA {
            ch.volume_slide_down = param & 0xF;
            ch.volume_slide_up = param >> 4;
        }
        let instr = ((word & 0x3F0) >> 4) as usize;
        if instr > 0 {
            ch.perf_sub_volume = 0x40;
            ch.period_slide_speed = 0;
            ch.period_slide_period = 0;
            ch.period_slide_limit = 0;
            let ins = self.instruments.get(instr - 1).copied().unwrap_or(0);
            ch.adsr = 0;
            if ins != 0 {
                let b = |o: u32| bus.r8(ins + o);
                ch.a_frames = b(2);
                ch.a_delta = if b(2) == 0 { 0 } else { (((b(3) as u32) << 8) / b(2) as u32) as u16 as i16 };
                ch.d_frames = b(4);
                ch.d_delta = divs(((b(5).wrapping_sub(b(3)) as i8 as i16) << 8) as i32, b(4) as i16);
                ch.s_frames = b(6);
                ch.r_frames = b(7);
                ch.r_delta = divs(((b(8).wrapping_sub(b(5)) as i8 as i16) << 8) as i32, b(7) as i16);
                ch.wavelength = b(1) & 7;
                ch.note_max_volume = b(0);
                ch.vibrato_current = 0;
                ch.vibrato_delay = b(13);
                ch.vibrato_depth = b(14) & 0xF;
                ch.vibrato_speed = b(15);
                ch.vibrato_period = 0;
                ch.hard_cut_release = b(14) & 0x80 != 0;
                ch.hard_cut = (b(14) & 0x70) >> 4;
                ch.ignore_square = false;
                ch.square_sliding_in = false;
                ch.square_wait = 0;
                ch.square_on = false;
                let s = 5u8.wrapping_sub(ch.wavelength);
                let (mut lo, mut hi) = (lsr_b(b(16), s), lsr_b(b(17), s));
                if lo as i8 > hi as i8 {
                    std::mem::swap(&mut lo, &mut hi);
                }
                ch.square_lower = lo;
                ch.square_upper = hi;
                ch.ignore_filter = 0;
                ch.filter_wait = 0;
                ch.filter_on = false;
                ch.filter_sliding_in = false;
                let mut speed = b(1) >> 3;
                let (mut lo, mut hi) = (b(12), b(19));
                if lo & 0x80 != 0 {
                    speed |= 0x20;
                }
                if hi & 0x80 != 0 {
                    speed |= 0x40;
                }
                ch.filter_speed = speed;
                lo &= 0x7F;
                hi &= 0x7F;
                if lo > hi {
                    std::mem::swap(&mut lo, &mut hi);
                }
                ch.filter_lower = lo;
                ch.filter_upper = hi;
                ch.filter_pos = 0x20;
                ch.perf_wait = 0;
                ch.perf_speed = b(20);
                ch.perf_current = 0;
                ch.instrument = ins;
                ch.perf_list = ins + 22;
            }
        }
        if cmd == 9 {
            ch.square_pos = lsr_b(param, 5u8.wrapping_sub(ch.wavelength));
            ch.plant_square = true;
            ch.ignore_square = true;
        }
        if cmd == 4 && self.revision != 0 {
            if (param as i8) < 0x40 {
                ch.ignore_filter = param;
            } else {
                ch.filter_pos = param - 0x40;
            }
        }
        ch.period_slide_on = false;
        let mut note = (word >> 10) as i16;
        if cmd == 3 || cmd == 5 {
            if cmd == 3 && param != 0 {
                ch.period_slide_speed = param as i16;
            }
            let mut slide = true;
            if note != 0 {
                let limit = (PERIODS[ch.track_period as usize & 63] as i16).wrapping_sub(PERIODS[note as usize] as i16);
                if limit.wrapping_add(ch.period_slide_period) == 0 {
                    slide = false;
                } else {
                    ch.period_slide_limit = limit.wrapping_neg();
                }
            }
            if slide {
                ch.period_slide_on = true;
                ch.period_slide_with_limit = true;
                note = 0;
            }
        }
        if note != 0 {
            ch.track_period = note;
            ch.plant_period = true;
        }
        if cmd == 1 {
            ch.period_slide_speed = -(param as i16);
            ch.period_slide_on = true;
            ch.period_slide_with_limit = false;
        }
        if cmd == 2 {
            ch.period_slide_speed = param as i16;
            ch.period_slide_on = true;
            ch.period_slide_with_limit = false;
        }
        if cmd == 0xC {
            let mut p = param as i16;
            if p <= 0x40 {
                ch.note_max_volume = p as u8;
            } else {
                p -= 0x50;
                if p >= 0 {
                    if p <= 0x40 {
                        for c in self.voices.iter_mut() {
                            c.track_master_volume = p as u8;
                        }
                    } else {
                        p -= 0x50;
                        if (0..=0x40).contains(&p) {
                            ch.track_master_volume = p as u8;
                        }
                    }
                }
            }
        }
    }

    /// One of a play list entry's two commands.
    fn plist_command(&mut self, v: usize, cmd: u8, param: u8) {
        let rev = self.revision;
        let ch = &mut self.voices[v];
        match cmd {
            0 if rev != 0 => {
                if param != 0 {
                    let mut p = param;
                    if ch.ignore_filter != 0 {
                        p = ch.ignore_filter;
                        ch.ignore_filter = 0;
                    }
                    ch.filter_pos = p;
                    ch.new_waveform = true;
                }
            }
            1 => {
                ch.period_perf_slide_speed = param as i16;
                ch.period_perf_slide_on = true;
            }
            2 => {
                ch.period_perf_slide_speed = -(param as i16);
                ch.period_perf_slide_on = true;
            }
            3 => {
                if ch.ignore_square {
                    ch.ignore_square = false;
                } else {
                    ch.square_pos = lsr_b(param, 5u8.wrapping_sub(ch.wavelength));
                }
            }
            4 => {
                if rev == 0 || param == 0 {
                    ch.square_on = !ch.square_on;
                    ch.square_init = ch.square_on;
                    ch.square_signum = 1;
                } else {
                    if param & 0xF != 0 {
                        ch.square_on = !ch.square_on;
                        ch.square_init = ch.square_on;
                        ch.square_signum = if param & 0xF == 0xF { 0xFF } else { 1 };
                    }
                    if param >> 4 != 0 {
                        ch.filter_on = !ch.filter_on;
                        ch.filter_init = ch.filter_on;
                        ch.filter_signum = if param >> 4 == 0xF { 0xFF } else { 1 };
                    }
                }
            }
            5 => {
                ch.perf_current = param.wrapping_sub(1);
                ch.perf_list = ch.instrument.wrapping_add(0x12 + 4 * param as u32);
            }
            6 => {
                let mut p = param as i16;
                if p <= 0x40 {
                    ch.note_max_volume = p as u8;
                } else {
                    p -= 0x50;
                    if p >= 0 {
                        if p <= 0x40 {
                            ch.perf_sub_volume = p as u8;
                        } else {
                            p -= 0x50;
                            if (0..=0x40).contains(&p) {
                                ch.track_master_volume = p as u8;
                            }
                        }
                    }
                }
            }
            7 => {
                ch.perf_speed = param;
                ch.perf_wait = param;
            }
            _ => {}
        }
    }

    fn process_frame(&mut self, v: usize, bus: &mut Bus) {
        let tempo = self.tempo as u8;
        let ch = &self.voices[v];
        if ch.hard_cut != 0 {
            let (mut track, mut note) = (ch.track, self.note_nr.wrapping_add(1));
            if note == self.track_length {
                note = 0;
                track = ch.next_track;
            }
            if let Some((word, _)) = self.row(bus, track, note) {
                if word & 0x3F0 != 0 {
                    let ch = &mut self.voices[v];
                    let mut range = tempo.wrapping_sub(ch.hard_cut);
                    if (range as i8) < 0 {
                        range = 0;
                    }
                    if !ch.note_cut_on {
                        ch.note_cut_on = true;
                        ch.note_cut_wait = range;
                        ch.hard_cut_release_f = tempo.wrapping_sub(range);
                    }
                    ch.hard_cut = 0;
                }
            }
        }
        let ch = &mut self.voices[v];
        if ch.note_cut_on {
            if ch.note_cut_wait == 0 {
                ch.note_cut_on = false;
                if ch.hard_cut_release {
                    let r = (bus.r8(ch.instrument.wrapping_add(8)) as u16) << 8;
                    let d0 = (ch.adsr as u16).wrapping_sub(r) as i16 as i32;
                    ch.r_frames = ch.hard_cut_release_f;
                    ch.r_delta = divs(d0, ch.hard_cut_release_f as i8 as i16).wrapping_neg();
                    ch.a_frames = 0;
                    ch.d_frames = 0;
                    ch.s_frames = 0;
                } else {
                    ch.note_max_volume = 0;
                }
            }
            ch.note_cut_wait = ch.note_cut_wait.wrapping_sub(1);
        }
        if ch.note_delay_on {
            if ch.note_delay_wait == 0 {
                self.process_step(v, bus);
            } else {
                ch.note_delay_wait -= 1;
            }
        }
        let ch = &mut self.voices[v];
        let ins = ch.instrument;
        let b = |o: u32| bus.r8(ins.wrapping_add(o));
        if ch.a_frames != 0 {
            ch.adsr = ch.adsr.wrapping_add(ch.a_delta);
            ch.a_frames -= 1;
            if ch.a_frames == 0 {
                ch.adsr = ((b(3) as u16) << 8) as i16;
            }
        } else if ch.d_frames != 0 {
            ch.adsr = ch.adsr.wrapping_add(ch.d_delta);
            ch.d_frames -= 1;
            if ch.d_frames == 0 {
                ch.adsr = ((b(5) as u16) << 8) as i16;
            }
        } else if ch.s_frames != 0 {
            ch.s_frames -= 1;
        } else if ch.r_frames != 0 {
            ch.adsr = ch.adsr.wrapping_add(ch.r_delta);
            ch.r_frames -= 1;
            if ch.r_frames == 0 {
                ch.adsr = ((b(8) as u16) << 8) as i16;
            }
        }
        // the volume slide
        let mut vol = ch.note_max_volume.wrapping_sub(ch.volume_slide_down).wrapping_add(ch.volume_slide_up);
        if (vol as i8) < 0 {
            vol = 0;
        }
        if (vol as i8) > 0x40 {
            vol = 0x40;
        }
        ch.note_max_volume = vol;
        // the portamento
        if ch.period_slide_on {
            let mut speed = ch.period_slide_speed;
            if ch.period_slide_with_limit {
                let d0 = ch.period_slide_period.wrapping_sub(ch.period_slide_limit);
                if d0 != 0 {
                    if d0 > 0 {
                        speed = speed.wrapping_neg();
                    }
                    ch.period_slide_period = if (d0.wrapping_add(speed) ^ d0) < 0 {
                        ch.period_slide_limit
                    } else {
                        ch.period_slide_period.wrapping_add(speed)
                    };
                    ch.plant_period = true;
                }
            } else {
                ch.period_slide_period = ch.period_slide_period.wrapping_add(speed);
                ch.plant_period = true;
            }
        }
        // the vibrato
        if ch.vibrato_depth != 0 {
            if ch.vibrato_delay != 0 {
                ch.vibrato_delay -= 1;
            } else {
                let p = VIBRATO[ch.vibrato_current as usize & 63] as i32 * ch.vibrato_depth as i32;
                ch.vibrato_period = ((p as i16) >> 7) | ((p >> 16) as i16 & i16::MIN);
                ch.plant_period = true;
                ch.vibrato_current = ch.vibrato_current.wrapping_add(ch.vibrato_speed) & 0x3F;
            }
        }
        // the play list
        if ins != 0 {
            if b(0x15) == ch.perf_current {
                if ch.perf_wait == 0 {
                    ch.period_perf_slide_speed = 0;
                } else {
                    ch.perf_wait -= 1;
                }
            } else {
                let w = ch.perf_wait;
                ch.perf_wait = w.wrapping_sub(1);
                // subq, bgt: waits from 2 to $7F only ($80 overflows)
                if !(2..=0x7F).contains(&w) {
                    let pl = ch.perf_list;
                    let word = bus.r16(pl);
                    let wave = ((word >> 7) & 7) as u8;
                    if wave != 0 {
                        ch.waveform = wave - 1;
                        ch.new_waveform = true;
                        ch.period_perf_slide_speed = 0;
                        ch.period_perf_slide_period = 0;
                    }
                    ch.period_perf_slide_on = false;
                    let (p1, p2) = (bus.r8(pl.wrapping_add(2)), bus.r8(pl.wrapping_add(3)));
                    self.plist_command(v, ((word >> 10) & 7) as u8, p1);
                    self.plist_command(v, (word >> 13) as u8, p2);
                    let ch = &mut self.voices[v];
                    let note = word & 0x3F;
                    if note != 0 {
                        ch.instr_period = note as i16;
                        ch.plant_period = true;
                        ch.fixed_note = word & 0x40 != 0;
                    }
                    ch.perf_list = ch.perf_list.wrapping_add(4);
                    ch.perf_current = ch.perf_current.wrapping_add(1);
                    ch.perf_wait = ch.perf_speed;
                }
            }
        }
        let ch = &mut self.voices[v];
        // the play list's portamento
        if ch.period_perf_slide_on {
            ch.period_perf_slide_period = ch.period_perf_slide_period.wrapping_sub(ch.period_perf_slide_speed);
            if ch.period_perf_slide_period != 0 {
                ch.plant_period = true;
            }
        }
        // the square's modulation
        if ch.waveform == 2 && ch.square_on {
            let w = ch.square_wait;
            ch.square_wait = w.wrapping_sub(1);
            if !(2..=0x7F).contains(&w) {
                let (lo, hi) = (ch.square_lower as i8, ch.square_upper as i8);
                let pos = ch.square_pos as i8;
                if ch.square_init {
                    ch.square_init = false;
                    if pos <= lo {
                        ch.square_sliding_in = true;
                        ch.square_signum = 1;
                    } else if pos >= hi {
                        ch.square_sliding_in = true;
                        ch.square_signum = 0xFF;
                    }
                }
                if pos == lo || pos == hi {
                    if ch.square_sliding_in {
                        ch.square_sliding_in = false;
                    } else {
                        ch.square_signum = ch.square_signum.wrapping_neg();
                    }
                }
                ch.square_pos = ch.square_pos.wrapping_add(ch.square_signum);
                ch.plant_square = true;
                ch.square_wait = bus.r8(ch.instrument.wrapping_add(0x12));
            }
        }
        // the filter's modulation
        if ch.filter_on {
            let w = ch.filter_wait;
            ch.filter_wait = w.wrapping_sub(1);
            if !(2..=0x7F).contains(&w) {
                let (lo, hi) = (ch.filter_lower as i8, ch.filter_upper as i8);
                let mut pos = ch.filter_pos;
                if ch.filter_init {
                    ch.filter_init = false;
                    if pos as i8 <= lo {
                        ch.filter_sliding_in = true;
                        ch.filter_signum = 1;
                    } else if pos as i8 >= hi {
                        ch.filter_sliding_in = true;
                        ch.filter_signum = 0xFF;
                    }
                }
                let cycles = if (ch.filter_speed as i8) < 4 { 5 - ch.filter_speed as i8 as i32 } else { 1 };
                for _ in 0..cycles {
                    if pos as i8 == lo || pos as i8 == hi {
                        if ch.filter_sliding_in {
                            ch.filter_sliding_in = false;
                        } else {
                            ch.filter_signum = ch.filter_signum.wrapping_neg();
                        }
                    }
                    pos = pos.wrapping_add(ch.filter_signum);
                }
                ch.filter_pos = pos;
                ch.new_waveform = true;
                let mut w = ch.filter_speed.wrapping_sub(3);
                if (w as i8) < 1 {
                    w = 1;
                }
                ch.filter_wait = w;
            }
        }
        let filter = (ch.filter_pos as i8 as i32 - 32) * FILTER_LEN;
        // the square, built in the voice's buffer
        if ch.waveform == 2 || ch.plant_square {
            let mut src = (self.block + SQUARES).wrapping_add_signed(filter);
            let mut which = lsl_b(ch.square_pos, 5u8.wrapping_sub(ch.wavelength)) as i16;
            if which > 0x20 {
                which = 0x40 - which;
            }
            which -= 1;
            if which < 0 {
                which = 0;
            }
            src = src.wrapping_add_signed((which << 7) as i32);
            let buf = self.block + VOICE0 + VOICE_SIZE * v as u32 + VOICE_SQUARE;
            let wl = ch.wavelength.min(5);
            let delta = 0x20u32 >> wl;
            for i in 0..4u32 << wl {
                bus.w8(buf + i, bus.r8(src.wrapping_add(i * delta)));
            }
            bus.w32(self.block + WAVE_TAB + 8, buf);
            ch.new_waveform = true;
            ch.plant_square = false;
        }
        if ch.waveform == 3 {
            ch.new_waveform = true;
        }
        if ch.new_waveform {
            let wf = ch.waveform as u32;
            let mut src = bus.r32(self.block + WAVE_TAB + 4 * wf);
            if wf != 2 {
                src = src.wrapping_add_signed(filter);
            }
            if wf < 2 {
                src = src.wrapping_add(WAVE_OFFSETS[ch.wavelength.min(5) as usize]);
            }
            if wf == 3 {
                let seed = self.wn_random;
                src = src.wrapping_add(seed & 0x4FF);
                let seed = seed.wrapping_add(0x222B98).rotate_right(8).wrapping_add(0xBEFF3) ^ 0x4B;
                self.wn_random = seed.wrapping_sub(0x1A4F);
            }
            ch.audio_source = src;
        }
        // the period
        let mut note = ch.instr_period;
        if !ch.fixed_note {
            note = note.wrapping_add(ch.transpose as i16).wrapping_add(ch.track_period - 1);
        }
        if note > 60 {
            note = 60;
        }
        let mut period = if note < 0 { before_periods(note) } else { PERIODS[note as usize] as i16 };
        if !ch.fixed_note {
            period = period.wrapping_add(ch.period_slide_period);
        }
        period = period.wrapping_add(ch.period_perf_slide_period).wrapping_add(ch.vibrato_period);
        ch.audio_period = period.clamp(0x71, 0xD60) as u16;
        // the volume, in 16 bits as the original's mulu and lsr.w do
        let mul = |a: u16, b: u16| ((a as u32 * b as u32) as u16) >> 6;
        let mut vol = (ch.adsr >> 8) as i8 as i16 as u16;
        vol = mul(vol, ch.note_max_volume as u16);
        vol = mul(vol, ch.perf_sub_volume as u16);
        vol = mul(vol, ch.track_master_volume as u16);
        vol = mul(vol, 0x40);
        ch.audio_volume = vol;
    }
}

/// A note below 0 makes the original read the words of its own code that
/// come before the period table. Those are bytes of a 68k binary and are not
/// copied here (ahx2play's table of them comes from the AHX 2.3d tracker, a
/// different binary). The port uses the longest period instead, which leaves
/// the fewest differing ticks on the one module of the corpus seen doing
/// this (FISH); the others differ from the reference.
fn before_periods(_note: i16) -> i16 {
    0xD60
}

include!("ahx_tables.rs");

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// One position, one track of 4 rows (row 0: C-3 with instrument 1), one
    /// instrument whose play list starts a 128-byte triangle.
    fn module() -> Vec<u8> {
        let mut d = b"THX\x01".to_vec();
        d.extend([0, 0]); // offset of the name, unused
        d.extend([0x00, 0x01]); // 50 Hz, 1 position
        d.extend([0, 0]); // restart position
        d.extend([4, 0, 1, 0]); // 4 rows a track, track 0 the highest, 1 instrument, no subsongs
        d.extend([0; 8]); // position 0: track 0 on every voice
        d.extend([25 << 2, 0x10, 0]); // C-3, instrument 1
        d.extend([0; 9]);
        // volume 64, wavelength 5; attack 1 frame to 64, decay 1 to 64, sustain 10, release 1 to 0
        d.extend([64, 5, 1, 64, 1, 64, 10, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0x20, 0x3F, 1, 0x1F, 1, 1]);
        d.extend([0x00, 0x80, 0, 0]); // play list: triangle
        d.extend(b"test\0");
        d
    }

    #[test]
    fn plays_a_triangle() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.ahx"), module()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "AHX");
        assert_eq!(e.playing.as_ref().unwrap().module_name, "test");
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // InitSound's first write comes where the Eagleplayer's start (the
        // waveforms, the song-length scan of this module) ends
        assert_eq!(log[0].2, 35_748_331);
        // the first tick's setup
        assert_eq!(&reg(0xDFF096)[..2], &[0x000F, 0x800F]);
        assert_eq!(reg(0xDFF0A4)[0], 0x140);
        // C-3's period, then the envelope's 64
        assert_eq!(reg(0xDFF0A6)[..2], [0x88, super::PERIODS[24]]);
        assert!(reg(0xDFF0A8).contains(&64));
        // voice 0's buffer: five copies of the 128-byte triangle
        let a = (reg(0xDFF0A0)[0] as u32) << 16 | reg(0xDFF0A2)[0] as u32;
        let buf: Vec<u8> = (0..0x280).map(|i| e.m.bus.r8(a + i)).collect();
        assert_eq!(&buf[..4], &[0, 4, 8, 12]);
        assert_eq!(buf[32], 127);
        assert_eq!(buf[64 + 32], 0x80);
        assert!((1..5).all(|k| buf[..128] == buf[128 * k..128 * (k + 1)]));
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}
