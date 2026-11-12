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
| CPU | the `m68k` crate as a 68000 (cycle counts per instruction, bus accesses stamped to the clock they happen on, level-6 interrupts taken at instruction boundaries) at 8.000 MHz |
| RAM | 4 MB at 0; the tune is loaded at `$10002`; the stack is at the top |
| YM2149 | at `$FF8800`-`$FF88FF`: even addresses read the selected register, writes with bit 1 of the address clear select a register, set write it; word writes put the high byte on the bus, so `move.l #$08000F00,$ff8800` sets R8; registers 0-13 with their masks, 14 and 15 read back what was written |
| MFP 68901 | at `$FFFA00`-`$FFFA25` (odd bytes): timers A-D with prescalers 4, 10, 16, 50, 64, 100, 200 of 2.4576 MHz, data registers (0 = 256), IER/IPR/ISR/IMR, vector register (high nibble + source number; default `$40`, no software end of interrupt: the in-service bits stay clear unless the tune sets bit 3), priority 15 (GPI7, timer A) to 0; timer A's event-count mode counts the STE DMA's frame ends; pulse-width modes are not modelled |
| STE DMA sound | `$FF8900`-`$FF8925`: frame start/end/counter, control (play, loop), mode (rate 6258 / 12517 / 25033 / 50066 Hz, mono bit), the microwire interface for the master volume (command 3); end of frame counts timer A |
| BIOS | `TRAP #1` GEMDOS `Malloc` ($48, from a 1 MB heap below the stack; -1 asks the free size) and `Sversion` ($30); `TRAP #14` XBIOS `Xbtimer` (31), `DoSound` (32, its byte-code run one step per tick), `Supexec` (38), `Blitmode` (64: no blitter); other calls return 0 |
| vectors | the MFP's ($100-$13C) and the division by zero go to an RTE; every other exception ends the tune |
| cookie jar | `_SND` = 3, `_MCH` = `$00010000` (STE) at `$900`, pointer at `$5A0` |
| video | `$FF820A` reads 2 (50 Hz), `$FF8260` reads 0 |

Timing. One clock: the CPU's 8 MHz. The YM is stepped every 8 of its 2 MHz clocks (32 CPU clocks);
the MFP and the DMA advance with every instruction. Init runs first and takes no time of the tune's
(the tune's audio starts when it returns, as in the reference player). Then play is called every
tick at the rate in the tags, with the CPU in supervisor mode, interrupt mask 3: the MFP's level-6
interrupts preempt it, run their handlers (digidrums, SID voices, sync-buzzer) and return into it.
Between calls the CPU idles on a stub; timer interrupts are taken there too. A vertical-blank tick
(the default and `!V`) is the ST's real frame: 160,256 clocks of its 8.021247 MHz CPU, which at a
rate of 50 is 50.05 Hz; a timer's tick is exact (`TC50` is 50.000 Hz). A play call that is still
running at the next tick delays it (a tick missed by a whole period is dropped).

Bounds. The CPU runs for one tick's worth of clocks at a time, whatever the tune does; an init
that has not returned after 1 s of CPU time, an illegal instruction, an address error or a jump
to the vector of an unset exception ends the tune, which then plays silence.

## 3. YM2149 model

Steps of 8 chip clocks (250 kHz). Tone counters count up and toggle at the period (a period of 0
holds the channel high: the sample-playing trick); the 17-bit noise register is shifted every
second step when the noise counter reaches R6 (input: bit 0 xor bit 2); the envelope advances
through 32 levels every R11/R12 steps (shapes by CONT/ATT/ALT/HOLD); a channel outputs when its
tone and noise sources (each OR-ed with its mixer disable bit) are both high, at the volume
register's 4-bit value (as level 2 v + 1) or the envelope's 5-bit level. Levels fall by 1.77 dB per
5-bit level (3.5 dB per volume step); the three channels share a saturating output stage,
`s / (1 + s / 3)`. Both constants were fitted to the reference player's chip (below), not taken
from a table. Mono, a 5 Hz high-pass for the DC; the STE DMA is added in stereo.

## 4. Sources and licence

Original implementation (GPL-2.0-or-later), written from the format description and the hardware
documentation, with these exceptions where behaviour was read from a reference:

- AtariAudio by Arnaud Carre (`github.com/arnaud-carre/AtariAudio`, MIT; its `LICENSE` file was
  read in the clone). Its machine model gave the choices of section 2 that no document fixes: the
  load address `$10002`, the stubs at `$500`, the cookie jar, which BIOS calls tunes need and what
  they return, DoSound's byte-code, the tag parser's behaviour, the STE DMA and microwire. No source
  text is copied (its 68000 is Musashi, its YM2149 a table measured on a real machine, its SNDH
  duration database a 1 MB table: none is used here). The MIT notice is kept in `LICENSE-AtariAudio`.
- libsc68 / sc68 and Hatari were not used, and their licences not checked (AtariAudio's MIT licence
  and the hardware documentation were enough).

## 5. Checks

Reference: AtariAudio 1.30, built from the clone with its YM2149 writes logged (a harness in the
scratch directory; nothing of it is in the repository), 10 s of the default subsong at 48 kHz.
The register writes are compared tick by tick (the reference's tick is 959 samples; the init's writes
count as the first tick's).

| set | result |
|---|---|
| 5 corpus files (modland `SNDH/`: Action, Bionic Commando, Blasteroids, Gem'X, Thrust), all ICE!-packed | 4 of 4 vertical-blank tunes: the ordered YM writes of all 470 compared ticks equal the reference's. Action (driven by timer C, with a volume-register digidrum at 7.6 kHz) differs in the phase of the timer writes: the reference runs its timer handlers on host samples, here they run on the clock |
| 33 further modland files (Count Zero, Daglish, Follin, Hubbard, Huelsbeck, Jess, Lap, Mad Max, Tao, Tomchi, Whittaker) | 27 equal in every tick; 6 differ only where timers are involved (Mad Max x3, Tao 13, Goldrunner, Populous: their writes per register are within 1 % in number, in different phase) |
| audio, 38 files | not sample-exact (another chip model: the reference uses a table measured on a real ST, with random start phases of the tones). Level at one, two and three channels at full volume: 8210 / 9466 / 9246 RMS (reference) against 8122 / 9357 / 9454; the 15 volume steps of one channel within 1.6 dB of the reference's; whole-file RMS 0.94 to 2.09 (mean 1.14) of the reference's; a 10 ms RMS-envelope correlation of 0.59 (median 0.65), low because of the 0.1 % tick drift and tone phases, not a difference of notes |

Not checked against any real ST recording. The remaining gaps are listed in the head of
`src/replay/sndh.rs` (blitter, pulse-width timers, USART, a length database for tunes without `TIME`
or `FRMS`).
