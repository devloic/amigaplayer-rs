# NTSP-system (`two.*`, magic `SPNT`) replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the "NTSP-system" module format played by the UADE/Eagleplayer player of the same name (file name
prefix `two`, 1990s game music from the Contraz group). The module is not a tracker song: it is a
**compressed sample stream** (about 22 kHz, 8 bit) that the player decodes on the fly and feeds to all four
Paula channels through two alternating chip-RAM buffers. There are no notes, no instruments, no effects and
no tempo. The whole behaviour is: decode, double-buffer, loop.

This is a behaviour description derived from write logs, DMA logs and black-box runs of the 68k reference
(plus the player's public behaviour description). It contains no code of the original. Times are in colour
clocks ("cc", PAL, 227 cc per raster line, 3 546 895 cc per second).

Status (checked by a private model that follows only this text, against the reference):

| item | result |
|---|---|
| 22 reference logs, 44 to 462 log lines each (synthetic modules, see README) | value and order of every write equal the reference; time of every write equals the SPEC 6 model within 2 cc |
| audio DMA word streams (all four channels) of 9 modules incl. restarts after the song end and tails that are not a multiple of 9 bytes | every word equals the model's buffer contents (about 230 000 words compared, 0 differences) |
| modules with real music | none exist in the available corpus (README) |

## 1. File format

All numbers big-endian.

| offset | size | meaning |
|---|---|---|
| 0 | 4 | magic, the ASCII characters `SPNT` |
| 4 | 4 | `N`: the length in bytes of the **decoded** sample stream |
| 8 | ... | the compressed stream (1.3) |

### 1.1 Detection

The module is accepted when the first four bytes are `SPNT` and `N` is not zero. The player then also
refuses (module too short) when `N < 16`: it computes the expected module size `8 + floor(5N / 9)` and
rejects the file when that is larger than `N`; in practice `N` from 0 to 15 is refused and 16 and up
accepted (checked with the reference). The prefix `two.` is shared with other formats (a `two.tme` file is a
different format): decide on the magic, never on the name. Nothing else in the file is checked; in particular
the file size is not compared with `8 + floor(5N / 9)`.

### 1.2 Song, subsongs, title, length

One song, no subsongs, no title, no author. The host's information values are:

- expected size: `8 + floor(5N / 9)` bytes;
- duration in seconds: `floor(floor(161 N / 64) / 55420) + 1` (the stream plays at one byte per 161 cc,
  about 22 030 bytes per second);
- number of interrupts: `floor(N / 1800)` (not shown by the host).

### 1.3 The compressed stream

The stream is a sequence of **blocks of 5 bytes**, each of which decodes to **9 bytes** of signed 8-bit
sample data, in order. `N` is not forced to be a multiple of 9: the decoder always works on chunks
(section 3) and decodes `floor(chunk length / 9)` blocks per chunk.

Block layout, bytes `b0` to `b4`:

- `b0`: bits 7 to 5 are the **shift** `s` (0 to 7), bits 4 to 0 a signed 5-bit value `v`.
- `b1` to `b4`: eight signed 4-bit values `u1` to `u8`, high nibble first: `u1`, `u2` in `b1`, `u3`, `u4` in
  `b2`, and so on.

Decoded bytes `y0` to `y8` (each is the low 8 bits of the result, two's complement):

- `y0 = v * 2^max(s-1, 0)`
- `y1..y8 = u * 2^s`

So the shift `s` is a block-wide coarse gain: with `s = 0` the nibbles are used as they are (range -8 to 7
and `v` -16 to 15 unshifted), with `s = 7` they are multiplied by 128 and the low 8 bits are kept (the high
bits are dropped, no clamping). There is no prediction and no delta coding: every output byte is derived from
the block alone.

Worked examples:

- `6B 12 F3 80 7F` has `s = 3`, `v = 11`: output `2C 08 10 F8 18 C0 00 38 F8`.
- `1F 9A 00 00 00` has `s = 0`, `v = -1`: output `FF F9 FA 00 00 00 00 00 00`.

## 2. Structure of the replay

Two sample buffers of **1800 bytes (900 words)** each are used, called here X and Y. In chip RAM they are
two consecutive blocks, Y directly after X, both zero at start. Their absolute address is chosen by the host
(any constant offset is fine; any even address works). All four Paula channels are always pointed at the same
buffer, with length 900 words, period 161, so the buffer takes `1800 * 161 = 289 800` cc (about 81.7 ms) to
play.

Volumes: channels 0 and 1 play at 64, channels 2 and 3 at 1. They carry the same data, so the output is
simply the stream at the full level with a 65/64 gain, equal on both sides.

One audio interrupt (channel 0's, the only audio interrupt that is enabled) is the whole timing source: each
time channel 0 starts a buffer, the interrupt handler decodes the next 1800 bytes into the other buffer and
points all channels at it. So the buffer that is playing is never written to.

State of the player: the position in the compressed data, the number of decoded bytes still to produce
(`LT`, starts at `N`), and which buffer is the next one to fill.

## 3. Decoding a chunk

The unit of work is a **chunk**: a request to produce `D` bytes (D = 1800 at the start of each buffer).

1. If `LT` is zero: no data is left, go to the end-of-stream case (3.2).
2. Otherwise let `D1 = min(D, LT)`. Decode `floor(D1 / 9)` blocks from the current compressed position into
   the buffer at the current offset, advancing the compressed position by 5 per block. `LT` decreases by
   `D1` (the full `D1`, not just the decoded part). The buffer offset advances by `D1` and `D` decreases by
   `D1`.
3. If `D` is now zero the buffer is complete. If not (only possible when `LT` ran out), repeat from step 1;
   `LT` is now zero, so step 1 sends it to 3.2.

Consequences:

- When `D1` is not a multiple of 9 the last `D1 mod 9` bytes of that chunk are **not written**. They keep
  whatever the buffer held before (the data of two buffers ago, or zeros at the very start). The reference
  shows this exactly (DMA words of the 4000-byte and 9005-byte modules).
- When `D1 < 9` no block is decoded at all and the compressed position does not move.
- `N` smaller than 1800 is not a supported case (the first buffer is decoded before the length is
  checked, `LT` goes below zero and is then treated as a huge unsigned number, so the decoder keeps reading
  beyond the module forever). Real streams are several buffers long. Not specified.

### 3.2 End of stream and the loop

When a request finds `LT` equal to zero with `D` bytes of the buffer still unfilled (`D` can be 1800, if
`N` is a multiple of 1800, or the rest after a partial chunk):

- the remaining `D` bytes of the buffer are set to zero;
- the compressed position and `LT` are reset to the start (`LT = N`);
- the host's **song end** event is signalled;
- the interrupt handler then finishes as usual (the buffer, with its zero tail, is pointed at by the channels).

The next interrupt starts decoding from the beginning of the stream, so the music loops. The silence is
only the zero tail (up to a whole buffer, 81.7 ms, when `N` is a multiple of 1800). The reference host
stops rendering at the song end (it renders in 0.1 s steps and stops at the end of the step in which the
event came), but the player continues: a log that goes beyond the song end shows the restart with a full
chunk from the beginning (loop7p in LOGS.md; the DMA words equal the first buffer of the stream again).

## 4. Start-up (player initialisation and start)

In this order:

1. Decode the first 1800 bytes into X (chunk, SPEC 3; `LT = N - 1800`). The next buffer to fill is Y.
2. Register writes, with T the time of the first write (6.1):

| write | value | time |
|---|---|---|
| AUD0LCH, AUD0LCL = address of X (high word, low word; the log lists the pair at the same cc) | | T |
| AUD1LC (pair) = X | | T+8 |
| AUD2LC (pair) = X | | T+16 |
| AUD3LC (pair) = X | | T+24 |
| AUD0LEN, AUD1LEN, AUD2LEN, AUD3LEN | 900 | T+42, T+48, T+54, T+60 |
| AUD0PER .. AUD3PER | 161 (0xA1) | T+70, T+76, T+82, T+88 |
| AUD0VOL, AUD1VOL, AUD2VOL, AUD3VOL | 64, 64, 1, 1 | T+96, T+102, T+110, T+116 |
| INTENA | 0x8080 (set AUD0) | T+201 |
| DMACON | 0x800F (set channels 0 to 3) | T+211 |

3. Playback begins when the DMACON write lands: the four channels start fetching X.

The interrupt vector for channel 0 is installed by the host-visible part of the start, before INTENA, with
no register write of its own.

## 5. The audio interrupt

Each time the interrupt is taken (entry time t = the time of its first write):

1. INTENA is written 0x0080 (clear AUD0 enable) at t; INTREQ is written 0x0080 (clear AUD0 request) at t+8.
2. A chunk of 1800 bytes is decoded into the next buffer (section 3), costing the time of section 6.2.
3. The next buffer's address is written to all four channels' LC registers (pair for channel 0, then 1, 2,
   3, in this order), 8 cc apart, starting at t + 8 + B (B = the cost of step 2).
4. INTENA is written 0x8080 (enable AUD0) 40 cc after the first LC write.
5. The two buffers swap roles: the buffer just pointed at is the one now to be played (after the current one
   ends); the other one is next to be filled.

The first interrupt comes right after the start (the channel's start counts as a "buffer start"); it is
therefore taken once with X still playing, and it fills Y. The next one comes when X has finished and Y has
begun, and it fills X again, and so on; every interrupt fills the buffer that the channels will play after the
current one.

LC, LEN, PER and VOL are never written again: only the LC pairs change. A reference log is therefore: the
start-up writes, then one 11-write group per interrupt (INTENA, INTREQ, 8 LC writes, INTENA).

Interrupt entry times in the reference (they come from the host's Paula and the CPU's interrupt entry; an
implementation on the same host receives them, it does not compute them):

- first entry: on a raster-line start (a multiple of 227 cc) plus 68 cc, the first line start after the
  DMACON write that is at least about 11 cc after it; seen 91, 145, 266, 294 cc after the DMACON write
  depending on where it falls (one case, with the DMACON write 5 cc before a line start, gave a first entry
  71 cc later than the rule);
- second entry: 289 705 cc after the first (a few cc less when the first was late by the 71 above);
- later entries: exactly 289 800 cc apart.

## 6. Timing model (what the reference's CPU spends)

Times are cc; the numbers are what the reference machine charges (its CPU is modelled with fixed per-
instruction costs: a shift costs the same whatever the shift count, so the block cost does not depend on the
value of `s` except for the s = 0 case below). The model reproduces all logs within 2 cc (the residual is a
jitter of 0 or 2 cc that depends on the chip bus; it shows even between identical runs of different data).

Let `n0` be the number of blocks in a chunk whose shift `s` is 0.

### 6.1 Start-up

`T = 51 749 + 2 * n0`, where `n0` counts the first chunk's 200 blocks and T is measured on the host's clock as the
reference run started it (the host gives the player's first call a fixed phase; the constant includes the
host entry cost and the decode of the first chunk). Everything after T is relative to T as in the table in
section 4.

### 6.2 Cost B of an interrupt's decoding (from INTREQ write to first LC write)

| case | B |
|---|---|
| a full chunk (1800 bytes decoded, `LT` was 1800 or more) | 51 202 + 2 * n0 |
| `LT` is zero at entry (nothing to decode, 1800 bytes to clear) | 20 011 |
| a partial chunk of r bytes (1 to 1799, `LT = r`), m = floor(r / 9): the chunk, then 1800 - r bytes cleared | 211 + 11 * (1800 - r) + C, with C = 193 when m = 0 and C = 207 + floor(509 * m / 2) + 2 * n0 when m >= 1 |

Notes:

- A decoded block costs 254.5 cc on average (509 cycles of the reference CPU at two cycles per cc); a full
  chunk of 200 blocks costs 50 900 plus a constant. A block with shift 0 costs 2 cc more.
- Clearing costs 11 cc per byte. The 211 cc is the fixed cost of the end-of-stream case (loop reset and the
  song end call into the host).
- Verified for r = 1, 5, 8, 9, 10, 17, 18, 27, 100, 450, 899, 900, 901, 1000, 1790, 1799 and for 777 (see LOGS.md).

### 6.3 After the decode

LC writes at t + 8 + B, +8, +16, +24; INTENA 0x8080 at t + 8 + B + 40.

## 7. Stop (not logged)

On stop, the player writes DMACON 0x000F (channels off) and sets the four volumes to zero; the interrupt
removal writes DMACON 0x0001, INTENA 0x0080 and INTREQ 0x0080. This is the player's documented behaviour; the
reference host never ran it in the logs (the render stops without a stop call), so it is not verified.

## 8. What an implementation must reproduce

- the decoder of 1.3 and the chunk rules of section 3, including the unwritten bytes of a tail that is not a
  multiple of 9 (they keep the old buffer content);
- the start-up write list and the interrupt write group, in order, with LC addresses of two alternating
  buffers (consecutive blocks of 1800 bytes);
- the song end event at the end-of-stream case, the loop (restart at the first block), and the zero tail;
- the section 6 costs for register-exact timing.
