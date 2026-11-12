//! G&T Game Systems (`DUX.*`, `*.dux`; Thomas Sutor and Gerald B…hner, 1990: the modules of
//! modland's "GT Game Systems").
//!
//! Clean-room, written from the behaviour specification `docs/specs/gtgamesystems/SPEC.md` and
//! black-box runs; no original code, and no byte of any player. The specification was drawn up from
//! observation only: the reference is the Wanted Team Eagleplayer "G&T Game Systems player module
//! V1.0 (8 Aug 2008)" on this project's 68000 engine; observed were the Paula register writes with
//! their colour clocks, the audio.device requests the player sent (fields and clocks), the times of
//! its calls, which module bytes it read and when, and the output of built modules (differential
//! probing). Neither the player nor any source of it was disassembled or read.
//!
//! A module is plain data: a size, eight sample entries (offset, length, volume, loop start, loop
//! length), three voice offsets, the samples, and per voice a list of 6-byte records (period, sample,
//! duration in ticks; period FFFF a rest, duration FFFF the end of the voice). Three voices on Paula
//! channels 0 to 2, a tick every 70 940 colour clocks (DeliTracker's default timer). Every voice loops
//! on its own and reports the song end each time it ends.
//!
//! The original plays through audio.device (one request a voice: `CMD_FLUSH` and `CMD_WRITE` for a
//! note, `CMD_WRITE` again for a loop, `AbortIO` for a rest), so what reaches Paula is what this
//! engine's audio.device emulation (`os.rs`) makes of those requests, and the port models that device
//! (SPEC section 7) with the moments at which the engine's device looks at the channels: the end of
//! every output sample, every frame, and the end of a tick's code. That device has a defect the port
//! keeps, since it is the reference's sound (SPEC section 7): an interrupt request left set by a
//! stopped channel makes the device end the next note's only pass right after it starts.
//!
//! The times are the original's: its code's cost along the path a tick takes, in 68000 cycles as the
//! engine counts them, with the half colour clock every library call drops (SPEC section 6), and the
//! song's start after the time its init takes (SPEC section 4).
//!
//! Choices where the specification is silent (none can panic or hang): a sample number of 8 or more
//! (the original reads its own memory there) plays nothing: a request for address 0, length 0 and
//! volume 0, without a loop; records read past the end of the file are zeros (a note of period 0 and
//! sample 0 lasting 65 536 ticks).

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paulaout::{Filter, OutputStage, COLOUR_CLOCKS_PAL};

pub const OPEN: Open = Open { name: "G&T Game Systems", detect, new };

/// Colour clocks between two ticks (DeliTracker's default timer).
const TICK: u64 = super::DT_TICK;
/// The sample data's offset in the file; sample and voice offsets count from here.
const DATA: usize = 0x70;

// The original's code, in 68000 cycles (SPEC section 6). A library call (BeginIO, AbortIO, the song
// end's report) falls at the cycles since the previous one, halved and rounded down.
const C_ENTRY: u64 = 786;
const C_IDLE: u64 = 458;
const C_NOTE: [u64; 3] = [2458, 2498, 2548];
const C_REST_ABORT: u64 = 1070;
const C_REST_AFTER: u64 = 918;
const C_END_REPORT: u64 = 150;
const C_END_NOTE: u64 = 48;
const C_END_REST: u64 = 46;
const C_ISSUE_FLUSH: u64 = 546;
const C_ISSUE_WRITE: u64 = 484;
const C_ISSUE_AFTER: u64 = 348;
const C_ISSUE_IDLE: u64 = 402;
const C_LOOP_WRITE: u64 = 852;
const C_LOOP_AFTER: u64 = 285;
const C_LOOP_NOTE: u64 = 473;
const C_LOOP_IDLE: u64 = 424;
const C_EXIT: u64 = 74;

// The song's start (SPEC section 4): the check, the init (cycles), the calls after it (colour clocks).
const START_CHECK: u64 = 413;
const INIT_BASE: u64 = 12883;
const INIT_RECORD: u64 = 43;
const INIT_SAMPLE: u64 = 8;
const START_CALLS: u64 = 1622;

/// The module's layout (SPEC sections 2 and 3): the size, the file offsets of the three voices.
struct Lay {
    voices: [usize; 3],
}

fn ends_voice(d: &[u8], o: usize) -> bool {
    o + 6 <= d.len() && d[o..o + 6].iter().all(|&b| b == 0xFF)
}

/// The original's check (SPEC section 3), with the voices in order.
fn layout(d: &[u8]) -> Option<Lay> {
    let size = be32(d, 0) as usize;
    if d.len() < DATA + 18 || size > d.len() || size < DATA + 18 || !ends_voice(d, size - 6) {
        return None;
    }
    if (0..8).any(|i| be32(d, 4 + 12 * i) as usize >= size) {
        return None;
    }
    let v: [usize; 3] = std::array::from_fn(|k| (be32(d, 0x64 + 4 * k) as usize).saturating_add(DATA));
    if v[0] + 6 > v[1] || v[1] + 6 > v[2] || v[2] + 6 > size {
        return None;
    }
    if !ends_voice(d, v[1] - 6) || !ends_voice(d, v[2] - 6) {
        return None;
    }
    Some(Lay { voices: v })
}

fn detect(d: &[u8]) -> bool {
    layout(d).is_some()
}

/// One audio.device request (an IOAudio): what the device reads when it uses it.
#[derive(Clone, Copy, Default)]
struct Req {
    data: u32,
    /// Bytes.
    len: u32,
    per: u16,
    vol: u16,
    /// Passes; 0: for ever.
    cycles: u16,
}

/// A channel of the engine's audio.device, for a player with one request a channel: whether the
/// request is playing, latched in the registers, and how many copies of it are queued.
#[derive(Clone, Copy, Default)]
struct Chan {
    playing: bool,
    latched: bool,
    queued: u32,
    passes: u32,
}

impl Chan {
    fn busy(&self) -> bool {
        self.playing || self.latched
    }
}

#[derive(Clone, Copy, Default)]
struct Voice {
    start: usize,
    pos: usize,
    count: u16,
    /// This tick took a note, and the note's loop (data, length), if its entry has one.
    note: bool,
    lp: Option<(u32, u32)>,
    req: Req,
    ch: Chan,
}

pub struct GtGameSystems {
    d: Vec<u8>,
    /// The sample data's guest address.
    data: u32,
    v: [Voice; 3],
    songend: bool,
    /// The next tick's due time (absolute), once the first tick has run.
    due: Option<u64>,
    intena_set: bool,
    /// The clock of the last library call, and the cycles run since.
    at: u64,
    cy: u64,
    /// The clock of the device's last look at the channels.
    served: Option<u64>,
    /// The output samples' grid as last seen: the end of a sample, and the output stage there.
    grid: Option<(u64, OutputStage)>,
}

/// The cycles of the init (SPEC section 4): voice 0's records to its end, the non-empty entries.
fn init_cycles(d: &[u8], lay: &Lay) -> u64 {
    let nz = (0..8).filter(|&i| be16(d, 8 + 12 * i) != 0).count() as u64;
    let mut n = 0u64;
    let mut o = lay.voices[0];
    loop {
        n += 1;
        if be16(d, o + 4) == 0xFFFF || o + 6 > d.len() {
            break;
        }
        o += 6;
    }
    INIT_BASE + INIT_RECORD * n + INIT_SAMPLE * nz
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let lay = layout(d)?;
    // the check, the init and the calls after it take this long before the song starts
    m.bus.clock += START_CHECK + init_cycles(d, &lay).div_ceil(2) + START_CALLS;
    Some(Box::new(GtGameSystems::build(d, base, &lay)))
}

impl GtGameSystems {
    fn build(d: &[u8], base: u32, lay: &Lay) -> Self {
        let voice = |k: usize| Voice { start: lay.voices[k], pos: lay.voices[k], count: 1, ..Voice::default() };
        GtGameSystems {
            d: d.to_vec(),
            data: base.wrapping_add(DATA as u32),
            v: [voice(0), voice(1), voice(2)],
            songend: false,
            due: None,
            intena_set: false,
            at: 0,
            cy: 0,
            served: None,
            grid: None,
        }
    }

    /// A library call now: the clock moves on by the cycles run since the last one, the half colour
    /// clock dropped.
    fn call(&mut self, hw: &mut Hw) {
        self.at += self.cy / 2;
        self.cy = 0;
        hw.bus.clock = self.at;
    }

    /// The request of a note of sample `s` at period `per` (SPEC section 5), and its loop.
    fn note_req(&self, s: u16, per: u16) -> (Req, Option<(u32, u32)>) {
        if s >= 8 {
            return (Req { data: 0, len: 0, per, vol: 0, cycles: 1 }, None);
        }
        let e = 4 + 12 * s as usize;
        let start = self.data.wrapping_add(be32(&self.d, e));
        let req = Req {
            data: start,
            len: be16(&self.d, e + 4) as i16 as i32 as u32,
            per,
            vol: be16(&self.d, e + 6) & 0x3FF,
            cycles: 1,
        };
        let ls = be16(&self.d, e + 8) as i16;
        let lp = (ls != 0)
            .then(|| (start.wrapping_add(ls as i32 as u32), be16(&self.d, e + 10) as i16 as i32 as u32));
        (req, lp)
    }

    /// The prepare pass of one voice (SPEC section 5).
    fn prepare(&mut self, hw: &mut Hw, k: usize) {
        self.v[k].note = false;
        self.v[k].count = self.v[k].count.wrapping_sub(1);
        if self.v[k].count != 0 {
            self.cy += C_IDLE;
            return;
        }
        let mut pos = self.v[k].pos;
        let mut restarted = false;
        if be16(&self.d, pos + 4) == 0xFFFF {
            self.cy += C_END_REPORT;
            self.call(hw);
            self.songend = true;
            pos = self.v[k].start;
            restarted = true;
        }
        let (per, s, dur) = (be16(&self.d, pos), be16(&self.d, pos + 2), be16(&self.d, pos + 4));
        self.v[k].pos = pos + 6;
        self.v[k].count = dur;
        if per == 0xFFFF {
            self.cy += C_REST_ABORT + if restarted { C_END_REST } else { 0 };
            self.call(hw);
            self.dev_abort(hw, k);
            self.cy += C_REST_AFTER;
            return;
        }
        let (req, lp) = self.note_req(s, per);
        let v = &mut self.v[k];
        v.req = req;
        v.lp = lp;
        v.note = true;
        self.cy += C_NOTE[k] + if restarted { C_END_NOTE } else { 0 };
    }

    // ---- the engine's audio.device (SPEC section 7) ----

    /// Puts the request in the channel's registers, for Paula's next reload.
    fn latch(&mut self, hw: &mut Hw, k: usize) {
        let r = self.v[k].req;
        hw.loc(k, r.data);
        hw.len(k, (r.len / 2) as u16);
        self.v[k].ch.latched = true;
    }

    fn dev_flush(&mut self, hw: &mut Hw, k: usize) {
        hw.dmacon(1 << k);
        self.v[k].ch = Chan::default();
    }

    fn dev_write(&mut self, hw: &mut Hw, k: usize) {
        let c = self.v[k].ch;
        if !c.busy() {
            self.latch(hw, k);
            hw.dmacon(1 << k);
            hw.dmacon(0x8000 | 1 << k);
        } else if !c.latched {
            self.latch(hw, k);
        } else {
            self.v[k].ch.queued = c.queued.saturating_add(1);
        }
    }

    fn dev_abort(&mut self, hw: &mut Hw, k: usize) {
        let c = &mut self.v[k].ch;
        if !c.busy() && c.queued == 0 {
            return;
        }
        c.queued = 0;
        c.latched = false;
        if c.playing {
            c.playing = false;
            hw.dmacon(1 << k);
        }
    }

    /// The device's look at the channels: a channel whose interrupt request is set while it plays
    /// or holds a request has started a pass.
    fn service_all(&mut self, hw: &mut Hw) {
        for k in 0..3 {
            let bit = 0x80u16 << k;
            if hw.bus.paula.intreq & bit == 0 || !self.v[k].ch.busy() {
                continue;
            }
            hw.bus.paula.write_intreq(bit);
            if self.v[k].ch.latched {
                if !self.v[k].ch.playing {
                    let r = self.v[k].req;
                    hw.per(k, r.per);
                    hw.vol(k, r.vol);
                    self.v[k].ch.playing = true;
                    self.v[k].ch.passes = 0;
                }
                let cycles = self.v[k].req.cycles as u32;
                let c = &mut self.v[k].ch;
                c.passes = c.passes.saturating_add(1);
                if cycles != 0 && c.passes >= cycles {
                    c.latched = false;
                    if c.queued > 0 {
                        c.queued -= 1;
                        self.latch(hw, k);
                    }
                }
            } else {
                hw.dmacon(1 << k);
                hw.bus.paula.write_intreq(bit);
                self.v[k].ch.playing = false;
            }
        }
        self.served = Some(hw.bus.clock);
    }

    /// Whether an output sample ends now. The output stage only tells the time to the end of the
    /// running sample, which right at an end is the length of the next one and can be one colour
    /// clock less than the longest: a grid seen at an earlier end, run forward, decides then.
    fn at_sample_end(&mut self, hw: &Hw) -> bool {
        let now = hw.bus.clock;
        let left = hw.bus.paula.out.until_sample() as u64;
        if let Some((end, g)) = &mut self.grid {
            while *end < now {
                let n = g.until_sample();
                g.step([0; 4], n);
                *end += n as u64;
            }
            if *end == now && g.until_sample() as u64 == left {
                return true;
            }
            if *end == now + left {
                return false;
            }
            // the grid was restarted since
            self.grid = None;
        }
        self.see_grid(hw);
        left as f32 > COLOUR_CLOCKS_PAL as f32 / hw.bus.paula.out.rate() as f32 - 0.5
    }

    /// Takes the output samples' grid from the output stage: the end of the running sample.
    fn see_grid(&mut self, hw: &Hw) {
        let mut g = hw.bus.paula.out.clone();
        g.set_filter(Filter::None);
        let n = g.until_sample();
        g.step([0; 4], n);
        self.grid = Some((hw.bus.clock + n as u64, g));
    }
}

impl Replayer for GtGameSystems {
    fn format(&self) -> &'static str {
        "G&T Game Systems"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        if !self.intena_set {
            // the audio interrupts on, so that the engine tells of them (`audio_irq_ch`)
            hw.bus.paula.write_intena(0x8000 | 0x7 << 7);
            self.intena_set = true;
        }
        let now = hw.bus.clock;
        self.due = Some(self.due.map_or(now, |d| d) + TICK);
        if self.grid.is_none() {
            self.see_grid(hw);
        }
        self.at = now;
        self.cy = C_ENTRY;
        for k in 0..3 {
            self.prepare(hw, k);
        }
        for k in 0..3 {
            if self.v[k].note {
                self.cy += C_ISSUE_FLUSH;
                self.call(hw);
                self.dev_flush(hw, k);
                self.cy += C_ISSUE_WRITE;
                self.call(hw);
                self.dev_write(hw, k);
                self.cy += C_ISSUE_AFTER;
            } else {
                self.cy += C_ISSUE_IDLE;
            }
        }
        for k in 0..3 {
            match (self.v[k].note, self.v[k].lp) {
                (true, Some((data, len))) => {
                    self.cy += C_LOOP_WRITE;
                    self.call(hw);
                    self.v[k].req = Req { data, len, cycles: 0, ..self.v[k].req };
                    self.dev_write(hw, k);
                    self.cy += C_LOOP_AFTER;
                }
                (true, None) => self.cy += C_LOOP_NOTE,
                (false, _) => self.cy += C_LOOP_IDLE,
            }
        }
        self.cy += C_EXIT;
        self.call(hw);
        // the device looks at the channels after the tick's code, with Paula as the last write left it
        self.service_all(hw);
        // a frame that began while the code ran is taken at once after it, and the device looks again,
        // Paula now brought up to the clock
        let (end, frame) = (hw.bus.clock, hw.bus.frame.max(1));
        if end / frame > now / frame {
            hw.bus.paula.sync(end, &hw.bus.ram);
            self.service_all(hw);
        }
    }

    /// The engine's device does not run on Paula's interrupts: it looks at the channels whenever the
    /// machine has run to the next thing that happens, which is the end of an output sample, a frame,
    /// or a tick (then after its code). A request raised in a sample is taken at that sample's end.
    fn audio_irq_ch(&mut self, ch: usize, hw: &mut Hw) -> bool {
        if ch > 2 {
            return false;
        }
        let bit = 0x80u16 << ch;
        if hw.bus.paula.intreq & bit == 0 || !self.v[ch].ch.busy() {
            return true;
        }
        let now = hw.bus.clock;
        // (after a look at this clock, the next is at the end of the next sample: the engine runs
        // the machine to `now + until_sample()`, which is `now` itself when that rounds to 0)
        let again = self.served == Some(now);
        let left = hw.bus.paula.out.until_sample() as u64;
        let end = if left == 0 || (!again && self.at_sample_end(hw)) { now } else { now + left };
        let frame = hw.bus.frame.max(1);
        let vblank = if again { (now / frame + 1) * frame } else { now.div_ceil(frame) * frame };
        let at = end.min(vblank);
        if self.due.is_some_and(|d| d <= at) {
            // the tick comes first: the device looks after its code
            return true;
        }
        if at > now {
            hw.bus.clock = at;
            hw.bus.paula.sync(at, &hw.bus.ram);
        }
        self.service_all(hw);
        true
    }
}

#[cfg(test)]
#[path = "gtgamesystems_tests.rs"]
mod tests;
