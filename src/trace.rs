//! DELI_TRACE=FILE: a debug trace of the 68k code running on the emulated machine. It records
//! what the code does that takes or waits for time, not each instruction: every guest call
//! (entry and return clocks), every host function the code calls (A-line stub, with the clock
//! before and after), every read of the beam counters and the CIAs and every CIA write (with the
//! PC and the clock), and, at the end of each outermost call, a histogram of the instructions it
//! executed (PC, executions, CPU cycles). Off unless the variable is set.

use std::collections::BTreeMap;
use std::io::Write;

pub struct Trace {
    out: std::io::BufWriter<std::fs::File>,
    /// The PC of the instruction about to run.
    pub pc: u32,
    hist: BTreeMap<u32, (u64, u64)>,
    /// DELI_TRACE_FLOW: also every jump (a PC that does not follow the previous one).
    flow: bool,
    prev: u32,
}

impl Trace {
    /// The trace file named by `DELI_TRACE`, for the first machine only.
    pub fn from_env() -> Option<Box<Trace>> {
        static ONCE: std::sync::Once = std::sync::Once::new();
        let path = std::env::var_os("DELI_TRACE")?;
        let mut t = None;
        ONCE.call_once(|| {
            t = std::fs::File::create(path).ok().map(|f| {
                Box::new(Trace { out: std::io::BufWriter::new(f), pc: 0, hist: BTreeMap::new(), flow: std::env::var_os("DELI_TRACE_FLOW").is_some(), prev: 0 })
            })
        });
        t
    }

    pub fn line(&mut self, s: std::fmt::Arguments) {
        let _ = writeln!(self.out, "{s}");
    }

    /// An instruction of `cycles` CPU cycles at `pc` is about to run.
    pub fn ins(&mut self, pc: u32, cycles: u64, clock: u64) {
        if self.flow && !(self.prev + 1..self.prev + 11).contains(&pc) {
            let _ = writeln!(self.out, "B {:06x} {pc:06x} clk={clock}", self.prev);
        }
        self.prev = pc;
        self.pc = pc;
        let h = self.hist.entry(pc).or_default();
        h.0 += 1;
        h.1 += cycles;
    }

    /// The end of an outermost guest call: the histogram of what it ran.
    pub fn dump(&mut self) {
        for (pc, (n, c)) in std::mem::take(&mut self.hist) {
            let _ = writeln!(self.out, "H {pc:06x} {n} {c}");
        }
        let _ = self.out.flush();
    }
}

impl Drop for Trace {
    fn drop(&mut self) {
        self.dump();
    }
}
