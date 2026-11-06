# ADPCM2 / ADPCM3 (Eagleplayer "ADPCM", and the standalone PlayADPCM) — behaviour spec

*Sanitised for publication: behaviour only.*

References: the 68k Eagleplayer "ADPCM Eagleplayer V0.85 (05.07.98)" (player name `ADPCM`, ADPCM routines
by Christian Buchner, player by Buggs of Defect), run through the host with DELI_WRITE_LOG / DELI_DMA_LOG,
and the standalone CLI player "PlayADPCM 1.1 (27.4.95)" by Christian Buchner (shipped as `PlayADPCM` with
Para Mi Amiga and as `play` with Sidewinder's Dancing Moon; the two files are identical). All times are in
colour clocks (cc, 3 546 895 Hz PAL).

These files are not music modules: they are one compressed mono sample stream. The codec is the 2-bit
and 3-bit ADPCM of the ZyXEL voice modems (the format of the AVM answering-machine voice files), with a
10-byte header. It is NOT IMA/DVI ADPCM: no step table, no index; the step size is multiplied by one
of six fixed factors after every code. The Eagleplayer decodes in 68k code into two 1024-byte chip
buffers played by two Paula channels (left and right, the same data), and needs the host to call it
back at the start of every 1024-sample block.

## 1. File format and detection

| offset | size | content |
|--------|------|---------|
| 0 | 5 | `ADPCM` |
| 5 | 1 | `2` (2-bit codes) or `3` (3-bit codes) |
| 6 | 4 | sample rate in Hz, big-endian unsigned 32-bit (Dancing Moon 16 726, Para Mi Amiga 8 526) |
| 10 | L | code stream, L = file size − 10 bytes, to the end of the file |

- Detection (Eagleplayer, the module-check entry): the first four bytes are `ADPC` and bytes 4–5 are `M2`
  or `M3`. Nothing else is checked (no size, no file name, no rate). PlayADPCM checks the same bytes
  (`ADPCM`, then `2` or `3`; anything else after `ADPCM` is "unsupported").
- Code order: ADPCM2 has 4 codes per byte, the most significant pair first (bits 7–6, 5–4, 3–2, 1–0).
  ADPCM3 packs 8 codes into each group of 3 bytes: read the group as a 24-bit big-endian number, the first
  code is bits 23–21, then 20–18, ... , the eighth is bits 2–0 (a plain MSB-first bit stream).
- Samples: ADPCM2 gives 4·L samples, ADPCM3 gives 8·floor(L/3) (see 4.3 for what the players do with a
  last incomplete group).
- One song, no subsongs, no title or author. Module info the Eagleplayer gives: samples 1, voices 1, play
  frequency = the header rate, sound system name `ADPCM2` / `ADPCM3`, and a duration in seconds:
  - ADPCM3: `floor((8·floor(L/3) + floor(rate/2)) / rate)` (rounded seconds; Dancing Moon: 218).
  - ADPCM2: intended `4·L` samples, but only the low 16 bits of L are multiplied by 4 (and kept to 16
    bits) while the high 16 bits of L stay as they are: samples = `(L & 0xFFFF0000) | ((4·L) & 0xFFFF)`,
    then the same rounding division. Para Mi Amiga 1 (L = 369 824) reports 43 s instead of 174 s. A port
    should report the correct value `round(4·L / rate)` and may note the original's figure.

## 2. Decoder (bit-exact, validated)

State: step `S` (unsigned 16-bit), predictor `P` (16-bit, wraps; no saturation). Both start at 0.

At the start of every decode call (one call per buffer fill, 4.2), if `S == 0` then `S = 5`. (`S` is never 0
in practice except at the very start and after a reset; the rule must still be applied per call.)

For each code `c` (in stream order):

ADPCM2 (2-bit code, bit 1 = sign, bit 0 = magnitude):

1. `D = floor(S / 2)`, plus `S` if bit 0 is set.
2. Multiplier `M = 0x5600` if bit 0 is set, else `0x3800`.

ADPCM3 (3-bit code, bit 2 = sign, bits 1–0 = magnitude m = 0..3):

1. `D = floor(S / 2) + m·S`.
2. Multiplier `M`: m = 0 → `0x399A`, 1 → `0x3A9F`, 2 → `0x4D14`, 3 → `0x6607`.

Both:

3. `D` is kept to 16 bits (`D mod 65536`); if the sign bit is set, `D = (−D) mod 65536`.
4. `P = (P + D) mod 65536` (interpreted as signed 16-bit). No clamping: large codes can wrap the predictor
   from positive to negative.
5. `S = floor((S·M + 0x2000) / 16384) mod 65536` (the product uses the old `S`; S·M is at most 32 bits).
6. Output sample = `P_signed >> 6` (arithmetic, i.e. floor division by 64), clamped to −128..127, as a
   signed 8-bit byte.

The six multipliers are the only table; they are also in `data/step_multipliers.bin` (six big-endian
16-bit words: the two ADPCM2 ones, then the four ADPCM3 ones).

Notes: the output is the predictor's bits 13–6 with clamping, so loud passages clip: Dancing Moon's
predictor reaches ±12 760 (±199 after the shift) and 0.73 % of its samples are clamped. With the real
files neither the predictor nor `D` ever wraps (checked over all three files), and `S` stays between 5 and
11 993. The decoded Para Mi Amiga has a constant DC offset of about −6.8, which is in the data. The decoder state is carried across buffers and only reset when the stream restarts (4.2).

## 3. What the Eagleplayer asks of the host (amplifier interface)

The player has no interrupt entry of its own. Its amplifier-init entry hands the host a tag list with:

- number of AudioStructs: 2;
- the AudioStruct array (2 × 100 bytes, cleared). AudioStruct 0: side field (offset 0x20) = +1 (left) and
  the interrupt field (offset 0x2C) = a pointer to an exec Interrupt structure (type 2, priority 1, no
  data) whose code is the player's buffer refill (4.2). AudioStruct 1: side −1 (right), no interrupt;
- flags 0x83: "direct" (the player sends its voices through the poke entries PokeAdr / PokeLen / PokePer /
  PokeVol and DMAMask, channel number 0 or 1), signed 8-bit samples, and bit 7 (meaning not known; no
  engine part read tests it).

What the host must do:

1. Treat the pokes for voices 0 and 1 as Paula channel 0 and 1 registers (the host already does this for
   direct players with up to 4 voices).
2. Call the AudioStruct-0 interrupt code (as exec's Cause() would, with its data pointer) once when voice
   0 starts playing (DMA switched on) and again every time voice 0 starts its next block, i.e. on every
   channel-0 audio interrupt of Paula. Under Eagleplayer the first call is made by its DMA-mask routine at
   the moment DMA is switched on and the later ones by the amplifier engine each time the voice wraps into
   its loop (the location last poked); on Paula, channel 0's audio interrupt gives the same sequence.
   The current host does not do this, which is why the player renders silence (it plays the zeroed first
   buffer forever).
3. Nothing else: no timer, no volume (the volume, balance and voices entries do nothing), no end routine
   (the player has no end-sound or stop-interrupt entry; the host must switch the two channels off itself).

The EP_SetSpeed entry (speed value `s` in percent) changes the period of both channels to
`per − trunc(per·s / 100)` (signed 16-bit arithmetic, truncation towards zero), at least 64. The host never
calls it.

## 4. Playback

### 4.1 Start (init + sound init + start interrupt)

- Period `per = floor(5·E / rate)` where E is the E-clock frequency (709 379 Hz PAL, so 5·E = 3 546 895;
  715 909 NTSC). The division is 32-by-16-bit: only the low 16 bits of the rate are used, and a rate
  below 55 Hz overflows (the period is then the low 16 bits of 5·E). Dancing Moon: 212 (16 730.6 Hz played),
  Para Mi Amiga: 416 (8 526.2 Hz). A rate of 0 would trap.
- Block size B = 256 bytes for ADPCM2, 384 bytes for ADPCM3: both are 1024 samples.
- Read position at the start of the code stream, decoder state reset.
- Two chip buffers of 1024 bytes, A and B (consecutive in the reference), both set to zero.
- Writes, in this order: DMA off for channels 0 and 1 (DMACON 0x0003); channel 0: location A, length 512
  words, period `per`, volume 64; channel 1: the same (location A, 512, `per`, 64); DMA on for channels 0
  and 1 (DMACON 0x8003). "Next buffer" = B.

So Paula first plays 1024 zero samples on both channels.

### 4.2 The refill (each AudioStruct-0 interrupt)

1. Take the next buffer X (B on the first call, then A, B, A, ...); write X to channel 0's location, then
   to channel 1's location (Paula will play X after the block that has just started).
2. Fill X. Let `R` = L − (bytes consumed so far):
   - `R <= 0` (the previous fill consumed the stream exactly): report song end to the host, restart
     (position to the start, decoder state reset to S = 0, P = 0), then fill X normally from the start.
   - `0 < R < B` (last, incomplete block): set all 1024 bytes of X to zero, decode `n = 3·floor(R/3)` bytes
     from the current position with the current state into the start of X (ADPCM2: 4n samples; ADPCM3:
     8n/3 samples, but at least one group of 3 bytes, i.e. 8 samples, even when n = 0: that group reads
     past the stream; the reference reads zeros there, and those samples are played), then restart (position to the start, state reset)
     and report song end. So up to 2 bytes at the end of the stream are never played (ADPCM2 loses up to
     8 samples; ADPCM3 loses the incomplete group as well).
   - otherwise: decode B bytes (1024 samples) into X, keep the new state and position.
3. Swap the next buffer.

The bytes Paula plays on channel 0 (and identically on channel 1), concatenated, are therefore:
1024 zeros, then the decoded stream in 1024-sample blocks, the last incomplete block zero-padded to 1024,
then the stream again from the start with a fresh decoder state (no silence between the loops when L is
a multiple of B; the padded block otherwise), forever.

Song end is reported during the fill of the padded last block (partial case), or during the fill of the
restarted first block (exact case); the host stops a little later. In the reference logs, song end comes
after 218.2 s (Dancing Moon), 173.5 s and 174.1 s (Para Mi Amiga 1 and 2).

### 4.3 Channel layout and output

Two Paula channels, 0 (left) and 1 (right), same buffer, same period, volume 64: mono on both sides.
Sample rate 3 546 895 / per (PAL). One block = 1024 · per cc (217 088 cc for 212, 425 984 cc for 416).

### 4.4 Timing (reference, measured on this host)

| # | write | value | time after previous write |
|---|-------|-------|---------------------------|
| 1 | DMACON | 0x0003 | start phase: 3028–3092 cc (ADPCM2), 3176–3304 cc (ADPCM3) into the run |
| 2 | AUD0LC = A | hi/lo word same clock | 7787 |
| 3 | AUD0LEN | 0x0200 | 89 |
| 4 | AUD0PER | per | 95 |
| 5 | AUD0VOL | 0x0040 | 85 |
| 6 | AUD1LC = A | | 91 |
| 7 | AUD1LEN | 0x0200 | 89 |
| 8 | AUD1PER | per | 95 |
| 9 | AUD1VOL | 0x0040 | 85 |
| 10 | DMACON | 0x8003 | 102 |

Each refill writes AUD0LC = X and, 19 cc later, AUD1LC = X (each a single 32-bit write: both words share a
clock). The AUD0LC write comes 117 cc after the host calls the refill.

- Refill 1 and later: called at Paula's channel-0 audio interrupt request, which in the host's Paula model
  falls at `F + 227 + (1022 + 1024·(k − 1))·per` for refill k (k = 1, 2, ...), where F is the clock of channel
  0's first DMA fetch (the raster decides F; it was 31–209 cc after the DMACON 0x8003 write). So refills
  are exactly 1024·per apart.
- Refill 0 (the one for the DMA start) is taken at the host's first audio-interrupt service after the start
  routine returned: 2115–2126 cc after F in the reference (host-defined).
- Fill cost: the 68k decode costs about 141–155 cc per sample for ADPCM2 and 155–173 cc per sample for
  ADPCM3 on this host's 68000 (about 145 000–177 000 cc per block, depending on the codes). While the cost
  stays below 1024·per the refills keep the times above (all real files: per 212 and 416). If it exceeds a
  block (per below about 140–173, rates above about 20–25 kHz), the location is written after Paula has
  already reloaded it, Paula plays the old buffer twice, and later refills slip by the excess (seen with a
  synthetic 22 050 Hz ADPCM3 file). A port need not reproduce this overload.

## 5. The standalone player (PlayADPCM / play)

PlayADPCM plays the same files through audio.device, not by writing Paula itself:

- Same header check, same decoder (identical routines), decoder state carried over the whole file and
  reset only per file (Para Mi Amiga's script plays part 1 then part 2: the state restarts for part 2).
- It reads the file in chunks of 4096 bytes (ADPCM2) or 6144 bytes (ADPCM3), each 16 384 samples, decodes
  each into one of two 16 384-byte chip buffers (alternating), and sends each buffer as a CMD_WRITE to two
  audio channels at once (one allocated from channels 0/3, the other from 1/2: one left, one right), with
  period `floor(5·E / rate)` (32-bit division), volume 64, one cycle, and the length in samples (4n for
  ADPCM2, floor(8n/3) for ADPCM3, n = bytes read; the last chunk is shorter). Both requests are started
  together; the next chunk of the same buffer waits until both of its writes have completed. There is no
  leading silent buffer and no loop: it plays the file once and exits.
- An ADPCM3 chunk whose length is not a multiple of 3 is decoded one group further (reading stale bytes),
  but only floor(8n/3) samples are played.
- Options FILTER / NOFILTER switch the audio filter (power LED) on / off; without them the filter is left as
  it is. It is not restored on exit.

So the Paula register writes of the standalone player are those of audio.device (ROM code), not of the
player; no player-defined register log exists for it. Its sample content equals the plain decode of the
file (section 2, one pass), played gap-free at rate 5·E / per in 16 384-sample writes.

## 6. Reproducing it in the host

Two equivalent ways, both bit-exact on the sample stream:

1. Keep the 68k player and add to the host the callback of 3 (call AudioStruct 0's interrupt on channel
   0's audio interrupt for direct-mode amplifier players). This is what produced the reference logs.
2. An open replayer: parse the header (1), decode with 2, and drive Paula channels 0 and 1 as in 4
   (start writes, one silent buffer, refill on channel 0's audio interrupt, partial-block and restart rule),
   with the times of 4.4.

For a host that implements the amplifier interface by mixing (as Eagleplayer's amplifier engines do), the
required output is: two voices, voice 0 left and voice 1 right, each the 8-bit stream of 4.2 at
3 546 895 / per samples per second, volume 64, the refill callback at every 1024-sample block of voice 0.
The engines then resample to their own mix rate; that output stage is the engine's, not the player's.

## 7. Output stage

The reference host currently classes the format as PC music (its name contains `adpcm`) and plays it
without Amiga filters (`filter=none(auto)`); the music is Amiga-made and played by Paula. The players do
not touch the LED filter (the Eagleplayer) or only on request (PlayADPCM). See README.
