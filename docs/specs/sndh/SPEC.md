# SNDH (Atari ST music, `.sndh`): format and player spec

SNDH is the music format of the Atari ST scene: the tune is the ST program's own 68000 code with
its data, in a file that starts with a small tag header. There is no Amiga reference player (the
Amiga 68k DeliPlayer chain accepts the files and plays them silent: the code writes the ST's sound
chip, not Paula), so the player here is a small Atari ST machine that runs the tune's code and
renders the YM2149 it drives. Implementation: `src/replay/sndh.rs` (header, machine, player),
`sndh_ym.rs` (YM2149), `sndh_mfp.rs` (MFP 68901), tests in `sndh_tests.rs`.

## 1. File

Big-endian. A file may be ICE!-packed (Pack-Ice 2.4: the repo's decrunchers undo it before the
player sees it). Detection: byte 0 is `$60` (a `bra`) and the ASCII `SNDH` is at offset 12.

| offset | content |
|---|---|
| 0, 4, 8 | `bra.w` to the init, exit and play routines (a `bra.s` is accepted: `$60 nn`) |
| 12 | `SNDH` |
| 16 ... | tags, up to the first of the three branch targets (the code) or `HDNS` |

Tags (a tag is found by its letters; any other byte between tags is skipped):

| tag | content |
|---|---|
| `TITL`, `COMM`, `RIPP`, `CONV`, `YEAR` | NUL-terminated strings: title, composer, ripper, converter, year |
| `##nn` | the number of subsongs, two decimal digits (1 to 128; anything else is 1) |
| `#!nn` | the default subsong (decimal string; out of range is 1) |
| `!#SN` | subsong names: one word per subsong, the offset of its name from the tag's start |
| `TIME` | one word per subsong: its length in seconds (the words start on an even file offset) |
| `FRMS` | one long per subsong: its length in player ticks (takes the place of `TIME`) |
| `TAnn`, `TBnn`, `TCnn`, `TDnn` | the player rate in Hz (decimal string): the tune is driven by that MFP timer |
| `!Vnn` | the player rate in Hz, driven by the vertical blank |
| `FLAG` | a flag string, skipped |
| `HDNS` | end of the tags |

Without a rate tag the rate is 50 Hz (vertical blank). A rate outside 1 to 2000 is ignored.
A length of 0 means unknown (the tune plays on).

Calls. Init (offset 0) is called with the subsong number (1-based) in d0; play (offset 8) is called
once per player tick; the exit routine (offset 4) is not called.

## 2. Machine

| part | model |
|---|---|
| CPU | the `m68k` crate as a 68000 (its cycle counts only bound a call), the registers kept from one call to the next, each call starting as after a reset (supervisor, mask 7) |
| RAM | 4 MB at 0; the tune is loaded at `$10002`; the stack is at the top; the vectors are zero but `$14` (division by zero) and timer C's (`$114`), which go to an RTE at `$500` (an unset vector runs the zeroed memory up to that RTE, which returns from the exception) |
| YM2149 | at `$FF8800`-`$FF88FF`: even addresses read the selected register, writes with bit 1 of the address clear select a register, set write it; word writes put the high byte on the bus, so `move.l #$08000F00,$ff8800` sets R8; registers 0-13 with their masks (R7 keeps 6 bits), 14 and 15 read back what was written |
| MFP 68901 | at `$FFFA00`-`$FFFA25` (odd bytes): timers A-D with prescalers 4, 10, 16, 50, 64, 100, 200 of 2.4576 MHz and the GPI7 line (an event timer with a count of 1); a timer counts only while its enable bit (IERA / IERB) is set, its mask bit (IMRA / IMRB) lets the handler be called; enabling a running-mode timer, or writing its data register while stopped, restarts it; fixed vectors `$134` A, `$120` B, `$114` C, `$110` D, `$13C` GPI7; no pending / in-service registers (they read back what was written); timer A's event-count mode and GPI7 count the STE DMA's frame ends; pulse-width modes are not modelled |
| STE DMA sound | `$FF8900`-`$FF8925`: frame start/end/counter, control (play, loop), mode (rate 6258 / 12517 / 25033 / 50066 Hz, mono bit; 50 kHz averaged in pairs), the microwire interface for the master volume (command 3); end of frame counts timer A |
| BIOS | `TRAP #1` GEMDOS `Malloc` ($48, from a 1 MB heap below the stack; -1 asks the free size) and `Sversion` ($30); `TRAP #14` XBIOS `Xbtimer` (31), `DoSound` (32, its byte-code run one step per tick), `Supexec` (38), `Blitmode` (64: 3, a blitter there and on, as the reference answers); other calls return 0 (and are collected as unhandled) |
| illegal | an illegal instruction, a line-A or line-F opcode and a STOP end the tune |
| cookie jar | `_SND` = 3, `_MCH` = `$00010000` (STE) at `$900`, pointer at `$5A0` |
| video | `$FF820A` reads 2 (50 Hz), `$FF8260` reads 0 |
| blitter | `$FF8A00`-`$FF8A3D` (`src/replay/sndh_blitter.rs`): halftone RAM, increments, addresses, end masks, counts, HOP, logical operation, line number (BUSY, SMUDGE), skew (FXSR, NFSR); writing the line number with BUSY set runs the whole transfer at once, through the CPU's bus (a transfer to `$FF8800` writes the YM2149). The model is AtariAudio 1.30's `Blitter.cpp` (MIT), ported, plus SMUDGE from the hardware documentation |
| other I/O | reads give `$FF` / `$FFFF`, writes are dropped; each such address (and each GEMDOS / XBIOS call not answered, each other TRAP) is collected, `SndhPlayer::unhandled`, printed by `examples/sndh_regs.rs` |

Timing. The machine lives on the output-sample clock, not the CPU's. A call (the init, the play
routine, a timer's handler) runs to its return at once: no machine time passes in it, the timers do
not advance, no interrupt comes in it, and all its register writes are stamped with the sample it
was made at. Per output sample, in this order: the YM makes its sample (the writes of the calls so
far are in), the DMA makes its sample (and may signal a frame end), then each timer A, B, C, D, GPI7
advances by one sample (a timer's prescaled rate added to an accumulator that takes one data-register
step per output rate passed, so its rate is exact on average and its underflows land on a sample;
one handler call per sample at most), and each that underflowed, enabled and unmasked, has its
handler called (SR `$2300`, return by RTE) there, its writes reaching the YM at the next sample.
At every player tick, before the sample, `DoSound`'s script is stepped and the play routine
called (SR `$2300`, return by RTS). A tick is a whole number of output samples: rate x 313 x 512 x 50
/ (tick rate x 8,021,247) rounded down, i.e. 958 at 48 kHz for a 50 Hz tune (the ST's frame of 313
lines of 512 clocks at its 8.021247 MHz clock; the 0.1 % short tick is the reference player's
and is kept so that the writes land on the same samples), for every kind of tick tag. Init runs
first and takes no time of the tune's. On a real ST a timer interrupt can preempt a play routine
that takes thousands of clocks; here it cannot, which is how the reference player runs the tunes.

Bounds. Every call has a budget of 8 MHz CPU clocks: an init 8,000,000 (1 s), a play call 641,024
(4 frames), a handler 160,256 (1 frame); an illegal instruction, a STOP or a call that exceeds
its budget ends the tune, which then plays silence, so a tune that never returns costs a bounded
amount of CPU time. (The reference player allows an init and a play call 500 frames.)

## 3. YM2149 model

Steps of 8 chip clocks (250 kHz). Tone counters count up and toggle at the period (a period of 0
holds the channel high: the sample-playing trick; the square waves start high, high, low); the
17-bit noise register is shifted every second step when the noise counter reaches R6 (input: bit 0
xor bit 2); the envelope advances through 32 levels every R11/R12 steps (its shape through a
128-entry table, the first 64 for the first pass, the second 64 for what repeats or holds; a chip
that has not had R13 written sits in the second pass with a silent envelope); a channel outputs when
its tone and noise sources (each OR-ed with its mixer disable bit, the state of the step before) are
both high, at the volume register's 4-bit value (as level 2 v + 1) or the envelope's 5-bit level.
A write of a tone period of 0 or 1 inside a timer handler restarts that channel's square wave
(flips it, counter to 0) when the handler returns (the sync-square effect).

The three channels' 5-bit levels (0 for a channel the mixer silences) are looked up in a
32 x 32 x 32 table of 16-bit outputs (a single channel at full volume 17,344): AtariAudio 1.30's
`s_ym2149RecordedMixTable` (MIT), measured on an ST by Paulo Simoes as 16 x 16 x 16 levels and
log-interpolated to 32 per channel by Arnaud Carre, embedded as data (`src/replay/sndh_ym_mix.bin`,
32768 little-endian u16). (The player first used a two-grid model fitted to it, 1 % rms off, which
kept the audio from being sample-exact: 0.2 to 34 % of the samples equal, 27 to 38 dB.) Each output sample is the integer average of the chip's steps
it spans (the output rate against 250 kHz, 5 or 6 steps at 48 kHz); the 2048 latest samples' moving
average is subtracted. The STE DMA is added in stereo at the reference's level for a centred sound.
The result is clamped to 16 bits and scaled to +-1.

## 4. Sources and licence

Original implementation (GPL-2.0-or-later), written from the format description and the hardware
documentation, with these exceptions where behaviour was read from a reference:

- AtariAudio by Arnaud Carre (`github.com/arnaud-carre/AtariAudio`, MIT; its `LICENSE` file was
  read in the clone). Its machine model gave the choices of section 2 that no document fixes: the
  load address `$10002`, the stubs at `$500`, the cookie jar, which BIOS calls tunes need and what
  they return, DoSound's byte-code, the tag parser's behaviour, the STE DMA and microwire. Taken
  from it under its MIT licence: the YM2149 mixing table (data, measured on a real machine) and the
  blitter's transfer model (`Blitter.cpp`, ported to Rust in `sndh_blitter.rs`). Not used: its
  68000 (Musashi) and its SNDH duration database. The MIT notice is kept in `LICENSE-AtariAudio`.
- libsc68 / sc68 and Hatari were not used, and their licences not checked (AtariAudio's MIT licence
  and the hardware documentation were enough).

## 5. Checks

Reference: AtariAudio 1.30, built from the clone with its YM2149 writes logged (a harness in the
scratch directory; nothing of it is in the repository), 10 s of the default subsong at 48 kHz.
`examples/sndh_regs.rs` prints this player's writes as `SAMPLE REG VALUE` and
`examples/sndh_wav.rs` renders a WAV, for the comparison.

| set | result |
|---|---|
| 5 corpus files (modland `SNDH/`: Action, Bionic Commando, Blasteroids, Gem'X, Thrust), all ICE!-packed | the ordered YM writes of all 470 compared ticks equal the reference's, each on the same output sample (Action is driven by timer C, with a volume-register digidrum at 7.6 kHz) |
| 33 further modland files (Count Zero, Daglish, Follin, Hubbard, Huelsbeck, Jess, Lap, Mad Max, Tao, Tomchi, Whittaker) | all 33 equal in every write (sample, register, value), the timer-driven ones (Mad Max x3, Tao 13, Goldrunner, Populous) included |
| audio, 38 files, every subsong (113), 30 s at 48 kHz | sample for sample the reference's in all 113, and the YM writes (sample, register, value) equal over the 30 s |
| refcmp rows 1723-1727 | IDENTICAL (were SIMILAR, 0.2 to 1 % of the samples equal, with the fitted mix) |
| blitter, 16 synthetic tunes of 64 random transfers each (all HOP and logical operations, skews, FXSR / NFSR, X counts 1-5, Y counts 1-3, negative increments, end masks), and one that writes YM registers by blitter | every destination word, the address and count registers read back, and the YM writes equal the reference's |
| unhandled I/O and BIOS calls, all 113 subsongs, 3000 ticks | none: no file at hand touches the blitter or anything else the machine lacks |

What the timer-driven tunes needed (the seven that differed in phase): the reference does not run
the CPU alongside the chips. It calls the play routine and each timer's handler to completion in
zero time, timers counting on the output-sample grid only while their enable bit is set; the earlier
model of this player (cycle-accurate, handlers preempting the play routine, timers on the CPU clock)
put the same writes some tens of samples later and in another order. Matching the reference on the
rest: the tick of a whole 958 samples, the low memory the tune can read (the vectors zero, the
reset-vector words left at 0 and 4 by each call, as the reference leaves them: Goldrunner reads
them), the envelope's initial position, the square waves' initial states.

Not checked against any real ST recording. The remaining gaps are listed in the head of
`src/replay/sndh.rs` (pulse-width timers, USART, a length database for tunes without `TIME` or
`FRMS`). The blitter follows the reference where the documented hardware may differ (the halftone
ANDed before the skew, the source buffer always loaded from below); no tune at hand uses it.
