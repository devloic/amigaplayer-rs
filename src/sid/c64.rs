// Port of NostalgicPlayer's C# port of libsidplayfp (by Simon White, Antti Lankila and
// Leandro Nini): Source/Ports/LibSidPlayFp/C64/C64.cs, C64Env.cs, C64CpuBus.cs, C64Sid.cs,
// Mmu.cs, C64/Banks/*.cs (SystemRamBank, ZeroRamBank, RomBank, KernalRomBank, BasicRomBank,
// CharacterRomBank, ColorRamBank, DisconnectedBusBank, IOBank, SidBank, ExtraSidBank,
// NullSid), SidEmu.cs and Builders/ReSidFpBuilder/ReSidFpEmu.cs, git e183943.
// Licence: GNU General Public License version 2 (LICENSE-LibSidPlayFp); the upstream
// files carry only NostalgicPlayer's header:
//   "This source, or parts thereof, may be used in any software as long the
//    license of NostalgicPlayer is keep. See the LICENSE file for more information."
//
// Structure changed for Rust: upstream is a web of objects (C64, its environment, the
// MMU, the banks, the chips) that hold references to each other. Here the machine is one
// struct, `C64`; the chips' code is `impl C64` in mos6510.rs, cia.rs and vic.rs, the
// banks are an enum per 4 K (CPU) or 256 byte (I/O) region instead of objects behind an
// interface, and an event is a number the machine dispatches on (sched.rs).
//
// No ROM image is part of this: the KERNAL bank holds what libsidplayfp installs when it
// is given no KERNAL (`KernalRomBank.Set(null)`: the interrupt entries and exits, RTS at
// the jump table, a halt at the RESET entry), the BASIC and character banks are zeros.

use super::cia::{CiaModel, Mos652x};
use super::mos6510::Mos6510;
use super::resid;
use super::sched::{Ev, EventClock, EventScheduler, Phase};
use super::vic::{Mos656x, VicModel};

// The opcodes the KERNAL and BASIC stand-ins are made of (C64/Cpu/Opcodes.cs)
const JSR_W: u8 = 0x20;
const RTI_N: u8 = 0x40;
const RTS_N: u8 = 0x60;
const PHA_N: u8 = 0x48;
const PLA_N: u8 = 0x68;
const TAY_N: u8 = 0xa8;
const LDA_B: u8 = 0xa9;
const TXA_N: u8 = 0x8a;
const TAX_N: u8 = 0xaa;
const NOP_N: u8 = 0xea;
const NOP_A: u8 = 0x0c;
const JMP_W: u8 = 0x4c;
const JMP_I: u8 = 0x6c;
const STA_A: u8 = 0x8d;
const SEI_N: u8 = 0x78;
const TYA_N: u8 = 0x98;

/// C64 model
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Model {
    /// PAL C64
    PalB = 0,
    /// NTSC C64
    NtscM,
    /// Old NTSC C64
    OldNtscM,
    /// C64 Drean
    PalN,
    /// C64 Brasil
    PalM,
}

struct ModelData {
    /// Colorburst frequency in Hertz
    color_burst: f64,
    /// Clock frequency divider
    divider: f64,
    /// Power line frequency in Hertz
    power_freq: f64,
    /// Video chip model
    vic_model: VicModel,
}

// Color burst frequencies:
//
// NTSC  - 3.579545455 MHz = 315/88 MHz
// PAL-B - 4.43361875 MHz = 283.75 * 15625 Hz + 25 Hz
// PAL-M - 3.57561149 MHz
// PAL-N - 3.58205625 MHz
const MODEL_DATA: [ModelData; 5] = [
    ModelData { color_burst: 4433618.75, divider: 18.0, power_freq: 50.0, vic_model: VicModel::Mos6569 }, // PAL-B
    ModelData { color_burst: 3579545.455, divider: 14.0, power_freq: 60.0, vic_model: VicModel::Mos6567R8 }, // NTSC-M
    ModelData { color_burst: 3579545.455, divider: 14.0, power_freq: 60.0, vic_model: VicModel::Mos6567R56A }, // Old NTSC-M
    ModelData { color_burst: 3582056.25, divider: 14.0, power_freq: 50.0, vic_model: VicModel::Mos6572 }, // PAL-N
    ModelData { color_burst: 3575611.49, divider: 14.0, power_freq: 50.0, vic_model: VicModel::Mos6573 }, // PAL-M
];

/// What the CPU reads in a 4 K region (Mmu.cpuReadMap).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReadBank {
    ZeroRam,
    Ram,
    Basic,
    Character,
    Io,
    Kernal,
}

/// Where the CPU writes in a 4 K region (Mmu.cpuWriteMap).
#[derive(Clone, Copy, PartialEq, Eq)]
enum WriteBank {
    ZeroRam,
    Ram,
    Io,
}

/// A 256 byte region of the I/O area (IOBank.map).
#[derive(Clone, Copy, PartialEq, Eq)]
enum IoBank {
    Vic,
    /// the base SID, mirrored each 32 bytes
    Sid,
    ColorRam,
    Cia1,
    Cia2,
    DisconnectedBus,
    /// an ExtraSidBank: index into `C64::extra_sid_banks`
    ExtraSid(u8),
}

/// What a 32 byte slot of an ExtraSidBank maps to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mapped {
    /// the bank that was there before
    Bank(IoBank),
    /// a SID chip: index into `C64::sids`
    Sid(u8),
}

/// Extra SID bank
struct ExtraSidBank {
    /// the I/O page (0..15) it sits at
    page: u8,
    /// SID mapping table. Maps a SID chip base address to a SID or to the underlying
    /// bank. Each 32 bytes another SID chip base address can be assigned to.
    mapper: [Mapped; MAPPER_SIZE],
}

/// Size of mapping table.
const MAPPER_SIZE: usize = 8;

#[inline]
fn mapper_index(address: u16) -> usize {
    (address >> 5) as usize & (MAPPER_SIZE - 1)
}

/// Unused data port bits emulation, as investigated by groepaz:
///
/// - There are 2 different unused bits, 1) the output bits, 2) the input bits
/// - The output bits can be (re)set when the data-direction is set to output
///   for those bits and the output bits will not drop-off to 0.
/// - When the data-direction for the unused bits is set to output then the
///   unused input bits can be (re)set by writing to them, when set to 1 the
///   drop-off timer will start which will cause the unused input bits to drop
///   down to 0 in a certain amount of time.
/// - When an unused input bit already had the drop-off timer running, and is
///   set to 1 again, the drop-off timer will restart.
/// - When an unused bit changes from output to input, and the current output
///   bit is 1, the drop-off timer will restart again
#[derive(Clone, Copy)]
struct DataBit {
    bit: u8,
    /// Cycle that should invalidate the bit
    data_set_clk: EventClock,
    /// Indicates if the bit is in the process of falling off
    is_falling_off: bool,
    /// Value of the bit
    data_set: u8,
}

/// $01 bits 6 and 7 fall-off cycles (1->0), average is about 350 msec for a 6510
/// and about 1500 msec for a 8500.
///
/// NOTE: fall-off cycles are heavily chip- and temperature dependent. As a
///       consequence it is very hard to find suitable realistic values that
///       always work and we can only tweak them based on test cases. (unless we
///       want to make it configurable or emulate temperature over time =))
///
///       It probably makes sense to tweak the values for a warmed up CPU, since
///       this is likely how (old) programs were coded and tested :)
///
/// NOTE: the unused bits of the 6510 seem to be much more temperature dependent
///       and the fall-off time decreases quicker and more drastically than on a
///       8500
///
/// cpuports.prg from the lorenz test suite will fail when the falloff takes more
/// than 1373 cycles. This suggests that he tested on a well warmed up c64 :)
/// He explicitly delays by ~1280 cycles and mentions capacitance, so he probably
/// even was aware of what happens.
const C64_CPU6510_DATA_PORT_FALL_OFF_CYCLES: EventClock = 350000;

impl DataBit {
    const fn new(bit: u8) -> DataBit {
        DataBit { bit, data_set_clk: 0, is_falling_off: false, data_set: 0 }
    }

    fn reset(&mut self) {
        self.is_falling_off = false;
        self.data_set = 0;
    }

    fn read_bit(&mut self, phi2_time: EventClock) -> u8 {
        if self.is_falling_off && self.data_set_clk < phi2_time {
            // Discharge the "capacitor"
            self.reset();
        }
        self.data_set
    }

    fn write_bit(&mut self, phi2_time: EventClock, value: u8) {
        self.data_set_clk = phi2_time + C64_CPU6510_DATA_PORT_FALL_OFF_CYCLES;
        self.data_set = value & (1 << self.bit);
        self.is_falling_off = true;
    }
}

/// One SID chip of the machine: upstream's C64Sid + SidEmu + ReSidFpEmu around the
/// reSIDfp emulation, with the buffer its samples collect in until the player takes them.
pub struct SidChip {
    pub sid: Box<resid::Sid>,
    /// the time (PHI1 clocks) the emulation has been run to
    access_clk: EventClock,
    /// 20 ms of samples at the output rate
    pub buffer: Vec<i16>,
    pub buffer_pos: usize,
    last_poke: [u8; 0x20],
    /// the model set by `model`
    pub chip_model: resid::ChipModel,
}

impl SidChip {
    pub fn new() -> SidChip {
        let mut s = SidChip { sid: Box::new(resid::Sid::new()), access_clk: 0, buffer: Vec::new(), buffer_pos: 0, last_poke: [0; 0x20], chip_model: resid::ChipModel::Mos6581 };
        s.reset_volume(0);
        s
    }

    /// ReSidFpEmu.Reset
    fn reset_volume(&mut self, volume: u8) {
        self.access_clk = 0;
        self.sid.reset();
        self.sid.write(0x18, volume);
    }

    /// C64Sid.Reset
    fn reset(&mut self) {
        self.last_poke = [0; 0x20];
        self.reset_volume(0xf);
    }

    /// ReSidFpEmu.Clock: runs the emulation up to the scheduler's time.
    #[inline]
    pub fn clock(&mut self, now: EventClock) {
        let cycles = now - self.access_clk;
        self.access_clk += cycles;
        let cycles = cycles.clamp(0, u32::MAX as i64) as u32;
        let pos = self.buffer_pos.min(self.buffer.len());
        self.buffer_pos = pos + self.sid.clock(cycles, &mut self.buffer[pos..]);
    }

    /// ReSidFpEmu.Sampling: the 20 ms buffer too.
    pub fn sampling(&mut self, system_clock: f32, freq: f32, method: resid::SamplingMethod) -> Result<(), &'static str> {
        self.sid.set_sampling_parameters(system_clock as f64, method, freq as f64)?;
        // 20 ms buffer
        let buffer_size = ((freq / 1000.0) * 20.0).ceil() as usize;
        self.buffer = vec![0; buffer_size];
        self.buffer_pos = 0;
        Ok(())
    }

    /// ReSidFpEmu.Model
    pub fn model(&mut self, model: resid::ChipModel, digi_boost: bool) {
        match model {
            resid::ChipModel::Mos6581 => self.sid.input(0),
            resid::ChipModel::Csg8580 => self.sid.input(if digi_boost { -32768 } else { 0 }),
        }
        self.sid.set_chip_model(model);
        self.chip_model = model;
    }
}

impl Default for SidChip {
    fn default() -> Self {
        Self::new()
    }
}

/// Commodore 64 emulator core.
///
/// It consists of the following chips:
/// - CPU 6510
/// - VIC-II 6567/6569/6572/6573
/// - CIA 6526/8521
/// - SID 6581/8580
/// - PLA 7700/82S100
/// - Color RAM 2114
/// - System RAM 4164-20/50464-150
/// - Character ROM 2332
/// - Basic ROM 2364
/// - Kernal ROM 2364
pub struct C64 {
    /// System event context
    pub sched: EventScheduler,
    /// CPU
    pub cpu: Mos6510,
    /// CIA1 and CIA2
    pub cia: [Mos652x; 2],
    /// VIC II
    pub vic: Mos656x,
    /// The SID chips: [0] is the base SID, the others the extra ones in the order added
    pub sids: Vec<SidChip>,

    /// System clock frequency
    cpu_frequency: f64,
    /// Number of sources asserting IRQ
    irq_count: i32,
    /// BA state
    old_ba_state: bool,

    // ---- Mmu ----
    // CPU port signals
    lo_ram: bool,
    hi_ram: bool,
    charen: bool,
    /// CPU read memory mapping in 4k chunks
    cpu_read_map: [ReadBank; 16],
    /// CPU write memory mapping in 4k chunks
    cpu_write_map: [WriteBank; 16],
    /// Random seed
    seed: u32,

    // ---- banks ----
    /// C64 RAM area (SystemRamBank)
    ram: Box<[u8; 0x10000]>,
    /// Kernal ROM, $E000-$FFFF: libsidplayfp's stand-in, not Commodore's
    kernal: Box<[u8; 0x2000]>,
    /// RESET vector as the stand-in set it, 0xfffc-0xfffd
    reset_vector: u16,
    /// BASIC ROM, $A000-$BFFF: zeros but for the traps the driver installs
    basic: Box<[u8; 0x2000]>,
    basic_trap: [u8; 3],
    basic_sub_tune: [u8; 11],
    /// Character ROM, $D000-$DFFF: zeros
    character: Box<[u8; 0x1000]>,
    /// Color RAM, $D800-$DBFF: 1K x 4-bit
    color_ram: Box<[u8; 0x400]>,
    /// IO region handler. 4k region, 16 chips, 256b banks
    io_map: [IoBank; 16],
    extra_sid_banks: Vec<ExtraSidBank>,

    // ---- ZeroRamBank ----
    // Unused bits of the data port
    data_bit6: DataBit,
    data_bit7: DataBit,
    // Value written to processor port
    dir: u8,
    data: u8,
    /// Value read from processor port
    data_read: u8,
    /// State of processor port pins
    proc_port_pins: u8,
}

impl Default for C64 {
    fn default() -> Self {
        Self::new()
    }
}

impl C64 {
    pub fn new() -> C64 {
        let mut c = C64 {
            sched: EventScheduler::new(),
            cpu: Mos6510::new(),
            cia: [Mos652x::new(), Mos652x::new()],
            vic: Mos656x::new(),
            sids: Vec::new(),
            cpu_frequency: Self::get_cpu_freq(Model::PalB),
            irq_count: 0,
            old_ba_state: true,
            lo_ram: false,
            hi_ram: false,
            charen: false,
            cpu_read_map: [ReadBank::Ram; 16],
            cpu_write_map: [WriteBank::Ram; 16],
            seed: 3686734,
            ram: Box::new([0; 0x10000]),
            kernal: Box::new([0; 0x2000]),
            reset_vector: 0,
            basic: Box::new([0; 0x2000]),
            basic_trap: [0; 3],
            basic_sub_tune: [0; 11],
            character: Box::new([0; 0x1000]),
            color_ram: Box::new([0; 0x400]),
            io_map: [IoBank::DisconnectedBus; 16],
            extra_sid_banks: Vec::new(),
            data_bit6: DataBit::new(6),
            data_bit7: DataBit::new(7),
            dir: 0,
            data: 0,
            data_read: 0,
            proc_port_pins: 0,
        };
        c.cpu_read_map[0] = ReadBank::ZeroRam;
        c.cpu_write_map[0] = WriteBank::ZeroRam;
        c.reset_io_bank();
        // We need at least some minimal interrupt handling (Player's constructor)
        c.set_kernal_stub();
        // upstream's chips reset themselves in their constructors
        c.cia_reset(0);
        c.cia_reset(1);
        c.sched.reset();
        c
    }

    // ------------------------------------------------------------------
    // C64Env
    // ------------------------------------------------------------------

    /// IRQ trigger signal.
    ///
    /// Calls permitted any time, but normally originated by chips at PHI1
    #[inline]
    pub fn interrupt_irq(&mut self, state: bool) {
        if state {
            if self.irq_count == 0 {
                self.cpu_trigger_irq();
            }
            self.irq_count = self.irq_count.wrapping_add(1);
        } else {
            self.irq_count = self.irq_count.wrapping_sub(1);
            if self.irq_count == 0 {
                self.cpu_clear_irq();
            }
        }
    }

    /// NMI trigger signal.
    ///
    /// Calls permitted any time, but normally originated by chips at PHI1
    #[inline]
    pub fn interrupt_nmi(&mut self) {
        self.cpu_trigger_nmi();
    }

    /// BA signal.
    ///
    /// Calls permitted during PHI1
    #[inline]
    pub fn set_ba(&mut self, state: bool) {
        // Only react to changes in state
        if state == self.old_ba_state {
            return;
        }
        self.old_ba_state = state;
        // Signal changes in BA to interested parties
        self.cpu_set_rdy(state);
    }

    #[inline]
    pub fn lightpen(&mut self, state: bool) {
        if !state {
            self.vic_trigger_lightpen();
        } else {
            self.vic_clear_lightpen();
        }
    }

    // ------------------------------------------------------------------
    // C64
    // ------------------------------------------------------------------

    /// Clock the emulation: fire the next event. `false` when no event is pending
    /// (cannot happen while the VIC-II runs).
    #[inline]
    pub fn clock(&mut self) -> bool {
        let Some(ev) = self.sched.clock() else { return false };
        let e = ev.0;
        if e < Ev::VIC.0 {
            self.cpu_event(ev);
        } else if e < Ev::CIA_BASE {
            self.vic_event(ev);
        } else {
            let k = e - Ev::CIA_BASE;
            self.cia_event((k / Ev::CIA_COUNT) as usize & 1, k % Ev::CIA_COUNT);
        }
        true
    }

    pub fn reset(&mut self) {
        self.sched.reset();

        self.cia_reset(0);
        self.cia_reset(1);
        self.vic_reset();
        // sidBank.Reset() and the extra SID banks'
        for s in &mut self.sids {
            s.reset();
        }
        *self.color_ram = [0; 0x400];
        self.mmu_reset();

        self.irq_count = 0;
        self.old_ba_state = true;
    }

    pub fn reset_cpu(&mut self) {
        self.cpu_reset();
    }

    /// Set the C64 model
    pub fn set_model(&mut self, model: Model) {
        self.cpu_frequency = Self::get_cpu_freq(model);
        self.vic_chip(MODEL_DATA[model as usize].vic_model);

        let rate = (self.cpu_frequency / MODEL_DATA[model as usize].power_freq) as u32;
        self.cia_set_day_of_time_rate(0, rate);
        self.cia_set_day_of_time_rate(1, rate);
    }

    /// Set the CIA model
    pub fn set_cia_model(&mut self, model: CiaModel) {
        self.cia_set_model(0, model);
        self.cia_set_model(1, model);
    }

    /// Get the CPU clock speed
    pub fn get_main_cpu_speed(&self) -> f64 {
        self.cpu_frequency
    }

    /// Set the base SID
    pub fn set_base_sid(&mut self, s: SidChip) {
        self.sids.clear();
        self.sids.push(s);
    }

    /// Add an extra SID
    pub fn add_extra_sid(&mut self, s: SidChip, address: u16) -> bool {
        // Check for valid address in the IO area range ($dxxx)
        if address & 0xf000 != 0xd000 {
            return false;
        }

        let idx = (address >> 8) as usize & 0xf;

        // Only allow second SID chip in SID area ($d400-$d7ff)
        // or IO area ($de00-$dfff)
        if idx < 0x4 || (idx > 0x7 && idx < 0xe) {
            return false;
        }
        if self.sids.is_empty() || self.sids.len() >= 3 {
            return false;
        }
        let sid_index = self.sids.len() as u8;
        self.sids.push(s);

        // Add new SID bank
        let bank = match self.extra_sid_banks.iter().position(|b| b.page as usize == idx) {
            Some(b) => b,
            None => {
                self.extra_sid_banks.push(ExtraSidBank { page: idx as u8, mapper: [Mapped::Bank(self.io_map[idx]); MAPPER_SIZE] });
                let b = self.extra_sid_banks.len() - 1;
                self.io_map[idx] = IoBank::ExtraSid(b as u8);
                b
            }
        };
        self.extra_sid_banks[bank].mapper[mapper_index(address)] = Mapped::Sid(sid_index);
        true
    }

    /// Remove all the SIDs
    pub fn clear_sids(&mut self) {
        self.sids.clear();
        self.reset_io_bank();
        self.extra_sid_banks.clear();
    }

    /// The time in PHI1 clocks.
    #[inline]
    pub fn time(&self) -> EventClock {
        self.sched.get_time(Phase::Phi1)
    }

    /// Runs every SID's emulation up to now (their samples go to their buffers).
    pub fn clock_sids(&mut self) {
        let now = self.sched.get_time(Phase::Phi1);
        for s in &mut self.sids {
            s.clock(now);
        }
    }

    fn get_cpu_freq(model: Model) -> f64 {
        // The crystal clock that drives the VIC II chip is four times
        // the color burst frequency
        let crystal_freq = MODEL_DATA[model as usize].color_burst * 4.0;

        // The VIC II produces the two-phase system clock
        // by running the input clock through a divider
        crystal_freq / MODEL_DATA[model as usize].divider
    }

    fn reset_io_bank(&mut self) {
        self.io_map = [
            IoBank::Vic,
            IoBank::Vic,
            IoBank::Vic,
            IoBank::Vic,
            IoBank::Sid,
            IoBank::Sid,
            IoBank::Sid,
            IoBank::Sid,
            IoBank::ColorRam,
            IoBank::ColorRam,
            IoBank::ColorRam,
            IoBank::ColorRam,
            IoBank::Cia1,
            IoBank::Cia2,
            IoBank::DisconnectedBus,
            IoBank::DisconnectedBus,
        ];
    }

    // ------------------------------------------------------------------
    // Mmu (IPla, ISidMemory, the CPU's bus)
    // ------------------------------------------------------------------

    fn set_cpu_port(&mut self, state: u8) {
        self.lo_ram = state & 1 != 0;
        self.hi_ram = state & 2 != 0;
        self.charen = state & 4 != 0;

        self.update_mapping_phi2();
    }

    /// This should actually return last byte read from VIC but since
    /// the VIC emulation currently does not fetch any values from memory
    /// we return a pseudo random value
    fn get_last_read_byte(&mut self) -> u8 {
        self.seed = self.seed.wrapping_mul(1664525).wrapping_add(1013904223);
        self.seed as u8
    }

    #[inline]
    fn get_phi2_time(&self) -> EventClock {
        self.sched.get_time(Phase::Phi2)
    }

    /// A byte of RAM, whatever is mapped over it.
    pub fn peek_ram(&self, addr: u16) -> u8 {
        self.ram[addr as usize]
    }

    /// Write one byte to memory
    pub fn write_mem_byte(&mut self, addr: u16, value: u8) {
        self.ram[addr as usize] = value;
    }

    /// Write two contiguous bytes to memory
    pub fn write_mem_word(&mut self, addr: u16, value: u16) {
        self.ram[addr as usize] = value as u8;
        self.ram[addr.wrapping_add(1) as usize] = (value >> 8) as u8;
    }

    /// Fill RAM area with a constant value
    pub fn fill_ram_value(&mut self, start: u16, value: u8, size: usize) {
        let start = start as usize;
        let end = (start + size).min(0x10000);
        self.ram[start..end].fill(value);
    }

    /// Copy a buffer into a RAM area (what does not fit below $10000 is left out)
    pub fn fill_ram(&mut self, start: u16, source: &[u8]) {
        let start = start as usize;
        let n = source.len().min(0x10000 - start);
        self.ram[start..start + n].copy_from_slice(&source[..n]);
    }

    /// Change the RESET vector
    pub fn install_reset_hook(&mut self, addr: u16) {
        self.kernal_set16(0xfffc, addr);
    }

    /// Set BASIC warm start address
    pub fn install_basic_trap(&mut self, addr: u16) {
        self.basic[0xa7ae & 0x1fff] = JMP_W;
        self.basic_set16(0xa7af, addr);
    }

    /// Set the start tune
    pub fn set_basic_sub_tune(&mut self, tune: u8) {
        self.basic[0xbf53 & 0x1fff] = LDA_B;
        self.basic[0xbf54 & 0x1fff] = tune;
        self.basic[0xbf55 & 0x1fff] = STA_A;
        self.basic_set16(0xbf56, 0x030c);
        self.basic[0xbf58 & 0x1fff] = JSR_W;
        self.basic_set16(0xbf59, 0xa82c);
        self.basic[0xbf5b & 0x1fff] = JMP_W;
        self.basic_set16(0xbf5c, 0xa7b1);
    }

    fn kernal_set(&mut self, address: u16, val: u8) {
        self.kernal[address as usize & 0x1fff] = val;
    }

    fn kernal_set16(&mut self, address: u16, val: u16) {
        self.kernal[address as usize & 0x1fff] = val as u8;
        self.kernal[address.wrapping_add(1) as usize & 0x1fff] = (val >> 8) as u8;
    }

    fn kernal_fill(&mut self, address: u16, data: &[u8; 5]) {
        let a = address as usize & 0x1fff;
        self.kernal[a..a + 5].copy_from_slice(data);
    }

    fn basic_set16(&mut self, address: u16, val: u16) {
        self.basic[address as usize & 0x1fff] = val as u8;
        self.basic[address.wrapping_add(1) as usize & 0x1fff] = (val >> 8) as u8;
    }

    /// KernalRomBank.Set(null), BasicRomBank.Set(null), CharacterRomBank.Set(null): what
    /// libsidplayfp puts in the ROM banks when it is given no ROM images.
    fn set_kernal_stub(&mut self) {
        const SAVE_REGS: [u8; 5] = [PHA_N, TXA_N, PHA_N, TYA_N, PHA_N];
        const RESTORE_REGS: [u8; 5] = [PLA_N, TAY_N, PLA_N, TAX_N, PLA_N];
        // Address of the jump table entries (upstream lists them with the real
        // routines' addresses, which it does not use)
        const KERNAL_FUNCTIONS: [u16; 78] = [
            0xFF81, 0xFF5B, // SCINIT
            0xFF84, 0xFDA3, // IOINIT
            0xFF87, 0xFD50, // RAMTAS
            0xFF8A, 0xFD15, // RESTOR
            0xFF8D, 0xFD1A, // VECTOR
            0xFF90, 0xFE18, // SETMSG
            0xFF93, 0xEDB9, // LSTNSA
            0xFF96, 0xEDC7, // TALKSA
            0xFF99, 0xFE25, // MEMTOP
            0xFF9C, 0xFE34, // MEMBOT
            0xFF9F, 0xEA87, // SCNKEY
            0xFFA2, 0xFE21, // SETTMO
            0xFFA5, 0xEE13, // IECIN.
            0xFFA8, 0xEDDD, // IECOUT
            0xFFAB, 0xEDEF, // UNTALK
            0xFFAE, 0xEDFE, // UNLSTN
            0xFFB1, 0xED0C, // LISTEN
            0xFFB4, 0xED09, // TALK
            0xFFB7, 0xFE07, // READST
            0xFFBA, 0xFE00, // SETLFS
            0xFFBD, 0xFDF9, // SETNAM
            0xFFC0, 0xF34A, // OPEN
            0xFFC3, 0xF291, // CLOSE
            0xFFC6, 0xF20E, // CHKIN
            0xFFC9, 0xF250, // CHKOUT
            0xFFCC, 0xF333, // CLRCHN
            0xFFCF, 0xF157, // CHRIN
            0xFFD2, 0xF1CA, // CHROUT
            0xFFD5, 0xF49E, // LOAD
            0xFFD8, 0xF5DD, // SAVE
            0xFFDB, 0xF6E4, // SETTIM
            0xFFDE, 0xF6DD, // RDTIM
            0xFFE1, 0xF6ED, // STOP
            0xFFE4, 0xF13E, // GETIN
            0xFFE7, 0xF32F, // CLALL
            0xFFEA, 0xF69B, // UDTIM
            0xFFED, 0xE505, // SCREEN
            0xFFF0, 0xE50A, // PLOT
            0xFFF3, 0xE500, // IOBASE
        ];

        self.kernal.fill(NOP_N);

        // IRQ routine
        self.kernal_set(0xea31, JMP_W);
        self.kernal_set16(0xea32, 0xea7e);
        self.kernal_set(0xea7e, NOP_A); // Clear IRQ
        self.kernal_set16(0xea7f, 0xdc0d);
        self.kernal_fill(0xea81, &RESTORE_REGS);
        self.kernal_set(0xea86, RTI_N); // Return from interrupt

        // RESET
        self.kernal_set(0xfce2, 0x02); // Halt

        // NMI entry point
        self.kernal_set(0xfe43, SEI_N);
        self.kernal_set(0xfe44, JMP_I); // Jump to NMI routine (Default: $fe47)
        self.kernal_set16(0xfe45, 0x0318);

        // NMI routine
        self.kernal_fill(0xfe47, &SAVE_REGS);
        self.kernal_fill(0xfebc, &RESTORE_REGS);
        self.kernal_set(0xfec1, RTI_N);

        // IRQ entry point
        self.kernal_fill(0xff48, &SAVE_REGS);
        self.kernal_set(0xff4d, JMP_I); // Jump to IRQ routine (Default: $ea31)
        self.kernal_set16(0xff4e, 0x0314);

        // Hardware vectors
        self.kernal_set16(0xfffa, 0xfe43); // NMI vector $fe43
        self.kernal_set16(0xfffc, 0xfce2); // RESET vector $fce2
        self.kernal_set16(0xfffe, 0xff48); // IRQ/BRK vector $ff48

        // Standard KERNAL functions called by some unclean rips
        // (as upstream: every word of the table, the "real address" column too)
        for addr in KERNAL_FUNCTIONS {
            self.kernal_set(addr, RTS_N);
        }

        // Backup reset vector
        self.reset_vector = 0xfce2;

        // BASIC: backup BASIC warm start (zeros)
        self.basic.fill(0);
        self.basic_trap = [0; 3];
        self.basic_sub_tune = [0; 11];
        self.character.fill(0);
    }

    /// Access memory as seen by CPU
    #[inline]
    pub fn cpu_read(&mut self, addr: u16) -> u8 {
        match self.cpu_read_map[(addr >> 12) as usize] {
            ReadBank::Ram => self.ram[addr as usize],
            ReadBank::ZeroRam => self.zero_ram_peek(addr),
            ReadBank::Kernal => self.kernal[addr as usize & 0x1fff],
            ReadBank::Basic => self.basic[addr as usize & 0x1fff],
            ReadBank::Character => self.character[addr as usize & 0xfff],
            ReadBank::Io => self.io_peek(self.io_map[(addr >> 8) as usize & 0xf], addr),
        }
    }

    /// Access memory as seen by CPU
    #[inline]
    pub fn cpu_write(&mut self, addr: u16, data: u8) {
        match self.cpu_write_map[(addr >> 12) as usize] {
            WriteBank::Ram => self.ram[addr as usize] = data,
            WriteBank::ZeroRam => self.zero_ram_poke(addr, data),
            WriteBank::Io => self.io_poke(self.io_map[(addr >> 8) as usize & 0xf], addr, data),
        }
    }

    fn mmu_reset(&mut self) {
        // SystemRamBank.Reset: initialize RAM with powerup pattern
        //
        // $0000: 00 00 ff ff ff ff 00 00 00 00 ff ff ff ff 00 00
        // ...
        // $4000: ff ff 00 00 00 00 ff ff ff ff 00 00 00 00 ff ff
        // ...
        // $8000: 00 00 ff ff ff ff 00 00 00 00 ff ff ff ff 00 00
        // ...
        // $c000: ff ff 00 00 00 00 ff ff ff ff 00 00 00 00 ff ff
        let mut byte = 0x00u8;
        for j in (0..0x10000).step_by(0x4000) {
            self.ram[j..j + 0x4000].fill(byte);
            byte = !byte;
            for i in (0x02..0x4000).step_by(0x08) {
                self.ram[j + i..j + i + 4].fill(byte);
            }
        }

        // ZeroRamBank.Reset
        self.data_bit6.reset();
        self.data_bit7.reset();
        self.dir = 0;
        self.data = 0x3f;
        self.data_read = 0x3f;
        self.proc_port_pins = 0x3f;
        self.update_cpu_port();

        // Reset the ROMs to undo the hacks applied
        // Restore original reset vector
        self.kernal_set16(0xfffc, self.reset_vector);
        // Restore original BASIC warm start
        self.basic[0xa7ae & 0x1fff..(0xa7ae & 0x1fff) + 3].copy_from_slice(&self.basic_trap);
        self.basic[0xbf53 & 0x1fff..(0xbf53 & 0x1fff) + 11].copy_from_slice(&self.basic_sub_tune);

        self.lo_ram = false;
        self.hi_ram = false;
        self.charen = false;

        self.update_mapping_phi2();
    }

    fn update_mapping_phi2(&mut self) {
        let k = if self.hi_ram { ReadBank::Kernal } else { ReadBank::Ram };
        self.cpu_read_map[0x0e] = k;
        self.cpu_read_map[0x0f] = k;
        let b = if self.lo_ram && self.hi_ram { ReadBank::Basic } else { ReadBank::Ram };
        self.cpu_read_map[0x0a] = b;
        self.cpu_read_map[0x0b] = b;

        if self.charen && (self.lo_ram || self.hi_ram) {
            self.cpu_read_map[0x0d] = ReadBank::Io;
            self.cpu_write_map[0x0d] = WriteBank::Io;
        } else {
            self.cpu_read_map[0x0d] = if !self.charen && (self.lo_ram || self.hi_ram) { ReadBank::Character } else { ReadBank::Ram };
            self.cpu_write_map[0x0d] = WriteBank::Ram;
        }
    }

    // ------------------------------------------------------------------
    // ZeroRamBank: area backed by RAM, including CPU port addresses 0 and 1.
    //
    // This is bit of a fake. We know that the CPU port is an internal
    // detail of the CPU, and therefore CPU should simply pay the price
    // for reading/writing to $00/$01.
    //
    // However, that would slow down all accesses, which is suboptimal. Therefore
    // we install this little hook to the 4k 0 region to deal with this.
    //
    // Implementation based on VICE code
    // ------------------------------------------------------------------

    fn zero_ram_peek(&mut self, address: u16) -> u8 {
        match address {
            0 => self.dir,
            1 => {
                let mut ret_val = self.data_read;

                // For unused bits in input mode, the value comes from the "capacitor"

                // Set real value of bit 6
                if self.dir & 0x40 == 0 {
                    ret_val &= !0x40;
                    ret_val |= self.data_bit6.read_bit(self.sched.get_time(Phase::Phi2));
                }

                // Set real value of bit 7
                if self.dir & 0x80 == 0 {
                    ret_val &= !0x80;
                    ret_val |= self.data_bit7.read_bit(self.sched.get_time(Phase::Phi2));
                }

                ret_val
            }
            _ => self.ram[address as usize],
        }
    }

    fn zero_ram_poke(&mut self, address: u16, mut value: u8) {
        match address {
            0 => {
                // When switching an unused bit from output (where it contained a
                // stable value) to input mode (where the input is floating), some
                // of the change is transferred to the floating input
                if self.dir != value {
                    let t = self.get_phi2_time();
                    // Check if bit 6 has flipped from 1 to 0
                    if self.dir & 0x40 != 0 && value & 0x40 == 0 {
                        self.data_bit6.write_bit(t, self.data);
                    }

                    // Check if bit 7 has flipped from 1 to 0
                    if self.dir & 0x80 != 0 && value & 0x80 == 0 {
                        self.data_bit7.write_bit(t, self.data);
                    }

                    self.dir = value;
                    self.update_cpu_port();
                }

                value = self.get_last_read_byte();
            }
            1 => {
                let t = self.get_phi2_time();
                // When writing to an unused bit that is output, charge the "capacitor",
                // otherwise don't touch it
                if self.dir & 0x40 != 0 {
                    self.data_bit6.write_bit(t, value);
                }

                if self.dir & 0x80 != 0 {
                    self.data_bit7.write_bit(t, value);
                }

                if self.data != value {
                    self.data = value;
                    self.update_cpu_port();
                }

                value = self.get_last_read_byte();
            }
            _ => {}
        }

        self.ram[address as usize] = value;
    }

    fn update_cpu_port(&mut self) {
        // Update data pins for which direction is OUTPUT
        self.proc_port_pins = (self.proc_port_pins & !self.dir) | (self.data & self.dir);

        self.data_read = (self.data | !self.dir) & (self.proc_port_pins | 0x17);

        self.set_cpu_port((self.data | !self.dir) & 0x07);

        if self.dir & 0x20 == 0 {
            self.data_read &= !0x20;
        }
        // (tape sense is not emulated)
    }

    // ------------------------------------------------------------------
    // IOBank and the banks behind it
    // ------------------------------------------------------------------

    fn io_peek(&mut self, bank: IoBank, addr: u16) -> u8 {
        match bank {
            IoBank::Vic => self.vic_read(addr as u8),
            IoBank::Sid => self.sid_peek(0, addr),
            IoBank::ColorRam => self.color_ram[addr as usize & 0x3ff],
            IoBank::Cia1 => self.cia_read(0, addr as u8),
            IoBank::Cia2 => self.cia_read(1, addr as u8),
            IoBank::DisconnectedBus => self.get_last_read_byte(),
            IoBank::ExtraSid(b) => match self.extra_sid_banks.get(b as usize).map(|x| x.mapper[mapper_index(addr)]) {
                Some(Mapped::Sid(s)) => self.sid_peek(s as usize, addr),
                // (the bank underneath is never an extra SID bank itself)
                Some(Mapped::Bank(IoBank::Sid)) => self.sid_peek(0, addr),
                Some(Mapped::Bank(_)) | None => self.get_last_read_byte(),
            },
        }
    }

    fn io_poke(&mut self, bank: IoBank, addr: u16, data: u8) {
        match bank {
            IoBank::Vic => self.vic_write(addr as u8, data),
            IoBank::Sid => self.sid_poke(0, addr, data),
            IoBank::ColorRam => self.color_ram[addr as usize & 0x3ff] = data & 0xf,
            IoBank::Cia1 => self.cia_write(0, addr as u8, data),
            IoBank::Cia2 => self.cia_write(1, addr as u8, data),
            IoBank::DisconnectedBus => {}
            IoBank::ExtraSid(b) => match self.extra_sid_banks.get(b as usize).map(|x| x.mapper[mapper_index(addr)]) {
                Some(Mapped::Sid(s)) => self.sid_poke(s as usize, addr, data),
                Some(Mapped::Bank(IoBank::Sid)) => self.sid_poke(0, addr, data),
                Some(Mapped::Bank(_)) | None => {}
            },
        }
    }

    /// C64Sid.Peek -> ReSidFpEmu.Read; without the chip, NullSid's 0xff.
    fn sid_peek(&mut self, n: usize, addr: u16) -> u8 {
        let now = self.sched.get_time(Phase::Phi1);
        match self.sids.get_mut(n) {
            Some(s) => {
                s.clock(now);
                s.sid.read(addr as u8 & 0x1f)
            }
            None => 0xff,
        }
    }

    /// C64Sid.Poke -> SidEmu.WriteReg -> ReSidFpEmu.Write
    fn sid_poke(&mut self, n: usize, addr: u16, data: u8) {
        let now = self.sched.get_time(Phase::Phi1);
        if let Some(s) = self.sids.get_mut(n) {
            s.last_poke[addr as usize & 0x1f] = data;
            s.clock(now);
            s.sid.write(addr as u8 & 0x1f, data);
        }
    }
}
