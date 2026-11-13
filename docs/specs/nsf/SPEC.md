# NSF (NES Sound Format, `.nsf`): format and player spec

An NSF file holds the music code and data of an NES (Famicom) game or of a homebrew tool: 6502
code that drives the NES's sound hardware, the APU of the Ricoh 2A03 CPU. The player here is a
small NES machine that runs that code and renders the APU it drives. Implementation:
`src/replay/nsf.rs` (header, machine, player), `nsf_cpu.rs` (the 2A03's 6502), `nsf_apu.rs`
(the APU), tests in `nsf_tests.rs`. Not a Paula replayer: rendered to PCM at the output rate on
the recorded-audio player (`crate::aiff::Stream`), as the AY and SNDH players are.

## Sources

Written for this project (GPL-2.0-or-later) from public hardware documentation only, the NESdev
wiki (nesdev.org/wiki):

- "NSF": header, bankswitching, the init procedure, the play-call rules, region and expansion
  bits.
- "Instruction reference": operations, flags, ADC / SBC overflow, the B flag, the JMP ($xxFF)
  wrap, cycle counts with the page-crossing and branch penalties, decimal mode disabled on the
  2A03.
- "CPU unofficial opcodes" (the opcode matrix with every unofficial opcode's mnemonic and
  addressing mode) and "Programming with unofficial opcodes" (what the combined operations do,
  their cycle counts).
- "Cycle reference chart": the CPU clocks.
- "APU", "APU Frame Counter", "APU Length Counter", "APU Envelope", "APU Sweep", "APU Pulse",
  "APU Triangle", "APU Noise", "APU DMC", "APU Mixer".

No emulator's or NSF player's code was read (game-music-emu, NSFPlay, Nestopia, FCEUX, Mesen,
Rust crates). No emulator crate is a dependency. libgme (through ffmpeg 6 on this machine) was
used only as a black box, to compare output (see Checks).

## 1. File

Little-endian. Detection: `NESM` `$1A` at offset 0, a header of `$80` bytes and some data after
it, at least one song, a load address of `$6000` or more.

| offset | size | field |
|---|---|---|
| `$00` | 5 | `NESM` `$1A` |
| `$05` | 1 | version (1; 2 = NSF2) |
| `$06` | 1 | number of songs |
| `$07` | 1 | starting song (1-based) |
| `$08` | 2 | load address |
| `$0A` | 2 | INIT address |
| `$0C` | 2 | PLAY address |
| `$0E`, `$2E`, `$4E` | 32 each | song name, artist, copyright (NUL-terminated; `<?>` read as empty) |
| `$6E` | 2 | NTSC play period in microseconds |
| `$70` | 8 | initial banks for `$5FF8-$5FFF` (all zero: not bankswitched) |
| `$78` | 2 | PAL play period in microseconds |
| `$7A` | 1 | region: bit 0 PAL, bit 1 dual PAL/NTSC |
| `$7B` | 1 | expansion chips: VRC6, VRC7, FDS, MMC5, Namco 163, Sunsoft 5B, VT02+ (bits 0-6) |
| `$7C` | 1 | NSF2 flags (not used) |
| `$7D` | 3 | NSF2 program length (0: to the end of the file) |
| `$80` | ... | the program |

## 2. What plays and what is refused

- Plays: NSF 1 and NSF 2 files using the 2A03's own sound only; NTSC, PAL and dual tunes;
  bankswitched and flat programs.
- Refused, with an error naming the chip ("NSF: needs the expansion sound chip VRC6, which is
  not emulated"): any file with one of bits 0-6 of `$7B` set. The engine reports this error rather
  than trying the other players, and the library's `detect` does not claim such a file.
- NSF2: the program length field is honoured; the metadata chunks after the program, the
  non-returning INIT and IRQ features of the flags byte are not.
- NSFe (`NSFE` magic) is not read.

## 3. Machine

| address | content |
|---|---|
| `$0000-$07FF` | RAM, mirrored to `$1FFF` |
| `$4000-$4013`, `$4015`, `$4017` | APU (writes), `$4015` (reads: status) |
| `$5FF8-$5FFF` | bank registers (writes; only in a bankswitched tune) |
| `$6000-$7FFF` | 8 KB RAM |
| `$8000-$FFFF` | the program, read only: eight 4 KB windows |
| others | reads give the address's high byte, writes are ignored |

Program image. Bankswitched: the program padded at its start by (load address AND `$0FFF`)
zero bytes, cut into 4 KB banks; window `i` (`$8000 + i * $1000`) shows bank (value written to
`$5FF8 + i`) modulo the number of banks. Not bankswitched: the program at the load address; a part
below `$8000` goes into the RAM at `$6000-$7FFF` (re-copied at each song's start), a part past
`$FFFF` is dropped.

Clocks: NTSC CPU 1,789,773 Hz (21.477272 MHz / 12), PAL 1,662,607 Hz (26.601712 MHz / 16). A
tune is PAL when its region byte says PAL only (bit 0 set, bit 1 clear); a dual tune plays as
NTSC (the wiki notes most players ignore the preference).

## 4. Calls

The init procedure, for song `n` (0-based): RAM `$0000-$07FF` and `$6000-$7FFF` cleared (then the
program part below `$8000` copied in), `$00` written to `$4000-$4013`, `$00` then `$0F` to
`$4015`, `$40` to `$4017`, the eight bank bytes written to `$5FF8-$5FFF` (bankswitched tunes),
then INIT called with A = `n`, X = 0 (NTSC) or 1 (PAL), Y = 0, S = `$FF` before the return
address is pushed, the I flag set.

A call pushes a return address of `$5000 - 1` (no tune runs code in that unmapped page) and runs
the CPU until the program counter is `$5000`. PLAY is called with S = `$FF` again, every play
period (the header's microseconds for the region: `period * CPU Hz / 1,000,000` cycles, kept
exact as a fraction; a period of 0 is taken as 16,639 / 19,997). The first PLAY comes on the
cycle INIT returns; a PLAY call due while the previous one runs is skipped. Between calls the CPU
is idle and the APU runs on. INIT runs at most 2 s of CPU time: then the host gives up on it and
starts the PLAY calls. An STP opcode ends the call it is in. INIT's time is part of the song's
time (the audio runs during INIT).

Interrupts are not delivered to the CPU (NSF 1 defines none): the frame counter's and the DMC's
IRQ flags are kept and reported by `$4015`. BRK is executed (vector at `$FFFE`).

## 5. CPU (`nsf_cpu.rs`)

The 6502 instruction set without decimal mode (ADC / SBC are binary whatever D says; D is kept),
instruction by instruction, each instruction's cycles counted: the base count by addressing mode
and kind (read, write, read-modify-write), +1 on a page crossing for the indexed reads, branches
2 / 3 / 4. The JMP (`$xxFF`) wrap, zero-page wrap of the indexed and indirect zero-page modes,
B and bit 5 pushed as 1 by PHP / BRK and ignored by PLP / RTI.

Unofficial opcodes: SLO, RLA, SRE, RRA, SAX, LAX, DCP, ISC, ANC, ALR, ARR (C = bit 6, V = bit 6 XOR
bit 5), AXS, the SBC `$EB` duplicate, every NOP form (with its operand read and its cycles), the
12 STP opcodes (the CPU stops). The unstable ones in their "magic constant `$FF`" forms: XAA
(A = X AND #), LAS (A = X = S = M AND S), AHX / SHX / SHY / TAS (the value AND the base address's
high byte + 1).

Not modelled: the dummy reads and writes within an instruction; writes land at the start of the
instruction's cycles (the APU sees them before the instruction's cycles are clocked).

## 6. APU (`nsf_apu.rs`)

Stepped every CPU cycle. Pulse timers on APU cycles (every second CPU cycle), the triangle timer
every CPU cycle, the noise and DMC periods in CPU cycles as the tables give them.

- Pulse (2): duty sequences 12.5 / 25 / 50 / 75 %, read downwards (0, 7, 6, ...); `$4003` / `$4007`
  restart the sequence and the envelope and load the length counter; muted when the period is
  below 8 or the sweep's target above `$7FF` (even with the sweep off). Sweep: change = period >>
  shift, negated as ones' complement on pulse 1 and two's complement on pulse 2, the target
  clamped at 0; on each half frame the period takes the target when the divider is 0, the sweep
  is on, the shift is not 0 and the channel is not muted; the divider reloads when 0 or when
  `$4001` was written.
- Triangle: 32-step sequence 15..0, 0..15; advances when both the linear and length counters are
  non-zero; linear counter reloaded while its reload flag is set (set by `$400B`, cleared on a
  quarter frame when the control bit is clear). Periods 0 and 1 run at their ultrasonic rate and
  average to a mid level in the output sample.
- Noise: 15-bit shift register starting at 1, feedback bit 0 XOR bit 1 (bit 6 in mode 1) into bit
  14; muted while bit 0 is set; NTSC / PAL period tables (4 ... 4068 / 3778 cycles).
- Envelopes (pulse, noise): start flag, divider, decay 15..0, loop; constant volume.
- Length counters: the 32-entry table; halted by the loop bit (`$4008` bit 7 for the triangle);
  forced to 0 and not loadable while their `$4015` bit is clear.
- DMC: NTSC / PAL rate tables (428 ... 54 / 398 ... 50 cycles per bit), sample at `$C000 + 64 A`,
  length `16 L + 1`; the memory reader fetches when the buffer is empty and bytes remain
  (address wraps `$FFFF` to `$8000`, loop restarts, else the IRQ flag when enabled), each fetch
  stalls the CPU 4 cycles; the output unit's 8-bit shift register moves the 7-bit level by +-2
  within 0..127, silence when the buffer was empty at the end of a byte; `$4011` sets the level.
  `$4015` bit 4 clear stops the sample, set restarts it if no bytes remain; any `$4015` write
  clears the DMC IRQ flag.
- Frame counter, in CPU cycles: 4-step NTSC quarter frames at 7457, 14913, 22371, 29829, half
  frames at 14913 and 29829, IRQ flag at 29829 (unless inhibited), length 29830; 5-step quarter
  at 7457, 14913, 22371, 37281, half at 14913 and 37281, no IRQ, length 37282; PAL 8313, 16627,
  24939, 33253 / 33254 and 41565 / 41566. A `$4017` write takes effect 3 or 4 CPU cycles later (by
  the APU cycle's parity): the sequence restarts, and both clocks come at once when bit 7 is set;
  bit 6 set clears the IRQ flag. Reading `$4015` clears the frame IRQ flag.

Not modelled: a `$4011` write landing on an output clock, the early RP2A03's noise period 2046,
the IRQ flag held over three cycles, the exact cycle of a DMC fetch's stall (always 4).

## 7. Output

The wiki's lookup-table mixer: `pulse_table[p1 + p2] + tnd_table[3 t + 2 n + d]`, with
`pulse_table[n] = 95.52 / (8128 / n + 100)` and `tnd_table[n] = 163.67 / (24329 / n + 100)`.
Each output sample is the mean of the CPU cycles it covers; then the NES's first-order high-pass
at 90 Hz (its state starts at the first sample's level, so the APU's resting level, the triangle
at 15, gives no click) and first-order low-pass at 14 kHz, times 2, clipped to +-1. Mono, the same
on both sides.

Length: the format states none. A song ends after 5 s in which the output sample before the
filters has not moved by more than 0.001 from the level it settled at; until then it plays on.
`duration_secs` is unknown. Subsongs: 0 to songs - 1, the starting one first; title = song name,
author = artist, format "NSF".

## 8. Checks

Unit tests (`nsf_tests.rs`): hand-assembled CPU snippets (ADC / SBC flags and overflow, no
decimal mode, compares, BIT, shifts, every addressing mode with the zero-page and JMP wraps,
JSR / RTS / PHP / PLP / BRK / RTI, cycle counts with page crossing and branches, unofficial
opcodes); APU values from the wiki's tables (length, noise, DMC), length counter enable / halt /
status, the frame counter's half-frame and IRQ cycles and the 5-step write, envelope decay,
sweep negation and muting, pulse and triangle pitch against `fCPU / (16 (t + 1))` and
`fCPU / (32 (t + 1))`, the noise LFSR's 32767-step period, the DMC level, IRQ and address wrap,
the mixer's table values; synthetic NSF files built in the test (INIT arguments A / X, PLAY rate
60 / 50 Hz, NTSC / PAL pitch, the init procedure's RAM clearing, bankswitching and its padding,
expansion chips refused, silence end, a hanging INIT / PLAY, STP, truncated and random files,
determinism, the engine and `Song`). The four downloaded test files (when present at
`dl-modules/NSF/`): detected as NSF and by no other replayer, every song plays (5 s, RMS > 0.01).

Against libgme (black box, ffmpeg's `libgme` demuxer, 48 kHz, 20 s, the 7 songs of the 4 files):
libgme trims a tune's leading silence (checked with a synthetic NSF whose tone starts at frame 30:
libgme's output starts at 9 ms), so ours is aligned on its first sound (0 to 2.0 s dropped).
The first note's fundamental equals libgme's on 7 of 7 (82.0 to 521.5 Hz, one 0.5 % apart:
419.7 / 417.5 Hz); RMS-envelope correlation 0.74 to 0.95 (0.84 to 0.90 per band below 8 kHz on the
lowest one), long-term spectrum correlation 0.98 to 0.99; level 0.53 to 0.86 of libgme's (which
clips at 1.41 on one file). PLAY here follows the header's period (16666 us, 60.002 Hz), and the
envelopes drift against libgme's by 0.16 % (15 to 20 ms in 12 s), the ratio 16666 / 16639 to the
NES's actual frame rate. Not sample-exact (waveform correlation below 0.2): different filters,
start phase and that drift. Renders 20 s of a tune in about 0.4 s.
