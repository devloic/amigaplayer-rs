use super::apu::*;
use super::cpu::*;
use super::*;
use crate::aiff::Stream;

// ------------------------------------------------------------------ 6502

struct Ram {
    mem: Vec<u8>,
}

impl Bus for Ram {
    fn read(&mut self, a: u16) -> u8 {
        self.mem[a as usize]
    }
    fn write(&mut self, a: u16, v: u8) {
        self.mem[a as usize] = v;
    }
}

/// Runs `code` at $8000 until PC reaches its end; returns the CPU, the memory and the cycles.
fn run(code: &[u8], setup: impl FnOnce(&mut Cpu, &mut Ram)) -> (Cpu, Ram, u64) {
    let mut m = Ram { mem: vec![0; 65536] };
    m.mem[0x8000..0x8000 + code.len()].copy_from_slice(code);
    let mut c = Cpu::new();
    c.pc = 0x8000;
    c.p = U;
    setup(&mut c, &mut m);
    let end = 0x8000 + code.len() as u16;
    for _ in 0..10_000 {
        if c.pc == end || c.jammed {
            break;
        }
        c.step(&mut m);
    }
    (c.clone(), m, c.cycles)
}

/// The cycles of the one instruction `code`.
fn cycles(code: &[u8], setup: impl FnOnce(&mut Cpu, &mut Ram)) -> u64 {
    let mut m = Ram { mem: vec![0; 65536] };
    m.mem[0x8000..0x8000 + code.len()].copy_from_slice(code);
    let mut c = Cpu::new();
    c.pc = 0x8000;
    setup(&mut c, &mut m);
    c.step(&mut m) as u64
}

#[test]
fn cpu_adc_sbc_flags() {
    // CLC; LDA #$7F; ADC #$01: signed overflow into the sign
    let (c, _, _) = run(&[0x18, 0xA9, 0x7F, 0x69, 0x01], |_, _| {});
    assert_eq!((c.a, c.p & (N | V | Z | C)), (0x80, N | V));
    // CLC; LDA #$FF; ADC #$01: zero and carry
    let (c, _, _) = run(&[0x18, 0xA9, 0xFF, 0x69, 0x01], |_, _| {});
    assert_eq!((c.a, c.p & (N | V | Z | C)), (0x00, Z | C));
    // SEC; LDA #$50; SBC #$F0: $60 with a borrow (C clear), no overflow
    let (c, _, _) = run(&[0x38, 0xA9, 0x50, 0xE9, 0xF0], |_, _| {});
    assert_eq!((c.a, c.p & (N | V | Z | C)), (0x60, 0));
    // SEC; LDA #$50; SBC #$B0: 80 - (-80) overflows
    let (c, _, _) = run(&[0x38, 0xA9, 0x50, 0xE9, 0xB0], |_, _| {});
    assert_eq!((c.a, c.p & (N | V | Z | C)), (0xA0, N | V));
    // CLC; LDA #$05; SBC #$03 (borrow in): 1, carry set
    let (c, _, _) = run(&[0x18, 0xA9, 0x05, 0xE9, 0x03], |_, _| {});
    assert_eq!((c.a, c.p & C), (0x01, C));
    // the unofficial SBC $EB is SBC #
    let (c, _, _) = run(&[0x38, 0xA9, 0x10, 0xEB, 0x01], |_, _| {});
    assert_eq!(c.a, 0x0F);
}

#[test]
fn cpu_no_decimal_mode() {
    // SED; CLC; LDA #$09; ADC #$01: binary $0A on the 2A03 (BCD would give $10); D stays set
    let (c, _, _) = run(&[0xF8, 0x18, 0xA9, 0x09, 0x69, 0x01], |_, _| {});
    assert_eq!(c.a, 0x0A);
    assert_ne!(c.p & D, 0);
    // SED; SEC; LDA #$10; SBC #$01: $0F
    let (c, _, _) = run(&[0xF8, 0x38, 0xA9, 0x10, 0xE9, 0x01], |_, _| {});
    assert_eq!(c.a, 0x0F);
}

#[test]
fn cpu_compare_bit_shifts() {
    // LDA #$40; CMP #$40: Z and C
    let (c, _, _) = run(&[0xA9, 0x40, 0xC9, 0x40], |_, _| {});
    assert_eq!(c.p & (N | Z | C), Z | C);
    // LDX #$01; CPX #$02: N (the difference $FF), no carry
    let (c, _, _) = run(&[0xA2, 0x01, 0xE0, 0x02], |_, _| {});
    assert_eq!(c.p & (N | Z | C), N);
    // BIT $10 with $C0 there and A = $01: N and V from memory, Z from the AND
    let (c, _, _) = run(&[0xA9, 0x01, 0x24, 0x10], |_, m| m.mem[0x10] = 0xC0);
    assert_eq!(c.p & (N | V | Z), N | V | Z);
    // SEC; LDA #$81; ROR A: $C0, carry from bit 0
    let (c, _, _) = run(&[0x38, 0xA9, 0x81, 0x6A], |_, _| {});
    assert_eq!((c.a, c.p & (C | N)), (0xC0, C | N));
    // ASL $20 ($80): 0, carry, zero
    let (c, m, _) = run(&[0x06, 0x20], |_, m| m.mem[0x20] = 0x80);
    assert_eq!((m.mem[0x20], c.p & (C | Z)), (0, C | Z));
    // LDX #$FF; INC $10,X wraps within the zero page (to $0F)
    let (_, m, _) = run(&[0xA2, 0xFF, 0xF6, 0x10], |_, _| {});
    assert_eq!(m.mem[0x0F], 1);
}

#[test]
fn cpu_addressing_modes() {
    // LDX #$04; LDA ($10,X): pointer at $14 -> $1234
    let (c, _, _) = run(&[0xA2, 0x04, 0xA1, 0x10], |_, m| {
        m.mem[0x14] = 0x34;
        m.mem[0x15] = 0x12;
        m.mem[0x1234] = 0x77;
    });
    assert_eq!(c.a, 0x77);
    // LDY #$10; LDA ($FF),Y: the pointer's high byte from $00 (zero-page wrap)
    let (c, _, _) = run(&[0xA0, 0x10, 0xB1, 0xFF], |_, m| {
        m.mem[0xFF] = 0xF8;
        m.mem[0x00] = 0x20;
        m.mem[0x2108] = 0x55;
    });
    assert_eq!(c.a, 0x55);
    // LDX #$01; LDY $FF,X... LDX $FF,Y: zero page,Y wraps
    let (c, _, _) = run(&[0xA0, 0x02, 0xB6, 0xFF], |_, m| m.mem[0x01] = 0x99);
    assert_eq!(c.x, 0x99);
    // JMP ($02FF): the high byte from $0200, not $0300
    let mut m = Ram { mem: vec![0; 65536] };
    m.mem[0x8000..0x8003].copy_from_slice(&[0x6C, 0xFF, 0x02]);
    (m.mem[0x2FF], m.mem[0x200], m.mem[0x300]) = (0x34, 0x12, 0x56);
    let mut c = Cpu::new();
    c.pc = 0x8000;
    c.step(&mut m);
    assert_eq!(c.pc, 0x1234);
}

#[test]
fn cpu_stack_and_subroutines() {
    // JSR $8006; (at $8003) LDA #$22; BRK-free end... the subroutine: LDX #$11; RTS
    let code = [0x20, 0x06, 0x80, 0xA9, 0x22, 0x60, 0xA2, 0x11, 0x60];
    let mut m = Ram { mem: vec![0; 65536] };
    m.mem[0x8000..0x8000 + code.len()].copy_from_slice(&code);
    let mut c = Cpu::new();
    c.pc = 0x8000;
    c.s = 0xFF;
    assert_eq!(c.step(&mut m), 6);
    assert_eq!((c.pc, c.s), (0x8006, 0xFD));
    // the return address pushed is the JSR's last byte
    assert_eq!((m.mem[0x1FF], m.mem[0x1FE]), (0x80, 0x02));
    c.step(&mut m);
    assert_eq!(c.step(&mut m), 6);
    assert_eq!((c.pc, c.s, c.x), (0x8003, 0xFF, 0x11));
    // PHP pushes B and bit 5 set; PLP ignores B (and keeps bit 5 set)
    let (c, m, _) = run(&[0x08, 0xA9, 0xFF, 0x48, 0x28], |c, _| {
        c.s = 0xFF;
        c.p = C;
    });
    assert_eq!(m.mem[0x1FF], C | B | U);
    assert_eq!(c.p, 0xFF & !B);
    // PLA sets N / Z
    let (c, _, _) = run(&[0xA9, 0x80, 0x48, 0xA9, 0x00, 0x68], |c, _| c.s = 0xFF);
    assert_eq!((c.a, c.p & (N | Z)), (0x80, N));
    // BRK: vector at $FFFE, pushes PC + 2 and P with B; RTI back
    let (c, m, _) = run(&[0x00, 0xEA], |c, m| {
        c.s = 0xFF;
        m.mem[0xFFFE] = 0x00;
        m.mem[0xFFFF] = 0x90;
        m.mem[0x9000] = 0x40; // RTI
    });
    assert_eq!(c.pc, 0x8002);
    assert_eq!((m.mem[0x1FF], m.mem[0x1FE]), (0x80, 0x02));
    assert_ne!(m.mem[0x1FD] & B, 0);
}

#[test]
fn cpu_cycle_counts() {
    assert_eq!(cycles(&[0xA9, 0x00], |_, _| {}), 2); // LDA #
    assert_eq!(cycles(&[0xA5, 0x00], |_, _| {}), 3); // LDA zp
    assert_eq!(cycles(&[0xBD, 0x10, 0x20], |c, _| c.x = 1), 4); // LDA abs,X
    assert_eq!(cycles(&[0xBD, 0xFF, 0x20], |c, _| c.x = 1), 5); // ... crossing a page
    assert_eq!(cycles(&[0x9D, 0x10, 0x20], |c, _| c.x = 1), 5); // STA abs,X: always 5
    assert_eq!(cycles(&[0xB1, 0x10], |c, m| {
        m.mem[0x10] = 0xFF;
        c.y = 1;
    }), 6); // LDA (zp),Y crossing
    assert_eq!(cycles(&[0x91, 0x10], |_, _| {}), 6); // STA (zp),Y
    assert_eq!(cycles(&[0xA1, 0x10], |_, _| {}), 6); // LDA (zp,X)
    assert_eq!(cycles(&[0xFE, 0x00, 0x20], |_, _| {}), 7); // INC abs,X
    assert_eq!(cycles(&[0xE6, 0x00], |_, _| {}), 5); // INC zp
    assert_eq!(cycles(&[0x0E, 0x00, 0x20], |_, _| {}), 6); // ASL abs
    assert_eq!(cycles(&[0x4C, 0x00, 0x20], |_, _| {}), 3); // JMP
    assert_eq!(cycles(&[0x6C, 0x00, 0x20], |_, _| {}), 5); // JMP ()
    assert_eq!(cycles(&[0x48], |_, _| {}), 3); // PHA
    assert_eq!(cycles(&[0x68], |_, _| {}), 4); // PLA
    assert_eq!(cycles(&[0x00], |_, _| {}), 7); // BRK
    // branches: 2 not taken, 3 taken, 4 taken across a page
    assert_eq!(cycles(&[0xD0, 0x10], |c, _| c.p = Z), 2);
    assert_eq!(cycles(&[0xD0, 0x10], |c, _| c.p = 0), 3);
    assert_eq!(cycles(&[0xD0, 0x80], |c, _| c.p = 0), 4);
    // unofficial: DCP abs,Y 7, LAX (zp),Y 5, SLO (zp,X) 8, NOP abs,X 4/5
    assert_eq!(cycles(&[0xDB, 0x00, 0x20], |_, _| {}), 7);
    assert_eq!(cycles(&[0xB3, 0x10], |_, _| {}), 5);
    assert_eq!(cycles(&[0x03, 0x10], |_, _| {}), 8);
    assert_eq!(cycles(&[0x1C, 0x00, 0x20], |_, _| {}), 4);
    assert_eq!(cycles(&[0x1C, 0xFF, 0x20], |c, _| c.x = 1), 5);
}

#[test]
fn cpu_unofficial_opcodes() {
    // LAX $10
    let (c, _, _) = run(&[0xA7, 0x10], |_, m| m.mem[0x10] = 0x85);
    assert_eq!((c.a, c.x, c.p & N), (0x85, 0x85, N));
    // LDA #$F0; LDX #$3C; SAX $20: $30
    let (_, m, _) = run(&[0xA9, 0xF0, 0xA2, 0x3C, 0x87, 0x20], |_, _| {});
    assert_eq!(m.mem[0x20], 0x30);
    // DCP $10 ($41 -> $40), A = $40: equal
    let (c, m, _) = run(&[0xA9, 0x40, 0xC7, 0x10], |_, m| m.mem[0x10] = 0x41);
    assert_eq!((m.mem[0x10], c.p & (Z | C)), (0x40, Z | C));
    // ISC $10 ($0F -> $10), SEC, A = $20: $10
    let (c, m, _) = run(&[0x38, 0xA9, 0x20, 0xE7, 0x10], |_, m| m.mem[0x10] = 0x0F);
    assert_eq!((m.mem[0x10], c.a), (0x10, 0x10));
    // SLO $10 ($81 -> $02, carry), A = $01: $03
    let (c, m, _) = run(&[0xA9, 0x01, 0x07, 0x10], |_, m| m.mem[0x10] = 0x81);
    assert_eq!((m.mem[0x10], c.a, c.p & C), (0x02, 0x03, C));
    // RLA, SRE, RRA
    let (c, _, _) = run(&[0x18, 0xA9, 0xFF, 0x27, 0x10], |_, m| m.mem[0x10] = 0x40);
    assert_eq!(c.a, 0x80);
    let (c, _, _) = run(&[0xA9, 0x0F, 0x47, 0x10], |_, m| m.mem[0x10] = 0x02);
    assert_eq!(c.a, 0x0E);
    let (c, _, _) = run(&[0x18, 0xA9, 0x10, 0x67, 0x10], |_, m| m.mem[0x10] = 0x03);
    // ROR $03 -> $01 with carry 1; ADC: $10 + $01 + 1
    assert_eq!(c.a, 0x12);
    // ANC #$80: C from N
    let (c, _, _) = run(&[0xA9, 0xFF, 0x0B, 0x80], |_, _| {});
    assert_eq!((c.a, c.p & (N | C)), (0x80, N | C));
    // ALR #$FF on $03: $01, carry
    let (c, _, _) = run(&[0xA9, 0x03, 0x4B, 0xFF], |_, _| {});
    assert_eq!((c.a, c.p & C), (0x01, C));
    // ARR #$FF on $C0, carry clear: $60, C = bit 6 (1), V = bit 6 ^ bit 5 (0)
    let (c, _, _) = run(&[0x18, 0xA9, 0xC0, 0x6B, 0xFF], |_, _| {});
    assert_eq!((c.a, c.p & (C | V)), (0x60, C));
    // AXS #$02: X = (A & X) - 2, carry (no borrow)
    let (c, _, _) = run(&[0xA9, 0x0F, 0xA2, 0x07, 0xCB, 0x02], |_, _| {});
    assert_eq!((c.x, c.p & C), (0x05, C));
    // the NOPs skip their operand
    let (c, _, _) = run(&[0x80, 0x55, 0x04, 0x10, 0x0C, 0x00, 0x20, 0x1A, 0xA9, 0x01], |_, _| {});
    assert_eq!((c.a, c.pc), (0x01, 0x800A));
    // STP stops the CPU
    let (c, _, _) = run(&[0xA9, 0x01, 0x02, 0xA9, 0x02], |_, _| {});
    assert!(c.jammed);
    assert_eq!(c.a, 0x01);
}

// ------------------------------------------------------------------ APU

fn clocks(a: &mut Apu, n: u32) {
    for _ in 0..n {
        a.clock();
    }
}

#[test]
fn apu_tables_from_the_docs() {
    // the wiki's length table, noise and DMC periods
    assert_eq!((LENGTH[0], LENGTH[1], LENGTH[8], LENGTH[0x18], LENGTH[0x1F]), (10, 254, 160, 192, 30));
    assert_eq!((NOISE_NTSC[0], NOISE_NTSC[8], NOISE_NTSC[15]), (4, 202, 4068));
    assert_eq!((NOISE_PAL[2], NOISE_PAL[15]), (14, 3778));
    assert_eq!((DMC_NTSC[0], DMC_NTSC[15], DMC_PAL[0], DMC_PAL[15]), (428, 54, 398, 50));
    // loaded through the registers
    let mut a = Apu::new(false);
    a.write(0x4015, 0x0F);
    a.write(0x4003, 1 << 3);
    a.write(0x4007, 0x1F << 3);
    a.write(0x400B, 0x18 << 3);
    a.write(0x400F, 0);
    assert_eq!((a.pulse[0].length, a.pulse[1].length, a.tri.length, a.noise.length), (254, 30, 192, 10));
    a.write(0x400E, 0x88);
    assert_eq!((a.noise.period, a.noise.mode), (202, true));
    a.write(0x4010, 0x0F);
    a.write(0x4012, 0x01);
    a.write(0x4013, 0x01);
    assert_eq!((a.dmc.period, a.dmc.sample_addr, a.dmc.sample_len), (54, 0xC040, 17));
    let mut p = Apu::new(true);
    p.write(0x400E, 0x0F);
    p.write(0x4010, 0x00);
    assert_eq!((p.noise.period, p.dmc.period), (3778, 398));
}

#[test]
fn apu_length_counter_enable_halt_and_status() {
    let mut a = Apu::new(false);
    // disabled: no load
    a.write(0x4003, 0x08);
    assert_eq!(a.pulse[0].length, 0);
    a.write(0x4015, 0x01);
    a.write(0x4003, 3 << 3); // index 3: 2
    assert_eq!(a.read_status() & 1, 1);
    // half-frame clocks at 14913 and 29829 (4-step mode)
    // (the write takes effect on the 3rd cycle, on an even one)
    a.write(0x4017, 0x40);
    clocks(&mut a, 3);
    clocks(&mut a, 14912);
    assert_eq!(a.pulse[0].length, 2);
    clocks(&mut a, 1);
    assert_eq!(a.pulse[0].length, 1);
    clocks(&mut a, 29829 - 14913);
    assert_eq!(a.pulse[0].length, 0);
    assert_eq!(a.read_status() & 1, 0);
    // halt (bit 5 of $4000) keeps it
    a.write(0x4000, 0x20);
    a.write(0x4003, 3 << 3);
    clocks(&mut a, 2 * 29830);
    assert_eq!(a.pulse[0].length, 2);
    // clearing the enable bit zeroes it
    a.write(0x4015, 0);
    assert_eq!(a.pulse[0].length, 0);
}

#[test]
fn apu_frame_counter_irq_and_five_step() {
    let mut a = Apu::new(false);
    a.write(0x4017, 0x00);
    clocks(&mut a, 3 + 29828);
    assert_eq!(a.read_status() & 0x40, 0);
    clocks(&mut a, 1);
    assert_eq!(a.read_status() & 0x40, 0x40);
    // the read cleared it
    assert_eq!(a.read_status() & 0x40, 0);
    // inhibit: never set
    a.write(0x4017, 0x40);
    clocks(&mut a, 3 * 29830);
    assert_eq!(a.read_status() & 0x40, 0);
    // 5-step: the write clocks both units at once (after 3 or 4 cycles), no IRQ
    let mut a = Apu::new(false);
    a.write(0x4015, 0x01);
    a.write(0x4003, 3 << 3);
    a.write(0x4017, 0x80);
    clocks(&mut a, 4);
    assert_eq!(a.pulse[0].length, 1);
    clocks(&mut a, 37282 * 2);
    assert_eq!(a.read_status() & 0x40, 0);
}

#[test]
fn apu_envelope_and_sweep() {
    let mut a = Apu::new(false);
    a.write(0x4015, 0x01);
    a.write(0x4000, 0x00); // envelope, divider period 0, no loop
    a.write(0x4003, 0x08);
    a.write(0x4017, 0x40);
    clocks(&mut a, 4 + 7457);
    assert_eq!(a.pulse[0].env.volume(), 15); // the start flag: decay = 15
    clocks(&mut a, 14913 - 7457);
    assert_eq!(a.pulse[0].env.volume(), 14);
    // constant volume
    a.write(0x4000, 0x17);
    assert_eq!(a.pulse[0].env.volume(), 7);
    // sweep negation: pulse 1 ones' complement, pulse 2 two's
    a.write(0x4002, 0x00);
    a.write(0x4003, 0x01); // period $100
    a.write(0x4001, 0x89); // enabled, period 0, negate, shift 1
    assert_eq!(a.pulse[0].target(), 0x7F);
    a.write(0x4006, 0x00);
    a.write(0x4007, 0x01);
    a.write(0x4005, 0x89);
    assert_eq!(a.pulse[1].target(), 0x80);
    // a target above $7FF mutes, even with the sweep off
    a.write(0x4001, 0x01);
    a.write(0x4002, 0xF0);
    a.write(0x4003, 0x07);
    assert!(a.pulse[0].muted());
    // period < 8 mutes
    a.write(0x4001, 0x08);
    a.write(0x4002, 0x07);
    a.write(0x4003, 0x00);
    assert!(a.pulse[0].muted());
}

/// Rising edges of `f(apu)` over one second of CPU cycles.
fn edges(a: &mut Apu, f: impl Fn(&Apu) -> u8) -> u32 {
    let (mut n, mut last) = (0, 0);
    for _ in 0..NTSC_HZ {
        a.clock();
        let o = f(a);
        if o > last && last == 0 {
            n += 1;
        }
        last = o;
    }
    n
}

#[test]
fn apu_pulse_and_triangle_pitch() {
    // pulse: fCPU / (16 (t + 1)); t = $FD: 440.4 Hz
    let mut a = Apu::new(false);
    a.write(0x4015, 0x0F);
    a.write(0x4000, 0xBF); // duty 2, halt, constant 15
    a.write(0x4001, 0x08);
    a.write(0x4002, 0xFD);
    a.write(0x4003, 0x00);
    let n = edges(&mut a, |a| a.pulse[0].output());
    let want = NTSC_HZ as f64 / (16.0 * 254.0);
    assert!((n as f64 - want).abs() <= 1.0, "{n} {want}");
    // triangle: fCPU / (32 (t + 1)), one octave lower; t = $FD
    let mut a = Apu::new(false);
    a.write(0x4015, 0x04);
    a.write(0x4008, 0xFF); // control (halt), linear reload 127
    a.write(0x400A, 0xFD);
    a.write(0x400B, 0x00);
    let n = edges(&mut a, |a| (a.tri.output() == 15) as u8);
    let want = NTSC_HZ as f64 / (32.0 * 254.0);
    assert!((n as f64 - want).abs() <= 1.0, "{n} {want}");
    // the linear counter stops the triangle: control clear, reload 1
    let mut a = Apu::new(false);
    a.write(0x4015, 0x04);
    a.write(0x4008, 0x01);
    a.write(0x400A, 0x10);
    a.write(0x400B, 0x08);
    clocks(&mut a, 30000);
    let s = a.tri.seq;
    clocks(&mut a, 1000);
    assert_eq!(a.tri.seq, s);
}

#[test]
fn apu_noise_and_dmc() {
    // the LFSR: 15 bits, mode 0 period 32767 steps
    let mut a = Apu::new(false);
    a.write(0x400E, 0x00); // period 4
    let start = a.noise.lfsr;
    let mut n = 0u32;
    loop {
        clocks(&mut a, 4);
        n += 1;
        if a.noise.lfsr == start || n > 40000 {
            break;
        }
    }
    assert_eq!(n, 32767);
    // the DMC: a $FF sample raises the level by 2 per bit up to 126 at most, then stays
    let mut a = Apu::new(false);
    a.write(0x4011, 0x70);
    a.write(0x4010, 0x0F);
    a.write(0x4012, 0x00);
    a.write(0x4013, 0x00); // 1 byte
    a.write(0x4015, 0x10);
    assert_eq!(a.dmc.wants(), Some(0xC000));
    a.dmc.fill(0xFF);
    assert_eq!(a.dmc.remaining, 0);
    clocks(&mut a, 54 * 20);
    assert!(a.dmc.level >= 0x7E, "{}", a.dmc.level);
    assert!(a.dmc.level <= 127);
    // the IRQ flag at the end of a sample when enabled, cleared by a $4015 write
    let mut a = Apu::new(false);
    a.write(0x4010, 0x80);
    a.write(0x4013, 0x00);
    a.write(0x4015, 0x10);
    a.dmc.fill(0);
    assert_eq!(a.read_status() & 0x80, 0x80);
    a.write(0x4015, 0x00);
    assert_eq!(a.read_status() & 0x80, 0);
    // the address wraps from $FFFF to $8000
    let mut a = Apu::new(false);
    a.write(0x4012, 0xFF);
    a.write(0x4013, 0x10);
    a.write(0x4015, 0x10);
    a.dmc.addr = 0xFFFF;
    a.dmc.fill(0);
    assert_eq!(a.dmc.addr, 0x8000);
}

#[test]
fn apu_mixer() {
    let tnd = |n: f32| if n == 0.0 { 0.0 } else { 163.67 / (24329.0 / n + 100.0) };
    // at rest the triangle holds 15 (step 0 of its sequence)
    let a = Apu::new(false);
    assert!((a.mix() - tnd(45.0)).abs() < 1e-6);
    let mut a = Apu::new(false);
    a.write(0x4011, 127);
    let want = tnd(45.0 + 127.0);
    assert!((a.mix() - want).abs() < 1e-6);
    a.write(0x4011, 0);
    a.write(0x4015, 0x01);
    a.write(0x4000, 0xBF);
    a.write(0x4002, 0xFD);
    a.write(0x4003, 0x00);
    clocks(&mut a, 600);
    let peak = (0..5000).map(|_| a.clock()).fold(0f32, f32::max);
    assert!((peak - 95.52 / (8128.0 / 15.0 + 100.0) - tnd(45.0)).abs() < 1e-6, "{peak}");
}

// ------------------------------------------------------------------ the file

/// An NSF file: `code` loaded at $8000, INIT and PLAY addresses, `songs`, the bank bytes and
/// the expansion-chip byte.
fn nsf_file(code: &[u8], init: u16, play: u16, songs: u8, banks: [u8; 8], chips: u8) -> Vec<u8> {
    let mut d = vec![0u8; 0x80];
    d[..5].copy_from_slice(MAGIC);
    d[5] = 1;
    d[6] = songs;
    d[7] = 1;
    d[8..10].copy_from_slice(&0x8000u16.to_le_bytes());
    d[0xA..0xC].copy_from_slice(&init.to_le_bytes());
    d[0xC..0xE].copy_from_slice(&play.to_le_bytes());
    d[0x0E..0x0E + 4].copy_from_slice(b"Test");
    d[0x2E..0x2E + 6].copy_from_slice(b"Nobody");
    d[0x4E..0x4E + 3].copy_from_slice(b"<?>");
    d[0x6E..0x70].copy_from_slice(&16666u16.to_le_bytes());
    d[0x70..0x78].copy_from_slice(&banks);
    d[0x78..0x7A].copy_from_slice(&19997u16.to_le_bytes());
    d[0x7B] = chips;
    d.extend_from_slice(code);
    d
}

/// INIT ($8000): stores A at $11 and X at $12, starts pulse 1 at period $0F0 + 2 * song (duty 2,
/// constant volume 15); PLAY: INC $10; RTS.
fn tone_nsf(songs: u8) -> Vec<u8> {
    let code = [
        0x85, 0x11, // STA $11
        0x86, 0x12, // STX $12
        0x0A, // ASL A
        0x18, 0x69, 0xF0, // CLC; ADC #$F0
        0x8D, 0x02, 0x40, // STA $4002
        0xA9, 0x01, 0x8D, 0x15, 0x40, // LDA #1; STA $4015
        0xA9, 0xBF, 0x8D, 0x00, 0x40, // LDA #$BF; STA $4000
        0xA9, 0x08, 0x8D, 0x01, 0x40, // LDA #$08; STA $4001
        0xA9, 0x00, 0x8D, 0x03, 0x40, // LDA #0; STA $4003
        0x60, // RTS
        0xE6, 0x10, 0x60, // PLAY: INC $10; RTS
    ];
    nsf_file(&code, 0x8000, 0x8020, songs, [0; 8], 0)
}

fn render(p: &mut NsfPlayer, secs: f64) -> Vec<f32> {
    let mut v = vec![0f32; (secs * 48000.0) as usize * 2];
    p.render(&mut v);
    v
}

/// The fundamental of the left channel by counting rising zero crossings (after 0.1 s).
fn freq(v: &[f32]) -> f64 {
    let l: Vec<f32> = v.iter().step_by(2).copied().collect();
    let (mut n, mut first, mut last) = (0, 0, 0);
    for i in 4801..l.len() {
        if l[i - 1] < 0.0 && l[i] >= 0.0 {
            if n == 0 {
                first = i;
            }
            last = i;
            n += 1;
        }
    }
    (n - 1) as f64 * 48000.0 / (last - first) as f64
}

#[test]
fn header_and_detection() {
    let d = tone_nsf(3);
    assert!(detect(&d));
    let h = parse(&d).unwrap();
    assert_eq!((h.songs, h.start, h.load, h.init, h.play), (3, 1, 0x8000, 0x8000, 0x8020));
    assert_eq!((h.name.as_str(), h.artist.as_str(), h.copyright.as_str()), ("Test", "Nobody", ""));
    assert_eq!((h.ntsc_us, h.pal_us, h.bankswitched(), h.pal()), (16666, 19997, false, false));
    assert_eq!(crate::open::detect(&d).as_deref(), Some("NSF"));
    // a wrong magic, no song, a load address below $6000, a header only
    let mut x = d.clone();
    x[4] = 0;
    assert!(!detect(&x));
    let mut x = d.clone();
    x[6] = 0;
    assert!(!detect(&x));
    let mut x = d.clone();
    x[9] = 0x50;
    assert!(!detect(&x));
    assert!(!detect(&d[..0x80]));
    // no other replayer's check takes it
    assert!(crate::replay::REPLAYERS.iter().all(|o| !(o.detect)(&d)), "claimed by an open replayer");
    // and NSF's check takes none of the synthetic modules of the other formats
    assert!(!detect(&crate::testdata::minimal_mod()));
}

#[test]
fn expansion_chips_are_refused() {
    let d = nsf_file(&[0x60], 0x8000, 0x8000, 1, [0; 8], 0x01);
    let e = NsfPlayer::new(&d, 48000).err().unwrap();
    assert!(e.contains("VRC6") && e.contains("not emulated"), "{e}");
    let d = nsf_file(&[0x60], 0x8000, 0x8000, 1, [0; 8], 0x24);
    let e = player(&d, 48000).err().unwrap();
    assert!(e.contains("FDS, Sunsoft 5B"), "{e}");
    assert_eq!(crate::open::detect(&d), None);
    // the engine reports the message rather than trying other players
    let mut eng = crate::engine::Engine::new(48000.0);
    let r = eng.play_data(std::path::Path::new("x.nsf"), d);
    assert!(r.is_err_and(|e| e.to_string().contains("FDS")));
}

#[test]
fn init_arguments_play_rate_and_pitch() {
    let d = tone_nsf(3);
    let mut p = NsfPlayer::new(&d, 48000).unwrap();
    assert_eq!(p.subsongs(), (0, 2, 0));
    let v = render(&mut p, 1.0);
    // INIT got A = song (0-based) and X = 0 (NTSC); PLAY ran at 1,000,000 / 16666 Hz
    assert_eq!((p.m.ram[0x11], p.m.ram[0x12]), (0, 0));
    let calls = p.m.ram[0x10];
    assert!((59..=61).contains(&calls), "{calls} PLAY calls in 1 s");
    let f = freq(&v);
    let want = NTSC_HZ as f64 / (16.0 * 241.0);
    assert!((f - want).abs() < 0.5, "{f} Hz, {want} expected");
    // song 3: A = 2, period $0F4
    p.set_subsong(2);
    let v = render(&mut p, 1.0);
    assert_eq!(p.m.ram[0x11], 2);
    let want = NTSC_HZ as f64 / (16.0 * 245.0);
    assert!((freq(&v) - want).abs() < 0.5);
    // a PAL-only tune: X = 1, the PAL clock and rate
    let mut d = d.clone();
    d[0x7A] = 1;
    let mut p = NsfPlayer::new(&d, 48000).unwrap();
    assert!(p.is_pal());
    let v = render(&mut p, 1.0);
    assert_eq!(p.m.ram[0x12], 1);
    assert!((49..=51).contains(&p.m.ram[0x10]));
    let want = PAL_HZ as f64 / (16.0 * 241.0);
    assert!((freq(&v) - want).abs() < 0.5);
    // a dual tune plays as NTSC
    d[0x7A] = 3;
    assert!(!NsfPlayer::new(&d, 48000).unwrap().is_pal());
}

#[test]
fn init_procedure_clears_ram() {
    // INIT: LDA $0300 -> $11; LDA $6000 -> $12; set $0300 and $6000; PLAY: RTS
    let code = [0xAD, 0x00, 0x03, 0x85, 0x11, 0xAD, 0x00, 0x60, 0x85, 0x12, 0xA9, 0x55, 0x8D, 0x00, 0x03, 0x8D, 0x00, 0x60, 0x60];
    let d = nsf_file(&code, 0x8000, 0x8012, 2, [0; 8], 0);
    let mut p = NsfPlayer::new(&d, 48000).unwrap();
    render(&mut p, 0.1);
    assert_eq!(p.m.ram[0x300], 0x55);
    p.set_subsong(1);
    render(&mut p, 0.1);
    assert_eq!((p.m.ram[0x11], p.m.ram[0x12]), (0, 0));
    // the stack: INIT is called with S at $FF minus the return address
    assert!(p.cpu.s == 0xFF);
}

#[test]
fn bankswitching() {
    // three 4 KB banks loaded at $8000 (bank n's first byte at $x000 + 1 is n + $A0).
    // INIT (in bank 0, at $8000): LDA $9001 -> $11; LDA #2; STA $5FF9; LDA $9001 -> $12; RTS
    let mut img = vec![0u8; 3 * 0x1000];
    for n in 0..3 {
        img[n * 0x1000 + 1] = 0xA0 + n as u8;
    }
    let code = [0xAD, 0x01, 0x90, 0x85, 0x11, 0xA9, 0x02, 0x8D, 0xF9, 0x5F, 0xAD, 0x01, 0x90, 0x85, 0x12, 0x60];
    img[0x10..0x10 + code.len()].copy_from_slice(&code);
    img[0x20] = 0x60;
    let d = nsf_file(&img, 0x8010, 0x8020, 1, [0, 1, 0, 0, 0, 0, 0, 0], 0);
    let mut p = NsfPlayer::new(&d, 48000).unwrap();
    render(&mut p, 0.05);
    assert_eq!((p.m.ram[0x11], p.m.ram[0x12]), (0xA1, 0xA2));
    // padding: a load address of $8005 with banks puts the data 5 bytes into bank 0
    let mut d2 = nsf_file(&[0xA9, 0x77, 0x85, 0x11, 0x60, 0x60], 0x8005, 0x8009, 1, [0, 0, 0, 0, 0, 0, 0, 1], 0);
    d2[8] = 0x05;
    let mut p = NsfPlayer::new(&d2, 48000).unwrap();
    render(&mut p, 0.05);
    assert_eq!(p.m.ram[0x11], 0x77);
    // without banks, $5FF8-$5FFF writes do nothing
    let d3 = nsf_file(&img, 0x8010, 0x8020, 1, [0; 8], 0);
    let mut p = NsfPlayer::new(&d3, 48000).unwrap();
    render(&mut p, 0.05);
    assert_eq!((p.m.ram[0x11], p.m.ram[0x12]), (0xA1, 0xA1));
}

#[test]
fn silence_ends_the_song() {
    // INIT and PLAY: RTS
    let d = nsf_file(&[0x60], 0x8000, 0x8000, 1, [0; 8], 0);
    let mut p = NsfPlayer::new(&d, 48000).unwrap();
    render(&mut p, 4.0);
    assert!(!p.ended());
    render(&mut p, 1.5);
    assert!(p.ended());
    // a steady tone does not end
    let mut p = NsfPlayer::new(&tone_nsf(1), 48000).unwrap();
    render(&mut p, 6.0);
    assert!(!p.ended());
    assert_eq!(p.duration_secs(), None);
}

#[test]
fn hostile_code_and_files() {
    // INIT never returns (JMP $8000): given up after 2 s, PLAY still called
    let code = [0x4C, 0x00, 0x80, 0xE6, 0x10, 0x60];
    let d = nsf_file(&code, 0x8000, 0x8003, 1, [0; 8], 0);
    let mut p = NsfPlayer::new(&d, 48000).unwrap();
    let t = std::time::Instant::now();
    render(&mut p, 3.0);
    assert!(t.elapsed().as_secs_f64() < 3.0);
    assert!(p.m.ram[0x10] > 30);
    // PLAY never returns: later calls skipped, no hang; STP in INIT
    let d = nsf_file(&[0x60, 0x4C, 0x01, 0x80], 0x8000, 0x8001, 1, [0; 8], 0);
    render(&mut NsfPlayer::new(&d, 48000).unwrap(), 1.0);
    let d = nsf_file(&[0x02], 0x8000, 0x8000, 1, [0; 8], 0);
    render(&mut NsfPlayer::new(&d, 48000).unwrap(), 0.5);
    // truncations and random bytes after the magic
    let t = tone_nsf(2);
    for n in 0..t.len() {
        if let Ok(mut p) = NsfPlayer::new(&t[..n], 48000) {
            render(&mut p, 0.02);
        }
    }
    let mut s = 0x9E37_79B9u32;
    for _ in 0..100 {
        let mut r = MAGIC.to_vec();
        for _ in 0..600 {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            r.push(s as u8);
        }
        if let Ok(mut p) = NsfPlayer::new(&r, 48000) {
            let (_, last, _) = p.subsongs();
            p.set_subsong(last);
            render(&mut p, 0.05);
        }
    }
}

#[test]
fn deterministic_and_engine() {
    let d = tone_nsf(2);
    let a = render(&mut NsfPlayer::new(&d, 44100).unwrap(), 0.5);
    let b = render(&mut NsfPlayer::new(&d, 44100).unwrap(), 0.5);
    assert_eq!(a, b);
    let mut p = player(&d, 48000).unwrap();
    assert_eq!(p.format(), "NSF");
    assert_eq!(p.subsongs(), (0, 1, 0));
    let mut v = vec![0f32; 9600];
    assert_eq!(p.render(&mut v), 4800);
    // through the library's Song
    let mut s = crate::Song::open("t.nsf", &d, 48000).unwrap();
    assert_eq!(s.format(), "NSF");
    let mut pcm = vec![0i16; 2 * 4800];
    s.render_i16(&mut pcm);
    assert!(pcm.iter().any(|&x| x != 0));
}

const DIR: &str = "/media/lolo/kingston2tb/claude-scratch/deliplayerrust/dl-modules/NSF";

/// The downloaded test files (when present here): detected by NSF alone, every song plays, not
/// silent.
#[test]
fn test_files_play() {
    let Ok(rd) = std::fs::read_dir(DIR) else { return };
    let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "nsf")).collect();
    files.sort();
    for f in files {
        let d = std::fs::read(&f).unwrap();
        assert!(detect(&d), "{f:?}");
        assert_eq!(crate::open::detect(&d).as_deref(), Some("NSF"), "{f:?}");
        assert!(crate::replay::REPLAYERS.iter().all(|o| !(o.detect)(&d)), "{f:?} claimed by an open replayer");
        let mut p = NsfPlayer::new(&d, 48000).unwrap();
        let (_, last, _) = p.subsongs();
        for s in 0..=last {
            p.set_subsong(s);
            let v = render(&mut p, 5.0);
            let rms = (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
            assert!(rms > 0.01, "{f:?} song {s}: rms {rms}");
        }
    }
}

/// `NSF_WAV=<file> [NSF_SONG=n] [NSF_SECS=s] NSF_OUT=<out.f32>`: raw mono float samples at 48 kHz
/// for comparisons.
#[test]
#[ignore]
fn dump() {
    let (Ok(f), Ok(out)) = (std::env::var("NSF_WAV"), std::env::var("NSF_OUT")) else { return };
    let d = std::fs::read(f).unwrap();
    let mut p = NsfPlayer::new(&d, 48000).unwrap();
    let s: u16 = std::env::var("NSF_SONG").ok().and_then(|s| s.parse().ok()).unwrap_or(p.cur);
    p.set_subsong(s);
    let secs: f64 = std::env::var("NSF_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(20.0);
    let v = render(&mut p, secs);
    println!("init took {} cycles", p.init_cycles);
    let b: Vec<u8> = v.iter().step_by(2).flat_map(|x| x.to_le_bytes()).collect();
    std::fs::write(out, b).unwrap();
}
