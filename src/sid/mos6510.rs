// Port of NostalgicPlayer's C# port of libsidplayfp (by Simon White, Antti Lankila and
// Leandro Nini): Source/Ports/LibSidPlayFp/C64/Cpu/Mos6510.cs, Flags.cs, Opcodes.cs,
// ICpuDataBus.cs, git e183943.
// Licence: GNU General Public License version 2 (LICENSE-LibSidPlayFp); the upstream
// files carry only NostalgicPlayer's header:
//   "This source, or parts thereof, may be used in any software as long the
//    license of NostalgicPlayer is keep. See the LICENSE file for more information."
//
// Cycle-exact 6502/6510 emulation core.
//
// Code is based on work by Simon A. White [sidplay2@yahoo.com].
// Original Java port by Ken Händel. Later on, it has been hacked to
// improve compatibility with Lorenz suite on VICE's test suite.
//
// How the structure differs from upstream: upstream's `Mos6510` object holds references to
// the event scheduler and to the data bus (`ICpuDataBus`), and its instruction table holds
// closures. Here the whole machine is one `C64` struct (see c64.rs): `Mos6510` is plain
// data (registers, flags, interrupt bookkeeping) and everything that touches the bus or the
// scheduler is a method of `impl C64` (`cpu_event`, `cpu_reset`, `cpu_trigger_irq`, ...). The
// bus (`ICpuDataBus.CpuRead/CpuWrite`) is `C64::cpu_read/cpu_write`. The three upstream
// events ("CPU-nosteal", "CPU-steal", "Remove IRQ") are `Ev::CPU_NOSTEAL`, `Ev::CPU_STEAL`,
// `Ev::CPU_CLEAR_INT`. The instruction table (0x101 opcodes x 8 cycles) is built once, by a
// translation of `BuildInstructionTable`, into a process-wide `OnceLock`; its entries are
// `{ func: fn(&mut C64), no_steal }` and every cycle function of upstream is a separate
// free function below. Register fields are named shortly (`pc` = register_ProgramCounter,
// `sp` = register_StackPointer, `a`/`x`/`y` = register_Accumulator/X/Y, `cycle_ea` =
// cycle_EffectiveAddress, `cycle_pointer`, `cycle_data`) and are public.
//
// Differences forced by Rust:
//  * `Mos6510::new()` cannot schedule (upstream's constructor calls `Initialize`, which
//    schedules the first CPU event); `cpu_reset` does it, so call it (after the scheduler's
//    reset) before clocking the machine.
//  * Upstream throws `HaltInstructionException` on a JAM opcode, which aborts the clock
//    call; the CPU's event is then never rescheduled. Here `Mos6510::halted` is set, the
//    CPU event that ran the opcode does not reschedule itself, and later CPU events (those
//    that set_rdy / trigger_nmi may still schedule) return at once. `cpu_reset` clears it.
//  * The table is indexed with a mask (0x7ff), so no program can index past it; upstream's
//    `cycleCount` cannot leave its opcode's row either (every row ends with a function that
//    sets it anew), the mask is only a guarantee.
//  * Upstream has no `TriggerRst`; `rst_flag` exists (as upstream) but is never set.
//  * Debug/trace code is not ported.
#![allow(non_upper_case_globals, dead_code)]

use std::sync::OnceLock;

use super::c64::C64;
use super::sched::{Ev, Phase};

/// Holds different opcode constants (Opcodes.cs)
pub mod op {
    pub const OpcodeMax: i32 = 0x100;
    pub const BRKn: u8 = 0x00;
    pub const JSRw: u8 = 0x20;
    pub const RTIn: u8 = 0x40;
    pub const RTSn: u8 = 0x60;
    pub const NOPb: u8 = 0x80;
    pub const NOPb1: u8 = 0x82;
    pub const NOPb2: u8 = 0xc2;
    pub const NOPb3: u8 = 0xe2;
    pub const NOPb4: u8 = 0x89;
    pub const LDYb: u8 = 0xa0;
    pub const CPYb: u8 = 0xc0;
    pub const CPXb: u8 = 0xe0;
    pub const ORAix: u8 = 0x01;
    pub const ANDix: u8 = 0x21;
    pub const EORix: u8 = 0x41;
    pub const ADCix: u8 = 0x61;
    pub const STAix: u8 = 0x81;
    pub const LDAix: u8 = 0xa1;
    pub const CMPix: u8 = 0xc1;
    pub const SBCix: u8 = 0xe1;
    pub const LDXb: u8 = 0xa2;
    pub const SLOix: u8 = 0x03;
    pub const RLAix: u8 = 0x23;
    pub const SREix: u8 = 0x43;
    pub const RRAix: u8 = 0x63;
    pub const SAXix: u8 = 0x83;
    pub const LAXix: u8 = 0xa3;
    pub const DCPix: u8 = 0xc3;
    pub const ISBix: u8 = 0xe3;
    pub const NOPz: u8 = 0x04;
    pub const NOPz1: u8 = 0x44;
    pub const NOPz2: u8 = 0x64;
    pub const BITz: u8 = 0x24;
    pub const STYz: u8 = 0x84;
    pub const LDYz: u8 = 0xa4;
    pub const CPYz: u8 = 0xc4;
    pub const CPXz: u8 = 0xe4;
    pub const ORAz: u8 = 0x05;
    pub const ANDz: u8 = 0x25;
    pub const EORz: u8 = 0x45;
    pub const ADCz: u8 = 0x65;
    pub const STAz: u8 = 0x85;
    pub const LDAz: u8 = 0xa5;
    pub const CMPz: u8 = 0xc5;
    pub const SBCz: u8 = 0xe5;
    pub const ASLz: u8 = 0x06;
    pub const ROLz: u8 = 0x26;
    pub const LSRz: u8 = 0x46;
    pub const RORz: u8 = 0x66;
    pub const STXz: u8 = 0x86;
    pub const LDXz: u8 = 0xa6;
    pub const DECz: u8 = 0xc6;
    pub const INCz: u8 = 0xe6;
    pub const SLOz: u8 = 0x07;
    pub const RLAz: u8 = 0x27;
    pub const SREz: u8 = 0x47;
    pub const RRAz: u8 = 0x67;
    pub const SAXz: u8 = 0x87;
    pub const LAXz: u8 = 0xa7;
    pub const DCPz: u8 = 0xc7;
    pub const ISBz: u8 = 0xe7;
    pub const PHPn: u8 = 0x08;
    pub const PLPn: u8 = 0x28;
    pub const PHAn: u8 = 0x48;
    pub const PLAn: u8 = 0x68;
    pub const DEYn: u8 = 0x88;
    pub const TAYn: u8 = 0xa8;
    pub const INYn: u8 = 0xc8;
    pub const INXn: u8 = 0xe8;
    pub const ORAb: u8 = 0x09;
    pub const ANDb: u8 = 0x29;
    pub const EORb: u8 = 0x49;
    pub const ADCb: u8 = 0x69;
    pub const LDAb: u8 = 0xa9;
    pub const CMPb: u8 = 0xc9;
    pub const SBCb: u8 = 0xe9;
    pub const SBCb1: u8 = 0xeb;
    pub const ASLn: u8 = 0x0a;
    pub const ROLn: u8 = 0x2a;
    pub const LSRn: u8 = 0x4a;
    pub const RORn: u8 = 0x6a;
    pub const TXAn: u8 = 0x8a;
    pub const TAXn: u8 = 0xaa;
    pub const DEXn: u8 = 0xca;
    pub const NOPn: u8 = 0xea;
    pub const NOPn1: u8 = 0x1a;
    pub const NOPn2: u8 = 0x3a;
    pub const NOPn3: u8 = 0x5a;
    pub const NOPn4: u8 = 0x7a;
    pub const NOPn5: u8 = 0xda;
    pub const NOPn6: u8 = 0xfa;
    pub const ANCb: u8 = 0x0b;
    pub const ANCb1: u8 = 0x2b;
    pub const ASRb: u8 = 0x4b;
    pub const ARRb: u8 = 0x6b;
    pub const ANEb: u8 = 0x8b;
    pub const XAAb: u8 = 0x8b;
    pub const LXAb: u8 = 0xab;
    pub const SBXb: u8 = 0xcb;
    pub const NOPa: u8 = 0x0c;
    pub const BITa: u8 = 0x2c;
    pub const JMPw: u8 = 0x4c;
    pub const JMPi: u8 = 0x6c;
    pub const STYa: u8 = 0x8c;
    pub const LDYa: u8 = 0xac;
    pub const CPYa: u8 = 0xcc;
    pub const CPXa: u8 = 0xec;
    pub const ORAa: u8 = 0x0d;
    pub const ANDa: u8 = 0x2d;
    pub const EORa: u8 = 0x4d;
    pub const ADCa: u8 = 0x6d;
    pub const STAa: u8 = 0x8d;
    pub const LDAa: u8 = 0xad;
    pub const CMPa: u8 = 0xcd;
    pub const SBCa: u8 = 0xed;
    pub const ASLa: u8 = 0x0e;
    pub const ROLa: u8 = 0x2e;
    pub const LSRa: u8 = 0x4e;
    pub const RORa: u8 = 0x6e;
    pub const STXa: u8 = 0x8e;
    pub const LDXa: u8 = 0xae;
    pub const DECa: u8 = 0xce;
    pub const INCa: u8 = 0xee;
    pub const SLOa: u8 = 0x0f;
    pub const RLAa: u8 = 0x2f;
    pub const SREa: u8 = 0x4f;
    pub const RRAa: u8 = 0x6f;
    pub const SAXa: u8 = 0x8f;
    pub const LAXa: u8 = 0xaf;
    pub const DCPa: u8 = 0xcf;
    pub const ISBa: u8 = 0xef;
    pub const BPLr: u8 = 0x10;
    pub const BMIr: u8 = 0x30;
    pub const BVCr: u8 = 0x50;
    pub const BVSr: u8 = 0x70;
    pub const BCCr: u8 = 0x90;
    pub const BCSr: u8 = 0xb0;
    pub const BNEr: u8 = 0xd0;
    pub const BEQr: u8 = 0xf0;
    pub const ORAiy: u8 = 0x11;
    pub const ANDiy: u8 = 0x31;
    pub const EORiy: u8 = 0x51;
    pub const ADCiy: u8 = 0x71;
    pub const STAiy: u8 = 0x91;
    pub const LDAiy: u8 = 0xb1;
    pub const CMPiy: u8 = 0xd1;
    pub const SBCiy: u8 = 0xf1;
    pub const SLOiy: u8 = 0x13;
    pub const RLAiy: u8 = 0x33;
    pub const SREiy: u8 = 0x53;
    pub const RRAiy: u8 = 0x73;
    pub const SHAiy: u8 = 0x93;
    pub const LAXiy: u8 = 0xb3;
    pub const DCPiy: u8 = 0xd3;
    pub const ISBiy: u8 = 0xf3;
    pub const NOPzx: u8 = 0x14;
    pub const NOPzx1: u8 = 0x34;
    pub const NOPzx2: u8 = 0x54;
    pub const NOPzx3: u8 = 0x74;
    pub const NOPzx4: u8 = 0xd4;
    pub const NOPzx5: u8 = 0xf4;
    pub const STYzx: u8 = 0x94;
    pub const LDYzx: u8 = 0xb4;
    pub const ORAzx: u8 = 0x15;
    pub const ANDzx: u8 = 0x35;
    pub const EORzx: u8 = 0x55;
    pub const ADCzx: u8 = 0x75;
    pub const STAzx: u8 = 0x95;
    pub const LDAzx: u8 = 0xb5;
    pub const CMPzx: u8 = 0xd5;
    pub const SBCzx: u8 = 0xf5;
    pub const ASLzx: u8 = 0x16;
    pub const ROLzx: u8 = 0x36;
    pub const LSRzx: u8 = 0x56;
    pub const RORzx: u8 = 0x76;
    pub const STXzy: u8 = 0x96;
    pub const LDXzy: u8 = 0xb6;
    pub const DECzx: u8 = 0xd6;
    pub const INCzx: u8 = 0xf6;
    pub const SLOzx: u8 = 0x17;
    pub const RLAzx: u8 = 0x37;
    pub const SREzx: u8 = 0x57;
    pub const RRAzx: u8 = 0x77;
    pub const SAXzy: u8 = 0x97;
    pub const LAXzy: u8 = 0xb7;
    pub const DCPzx: u8 = 0xd7;
    pub const ISBzx: u8 = 0xf7;
    pub const CLCn: u8 = 0x18;
    pub const SECn: u8 = 0x38;
    pub const CLIn: u8 = 0x58;
    pub const SEIn: u8 = 0x78;
    pub const TYAn: u8 = 0x98;
    pub const CLVn: u8 = 0xb8;
    pub const CLDn: u8 = 0xd8;
    pub const SEDn: u8 = 0xf8;
    pub const ORAay: u8 = 0x19;
    pub const ANDay: u8 = 0x39;
    pub const EORay: u8 = 0x59;
    pub const ADCay: u8 = 0x79;
    pub const STAay: u8 = 0x99;
    pub const LDAay: u8 = 0xb9;
    pub const CMPay: u8 = 0xd9;
    pub const SBCay: u8 = 0xf9;
    pub const TXSn: u8 = 0x9a;
    pub const TSXn: u8 = 0xba;
    pub const SLOay: u8 = 0x1b;
    pub const RLAay: u8 = 0x3b;
    pub const SREay: u8 = 0x5b;
    pub const RRAay: u8 = 0x7b;
    pub const SHSay: u8 = 0x9b;
    pub const TASay: u8 = 0x9b;
    pub const LASay: u8 = 0xbb;
    pub const DCPay: u8 = 0xdb;
    pub const ISBay: u8 = 0xfb;
    pub const NOPax: u8 = 0x1c;
    pub const NOPax1: u8 = 0x3c;
    pub const NOPax2: u8 = 0x5c;
    pub const NOPax3: u8 = 0x7c;
    pub const NOPax4: u8 = 0xdc;
    pub const NOPax5: u8 = 0xfc;
    pub const SHYax: u8 = 0x9c;
    pub const LDYax: u8 = 0xbc;
    pub const ORAax: u8 = 0x1d;
    pub const ANDax: u8 = 0x3d;
    pub const EORax: u8 = 0x5d;
    pub const ADCax: u8 = 0x7d;
    pub const STAax: u8 = 0x9d;
    pub const LDAax: u8 = 0xbd;
    pub const CMPax: u8 = 0xdd;
    pub const SBCax: u8 = 0xfd;
    pub const ASLax: u8 = 0x1e;
    pub const ROLax: u8 = 0x3e;
    pub const LSRax: u8 = 0x5e;
    pub const RORax: u8 = 0x7e;
    pub const SHXay: u8 = 0x9e;
    pub const LDXay: u8 = 0xbe;
    pub const DECax: u8 = 0xde;
    pub const INCax: u8 = 0xfe;
    pub const SLOax: u8 = 0x1f;
    pub const RLAax: u8 = 0x3f;
    pub const SREax: u8 = 0x5f;
    pub const RRAax: u8 = 0x7f;
    pub const SHAay: u8 = 0x9f;
    pub const LAXay: u8 = 0xbf;
    pub const DCPax: u8 = 0xdf;
    pub const ISBax: u8 = 0xff;
    pub const ASOix: u8 = SLOix;
    pub const LSEix: u8 = SREix;
    pub const AXSix: u8 = SAXix;
    pub const DCMix: u8 = DCPix;
    pub const INSix: u8 = ISBix;
    pub const ASOz: u8 = SLOz;
    pub const LSEz: u8 = SREz;
    pub const AXSz: u8 = SAXz;
    pub const DCMz: u8 = DCPz;
    pub const INSz: u8 = ISBz;
    pub const ALRb: u8 = ASRb;
    pub const OALb: u8 = LXAb;
    pub const ASOa: u8 = SLOa;
    pub const LSEa: u8 = SREa;
    pub const AXSa: u8 = SAXa;
    pub const DCMa: u8 = DCPa;
    pub const INSa: u8 = ISBa;
    pub const ASOiy: u8 = SLOiy;
    pub const LSEiy: u8 = SREiy;
    pub const AXSiy: u8 = SHAiy;
    pub const DCMiy: u8 = DCPiy;
    pub const INSiy: u8 = ISBiy;
    pub const ASOzx: u8 = SLOzx;
    pub const LSEzx: u8 = SREzx;
    pub const AXSzy: u8 = SAXzy;
    pub const DCMzx: u8 = DCPzx;
    pub const INSzx: u8 = ISBzx;
    pub const ASOay: u8 = SLOay;
    pub const LSEay: u8 = SREay;
    pub const DCMay: u8 = DCPay;
    pub const INSay: u8 = ISBay;
    pub const SAYax: u8 = SHYax;
    pub const XASay: u8 = SHXay;
    pub const ASOax: u8 = SLOax;
    pub const LSEax: u8 = SREax;
    pub const AXAay: u8 = SHAay;
    pub const DCMax: u8 = DCPax;
    pub const INSax: u8 = ISBax;
    pub const SKBn: u8 = NOPb;
    pub const SKWn: u8 = NOPa;
}

/// IRQ/NMI magic limit values.
/// Need to be larger than about 0x103 * 8,
/// but can't be min/max for interger type
const MAX: i32 = 65536;

/// Stack page location
const SP_PAGE: u8 = 0x01;

/// Status register interrupt bit
pub const SR_INTERRUPT: i32 = 2;

// Magic values for LXA and ANE undocumented instructions.
// Magic may be EE, EF, FE or FF, but most emulators seem to use EE.
// The constants here defined are based on VICE testsuite which
// refers to some real case usage of the opcodes
const LXA_MAGIC: u8 = 0xee;
const ANE_MAGIC: u8 = 0xef;

const INTERRUPT_DELAY: i32 = 2;

/// Mask for the instruction table index (the table has 0x101 << 3 entries)
const TABLE_MASK: usize = 0x7ff;

/// Processor Status Register
#[derive(Clone, Copy, Default, Debug)]
pub struct Flags {
    /// Carry
    pub c: bool,
    /// Zero
    pub z: bool,
    /// Interrupt disabled
    pub i: bool,
    /// Decimal
    pub d: bool,
    /// Overflow
    pub v: bool,
    /// Negative
    pub n: bool,
}

impl Flags {
    #[inline]
    pub fn reset(&mut self) {
        self.c = false;
        self.z = false;
        self.i = false;
        self.d = false;
        self.v = false;
        self.n = false;
    }

    /// Set N and Z flag values
    #[inline]
    pub fn set_nz(&mut self, value: u8) {
        self.z = value == 0;
        self.n = (value & 0x80) != 0;
    }

    /// Get status register value
    #[inline]
    pub fn get(&self) -> u8 {
        let mut sr = 0;
        if self.c {
            sr |= 0x01;
        }
        if self.z {
            sr |= 0x02;
        }
        if self.i {
            sr |= 0x04;
        }
        if self.d {
            sr |= 0x08;
        }
        if self.v {
            sr |= 0x40;
        }
        if self.n {
            sr |= 0x80;
        }
        sr
    }

    /// Set status register value
    #[inline]
    pub fn set(&mut self, sr: u8) {
        self.c = (sr & 0x01) != 0;
        self.z = (sr & 0x02) != 0;
        self.i = (sr & 0x04) != 0;
        self.d = (sr & 0x08) != 0;
        self.v = (sr & 0x40) != 0;
        self.n = (sr & 0x80) != 0;
    }
}

/// One cycle of an instruction
#[derive(Clone, Copy)]
struct ProcessorCycle {
    func: fn(&mut C64),
    /// A cycle that writes (cannot be stolen by the VIC-II)
    no_steal: bool,
}

/// The processor state (upstream's `Mos6510` fields)
pub struct Mos6510 {
    /// Set when a JAM opcode was executed (upstream: HaltInstructionException).
    /// Cleared by `cpu_reset`.
    pub halted: bool,

    /// Current instruction and sub-cycle within instruction
    pub cycle_count: i32,

    /// When IRQ was triggered. -MAX means "during some previous instruction", MAX means "no IRQ"
    pub interrupt_cycle: i32,

    /// IRQ asserted on CPU
    pub irq_asserted_on_pin: bool,

    /// NMI requested?
    pub nmi_flag: bool,

    /// RST requested?
    pub rst_flag: bool,

    /// RDY pin state (stop CPU on read)
    pub rdy: bool,

    /// Address low summer carry
    pub adl_carry: bool,

    pub d1x1: bool,

    /// The RDY pin state during last throw away read
    pub rdy_on_throw_away_read: bool,

    /// Status register
    pub flags: Flags,

    /// Data regarding current instruction
    pub pc: u16,
    pub cycle_ea: u16,
    pub cycle_pointer: u16,

    pub cycle_data: u8,
    pub sp: u8,
    pub a: u8,
    pub x: u8,
    pub y: u8,

    /// Table of CPU opcode implementations
    table: &'static [ProcessorCycle],
}

impl Default for Mos6510 {
    fn default() -> Self {
        Self::new()
    }
}

impl Mos6510 {
    /// Constructor. Does not schedule anything: `C64::cpu_reset` does.
    pub fn new() -> Self {
        let mut cpu = Mos6510 {
            halted: false,
            cycle_count: 0,
            interrupt_cycle: MAX,
            irq_asserted_on_pin: false,
            nmi_flag: false,
            rst_flag: false,
            rdy: true,
            adl_carry: false,
            d1x1: false,
            rdy_on_throw_away_read: false,
            flags: Flags::default(),
            pc: 0,
            cycle_ea: 0,
            cycle_pointer: 0,
            cycle_data: 0,
            sp: 0,
            a: 0,
            x: 0,
            y: 0,
            table: instruction_table(),
        };

        // Initialize processor registers
        cpu.a = 0;
        cpu.x = 0;
        cpu.y = 0;

        cpu.cycle_ea = 0;
        cpu.cycle_data = 0;

        cpu.initialize_state();
        cpu
    }

    /// Initialize CPU emulation (registers), the part of upstream's `Initialize` that does
    /// not schedule.
    fn initialize_state(&mut self) {
        // Reset stack
        self.sp = 0xff;

        // Reset cycle count
        self.cycle_count = ((op::BRKn as i32) << 3) + 6; // FetchNextOpcode

        // Reset status register
        self.flags.reset();

        // Set PC to some value
        self.pc = 0;

        // IRQ pending check
        self.irq_asserted_on_pin = false;
        self.nmi_flag = false;
        self.rst_flag = false;
        self.interrupt_cycle = MAX;

        // Signals
        self.rdy = true;
        self.d1x1 = false;

        // (not upstream) a reset starts a CPU that has not halted
        self.halted = false;
    }

    /// Status register value
    pub fn status(&self) -> u8 {
        self.flags.get()
    }

    #[inline]
    fn check_interrupts(&self) -> bool {
        self.rst_flag || self.nmi_flag || (self.irq_asserted_on_pin && !self.flags.i)
    }

    /// Evaluate when to execute an interrupt. Calling this method can
    /// also result in the decision that no interrupt at all needs to be
    /// scheduled
    #[inline]
    fn calculate_interrupt_trigger_cycle(&mut self) {
        // Interrupt cycle not going to trigger?
        if self.interrupt_cycle == MAX && self.check_interrupts() {
            self.interrupt_cycle = self.cycle_count;
        }
    }
}

/// Initialize CPU emulation (registers) and schedule the first cycle
fn initialize(c: &mut C64) {
    c.cpu.initialize_state();
    c.sched.schedule_phase(Ev::CPU_NOSTEAL, 0, Phase::Phi2);
}

impl C64 {
    /// Dispatcher for the CPU's events
    #[inline]
    pub fn cpu_event(&mut self, ev: Ev) {
        match ev {
            Ev::CPU_NOSTEAL => event_without_steals(self),
            Ev::CPU_STEAL => event_with_steals(self),
            Ev::CPU_CLEAR_INT => remove_irq(self),
            _ => {}
        }
    }

    /// Handle bus access signals. When RDY line is asserted, the CPU
    /// will pause when executing the next read operation
    pub fn cpu_set_rdy(&mut self, new_rdy: bool) {
        self.cpu.rdy = new_rdy;

        if self.cpu.rdy {
            self.sched.cancel(Ev::CPU_STEAL);
            self.sched.schedule_phase(Ev::CPU_NOSTEAL, 0, Phase::Phi2);
        } else {
            self.sched.cancel(Ev::CPU_NOSTEAL);
            self.sched.schedule_phase(Ev::CPU_STEAL, 0, Phase::Phi2);
        }
    }

    /// Reset CPU emulation
    pub fn cpu_reset(&mut self) {
        // Internal stuff
        initialize(self);

        // Set processor port to the default values
        self.cpu_write(0, 0x2f);
        self.cpu_write(1, 0x37);

        // Requires external bits
        // Read from reset vector for program entry point
        let lo = self.cpu_read(0xfffc);
        self.cpu.cycle_ea = (self.cpu.cycle_ea & 0xff00) | lo as u16;
        let hi = self.cpu_read(0xfffd);
        self.cpu.cycle_ea = (self.cpu.cycle_ea & 0x00ff) | ((hi as u16) << 8);
        self.cpu.pc = self.cpu.cycle_ea;
    }

    /// Trigger NMI interrupt on the CPU. Calling this method flags that
    /// CPU must enter the NMI routine at earliest opportunity. There is
    /// no way to cancel NMI request once given
    pub fn cpu_trigger_nmi(&mut self) {
        self.cpu.nmi_flag = true;
        self.cpu.calculate_interrupt_trigger_cycle();

        // Maybe process 1 clock of interrupt delay
        if !self.cpu.rdy {
            self.sched.cancel(Ev::CPU_STEAL);
            self.sched.schedule_phase(Ev::CPU_STEAL, 0, Phase::Phi2);
        }
    }

    /// Pull IRQ line low on CPU
    pub fn cpu_trigger_irq(&mut self) {
        self.cpu.irq_asserted_on_pin = true;
        self.cpu.calculate_interrupt_trigger_cycle();

        // Maybe process 1 clock of interrupt delay
        if !self.cpu.rdy && (self.cpu.interrupt_cycle == self.cpu.cycle_count) {
            self.sched.cancel(Ev::CPU_STEAL);
            self.sched.schedule_phase(Ev::CPU_STEAL, 0, Phase::Phi2);
        }
    }

    /// Inform CPU that IRQ is no longer pulled low
    pub fn cpu_clear_irq(&mut self) {
        self.cpu.irq_asserted_on_pin = false;
        self.sched.schedule_phase(Ev::CPU_CLEAR_INT, INTERRUPT_DELAY as u32, Phase::Phi1);
    }
}

// ---------------------------------------------------------------------------------------
// Interrupt routines
// ---------------------------------------------------------------------------------------

fn interrupts_and_next_opcode(c: &mut C64) {
    if c.cpu.cycle_count > c.cpu.interrupt_cycle.wrapping_add(INTERRUPT_DELAY) {
        let pc = c.cpu.pc;
        c.cpu_read(pc);
        c.cpu.cycle_count = (op::BRKn as i32) << 3;
        c.cpu.d1x1 = true;
        c.cpu.interrupt_cycle = MAX;
    } else {
        fetch_next_opcode(c);
    }
}

fn irq_lo_request(c: &mut C64) {
    let ea = c.cpu.cycle_ea;
    let v = c.cpu_read(ea);
    c.cpu.pc = (c.cpu.pc & 0xff00) | v as u16;
    c.cpu.d1x1 = false;
}

fn irq_hi_request(c: &mut C64) {
    let ea = c.cpu.cycle_ea.wrapping_add(1);
    let v = c.cpu_read(ea);
    c.cpu.pc = (c.cpu.pc & 0x00ff) | ((v as u16) << 8);
    c.cpu.flags.i = true;
}

// ---------------------------------------------------------------------------------------
// Fetch methods
// ---------------------------------------------------------------------------------------

fn fetch_next_opcode(c: &mut C64) {
    c.cpu.rdy_on_throw_away_read = false;

    let pc = c.cpu.pc;
    let opcode = c.cpu_read(pc);
    c.cpu.cycle_count = (opcode as i32) << 3;
    c.cpu.pc = c.cpu.pc.wrapping_add(1);

    if !c.cpu.check_interrupts() {
        c.cpu.interrupt_cycle = MAX;
    } else if c.cpu.interrupt_cycle != MAX {
        c.cpu.interrupt_cycle = -MAX;
    }
}

/// Read the next opcode byte from memory (and throw it away)
fn throw_away_fetch(c: &mut C64) {
    let pc = c.cpu.pc;
    c.cpu_read(pc);
}

/// Issue throw-away read and fix address. Some people use these to
/// ACK CIA IRQs
fn throw_away_read(c: &mut C64) {
    let ea = c.cpu.cycle_ea;
    c.cpu_read(ea);
    if c.cpu.adl_carry {
        c.cpu.cycle_ea = c.cpu.cycle_ea.wrapping_add(0x100);
    }
}

/// Fetch value, increment PC
/// Addressing modes: Immediate
///                   Relative
fn fetch_data_byte(c: &mut C64) {
    let pc = c.cpu.pc;
    c.cpu.cycle_data = c.cpu_read(pc);
    if !c.cpu.d1x1 {
        c.cpu.pc = c.cpu.pc.wrapping_add(1);
    }
}

/// Fetch low address byte, increment PC
/// Addressing modes: Stack manipulation
///                   Absolute
///                   Zero page
///                   Zero page indexed
///                   Absolute indexed
///                   Absolute indirect
fn fetch_low_addr(c: &mut C64) {
    let pc = c.cpu.pc;
    c.cpu.cycle_ea = c.cpu_read(pc) as u16;
    c.cpu.pc = c.cpu.pc.wrapping_add(1);
}

/// Read from address, add index register X to it
/// Addressing modes: Zero page indexed
fn fetch_low_addr_x(c: &mut C64) {
    fetch_low_addr(c);
    c.cpu.cycle_ea = c.cpu.cycle_ea.wrapping_add(c.cpu.x as u16) & 0xff;
}

/// Read from address, add index register Y to it
/// Addressing modes: Zero page indexed
fn fetch_low_addr_y(c: &mut C64) {
    fetch_low_addr(c);
    c.cpu.cycle_ea = c.cpu.cycle_ea.wrapping_add(c.cpu.y as u16) & 0xff;
}

/// Fetch high address byte, increment PC (absolute addressing)
/// Low byte must have been obtained first!
/// Addressing modes: Absolute
fn fetch_high_addr(c: &mut C64) {
    // Get the high byte of an address from memory
    let pc = c.cpu.pc;
    let v = c.cpu_read(pc);
    c.cpu.cycle_ea = (c.cpu.cycle_ea & 0x00ff) | ((v as u16) << 8);
    c.cpu.pc = c.cpu.pc.wrapping_add(1);
}

/// Fetch high byte of address, add index register X to low address
/// byte, increment PC
/// Addressing modes: Absolute indexed
fn fetch_high_addr_x(c: &mut C64) {
    c.cpu.cycle_ea = c.cpu.cycle_ea.wrapping_add(c.cpu.x as u16);
    c.cpu.adl_carry = c.cpu.cycle_ea > 0xff;
    fetch_high_addr(c);
}

/// Same as FetchHighAddrX except doesn't worry about page crossing
fn fetch_high_addr_x2(c: &mut C64) {
    fetch_high_addr_x(c);
    if !c.cpu.adl_carry {
        c.cpu.cycle_count += 1;
    }
}

/// Fetch high byte of address, add index register Y to low address
/// byte, increment PC
/// Addressing modes: Absolute indexed
fn fetch_high_addr_y(c: &mut C64) {
    c.cpu.cycle_ea = c.cpu.cycle_ea.wrapping_add(c.cpu.y as u16);
    c.cpu.adl_carry = c.cpu.cycle_ea > 0xff;
    fetch_high_addr(c);
}

/// Same as FetchHighAddrY except doesn't worry about page crossing
fn fetch_high_addr_y2(c: &mut C64) {
    fetch_high_addr_y(c);
    if !c.cpu.adl_carry {
        c.cpu.cycle_count += 1;
    }
}

/// Fetch pointer address low, increment PC
/// Addressing modes: Absolute indirect
///                   Indirect indexed (post Y)
fn fetch_low_pointer(c: &mut C64) {
    let pc = c.cpu.pc;
    c.cpu.cycle_pointer = c.cpu_read(pc) as u16;
    c.cpu.pc = c.cpu.pc.wrapping_add(1);
}

/// Add X to it
/// Addressing modes: Indexed indirect (pre X)
fn fetch_low_pointer_x(c: &mut C64) {
    let lo = c.cpu.cycle_pointer.wrapping_add(c.cpu.x as u16) & 0xff;
    c.cpu.cycle_pointer = (c.cpu.cycle_pointer & 0xff00) | lo;
}

/// Fetch pointer address high, increment PC
/// Addressing modes: Absolute indirect
fn fetch_high_pointer(c: &mut C64) {
    let pc = c.cpu.pc;
    let v = c.cpu_read(pc);
    c.cpu.cycle_pointer = (c.cpu.cycle_pointer & 0x00ff) | ((v as u16) << 8);
    c.cpu.pc = c.cpu.pc.wrapping_add(1);
}

/// Fetch effective address low
/// Addressing modes: Indirect
///                   Indexed indirect (pre X)
///                   Indirect indexed (post Y)
fn fetch_low_eff_addr(c: &mut C64) {
    let p = c.cpu.cycle_pointer;
    c.cpu.cycle_ea = c.cpu_read(p) as u16;
}

/// Fetch effective address high
/// Addressing modes: Indirect
///                   Indexed indirect (pre X)
fn fetch_high_eff_addr(c: &mut C64) {
    let lo = c.cpu.cycle_pointer.wrapping_add(1) & 0xff;
    c.cpu.cycle_pointer = (c.cpu.cycle_pointer & 0xff00) | lo;
    let p = c.cpu.cycle_pointer;
    let v = c.cpu_read(p);
    c.cpu.cycle_ea = (c.cpu.cycle_ea & 0x00ff) | ((v as u16) << 8);
}

/// Fetch effective address high, add Y to low byte of effective
/// address
/// Addressing modes: Indirect indexed (post Y)
fn fetch_high_eff_addr_y(c: &mut C64) {
    c.cpu.cycle_ea = c.cpu.cycle_ea.wrapping_add(c.cpu.y as u16);
    c.cpu.adl_carry = c.cpu.cycle_ea > 0xff;
    fetch_high_eff_addr(c);
}

/// Same as FetchHighEffAddrY except doesn't worry about page crossing
fn fetch_high_eff_addr_y2(c: &mut C64) {
    fetch_high_eff_addr_y(c);
    if !c.cpu.adl_carry {
        c.cpu.cycle_count += 1;
    }
}

// ---------------------------------------------------------------------------------------
// Common data accessing methods
// ---------------------------------------------------------------------------------------
// Data accessing operations as described in 64doc by John West and Marko Makela

fn fetch_eff_addr_data_byte(c: &mut C64) {
    let ea = c.cpu.cycle_ea;
    c.cpu.cycle_data = c.cpu_read(ea);
}

fn put_eff_addr_data_byte(c: &mut C64) {
    let (ea, d) = (c.cpu.cycle_ea, c.cpu.cycle_data);
    c.cpu_write(ea, d);
}

/// Push data on the stack
fn push(c: &mut C64, data: u8) {
    let addr = ((SP_PAGE as u16) << 8) | c.cpu.sp as u16;
    c.cpu_write(addr, data);
    c.cpu.sp = c.cpu.sp.wrapping_sub(1);
}

/// Pop data from the stack
fn pop(c: &mut C64) -> u8 {
    c.cpu.sp = c.cpu.sp.wrapping_add(1);
    let addr = ((SP_PAGE as u16) << 8) | c.cpu.sp as u16;
    c.cpu_read(addr)
}

/// Push program counter low byte on stack, decrement S
fn push_low_pc(c: &mut C64) {
    let d = c.cpu.pc as u8;
    push(c, d);
}

/// Push program counter high byte on stack, decrement S
fn push_high_pc(c: &mut C64) {
    let d = (c.cpu.pc >> 8) as u8;
    push(c, d);
}

/// Increment stack and pull program counter low byte from stack
fn pop_low_pc(c: &mut C64) {
    let v = pop(c);
    c.cpu.cycle_ea = (c.cpu.cycle_ea & 0xff00) | v as u16;
}

/// Increment stack and pull program counter high byte from stack
fn pop_high_pc(c: &mut C64) {
    let v = pop(c);
    c.cpu.cycle_ea = (c.cpu.cycle_ea & 0x00ff) | ((v as u16) << 8);
}

fn waste_cycle(_c: &mut C64) {}

fn brk_push_low_pc(c: &mut C64) {
    push_low_pc(c);

    if c.cpu.rst_flag {
        // rst = %10x
        c.cpu.cycle_ea = 0xfffc;
    } else if c.cpu.nmi_flag {
        // nmi = %01x
        c.cpu.cycle_ea = 0xfffa;
    } else {
        // irq = %11x
        c.cpu.cycle_ea = 0xfffe;
    }

    c.cpu.rst_flag = false;
    c.cpu.nmi_flag = false;
    c.cpu.calculate_interrupt_trigger_cycle();
}

// ---------------------------------------------------------------------------------------
// Common instruction opcodes
// ---------------------------------------------------------------------------------------
// See the 6510 assembly book for more information on these instructions

fn cld_instr(c: &mut C64) {
    c.cpu.flags.d = false;
    interrupts_and_next_opcode(c);
}

fn cli_instr(c: &mut C64) {
    c.cpu.flags.i = false;
    c.cpu.calculate_interrupt_trigger_cycle();
    interrupts_and_next_opcode(c);
}

fn jmp_instr(c: &mut C64) {
    c.cpu.pc = c.cpu.cycle_ea;

    interrupts_and_next_opcode(c);
}

fn pha_instr(c: &mut C64) {
    let a = c.cpu.a;
    push(c, a);
}

/// RTI does not delay the IRQ I flag change as it is set 3 cycles
/// before the end of the opcode, and thus the 6510 has enough time
/// to call the interrupt routine as soon as the opcode ends, if
/// necessary
fn rti_instr(c: &mut C64) {
    c.cpu.pc = c.cpu.cycle_ea;
    interrupts_and_next_opcode(c);
}

fn rts_instr(c: &mut C64) {
    let ea = c.cpu.cycle_ea;
    c.cpu_read(ea);
    c.cpu.pc = c.cpu.cycle_ea;
    c.cpu.pc = c.cpu.pc.wrapping_add(1);
}

fn sed_instr(c: &mut C64) {
    c.cpu.flags.d = true;
    interrupts_and_next_opcode(c);
}

fn sei_instr(c: &mut C64) {
    c.cpu.flags.i = true;
    interrupts_and_next_opcode(c);

    if !c.cpu.rst_flag && !c.cpu.nmi_flag && (c.cpu.interrupt_cycle != MAX) {
        c.cpu.interrupt_cycle = MAX;
    }
}

fn sta_instr(c: &mut C64) {
    c.cpu.cycle_data = c.cpu.a;
    put_eff_addr_data_byte(c);
}

fn stx_instr(c: &mut C64) {
    c.cpu.cycle_data = c.cpu.x;
    put_eff_addr_data_byte(c);
}

fn sty_instr(c: &mut C64) {
    c.cpu.cycle_data = c.cpu.y;
    put_eff_addr_data_byte(c);
}

// ---------------------------------------------------------------------------------------
// Common instruction undocumented opcodes
// ---------------------------------------------------------------------------------------
// See documented 6502-nmo.opc by Adam Vardy for more details

/// Perform SH* instructions
fn sh_instr(c: &mut C64) {
    let mut tmp = (c.cpu.cycle_ea >> 8) as u8;

    // When the addressing/indexing causes a page boundary crossing
    // the high byte of the target address is ANDed with the value stored
    if c.cpu.adl_carry {
        let hi = tmp & c.cpu.cycle_data;
        c.cpu.cycle_ea = (c.cpu.cycle_ea & 0x00ff) | ((hi as u16) << 8);
    } else {
        tmp = tmp.wrapping_add(1);
    }

    // When a DMA is going on (the CPU is halted by the VIC-II)
    // while the instruction sha/shx/shy executes then the last
    // term of the ANDing (ADH+1) drops off
    //
    // http://sourceforge.net/p/vice-emu/bugs/578/
    if !c.cpu.rdy_on_throw_away_read {
        c.cpu.cycle_data &= tmp;
    }

    put_eff_addr_data_byte(c);
}

/// Undocumented - This opcode stores the result of A AND X AND ADH+1
/// in memory
fn axa_instr(c: &mut C64) {
    c.cpu.cycle_data = c.cpu.x & c.cpu.a;
    sh_instr(c);
}

/// Undocumented - This opcode ANDs the contents of the Y register
/// with ADH+1 and stores the result in memory
fn say_instr(c: &mut C64) {
    c.cpu.cycle_data = c.cpu.y;
    sh_instr(c);
}

/// Undocumented - This opcode ANDs the contents of the X register
/// with ADH+1 and stores the result in memory
fn xas_instr(c: &mut C64) {
    c.cpu.cycle_data = c.cpu.x;
    sh_instr(c);
}

/// Undocumented - AXS ANDs the contents of the A and X registers
/// (without changing the contents of either register) and stores the
/// result in memory. AXS does not affect any flags in the processor
/// status register
fn axs_instr(c: &mut C64) {
    c.cpu.cycle_data = c.cpu.a & c.cpu.x;
    put_eff_addr_data_byte(c);
}

// ---------------------------------------------------------------------------------------
// Arithmetic operations
// ---------------------------------------------------------------------------------------

/// BCD adding
fn do_adc(c: &mut C64) {
    let cpu = &mut c.cpu;
    let cy: u32 = if cpu.flags.c { 1 } else { 0 };
    let a: u32 = cpu.a as u32;
    let s: u32 = cpu.cycle_data as u32;
    let reg_ac2: u32 = a.wrapping_add(s).wrapping_add(cy);

    if cpu.flags.d {
        // BCD mode
        let mut lo: u32 = (a & 0x0f) + (s & 0x0f) + cy;
        let mut hi: u32 = (a & 0xf0) + (s & 0xf0);

        if lo > 0x09 {
            lo += 0x06;
        }

        if lo > 0x0f {
            hi += 0x10;
        }

        cpu.flags.z = (reg_ac2 & 0xff) == 0;
        cpu.flags.n = (hi & 0x80) != 0;
        cpu.flags.v = (((hi ^ a) & 0x80) != 0) && (((a ^ s) & 0x80) == 0);

        if hi > 0x90 {
            hi += 0x60;
        }

        cpu.flags.c = hi > 0xff;
        cpu.a = (hi | (lo & 0x0f)) as u8;
    } else {
        // Binary mode
        cpu.flags.c = reg_ac2 > 0xff;
        cpu.flags.v = (((reg_ac2 ^ a) & 0x80) != 0) && (((a ^ s) & 0x80) == 0);
        cpu.a = (reg_ac2 & 0xff) as u8;
        cpu.flags.set_nz(cpu.a);
    }
}

/// BCD subtracting
fn do_sbc(c: &mut C64) {
    let cpu = &mut c.cpu;
    let cy: u32 = if cpu.flags.c { 0 } else { 1 };
    let a: u32 = cpu.a as u32;
    let s: u32 = cpu.cycle_data as u32;
    let reg_ac2: u32 = a.wrapping_sub(s).wrapping_sub(cy);

    cpu.flags.c = reg_ac2 < 0x100;
    cpu.flags.v = (((reg_ac2 ^ a) & 0x80) != 0) && (((a ^ s) & 0x80) != 0);
    cpu.flags.set_nz(reg_ac2 as u8);

    if cpu.flags.d {
        // BCD mode
        let mut lo: u32 = (a & 0x0f).wrapping_sub(s & 0x0f).wrapping_sub(cy);
        let mut hi: u32 = (a & 0xf0).wrapping_sub(s & 0xf0);

        if (lo & 0x10) != 0 {
            lo = lo.wrapping_sub(0x06);
            hi = hi.wrapping_sub(0x10);
        }

        if (hi & 0x100) != 0 {
            hi = hi.wrapping_sub(0x60);
        }

        cpu.a = (hi | (lo & 0x0f)) as u8;
    } else {
        // Binary mode
        cpu.a = (reg_ac2 & 0xff) as u8;
    }
}

// ---------------------------------------------------------------------------------------
// Generic instruction opcodes
// ---------------------------------------------------------------------------------------
// See the 6510 assembly book for more information on these instructions

fn adc_instr(c: &mut C64) {
    do_adc(c);
    interrupts_and_next_opcode(c);
}

fn and_instr(c: &mut C64) {
    c.cpu.a &= c.cpu.cycle_data;
    c.cpu.flags.set_nz(c.cpu.a);
    interrupts_and_next_opcode(c);
}

/// Undocumented - For a detailed explanation of this opcode look at:
/// http://visual6502.org/wiki/index.php?title=6502_Opcode_8B_(XAA,_ANE)
fn ane_instr(c: &mut C64) {
    c.cpu.a = (c.cpu.a | ANE_MAGIC) & c.cpu.x & c.cpu.cycle_data;
    c.cpu.flags.set_nz(c.cpu.a);
    interrupts_and_next_opcode(c);
}

fn asl_instr(c: &mut C64) {
    put_eff_addr_data_byte(c);
    c.cpu.flags.c = (c.cpu.cycle_data & 0x80) != 0;
    c.cpu.cycle_data <<= 1;
    c.cpu.flags.set_nz(c.cpu.cycle_data);
}

fn asla_instr(c: &mut C64) {
    c.cpu.flags.c = (c.cpu.a & 0x80) != 0;
    c.cpu.a <<= 1;
    c.cpu.flags.set_nz(c.cpu.a);
    interrupts_and_next_opcode(c);
}

fn fix_branch(c: &mut C64) {
    // Throw away read
    let ea = c.cpu.cycle_ea;
    c.cpu_read(ea);

    // Fix address
    c.cpu.pc = c.cpu.pc.wrapping_add(if c.cpu.cycle_data < 0x80 { 0x0100 } else { 0xff00 });
}

fn branch_instr(c: &mut C64, condition: bool) {
    // 2 cycles spent before arriving here. spend 0 - 2 cycles here;
    // - condition false: Continue immediately to FetchNextInstr.
    //
    // Otherwise read the byte following the opcode (which is already scheduled to occur on this cycle).
    // This effort is wasted. Then calculate address of the branch target. If branch is on same page,
    // then continue at that insn on next cycle (this delays IRQs by 1 clock for some reason, allegedly).
    //
    // If the branch is in different memory page, issue a spurious read with wrong high byte before
    // continuing at the correct address
    if condition {
        // Issue the spurious read for next insn here
        let pc = c.cpu.pc;
        c.cpu_read(pc);

        let cpu = &mut c.cpu;
        cpu.cycle_ea = cpu.pc & 0x00ff;
        cpu.cycle_ea = cpu.cycle_ea.wrapping_add(cpu.cycle_data as u16);
        cpu.adl_carry = (cpu.cycle_ea > 0xff) != (cpu.cycle_data > 0x7f);
        cpu.cycle_ea = (cpu.cycle_ea & 0x00ff) | (cpu.pc & 0xff00);

        cpu.pc = cpu.cycle_ea;

        // Check for page boundary crossing
        if !cpu.adl_carry {
            // Skip next throw away read
            cpu.cycle_count += 1;

            // Hack: delay the interrupt past this instruction
            if cpu.interrupt_cycle >> 3 == cpu.cycle_count >> 3 {
                cpu.interrupt_cycle += 2;
            }
        }
    } else {
        // Branch not taken: skip the following spurious read insn and go to FetchNextInstr immediately
        interrupts_and_next_opcode(c);
    }
}

fn bcc_instr(c: &mut C64) {
    let f = !c.cpu.flags.c;
    branch_instr(c, f);
}

fn bcs_instr(c: &mut C64) {
    let f = c.cpu.flags.c;
    branch_instr(c, f);
}

fn beq_instr(c: &mut C64) {
    let f = c.cpu.flags.z;
    branch_instr(c, f);
}

fn bit_instr(c: &mut C64) {
    c.cpu.flags.z = (c.cpu.a & c.cpu.cycle_data) == 0;
    c.cpu.flags.n = (c.cpu.cycle_data & 0x80) != 0;
    c.cpu.flags.v = (c.cpu.cycle_data & 0x40) != 0;
    interrupts_and_next_opcode(c);
}

fn bmi_instr(c: &mut C64) {
    let f = c.cpu.flags.n;
    branch_instr(c, f);
}

fn bne_instr(c: &mut C64) {
    let f = !c.cpu.flags.z;
    branch_instr(c, f);
}

fn bpl_instr(c: &mut C64) {
    let f = !c.cpu.flags.n;
    branch_instr(c, f);
}

fn bvc_instr(c: &mut C64) {
    let f = !c.cpu.flags.v;
    branch_instr(c, f);
}

fn bvs_instr(c: &mut C64) {
    let f = c.cpu.flags.v;
    branch_instr(c, f);
}

fn clc_instr(c: &mut C64) {
    c.cpu.flags.c = false;
    interrupts_and_next_opcode(c);
}

fn clv_instr(c: &mut C64) {
    c.cpu.flags.v = false;
    interrupts_and_next_opcode(c);
}

fn compare(c: &mut C64, data: u8) {
    let tmp: u16 = (data as i32 - c.cpu.cycle_data as i32) as u16;
    c.cpu.flags.set_nz(tmp as u8);
    c.cpu.flags.c = tmp < 0x100;
    interrupts_and_next_opcode(c);
}

fn cmp_instr(c: &mut C64) {
    let d = c.cpu.a;
    compare(c, d);
}

fn cpx_instr(c: &mut C64) {
    let d = c.cpu.x;
    compare(c, d);
}

fn cpy_instr(c: &mut C64) {
    let d = c.cpu.y;
    compare(c, d);
}

fn dec_instr(c: &mut C64) {
    put_eff_addr_data_byte(c);
    c.cpu.cycle_data = c.cpu.cycle_data.wrapping_sub(1);
    c.cpu.flags.set_nz(c.cpu.cycle_data);
}

fn dex_instr(c: &mut C64) {
    c.cpu.x = c.cpu.x.wrapping_sub(1);
    c.cpu.flags.set_nz(c.cpu.x);
    interrupts_and_next_opcode(c);
}

fn dey_instr(c: &mut C64) {
    c.cpu.y = c.cpu.y.wrapping_sub(1);
    c.cpu.flags.set_nz(c.cpu.y);
    interrupts_and_next_opcode(c);
}

fn eor_instr(c: &mut C64) {
    c.cpu.a ^= c.cpu.cycle_data;
    c.cpu.flags.set_nz(c.cpu.a);
    interrupts_and_next_opcode(c);
}

fn inc_instr(c: &mut C64) {
    put_eff_addr_data_byte(c);
    c.cpu.cycle_data = c.cpu.cycle_data.wrapping_add(1);
    c.cpu.flags.set_nz(c.cpu.cycle_data);
}

fn inx_instr(c: &mut C64) {
    c.cpu.x = c.cpu.x.wrapping_add(1);
    c.cpu.flags.set_nz(c.cpu.x);
    interrupts_and_next_opcode(c);
}

fn iny_instr(c: &mut C64) {
    c.cpu.y = c.cpu.y.wrapping_add(1);
    c.cpu.flags.set_nz(c.cpu.y);
    interrupts_and_next_opcode(c);
}

fn lda_instr(c: &mut C64) {
    c.cpu.a = c.cpu.cycle_data;
    c.cpu.flags.set_nz(c.cpu.a);
    interrupts_and_next_opcode(c);
}

fn ldx_instr(c: &mut C64) {
    c.cpu.x = c.cpu.cycle_data;
    c.cpu.flags.set_nz(c.cpu.x);
    interrupts_and_next_opcode(c);
}

fn ldy_instr(c: &mut C64) {
    c.cpu.y = c.cpu.cycle_data;
    c.cpu.flags.set_nz(c.cpu.y);
    interrupts_and_next_opcode(c);
}

fn lsr_instr(c: &mut C64) {
    put_eff_addr_data_byte(c);
    c.cpu.flags.c = (c.cpu.cycle_data & 0x01) != 0;
    c.cpu.cycle_data >>= 1;
    c.cpu.flags.set_nz(c.cpu.cycle_data);
}

fn lsra_instr(c: &mut C64) {
    c.cpu.flags.c = (c.cpu.a & 0x01) != 0;
    c.cpu.a >>= 1;
    c.cpu.flags.set_nz(c.cpu.a);
    interrupts_and_next_opcode(c);
}

fn ora_instr(c: &mut C64) {
    c.cpu.a |= c.cpu.cycle_data;
    c.cpu.flags.set_nz(c.cpu.a);
    interrupts_and_next_opcode(c);
}

fn pla_instr(c: &mut C64) {
    let v = pop(c);
    c.cpu.a = v;
    c.cpu.flags.set_nz(v);
}

fn rol_instr(c: &mut C64) {
    let new_c = (c.cpu.cycle_data & 0x80) != 0;
    put_eff_addr_data_byte(c);
    c.cpu.cycle_data <<= 1;

    if c.cpu.flags.c {
        c.cpu.cycle_data |= 0x01;
    }

    c.cpu.flags.set_nz(c.cpu.cycle_data);
    c.cpu.flags.c = new_c;
}

fn rola_instr(c: &mut C64) {
    let new_c = (c.cpu.a & 0x80) != 0;
    c.cpu.a <<= 1;

    if c.cpu.flags.c {
        c.cpu.a |= 0x01;
    }

    c.cpu.flags.set_nz(c.cpu.a);
    c.cpu.flags.c = new_c;
    interrupts_and_next_opcode(c);
}

fn ror_instr(c: &mut C64) {
    let new_c = (c.cpu.cycle_data & 0x01) != 0;
    put_eff_addr_data_byte(c);
    c.cpu.cycle_data >>= 1;

    if c.cpu.flags.c {
        c.cpu.cycle_data |= 0x80;
    }

    c.cpu.flags.set_nz(c.cpu.cycle_data);
    c.cpu.flags.c = new_c;
}

fn rora_instr(c: &mut C64) {
    let new_c = (c.cpu.a & 0x01) != 0;
    c.cpu.a >>= 1;

    if c.cpu.flags.c {
        c.cpu.a |= 0x80;
    }

    c.cpu.flags.set_nz(c.cpu.a);
    c.cpu.flags.c = new_c;
    interrupts_and_next_opcode(c);
}

fn sbx_instr(c: &mut C64) {
    let tmp: u32 = ((c.cpu.x & c.cpu.a) as u32).wrapping_sub(c.cpu.cycle_data as u32);
    c.cpu.x = (tmp & 0xff) as u8;
    c.cpu.flags.set_nz(c.cpu.x);
    c.cpu.flags.c = tmp < 0x100;
    interrupts_and_next_opcode(c);
}

fn sbc_instr(c: &mut C64) {
    do_sbc(c);
    interrupts_and_next_opcode(c);
}

fn sec_instr(c: &mut C64) {
    c.cpu.flags.c = true;
    interrupts_and_next_opcode(c);
}

fn shs_instr(c: &mut C64) {
    c.cpu.sp = c.cpu.a & c.cpu.x;
    c.cpu.cycle_data = c.cpu.sp;
    sh_instr(c);
}

fn tax_instr(c: &mut C64) {
    c.cpu.x = c.cpu.a;
    c.cpu.flags.set_nz(c.cpu.x);
    interrupts_and_next_opcode(c);
}

fn tay_instr(c: &mut C64) {
    c.cpu.y = c.cpu.a;
    c.cpu.flags.set_nz(c.cpu.y);
    interrupts_and_next_opcode(c);
}

fn tsx_instr(c: &mut C64) {
    c.cpu.x = c.cpu.sp;
    c.cpu.flags.set_nz(c.cpu.x);
    interrupts_and_next_opcode(c);
}

fn txa_instr(c: &mut C64) {
    c.cpu.a = c.cpu.x;
    c.cpu.flags.set_nz(c.cpu.a);
    interrupts_and_next_opcode(c);
}

fn txs_instr(c: &mut C64) {
    c.cpu.sp = c.cpu.x;
    interrupts_and_next_opcode(c);
}

fn tya_instr(c: &mut C64) {
    c.cpu.a = c.cpu.y;
    c.cpu.flags.set_nz(c.cpu.a);
    interrupts_and_next_opcode(c);
}

/// Upstream throws HaltInstructionException ("Illegal instruction $xx at address $xxxx");
/// here the CPU is marked halted (see the header).
fn invalid_opcode(c: &mut C64) {
    c.cpu.halted = true;
}

// ---------------------------------------------------------------------------------------
// Generic instruction undocumented opcodes
// ---------------------------------------------------------------------------------------
// See documented 6502-nmo.opc by Adam Vardy for more details

/// Undocumented - This opcode ANDs the contents of the A register
/// with an immediate value and then LSRs the result
fn alr_instr(c: &mut C64) {
    c.cpu.a &= c.cpu.cycle_data;
    c.cpu.flags.c = (c.cpu.a & 0x01) != 0;
    c.cpu.a >>= 1;
    c.cpu.flags.set_nz(c.cpu.a);
    interrupts_and_next_opcode(c);
}

/// Undocumented - ANC ANDs the contents of the A register with an
/// immediate value and then moves bit 7 of A into the Carry flag.
/// This opcode works basically identically to AND #immed. except
/// that the Carry flag is set to the same state that the Negative
/// flag is set to
fn anc_instr(c: &mut C64) {
    c.cpu.a &= c.cpu.cycle_data;
    c.cpu.flags.set_nz(c.cpu.a);
    c.cpu.flags.c = c.cpu.flags.n;
    interrupts_and_next_opcode(c);
}

/// Undocumented - This opcode ANDs the contents of the A register
/// with an immediate value and then RORs the result. (Implementation
/// based on that of Frodo C64 Emulator)
fn arr_instr(c: &mut C64) {
    let cpu = &mut c.cpu;
    let data: u8 = cpu.cycle_data & cpu.a;
    cpu.a = data >> 1;

    if cpu.flags.c {
        cpu.a |= 0x80;
    }

    if cpu.flags.d {
        cpu.flags.n = cpu.flags.c;
        cpu.flags.z = cpu.a == 0;
        cpu.flags.v = ((data ^ cpu.a) & 0x40) != 0;

        if (data & 0x0f) as u32 + (data & 0x01) as u32 > 5 {
            cpu.a = (cpu.a & 0xf0) | ((cpu.a as u32 + 6) & 0x0f) as u8;
        }

        cpu.flags.c = ((data as u32 + (data & 0x10) as u32) & 0x1f0) > 0x50;
        if cpu.flags.c {
            cpu.a = cpu.a.wrapping_add(0x60);
        }
    } else {
        cpu.flags.set_nz(cpu.a);
        cpu.flags.c = (cpu.a & 0x40) != 0;
        cpu.flags.v = ((cpu.a & 0x40) ^ ((cpu.a & 0x20) << 1)) != 0;
    }

    interrupts_and_next_opcode(c);
}

/// Undocumented - This opcode ASLs the contents of a memory
/// location and then ORs the result with the accumulator
fn aso_instr(c: &mut C64) {
    put_eff_addr_data_byte(c);
    c.cpu.flags.c = (c.cpu.cycle_data & 0x80) != 0;
    c.cpu.cycle_data <<= 1;
    c.cpu.a |= c.cpu.cycle_data;
    c.cpu.flags.set_nz(c.cpu.a);
}

/// Undocumented - This opcode DECs the contents of a memory location
/// and then CMPs the result with the A register
fn dcm_instr(c: &mut C64) {
    put_eff_addr_data_byte(c);
    c.cpu.cycle_data = c.cpu.cycle_data.wrapping_sub(1);

    let tmp: u16 = (c.cpu.a as i32 - c.cpu.cycle_data as i32) as u16;
    c.cpu.flags.set_nz(tmp as u8);
    c.cpu.flags.c = tmp < 0x100;
}

/// Undocumented - This opcode INCs the contents of a memory location
/// and then SBCs the result from the A register
fn ins_instr(c: &mut C64) {
    put_eff_addr_data_byte(c);
    c.cpu.cycle_data = c.cpu.cycle_data.wrapping_add(1);
    do_sbc(c);
}

/// Undocumented - This opcode ANDs the contents of a memory location
/// with the contents of the stack pointer register and stores the
/// result in the accumulator, the X register, and the stack pointer.
/// Affected flags: N Z
fn las_instr(c: &mut C64) {
    c.cpu.cycle_data &= c.cpu.sp;
    c.cpu.flags.set_nz(c.cpu.cycle_data);
    c.cpu.a = c.cpu.cycle_data;
    c.cpu.x = c.cpu.cycle_data;
    c.cpu.sp = c.cpu.cycle_data;
    interrupts_and_next_opcode(c);
}

/// Undocumented - This opcode loads both the accumulator and the X
/// register with the contents of a memory location
fn lax_instr(c: &mut C64) {
    c.cpu.x = c.cpu.cycle_data;
    c.cpu.a = c.cpu.cycle_data;
    c.cpu.flags.set_nz(c.cpu.a);
    interrupts_and_next_opcode(c);
}

/// Undocumented - LSE LSRs the contents of a memory location and
/// then EORs the result with the accumulator
fn lse_instr(c: &mut C64) {
    put_eff_addr_data_byte(c);
    c.cpu.flags.c = (c.cpu.cycle_data & 0x01) != 0;
    c.cpu.cycle_data >>= 1;
    c.cpu.a ^= c.cpu.cycle_data;
    c.cpu.flags.set_nz(c.cpu.a);
}

/// Undocumented - This opcode ORs the A register with #xx (the
/// "magic" value), ANDs the result with an immediate value, and then
/// stores the result in both A and X
fn oal_instr(c: &mut C64) {
    c.cpu.a = c.cpu.cycle_data & (c.cpu.a | LXA_MAGIC);
    c.cpu.x = c.cpu.a;
    c.cpu.flags.set_nz(c.cpu.x);
    interrupts_and_next_opcode(c);
}

/// Undocumented - RLA ROLs the contents of a memory location and
/// then ANDs the result with the accumulator
fn rla_instr(c: &mut C64) {
    let new_c = (c.cpu.cycle_data & 0x80) != 0;
    put_eff_addr_data_byte(c);
    c.cpu.cycle_data <<= 1;

    if c.cpu.flags.c {
        c.cpu.cycle_data |= 0x01;
    }

    c.cpu.flags.c = new_c;
    c.cpu.a &= c.cpu.cycle_data;
    c.cpu.flags.set_nz(c.cpu.a);
}

/// Undocumented - RRA RORs the contents of a memory location and
/// then ADCs the result with the accumulator
fn rra_instr(c: &mut C64) {
    let new_c = (c.cpu.cycle_data & 0x01) != 0;
    put_eff_addr_data_byte(c);
    c.cpu.cycle_data >>= 1;

    if c.cpu.flags.c {
        c.cpu.cycle_data |= 0x80;
    }

    c.cpu.flags.c = new_c;
    do_adc(c);
}

// ---------------------------------------------------------------------------------------
// Private methods
// ---------------------------------------------------------------------------------------

/// When AEC signal is high, no stealing is possible
fn event_without_steals(c: &mut C64) {
    // (not upstream) after a JAM nothing runs any more
    if c.cpu.halted {
        return;
    }

    let instr = c.cpu.table[(c.cpu.cycle_count as usize) & TABLE_MASK];
    c.cpu.cycle_count += 1;
    (instr.func)(c);

    // (not upstream) upstream's exception skips the rescheduling
    if c.cpu.halted {
        return;
    }
    c.sched.schedule(Ev::CPU_NOSTEAL, 1);
}

const CLI_CYCLE: i32 = (op::CLIn as i32) << 3;
const SEI_CYCLE: i32 = (op::SEIn as i32) << 3;
const SHAIY_CYCLE: i32 = ((op::SHAiy as i32) << 3) + 3;
const SHSAY_CYCLE: i32 = ((op::SHSay as i32) << 3) + 2;
const SHYAX_CYCLE: i32 = ((op::SHYax as i32) << 3) + 2;
const SHXAY_CYCLE: i32 = ((op::SHXay as i32) << 3) + 2;
const SHAAY_CYCLE: i32 = ((op::SHAay as i32) << 3) + 2;

/// When AEC signal is low, steals permitted
fn event_with_steals(c: &mut C64) {
    // (not upstream) after a JAM nothing runs any more
    if c.cpu.halted {
        return;
    }

    if c.cpu.table[(c.cpu.cycle_count as usize) & TABLE_MASK].no_steal {
        let instr = c.cpu.table[(c.cpu.cycle_count as usize) & TABLE_MASK];
        c.cpu.cycle_count += 1;
        (instr.func)(c);

        // (not upstream) upstream's exception skips the rescheduling
        if c.cpu.halted {
            return;
        }
        c.sched.schedule(Ev::CPU_STEAL, 1);
    } else {
        let cpu = &mut c.cpu;
        match cpu.cycle_count {
            CLI_CYCLE => {
                cpu.flags.i = false;
                if cpu.irq_asserted_on_pin && (cpu.interrupt_cycle == MAX) {
                    cpu.interrupt_cycle = -MAX;
                }
            }

            SEI_CYCLE => {
                cpu.flags.i = true;
                if !cpu.rst_flag && !cpu.nmi_flag && (cpu.cycle_count <= cpu.interrupt_cycle.wrapping_add(INTERRUPT_DELAY)) {
                    cpu.interrupt_cycle = MAX;
                }
            }

            SHAIY_CYCLE | SHSAY_CYCLE | SHYAX_CYCLE | SHXAY_CYCLE | SHAAY_CYCLE => {
                // Save rdy state for SH* instructions
                cpu.rdy_on_throw_away_read = true;
            }

            _ => {}
        }

        // Even while stalled, the CPU can still process first clock of
        // interrupt delay, but only the first one
        if cpu.interrupt_cycle == cpu.cycle_count {
            cpu.interrupt_cycle -= 1;
        }
    }
}

fn remove_irq(c: &mut C64) {
    if !c.cpu.rst_flag && !c.cpu.nmi_flag && (c.cpu.interrupt_cycle != MAX) {
        c.cpu.interrupt_cycle = MAX;
    }
}

/// Push P on stack, decrement S
fn push_sr(c: &mut C64) {
    // Set the B flag, 0 for hardware interrupts
    // and 1 for BRK and PHP. Bit 5 is always 1
    // https://wiki.nesdev.org/w/index.php?title=Status_flags#The_B_flag
    let v = c.cpu.flags.get() | if c.cpu.d1x1 { 0x20 } else { 0x30 };
    push(c, v);
}

/// Increment S, pop P off stack
fn pop_sr(c: &mut C64) {
    let v = pop(c);
    c.cpu.flags.set(v);
    c.cpu.calculate_interrupt_trigger_cycle();
}

/// The table, built once
fn instruction_table() -> &'static [ProcessorCycle] {
    static TABLE: OnceLock<Box<[ProcessorCycle]>> = OnceLock::new();
    TABLE.get_or_init(build_instruction_table)
}

/// Build up the processor instruction table
fn build_instruction_table() -> Box<[ProcessorCycle]> {
    // An entry that is never built is never reached (every row ends with a function
    // that sets cycleCount anew); if it were, it halts
    let mut t: Vec<ProcessorCycle> = vec![ProcessorCycle { func: invalid_opcode, no_steal: false }; 0x101 << 3];

    for i in 0u32..0x100 {
        // So: what cycles are marked as stealable? Rules are:
        //
        // - CPU performs either read or write at every cycle. Reads are
        //   always stealable. Writes are rare.
        //
        // - Every instruction begins with a sequence of reads. Writes,
        //   if any, are at the end for most instructions

        let mut build_cycle: usize = (i << 3) as usize;
        let opc = i as u8;

        let mut read = false; // AccessMode.READ
        let mut legal_mode = true;
        let mut legal_instr = true;

        macro_rules! ins {
            ($f:expr) => {{
                t[build_cycle].func = $f;
                build_cycle += 1;
            }};
        }
        macro_rules! ins_ns {
            ($f:expr) => {{
                t[build_cycle].no_steal = true;
                t[build_cycle].func = $f;
                build_cycle += 1;
            }};
        }

        match opc {
            // Accumulator or Implied addressing
            op::ASLn | op::CLCn | op::CLDn | op::CLIn | op::CLVn | op::DEXn |
            op::DEYn | op::INXn | op::INYn | op::LSRn | op::NOPn | op::NOPn1 | op::NOPn2 | op::NOPn3 | op::NOPn4 | op::NOPn5 | op::NOPn6 | op::PHAn |
            op::PHPn | op::PLAn | op::PLPn | op::ROLn | op::RORn |
            op::SECn | op::SEDn | op::SEIn | op::TAXn | op::TAYn |
            op::TSXn | op::TXAn | op::TXSn | op::TYAn => {
                ins!(throw_away_fetch);
            }

            // Immediate and Relative addressing mode handler
            op::ADCb | op::ANDb | op::ANCb | op::ANCb1 | op::ANEb | op::ASRb | op::ARRb |
            op::BCCr | op::BCSr | op::BEQr | op::BMIr | op::BNEr | op::BPLr |
            op::BRKn | op::BVCr | op::BVSr | op::CMPb | op::CPXb | op::CPYb |
            op::EORb | op::LDAb | op::LDXb | op::LDYb | op::LXAb | op::NOPb | op::NOPb1 | op::NOPb2 | op::NOPb3 | op::NOPb4 |
            op::ORAb | op::SBCb | op::SBCb1 | op::SBXb | op::RTIn | op::RTSn => {
                ins!(fetch_data_byte);
            }

            // Zero page addressing mode handler - read & RMW
            op::ADCz | op::ANDz | op::BITz | op::CMPz | op::CPXz | op::CPYz |
            op::EORz | op::LAXz | op::LDAz | op::LDXz | op::LDYz | op::ORAz |
            op::NOPz | op::NOPz1 | op::NOPz2 | op::SBCz |
            op::ASLz | op::DCPz | op::DECz | op::INCz | op::ISBz | op::LSRz |
            op::ROLz | op::RORz | op::SREz | op::SLOz | op::RLAz | op::RRAz => {
                read = true;
                ins!(fetch_low_addr);
            }

            op::SAXz | op::STAz | op::STXz | op::STYz => {
                ins!(fetch_low_addr);
            }

            // Zero page with X offset addressing mode handler
            // These issue extra reads on the 0 page, but we don't care about it
            // because there are no detectable effects from them. These reads
            // occur during the "wasted" cycle
            op::ADCzx | op::ANDzx | op::CMPzx | op::EORzx | op::LDAzx | op::LDYzx |
            op::NOPzx | op::NOPzx1 | op::NOPzx2 | op::NOPzx3 | op::NOPzx4 | op::NOPzx5 | op::ORAzx | op::SBCzx |
            op::ASLzx | op::DCPzx | op::DECzx | op::INCzx | op::ISBzx | op::LSRzx |
            op::RLAzx | op::ROLzx | op::RORzx | op::RRAzx | op::SLOzx | op::SREzx => {
                read = true;
                ins!(fetch_low_addr_x);

                // Operate on 0 page in read mode. Truly side-effect free
                ins!(waste_cycle);
            }

            op::STAzx | op::STYzx => {
                ins!(fetch_low_addr_x);

                // Operate on 0 page in read mode. Truly side-effect free
                ins!(waste_cycle);
            }

            // Zero page with Y offset addressing mode handler
            op::LDXzy | op::LAXzy => {
                read = true;
                ins!(fetch_low_addr_y);

                // Operate on 0 page in read mode. Truly side-effect free
                ins!(waste_cycle);
            }

            op::STXzy | op::SAXzy => {
                ins!(fetch_low_addr_y);

                // Operate on 0 page in read mode. Truly side-effect free
                ins!(waste_cycle);
            }

            // Absolute addressing mode handler
            op::ADCa | op::ANDa | op::BITa | op::CMPa | op::CPXa | op::CPYa |
            op::EORa | op::LAXa | op::LDAa | op::LDXa | op::LDYa | op::NOPa |
            op::ORAa | op::SBCa |
            op::ASLa | op::DCPa | op::DECa | op::INCa | op::ISBa | op::LSRa |
            op::ROLa | op::RORa | op::SLOa | op::SREa | op::RLAa | op::RRAa => {
                read = true;
                ins!(fetch_low_addr);
                ins!(fetch_high_addr);
            }

            op::JMPw | op::SAXa | op::STAa | op::STXa | op::STYa => {
                ins!(fetch_low_addr);
                ins!(fetch_high_addr);
            }

            op::JSRw => {
                ins!(fetch_low_addr);
            }

            // Absolute with X offset addressing mode handler (read)
            op::ADCax | op::ANDax | op::CMPax | op::EORax | op::LDAax |
            op::LDYax | op::NOPax | op::NOPax1 | op::NOPax2 | op::NOPax3 | op::NOPax4 | op::NOPax5 | op::ORAax | op::SBCax => {
                read = true;
                ins!(fetch_low_addr);
                ins!(fetch_high_addr_x2);

                // This cycle is skipped if the address is already correct.
                // Otherwise, it will be read and ignored
                ins!(throw_away_read);
            }

            // Absolute X (RMW; no page crossing handled, always reads before writing)
            op::ASLax | op::DCPax | op::DECax | op::INCax | op::ISBax |
            op::LSRax | op::RLAax | op::ROLax | op::RORax | op::RRAax |
            op::SLOax | op::SREax => {
                read = true;
                ins!(fetch_low_addr);
                ins!(fetch_high_addr_x);
                ins!(throw_away_read);
            }

            op::SHYax | op::STAax => {
                ins!(fetch_low_addr);
                ins!(fetch_high_addr_x);
                ins!(throw_away_read);
            }

            // Absolute with Y offset addressing mode handler (read)
            op::ADCay | op::ANDay | op::CMPay | op::EORay | op::LASay |
            op::LAXay | op::LDAay | op::LDXay | op::ORAay | op::SBCay => {
                read = true;

                ins!(fetch_low_addr);
                ins!(fetch_high_addr_y2);
                ins!(throw_away_read);
            }

            // Absolute Y (no page crossing handled)
            op::DCPay | op::ISBay | op::RLAay | op::RRAay | op::SLOay |
            op::SREay => {
                read = true;
                ins!(fetch_low_addr);
                ins!(fetch_high_addr_y);
                ins!(throw_away_read);
            }

            op::SHAay | op::SHSay | op::SHXay | op::STAay => {
                ins!(fetch_low_addr);
                ins!(fetch_high_addr_y);
                ins!(throw_away_read);
            }

            // Absolute indirect addressing mode handler
            op::JMPi => {
                ins!(fetch_low_pointer);
                ins!(fetch_high_pointer);
                ins!(fetch_low_eff_addr);
                ins!(fetch_high_eff_addr);
            }

            // Indexed with X preinc addressing mode handler
            op::ADCix | op::ANDix | op::CMPix | op::EORix | op::LAXix | op::LDAix |
            op::ORAix | op::SBCix |
            op::DCPix | op::ISBix | op::SLOix | op::SREix | op::RLAix | op::RRAix => {
                read = true;
                ins!(fetch_low_pointer);
                ins!(fetch_low_pointer_x);
                ins!(fetch_low_eff_addr);
                ins!(fetch_high_eff_addr);
            }

            op::SAXix | op::STAix => {
                ins!(fetch_low_pointer);
                ins!(fetch_low_pointer_x);
                ins!(fetch_low_eff_addr);
                ins!(fetch_high_eff_addr);
            }

            // Indexed with Y postinc addressing mode handler (read)
            op::ADCiy | op::ANDiy | op::CMPiy | op::EORiy | op::LAXiy |
            op::LDAiy | op::ORAiy | op::SBCiy => {
                read = true;
                ins!(fetch_low_pointer);
                ins!(fetch_low_eff_addr);
                ins!(fetch_high_eff_addr_y2);
                ins!(throw_away_read);
            }

            // Indexed Y (no page crossing handled)
            op::DCPiy | op::ISBiy | op::RLAiy | op::RRAiy | op::SLOiy |
            op::SREiy => {
                read = true;
                ins!(fetch_low_pointer);
                ins!(fetch_low_eff_addr);
                ins!(fetch_high_eff_addr_y);
                ins!(throw_away_read);
            }

            op::SHAiy | op::STAiy => {
                ins!(fetch_low_pointer);
                ins!(fetch_low_eff_addr);
                ins!(fetch_high_eff_addr_y);
                ins!(throw_away_read);
            }

            _ => {
                legal_mode = false;
            }
        }

        if read {
            ins!(fetch_eff_addr_data_byte);
        }

        //----------------------------------------------------------------------
        // Addressing modes finished, other cycles are instruction dependent
        match opc {
            op::ADCz | op::ADCzx | op::ADCa | op::ADCax | op::ADCay | op::ADCix |
            op::ADCiy | op::ADCb => {
                ins!(adc_instr);
            }

            op::ANCb | op::ANCb1 => {
                ins!(anc_instr);
            }

            op::ANDz | op::ANDzx | op::ANDa | op::ANDax | op::ANDay | op::ANDix |
            op::ANDiy | op::ANDb => {
                ins!(and_instr);
            }

            op::ANEb => {
                // Also known as XAA
                ins!(ane_instr);
            }

            op::ARRb => {
                ins!(arr_instr);
            }

            op::ASLn => {
                ins!(asla_instr);
            }

            op::ASLz | op::ASLzx | op::ASLa | op::ASLax => {
                ins_ns!(asl_instr);
                ins_ns!(put_eff_addr_data_byte);
            }

            op::ASRb => {
                // Also known as ALR
                ins!(alr_instr);
            }

            op::BCCr => {
                ins!(bcc_instr);
                ins!(fix_branch);
            }

            op::BCSr => {
                ins!(bcs_instr);
                ins!(fix_branch);
            }

            op::BEQr => {
                ins!(beq_instr);
                ins!(fix_branch);
            }

            op::BITz | op::BITa => {
                ins!(bit_instr);
            }

            op::BMIr => {
                ins!(bmi_instr);
                ins!(fix_branch);
            }

            op::BNEr => {
                ins!(bne_instr);
                ins!(fix_branch);
            }

            op::BPLr => {
                ins!(bpl_instr);
                ins!(fix_branch);
            }

            op::BRKn => {
                ins_ns!(push_high_pc);
                ins_ns!(brk_push_low_pc);
                ins_ns!(push_sr);
                ins!(irq_lo_request);
                ins!(irq_hi_request);
                ins!(fetch_next_opcode);
            }

            op::BVCr => {
                ins!(bvc_instr);
                ins!(fix_branch);
            }

            op::BVSr => {
                ins!(bvs_instr);
                ins!(fix_branch);
            }

            op::CLCn => {
                ins!(clc_instr);
            }

            op::CLDn => {
                ins!(cld_instr);
            }

            op::CLIn => {
                ins!(cli_instr);
            }

            op::CLVn => {
                ins!(clv_instr);
            }

            op::CMPz | op::CMPzx | op::CMPa | op::CMPax | op::CMPay | op::CMPix |
            op::CMPiy | op::CMPb => {
                ins!(cmp_instr);
            }

            op::CPXz | op::CPXa | op::CPXb => {
                ins!(cpx_instr);
            }

            op::CPYz | op::CPYa | op::CPYb => {
                ins!(cpy_instr);
            }

            op::DCPz | op::DCPzx | op::DCPa | op::DCPax | op::DCPay | op::DCPix |
            op::DCPiy => {
                // Also known as DCM
                ins_ns!(dcm_instr);
                ins_ns!(put_eff_addr_data_byte);
            }

            op::DECz | op::DECzx | op::DECa | op::DECax => {
                ins_ns!(dec_instr);
                ins_ns!(put_eff_addr_data_byte);
            }

            op::DEXn => {
                ins!(dex_instr);
            }

            op::DEYn => {
                ins!(dey_instr);
            }

            op::EORz | op::EORzx | op::EORa | op::EORax | op::EORay | op::EORix |
            op::EORiy | op::EORb => {
                ins!(eor_instr);
            }

            op::INCz | op::INCzx | op::INCa | op::INCax => {
                ins_ns!(inc_instr);
                ins_ns!(put_eff_addr_data_byte);
            }

            op::INXn => {
                ins!(inx_instr);
            }

            op::INYn => {
                ins!(iny_instr);
            }

            op::ISBz | op::ISBzx | op::ISBa | op::ISBax | op::ISBay | op::ISBix |
            op::ISBiy => {
                // Also known as INS
                ins_ns!(ins_instr);
                ins_ns!(put_eff_addr_data_byte);
            }

            op::JSRw => {
                ins!(waste_cycle);
                ins_ns!(push_high_pc);
                ins_ns!(push_low_pc);
                ins!(fetch_high_addr);
                ins!(jmp_instr);
            }

            op::JMPw | op::JMPi => {
                ins!(jmp_instr);
            }

            op::LASay => {
                ins!(las_instr);
            }

            op::LAXz | op::LAXzy | op::LAXa | op::LAXay | op::LAXix | op::LAXiy => {
                ins!(lax_instr);
            }

            op::LDAz | op::LDAzx | op::LDAa | op::LDAax | op::LDAay | op::LDAix |
            op::LDAiy | op::LDAb => {
                ins!(lda_instr);
            }

            op::LDXz | op::LDXzy | op::LDXa | op::LDXay | op::LDXb => {
                ins!(ldx_instr);
            }

            op::LDYz | op::LDYzx | op::LDYa | op::LDYax | op::LDYb => {
                ins!(ldy_instr);
            }

            op::LSRn => {
                ins!(lsra_instr);
            }

            op::LSRz | op::LSRzx | op::LSRa | op::LSRax => {
                ins_ns!(lsr_instr);
                ins_ns!(put_eff_addr_data_byte);
            }

            op::NOPn | op::NOPn1 | op::NOPn2 | op::NOPn3 | op::NOPn4 | op::NOPn5 | op::NOPn6 |
            op::NOPb | op::NOPb1 | op::NOPb2 | op::NOPb3 | op::NOPb4 |
            op::NOPz | op::NOPz1 | op::NOPz2 |
            op::NOPzx | op::NOPzx1 | op::NOPzx2 | op::NOPzx3 | op::NOPzx4 | op::NOPzx5 |
            op::NOPa | op::NOPax | op::NOPax1 | op::NOPax2 | op::NOPax3 | op::NOPax4 | op::NOPax5 => {
                // NOPb NOPz NOPzx - Also known as SKBn
                // NOPa NOPax      - Also known as SKWn
            }

            op::LXAb => {
                // Also known as OAL
                ins!(oal_instr);
            }

            op::ORAz | op::ORAzx | op::ORAa | op::ORAax | op::ORAay | op::ORAix |
            op::ORAiy | op::ORAb => {
                ins!(ora_instr);
            }

            op::PHAn => {
                ins_ns!(pha_instr);
            }

            op::PHPn => {
                ins_ns!(push_sr);
            }

            op::PLAn => {
                // Should read the value at current stack register.
                // Truely side-effect free
                ins!(waste_cycle);
                ins!(pla_instr);
            }

            op::PLPn => {
                // Should read the value at current stack register.
                // Truely side-effect free
                ins!(waste_cycle);
                ins!(pop_sr);
            }

            op::RLAz | op::RLAzx | op::RLAix | op::RLAa | op::RLAax | op::RLAay |
            op::RLAiy => {
                ins_ns!(rla_instr);
                ins_ns!(put_eff_addr_data_byte);
            }

            op::ROLn => {
                ins!(rola_instr);
            }

            op::ROLz | op::ROLzx | op::ROLa | op::ROLax => {
                ins_ns!(rol_instr);
                ins_ns!(put_eff_addr_data_byte);
            }

            op::RORn => {
                ins!(rora_instr);
            }

            op::RORz | op::RORzx | op::RORa | op::RORax => {
                ins_ns!(ror_instr);
                ins_ns!(put_eff_addr_data_byte);
            }

            op::RRAa | op::RRAax | op::RRAay | op::RRAz | op::RRAzx | op::RRAix |
            op::RRAiy => {
                ins_ns!(rra_instr);
                ins_ns!(put_eff_addr_data_byte);
            }

            op::RTIn => {
                // Should read the value at current stack register.
                // Truely side-effect free
                ins!(waste_cycle);
                ins!(pop_sr);
                ins!(pop_low_pc);
                ins!(pop_high_pc);
                ins!(rti_instr);
            }

            op::RTSn => {
                // Should read the value at current stack register.
                // Truely side-effect free
                ins!(waste_cycle);
                ins!(pop_low_pc);
                ins!(pop_high_pc);
                ins!(rts_instr);
            }

            op::SAXz | op::SAXzy | op::SAXa | op::SAXix => {
                // Also known as AXS
                ins_ns!(axs_instr);
            }

            op::SBCz | op::SBCzx | op::SBCa | op::SBCax | op::SBCay | op::SBCix |
            op::SBCiy | op::SBCb | op::SBCb1 => {
                ins!(sbc_instr);
            }

            op::SBXb => {
                ins!(sbx_instr);
            }

            op::SECn => {
                ins!(sec_instr);
            }

            op::SEDn => {
                ins!(sed_instr);
            }

            op::SEIn => {
                ins!(sei_instr);
            }

            op::SHAay | op::SHAiy => {
                // Also known as AXA
                ins_ns!(axa_instr);
            }

            op::SHSay => {
                // Also known as TAS
                ins_ns!(shs_instr);
            }

            op::SHXay => {
                // Also known as XAS
                ins_ns!(xas_instr);
            }

            op::SHYax => {
                // Also known as SAY
                ins_ns!(say_instr);
            }

            op::SLOz | op::SLOzx | op::SLOa | op::SLOax | op::SLOay | op::SLOix |
            op::SLOiy => {
                // Also known as ASO
                ins_ns!(aso_instr);
                ins_ns!(put_eff_addr_data_byte);
            }

            op::SREz | op::SREzx | op::SREa | op::SREax | op::SREay | op::SREix |
            op::SREiy => {
                // Also known as LSE
                ins_ns!(lse_instr);
                ins_ns!(put_eff_addr_data_byte);
            }

            op::STAz | op::STAzx | op::STAa | op::STAax | op::STAay | op::STAix |
            op::STAiy => {
                ins_ns!(sta_instr);
            }

            op::STXz | op::STXzy | op::STXa => {
                ins_ns!(stx_instr);
            }

            op::STYz | op::STYzx | op::STYa => {
                ins_ns!(sty_instr);
            }

            op::TAXn => {
                ins!(tax_instr);
            }

            op::TAYn => {
                ins!(tay_instr);
            }

            op::TSXn => {
                ins!(tsx_instr);
            }

            op::TXAn => {
                ins!(txa_instr);
            }

            op::TXSn => {
                ins!(txs_instr);
            }

            op::TYAn => {
                ins!(tya_instr);
            }

            _ => {
                legal_instr = false;
            }
        }

        // Missing an addressing mode or implementation makes opcode invalid.
        // These are normally called HLT instructions. In the hardware, the
        // CPU state machine locks up and will never recover
        if !(legal_mode || legal_instr) {
            ins!(invalid_opcode);
        }

        // Check for IRQ triggers or fetch next opcode...
        t[build_cycle].func = interrupts_and_next_opcode;
    }

    t.into_boxed_slice()
}
