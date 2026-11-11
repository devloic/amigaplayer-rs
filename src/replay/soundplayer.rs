//! SoundPlayer (`sjs.*` with a sample companion `smp.*`), the music of the Amiga
//! game Lemmings and its add-on and sequel releases, by Scott Johnston.
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's SoundPlayer and the structure
//! of the corpus files), and this player was written from it alone and checked
//! against those writes.
//!
//! A song file is a three-byte header (timer word, low byte first; voice mask) and
//! a grid of 12-byte rows of four (note, instrument, command) groups; each voice
//! walks the grid on its own. The companion is a run of IFF 8SVX `FORM` chunks
//! whose positions (in steps of four bytes) number the instrument slots. The
//! timer sets the tick; every sixth tick is a row tick. A note stops a voice's DMA
//! on the row tick and starts the sample on the next tick.
//!
//! Every write falls at the reference's colour clock: the tick is a ledger of the
//! cycles its passes take (the constants below). The voices the mask leaves out
//! walk their column like the others (holds, LED commands, the restart).

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "SoundPlayer", detect, new };

/// Paula periods of the notes 1..39.
const PERIODS: [u16; 39] = [
    0x434, 0x3F8, 0x3C0, 0x358, 0x328, 0x2FA, 0x2D0, 0x2A6, 0x280, 0x25C, 0x23A, 0x21A, 0x1FC, 0x1E0, 0x1C5, 0x1AC,
    0x194, 0x17D, 0x168, 0x153, 0x140, 0x12E, 0x11D, 0x10D, 0x0FE, 0x0F0, 0x0E2, 0x0D6, 0x0CA, 0x0BE, 0x0B4, 0x0AA,
    0x0A0, 0x097, 0x08F, 0x087, 0x07F, 0x078, 0x071,
];

/// Recognition by content: the first row has no notes or instruments and one
/// command on every playing voice.
fn detect(b: &[u8]) -> bool {
    if b.len() < 15 {
        return false;
    }
    if b[1] == 0 || b[2] == 0 || !(0x0B..=0xA0).contains(&b[1]) {
        return false;
    }
    if b[3] != 0 || b[4] != 0 || be16(b, 6) != 0 || b[9] != 0 || b[10] != 0 || be16(b, 12) != 0 {
        return false;
    }
    let c = b[5];
    c != 0 && (b[2] == 7 || (b[2] == 15 && b[14] == c)) && b[11] == c && b[8] == c
}

/// The names the sample companion may have: `smp.NAME` for `sjs.NAME` (the prefix
/// in any case), else `smp.set`.
fn companion(file: &str) -> Vec<String> {
    let file = file.rsplit(['/', ':']).next().unwrap_or(file);
    let mut v = Vec::new();
    if let Some(p) = file.get(..4) {
        if p.eq_ignore_ascii_case("sjs.") {
            v.push(format!("smp.{}", &file[4..]));
        }
    }
    v.push("smp.set".to_string());
    v
}

#[derive(Clone, Copy, Default)]
struct Slot {
    start: u32,
    len: u16,
    loop_start: u32,
    loop_len: u16,
}

fn find(d: &[u8], tag: &[u8; 4], from: usize, window: usize) -> Option<usize> {
    (from..from + window).find(|&i| d.get(i..i + 4) == Some(&tag[..]))
}

/// The slots of the sample file; `base` is where its first byte lies in memory.
fn slots(d: &[u8], base: u32) -> [Slot; 39] {
    let mut s = [Slot::default(); 39];
    let (mut cur, mut slot) = (0usize, 1usize);
    loop {
        if d.get(cur..cur + 4) == Some(b"FORM") {
            if let (Some(v), Some(b)) = (find(d, b"VHDR", cur, 0x400), find(d, b"BODY", cur, 0x800)) {
                let one = be32(d, v + 8);
                let rep = be32(d, v + 12);
                let start = base.wrapping_add(b as u32 + 8);
                s[slot] = Slot {
                    start,
                    len: (one >> 1) as u16,
                    loop_start: start.wrapping_add((one as u16 as i16) as i32 as u32),
                    loop_len: (rep >> 1) as u16,
                };
            }
            cur = cur.saturating_add(8).saturating_add(be32(d, cur + 4) as usize);
        } else {
            cur += 4;
        }
        if cur >= d.len() || slot == 38 {
            break;
        }
        slot += 1;
    }
    s
}

#[derive(Clone, Copy)]
struct Voice {
    row: i32,
    hold: u8,
    loopsave: i32,
    loopcnt: u8,
    vol: u8,
    per: u16,
    pend_start: u32,
    pend_len: u16,
    loop_addr: u32,
    loop_len: u16,
    hold_loop: bool,
    env_speed: u8,
    env_cnt: u8,
    env_up: bool,
    /// The fetched group of the current row tick.
    fetched: Option<[u8; 3]>,
}

impl Default for Voice {
    fn default() -> Self {
        Voice {
            row: 0,
            hold: 0,
            loopsave: 0,
            loopcnt: 0xFF,
            vol: 0,
            per: 0,
            pend_start: 0,
            pend_len: 0,
            loop_addr: 0,
            loop_len: 0,
            hold_loop: false,
            env_speed: 0,
            env_cnt: 0,
            env_up: true,
            fetched: None,
        }
    }
}

pub struct Sjs {
    d: Vec<u8>,
    title: String,
    timer: u32,
    mask: u8,
    slots: [Slot; 39],
    silence: u32,
    v: [Voice; 4],
    ticks: u32,
    songend: bool,
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let file = m.bus.read_cstr(m.deli.file_buf);
    let mut sample = None;
    for name in companion(&file) {
        if let Ok(s) = crate::deli::read_companion(m, &name).1 {
            sample = Some(s);
            break;
        }
    }
    let sample = sample?;
    // a silent long, then the sample file, then zeros for loops that run past it
    let size = 4 + sample.len() as u32 + 0x20000;
    let silence = m.alloc(size);
    if silence == 0 {
        return None;
    }
    m.bus.write_bytes(silence + 4, &sample);
    let mut r = Sjs {
        d: d.to_vec(),
        title: file,
        timer: d[0] as u32 | (d[1] as u32) << 8,
        mask: d[2],
        slots: slots(&sample, silence + 4),
        silence,
        v: Default::default(),
        ticks: 0,
        songend: false,
    };
    let mut hw = Hw { bus: &mut m.bus };
    r.start(&mut hw);
    Some(Box::new(r))
}

impl Sjs {
    fn start(&mut self, hw: &mut Hw) {
        self.v = Default::default();
        self.ticks = 0;
        self.songend = false;
        for ch in 0..8 {
            hw.vol(ch & 3, 0);
        }
        for ch in 0..4 {
            hw.dmacon(1 << ch);
            hw.loc(ch, self.silence);
            hw.len(ch, 1);
        }
    }

    /// The group of row `row` (1-based) of voice `ch`; zeros past the file's end.
    fn group(&self, row: i32, ch: usize) -> [u8; 3] {
        let mut g = [0u8; 3];
        if row >= 1 {
            let o = 3 + 12 * (row as usize - 1) + 3 * ch;
            for (i, x) in g.iter_mut().enumerate() {
                *x = self.d.get(o + i).copied().unwrap_or(0);
            }
        }
        g
    }

    /// Runs a row's command at its place in the tick; returns what it costs.
    fn command(&mut self, ch: usize, c: u8, hw: &mut Hw, clk: &mut Clk) -> u64 {
        let v = &mut self.v[ch];
        match c {
            // bset / bclr #1,$bfe001: the LED filter off, on
            0x01 | 0x02 => {
                clk.at(hw, CMD_LED_AT);
                if c == 1 {
                    hw.bus.ciaa.pra |= 2;
                } else {
                    hw.bus.ciaa.pra &= !2;
                }
                return CMD_LED;
            }
            0x03..=0x42 => {
                v.vol = c - 3;
                clk.at(hw, CMD_VOL_AT + CMD_VOICE * ch as u64);
                hw.vol(ch, v.vol as u16);
                return CMD_VOL[ch];
            }
            0x43 => {
                clk.at(hw, CMD_STOP_AT + CMD_VOICE * ch as u64);
                hw.dmacon(1 << ch);
                return CMD_STOP;
            }
            0x57..=0x88 => {
                if v.hold == 0 {
                    v.hold = c - 0x56;
                }
                return CMD_HOLD;
            }
            0xA7..=0xB0 => {
                v.env_up = true;
                v.env_speed = c - 0xA6;
                v.env_cnt = v.env_speed;
            }
            0xB1..=0xBA => {
                v.env_up = false;
                v.env_speed = c - 0xB0;
                v.env_cnt = v.env_speed;
            }
            0xCF => v.hold_loop = true,
            0xD0 => v.hold_loop = false,
            0xD2..=0xDB => {
                if v.loopcnt == 0xFF {
                    v.loopsave = v.row;
                    v.loopcnt = c - 0xD1;
                }
            }
            0xDC => {
                if v.loopcnt == 0 {
                    v.loopcnt = 0xFF;
                } else {
                    v.loopcnt -= 1;
                    v.row = v.loopsave;
                }
            }
            0xDD => v.row -= 1,
            0xDE => {
                v.row = 0;
                self.songend = true;
                // the original tells the host that the song has ended: the host goes on at a
                // whole colour clock, so a call at an odd cycle drops that cycle for the rest
                // of the tick
                clk.h = (clk.h + CMD_RESTART_CALL) & !1;
                return CMD_RESTART - CMD_RESTART_CALL;
            }
            0xDF..=0xE4 => hw.bus.write_word(0xDFF09E, [0x8001, 0x8002, 0x8004, 0x8010, 0x8020, 0x8040][(c - 0xDF) as usize]),
            0xF9..=0xFD => hw.bus.write_word(0xDFF09E, [0x0001, 0x0002, 0x0004, 0x0010, 0x0020][(c - 0xF9) as usize]),
            // sync flags and the unused values change no Paula register
            _ => {}
        }
        // (the cost of the commands no corpus file has is not known: that of no command)
        CMD_NONE
    }
}

// The tick's times in 68000 cycles (half colour clocks) from the timer interrupt's call, as the
// reference's runs give them (register writes with half-cycle times and the cycles of the code
// the tick runs; 31 modules). The tick is a run of passes over the four voices: the loops, the
// starts of the samples and, on a row tick, the hold counters, the rows' groups, the notes
// (DMA off) and the commands. Each pass costs a fixed time to reach, a fixed time between two
// voices and, per voice, the cost of what the voice does in it; a write falls a fixed time
// into its voice's part. Nothing else enters: a tick's writes follow from the song alone.
/// To the first voice's loop; between two voices of the pass.
const TO_LOOP: u64 = 458;
const LOOP_GAP: u64 = 34;
/// The loop pass of a voice with the empty word / with a loop: location, length, cost.
const LOOP_LOC: [u64; 2] = [76, 88];
const LOOP_LEN: [u64; 2] = [92, 108];
const LOOP_COST: [u64; 2] = [145, 161];
/// To the pass of the samples' starts; between two voices; a voice that starts none.
const TO_START: u64 = 76;
const START_GAP: u64 = 34;
const START_NONE: u64 = 45;
/// A start: location, length, period; the volume, the DMA on and the cost, by voice.
const START_LOC: u64 = 49;
const START_LEN: u64 = 69;
const START_PER: u64 = 89;
const START_VOL: [u64; 4] = [480, 534, 588, 619];
const START_DMA: [u64; 4] = [570, 624, 678, 699];
const START_COST: [u64; 4] = [642, 696, 750, 771];
/// A row tick: to the pass of the hold counters; between two voices; a voice not on hold, a
/// voice that stays on hold, a voice whose hold ends.
const TO_HOLD: u64 = 152;
const HOLD_GAP: u64 = 26;
const HOLD_FREE: u64 = 85;
const HOLD_STAYS: u64 = 90;
const HOLD_ENDS: u64 = 122;
/// To the pass that reads the rows' groups; between two voices; a voice on hold, a voice that
/// reads its group.
const TO_FETCH: u64 = 64;
const FETCH_GAP: u64 = 30;
const FETCH_NONE: u64 = 53;
const FETCH: u64 = 135;
/// To the pass of the notes; between two voices; a voice without a note; a note: its DMA off
/// and its cost.
const TO_NOTE: u64 = 84;
const NOTE_GAP: u64 = 26;
const NOTE_NONE: u64 = 45;
const NOTE_DMA: u64 = 45;
const NOTE_COST: u64 = 293;
/// To the pass of the commands; between two voices; no command.
const TO_CMD: u64 = 76;
const CMD_GAP: u64 = 34;
const CMD_NONE: u64 = 96;
/// The LED commands (the CIA write and the cost), a hold, a stop (DMA off of voice 0 and the cost),
/// a volume (the write of voice 0 and the cost by voice); a voice's write is this much after the
/// voice before's.
const CMD_LED_AT: u64 = 114;
const CMD_LED: u64 = 130;
const CMD_HOLD: u64 = 147;
const CMD_STOP_AT: u64 = 129;
const CMD_STOP: u64 = 214;
const CMD_VOL_AT: u64 = 329;
const CMD_VOL: [u64; 4] = [395, 418, 441, 454];
const CMD_VOICE: u64 = 23;
/// The restart: the host call (song end) and the cost.
const CMD_RESTART_CALL: u64 = 208;
const CMD_RESTART: u64 = 284;

impl Replayer for Sjs {
    fn format(&self) -> &'static str {
        "SoundPlayer"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick_clocks(&self) -> u64 {
        (self.timer as u64 + 1) * 5
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.ticks += 1;
        let mut clk = Clk { base: hw.bus.clock, h: TO_LOOP };
        let silence = self.silence;
        // the loops of the four voices (the sample or the empty one)
        for ch in 0..4 {
            if ch > 0 {
                clk.h += LOOP_GAP;
            }
            let v = &mut self.v[ch];
            let l = (v.loop_len != 0) as usize;
            if !v.hold_loop {
                clk.at(hw, LOOP_LOC[l]);
                hw.loc(ch, if l == 1 { v.loop_addr } else { silence });
                clk.at(hw, LOOP_LEN[l]);
                hw.len(ch, if l == 1 { v.loop_len } else { 1 });
            }
            clk.h += LOOP_COST[l];
            // (no corpus file has a fade or a held loop: their times are not known)
            if v.env_speed != 0 {
                v.env_cnt = v.env_cnt.wrapping_sub(1);
                if v.env_cnt == 0 {
                    v.env_cnt = v.env_speed;
                    clk.at(hw, 0);
                    if v.env_up {
                        if v.vol == 63 {
                            v.env_speed = 0;
                        } else {
                            v.vol += 1;
                            hw.vol(ch, v.vol as u16);
                        }
                    } else if v.vol == 0 {
                        v.env_speed = 0;
                    } else {
                        v.vol -= 1;
                        hw.vol(ch, v.vol as u16);
                    }
                }
            }
        }
        // the samples the last row tick's notes start
        clk.h += TO_START;
        for ch in 0..4 {
            if ch > 0 {
                clk.h += START_GAP;
            }
            let v = &mut self.v[ch];
            if v.pend_len == 0 {
                clk.h += START_NONE;
                continue;
            }
            clk.at(hw, START_LOC);
            hw.loc(ch, v.pend_start);
            clk.at(hw, START_LEN);
            hw.len(ch, v.pend_len);
            clk.at(hw, START_PER);
            hw.per(ch, v.per);
            clk.at(hw, START_VOL[ch]);
            hw.vol(ch, v.vol as u16);
            clk.at(hw, START_DMA[ch]);
            hw.dmacon(0x8000 | 1 << ch);
            v.pend_len = 0;
            clk.h += START_COST[ch];
        }
        if self.ticks % 6 != 0 {
            return;
        }
        // the row: every voice walks its column, the voices the mask leaves out too (their
        // holds and commands run; the corpus has no note in them, none is played here)
        clk.h += TO_HOLD;
        let mut fetch = [false; 4];
        for ch in 0..4 {
            if ch > 0 {
                clk.h += HOLD_GAP;
            }
            let v = &mut self.v[ch];
            fetch[ch] = true;
            clk.h += if v.hold == 0 {
                HOLD_FREE
            } else {
                v.hold -= 1;
                fetch[ch] = v.hold == 0;
                if fetch[ch] {
                    HOLD_ENDS
                } else {
                    HOLD_STAYS
                }
            };
        }
        clk.h += TO_FETCH;
        for ch in 0..4 {
            if ch > 0 {
                clk.h += FETCH_GAP;
            }
            self.v[ch].fetched = None;
            if fetch[ch] {
                self.v[ch].row += 1;
                let row = self.v[ch].row;
                self.v[ch].fetched = Some(self.group(row, ch));
                clk.h += FETCH;
            } else {
                clk.h += FETCH_NONE;
            }
        }
        clk.h += TO_NOTE;
        for ch in 0..4 {
            if ch > 0 {
                clk.h += NOTE_GAP;
            }
            let (note, ins) = match self.v[ch].fetched {
                Some([note, ins, _]) if note != 0 && self.mask & (1 << ch) != 0 => (note, ins),
                _ => {
                    clk.h += NOTE_NONE;
                    continue;
                }
            };
            clk.at(hw, NOTE_DMA);
            hw.dmacon(1 << ch);
            let s = if (1..=38).contains(&ins) { self.slots[ins as usize] } else { Slot::default() };
            let v = &mut self.v[ch];
            // notes above the table have no period (unexercised)
            v.per = PERIODS.get(note as usize - 1).copied().unwrap_or(0);
            v.pend_start = s.start;
            v.pend_len = s.len;
            v.loop_addr = s.loop_start;
            v.loop_len = s.loop_len;
            clk.h += NOTE_COST;
        }
        clk.h += TO_CMD;
        for ch in 0..4 {
            if ch > 0 {
                clk.h += CMD_GAP;
            }
            let cost = match self.v[ch].fetched {
                Some([_, _, c]) if c != 0 => self.command(ch, c, hw, &mut clk),
                _ => CMD_NONE,
            };
            clk.h += cost;
        }
    }
}

/// The tick's time in 68000 cycles (half colour clocks) from its start.
struct Clk {
    base: u64,
    h: u64,
}

impl Clk {
    /// The next write falls `off` cycles into the current part of the tick.
    fn at(&mut self, hw: &mut Hw, off: u64) {
        hw.bus.clock = self.base + (self.h + off) / 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    /// Timer word 6539 (0x198B), 3 voices; row 1 sets volume 0x1E-3, row 2 plays
    /// note 20 with instrument 1, row 3 stops the voice, row 4 restarts the song.
    fn song() -> Vec<u8> {
        let mut d = vec![0x8B, 0x19, 7];
        let mut row = |g: [u8; 3]| {
            for _ in 0..3 {
                d.extend_from_slice(&g);
            }
            d.extend_from_slice(&[0, 0, 0]);
        };
        row([0, 0, 0x1E]);
        row([20, 1, 0]);
        row([0, 0, 0x43]);
        row([0, 0, 0xDE]);
        d
    }

    /// One FORM at 0: VHDR at 12 with one-shot 16 bytes and repeat 8, BODY at 40.
    fn smp() -> Vec<u8> {
        let mut s = b"FORM".to_vec();
        s.extend_from_slice(&60u32.to_be_bytes());
        s.extend_from_slice(b"8SVXVHDR");
        s.extend_from_slice(&20u32.to_be_bytes());
        s.extend_from_slice(&16u32.to_be_bytes());
        s.extend_from_slice(&8u32.to_be_bytes());
        s.extend_from_slice(&[0; 12]);
        s.extend_from_slice(b"BODY");
        s.extend_from_slice(&24u32.to_be_bytes());
        s.extend_from_slice(&[0x40; 24]);
        s
    }

    #[test]
    fn recognises_by_content() {
        let d = song();
        assert!(detect(&d));
        let mut e = d.clone();
        e[1] = 0x0A;
        assert!(!detect(&e));
        let mut e = d.clone();
        e[3] = 1;
        assert!(!detect(&e));
        let mut e = d.clone();
        e[2] = 15;
        assert!(!detect(&e));
        assert!(!detect(&d[..14]));
        assert_eq!(companion("SJS.x"), ["smp.x", "smp.set"]);
    }

    #[test]
    fn slots_follow_the_scan() {
        let mut s = vec![0u8; 8];
        s.extend_from_slice(&smp());
        let sl = slots(&s, 1000);
        // the FORM is at 8: slot 3
        assert_eq!(sl[3].len, 8);
        assert_eq!(sl[3].loop_len, 4);
        assert_eq!(sl[3].start, 1000 + 8 + 40 + 8);
        assert_eq!(sl[3].loop_start, sl[3].start + 16);
        assert_eq!(sl[1].len, 0);
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn plays_a_note_one_tick_after_the_row() {
        let dir = Path::new("/sjs-test");
        let path = dir.join("sjs.test");
        let mut e = Engine::new(48000.0);
        e.m.os.mem_files.insert(dir.join("smp.test"), smp());
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(&path, song()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "SoundPlayer");
        let mut out = vec![0f32; 2 * 800 * 40];
        e.render(&mut out);
        let log: Vec<(u32, u16)> = e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect();
        // the set-up's two zeros, volume 0x1B at the first row tick, then the note's start with it
        assert_eq!(reg(&log, 0xDFF0A8)[..4], [0, 0, 0x1B, 0x1B]);
        assert_eq!(reg(&log, 0xDFF0A6)[0], 0x153);
        assert!(reg(&log, 0xDFF0A4).contains(&8));
        assert!(reg(&log, 0xDFF096).contains(&0x8001));
    }
}
