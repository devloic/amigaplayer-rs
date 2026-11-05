# ADPCM mono (Eagleplayer "ADPCM_mono", player name "Adaptative Delta Pulse Code Modulation") — behaviour spec

*Sanitised for publication: behaviour only.*

Reference: the 68k Eagleplayer "Adaptative Delta Pulse Code Modulation player module V1.1 (29 Sep 2014)"
(file name ADPCM_mono, UADE players directory), run through the host with DELI_WRITE_LOG / DELI_DMA_LOG.
All times are in colour clocks (cc, 3 546 895 Hz PAL).

This is not a tracker: it is a sample streamer. The 68k player itself decodes a raw 4-bit ADPCM stream
into signed 8-bit PCM in two 1024-byte chip buffers and double-buffers them on all four Paula channels,
driven by the audio channel 0 block interrupt. Nothing is decoded on the host side; Paula plays the
decoded bytes.

## 1. File format and detection

- The file is headerless: every byte is two 4-bit codes, high nibble first, then low nibble.
- Only an even number of bytes is used: an odd last byte is ignored for audio (it is still read past the
  end of the stream, see 5.3, which matters only for timing).
- Number of decoded samples `L = 2 * (filesize rounded down to even)`.
- Detection (both check entry points are the same):
  1. Reject if the first four bytes are `ADPC` (these are the ADPCM2/ADPCM3 files of another format,
     which start with `ADPCM2` / `ADPCM3`).
  2. The file NAME must end in the suffix `.ADPCM` (a suffix, not a prefix). The comparison walks the name
     backwards and has a quirk: `m`/`M`, `c`/`C`, `d`/`D`, `a`/`A` each accept either case, and the dot is
     required, but the `p` position only accepts an upper-case `P`; a lower-case `p` is accepted only as
     part of a run of lower-case `p`s that is itself preceded by an upper-case `P`. In practice:
     `x.ADPCM`, `x.adPcm`, `x.ADPcm` are accepted; `x.adpcm` (all lower case) is REJECTED. Verified on
     the reference (`sine.adpcm` rejected, `mixed.ADPcm` accepted). Exact rule, reading the name from its
     end: one of `mM`, one of `cC`, zero or more `p`, one `P`, one of `dD`, one of `aA`, then `.`.
  3. Nothing else is checked; any content is accepted.
- There are no subsongs (one song, number 0), no title, no author fields. Module info: load size = file
  size; prefix shown as `.ADPCM`.
- Duration in seconds reported by the module info: `floor(floor(L * 125 / 64) / 55420) + 1`
  (the quotient is 16-bit; irrelevant for real sizes). The duration entry point returns
  `floor(L / 1024) * 25600` (unit as the Eagleplayer duration call defines it; not used by the host's
  playback).

## 2. Decoder (bit-exact, validated)

State: predictor `P` (signed, starts 0), step index `I` (0..88, starts 0).

Step table: the standard IMA/DVI ADPCM table of 89 steps:

    7 8 9 10 11 12 13 14 16 17 19 21 23 25 28 31 34 37 41 45 50 55 60 66 73 80 88 97 107 118 130 143
    157 173 190 209 230 253 279 307 337 371 408 449 494 544 598 658 724 796 876 963 1060 1166 1282
    1411 1552 1707 1878 2066 2272 2499 2749 3024 3327 3660 4026 4428 4871 5358 5894 6484 7132 7845
    8630 9493 10442 11487 12635 13899 15289 16818 18500 20350 22385 24623 27086 29794 32767

Index adjustment by the low 3 bits of the code (magnitude m = code & 7): m = 0..3 → −1, 4 → +2,
5 → +4, 6 → +6, 7 → +8 (bit 3 does not matter).

For every 4-bit code `n` (high nibble of a byte, then low nibble):

1. `S = STEP[I]`, `m = n & 7`, `v = S * (2m + 1)`; if bit 3 of `n` is set, `v = −v`.
2. `D = floor(v / 8)` (arithmetic shift right by 3: for negative values this rounds towards minus
   infinity, so the negative difference is NOT the mirror of the positive one; e.g. step 7, m 0 gives +0
   and −1; step 8, m 0 gives +1 and −1).
3. Clamp `D` to −32768..32767 (only the largest steps exceed it). This clamp happens BEFORE adding.
4. `P = clamp(P + D, −32768, 32767)`.
5. `I = clamp(I + adjust(m), 0, 88)`.
6. Output byte = the high byte of the 16-bit `P` (i.e. `P >> 8` arithmetically, as a signed 8-bit
   sample).

Samples are written in stream order: byte k of the input gives output bytes 2k (high nibble) and 2k+1
(low nibble).

## 3. Buffers and Paula setup

Two chip buffers of 1024 bytes (512 words) each, called A (the one filled first) and B. In the reference
they are consecutive: A at 0x7FE790, B at 0x7FEB90 (the player's own memory; any chip buffers work, the
log comparison maps a constant offset).

The same buffer is played on all four channels, same period (125), same length (512 words). Volumes:
channel 0 = 64, channel 1 = 64, channel 2 = 1, channel 3 = 1. (So the left side is ch0 at 64 plus ch3 at
1, the right side ch1 at 64 plus ch2 at 1: mono.) Sample rate 3 546 895 / 125 = 28 375 Hz; one buffer
lasts exactly 1024 × 125 = 128 000 cc.

The fill routine ("fill buffer X", used by init and by the interrupt) is described in 5.

## 4. Start (init + start interrupt)

In this order (the host calls the init entry, then the start-interrupt entry):

| # | write | value | time after previous write |
|---|-------|-------|---------------------------|
| 1 | AUD0VOL | 0x0040 | (host start phase; 597..626 cc into the run in the reference) |
| 2 | AUD1VOL | 0x0040 | 6 |
| 3 | AUD2VOL | 0x0001 | 8 |
| 4 | AUD3VOL | 0x0001 | 6 |
|   | fill buffer A: decode 1024 samples (always a full decode, see below) | | |
| 5 | AUD0LC = A | hi and lo words | decode cost (5.2) − 53 |
| 6 | AUD1LC = A | | 8 |
| 7 | AUD2LC = A | | 8 |
| 8 | AUD3LC = A | | 8 |
| 9..12 | AUD0LEN..AUD3LEN | 0x0200 | 18, then 6, 6, 6 |
| 13..16 | AUD0PER..AUD3PER | 0x007D | 8, then 6, 6, 6 |
| 17 | INTENA | 0x8080 (enable audio 0 interrupt) | 85 |
| 18 | DMACON | 0x800F (DMA on, all 4 voices) | 10 |

Each LC is one 32-bit write: in the log the high word and the low word carry the same time.

The init decode is special: it always decodes 1024 samples and subtracts 1024 from the remaining-sample
count WITHOUT checking the length. For a stream of L ≥ 1024 samples (file ≥ 512 bytes) this is the normal
first buffer. For shorter files see README (it reads past the module and the remaining count wraps).

After init, the "next buffer" to fill is B.

## 5. The audio-0 interrupt (the only per-tick work)

Paula raises the channel 0 audio interrupt when DMA starts (first word fetched) and then each time it
reloads the location at the end of a 512-word block, i.e. every 128 000 cc. Because the next location is
written into the LC registers while the current block plays, the block written in interrupt k becomes the
block Paula plays after the one that started at interrupt k.

### 5.1 Sequence per interrupt

1. INTENA = 0x0080 (disable audio 0 interrupt) — 68 cc after Paula raised the request (measured on the
   first interrupt, where the request coincides with the first DMA fetch of channel 0; the reference
   timeline is consistent with the same latency for all later ones).
2. INTREQ = 0x0080 (acknowledge) — +8.
3. Fill the "next" buffer (5.2 / 5.3).
4. AUD0LC, AUD1LC, AUD2LC, AUD3LC = that buffer, 8 cc apart. The first LC write is `fill cost` after the
   INTREQ write.
5. INTENA = 0x8080 — 16 cc after AUD3LC.
6. Swap: the other buffer becomes "next".

Observed spacing of the INTENA 0x0080 writes: the second comes 128 000 − 23 cc after the first, every
later one exactly 128 000 cc after the previous one (the first block starts on DMA start, later blocks on
Paula's period counter; a Paula model reproduces it).

The buffers therefore alternate A, B, A, B ... in the LC writes: init A, interrupt 0 → B, interrupt 1 → A, ...

### 5.2 Fill (normal case) and the decode cost

Keep a read position in the stream, the decoder state (P, I) and the number of samples still to decode
`R` (starts at L; after init R = L − 1024).

Fill of one 1024-byte buffer:

- If `R == 0` at entry: the whole buffer is set to zero, the song end is reported to the host, and the
  stream restarts (read position to the start, R = L, P = 0, I = 0). Cost from INTREQ to the first LC
  write: 228 + 256 × 15 = 4068 cc.
- Otherwise let `c = min(1024, R)`. The decoder ALWAYS runs for 1024 samples (512 input bytes) from the
  read position into the buffer, even when c < 1024: past the even end of the stream it reads the odd byte
  if there is one and then whatever follows the module in memory (zero bytes in the host). R decreases by
  c and the read position advances by c samples.
  - If c == 1024: done.
  - If c < 1024 (the stream ended inside this buffer): bytes c..1023 of the buffer are set to zero
    (overwriting the over-read samples, so they are never heard), the song end is reported, and the
    stream restarts as above. Extra cost after the decode: 133 + 15 × ((1024 − c) / 4) cc.

Decode cost of one 1024-sample decode (from INTREQ to the first LC write in the interrupt; subtract 53
for the init decode, which is measured from the AUD3VOL write):

    63 507 + 10 × (samples whose P + D had to be saturated)
           + 6.5 × (samples whose I + adjust was < 0 or ≥ 88)

(note: ≥ 88, i.e. reaching index 88 exactly also counts, not only exceeding it). The counts include the
over-read samples of a final partial buffer. Round the sum to the nearest cc. Validated to ±3 cc
(mostly ±1) on 196 full decodes, 8 init decodes and 9 end-of-stream fills.

### 5.3 Consequences for the sample stream

The bytes Paula plays, concatenated, are:

    decode(stream)  followed by  zeros up to the next multiple of 1024 that is STRICTLY greater than L

and this whole sequence repeats (the decoder state is reset at each restart). So a stream whose length is
an exact multiple of 1024 samples is followed by one full buffer of silence; any other length has its last
buffer zero-padded. The song end is reported during the interrupt that fills that last (padded or silent)
buffer, i.e. about one buffer (128 000 cc) before it starts playing; the host stops shortly after the
report, so in the reference logs the padded block is only partly played.

## 6. Stop (not seen in the logs: the host stops after the song end)

- End of sound: DMACON = 0x000F, then volume 0 on AUD0VOL..AUD3VOL.
- Stop interrupt: DMACON = 0x0001, INTENA = 0x0080, INTREQ = 0x0080, then the interrupt is unhooked.

## 7. Output stage

The player is an Amiga player of PC-origin data; the reference host plays it with the A500 filter on
(auto mode reports `filter=a500(auto)` for it), LED on throughout.
