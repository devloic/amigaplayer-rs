# Sean Connolly (Sean "Odie" Connolly, 1992-95): behaviour specification

*Sanitised for publication: behaviour only.*

Format family: a four-voice game-music driver by Sean Connolly. The module is a
flat blob that contains the original driver code, its tables and the music
data. The Eagleplayer is named "Sean Connolly" (UADE player name
SeanConnolly); the file-name prefixes are `s-c.` and `scn.`. This format is
NOT the one called Sean Conran elsewhere in the project; do not confuse them.

All numbers are hexadecimal when written 0x.., otherwise decimal. "cc" is a
colour clock (the unit of the reference write logs, 2 CPU cycles each).
"Offset" means a byte offset from the start of the module file. All multi-byte
values in the module are big-endian.

The behaviour below was measured on one real module (the game Surf Ninjas,
7 subsongs) and four synthetic variations of it (see README.md and LOGS.md).
Parts that only the synthetic modules exercised are marked "synthetic only".

---------------------------------------------------------------------------

## 1. Detection

The module starts with a block of branch-opcode words: the words at offsets 0,
4 and 8 must all equal 0x6000 (each is followed by a 16-bit displacement
that is relative to the displacement word itself). Entry points are the
targets of the branches at offsets 0, 4, 8 and 12:

| entry | role |
| --- | --- |
| 0 | init(subsong), subsong number passed in a data register |
| 1 | second init for a sound-effect voice set (not used by the player) |
| 2 | play tick, called once per frame |
| 3 | stop |

Three driver generations exist. They are told apart by the first four bytes
of code at the target of the entry-0 branch, followed by the next four bytes:

| first long at init target | following long | generation |
| --- | --- | --- |
| 0x48e77ffe | 0xe98841fa | type 2 (the only one available; this spec) |
| 0x48e77ffe | 0x323c00dd | type 1 (not available, not specified) |
| 0x48e740f0 | 0x4a006b0a | type 3 (not available, not specified) |

Anything else is not a Sean Connolly module. If the corpus is searched by
file name, the prefixes `s-c.` and `scn.` are used, but the byte test above is
the authoritative one.

### 1.1 Locating the tables

The tables below live inside the module, at positions that this one module
fixes. For other type-2 modules the same tables exist but at other offsets;
the original wrapper finds them by scanning the driver code for the places
where it loads the address of each table. This was not reverse engineered
further, because only one module is available. A port should (a) parse this
module with the offsets of section 2, and (b) treat the table positions as
module parameters whose derivation from the driver code is unverified (see
README.md).

---------------------------------------------------------------------------

## 2. Module layout (offsets for the measured module)

| offset | size | content |
| --- | --- | --- |
| 0x00 | 16 | four branch words (section 1) |
| 0x10 | up to 0x13a | info text, ends at a zero long (offset 0x14a) |
| 0x14e | 0x11c | driver state area (zeroed by init, section 6) |
| 0x28a | 8 | four words 1, 2, 4, 8 (one DMA bit per voice) |
| 0x292 | 64 | "shadow" register image, 16 bytes per voice (section 7) |
| 0x2d2 | 64 | "last written" register image, same layout |
| 0x312 | 6 | noise generator state: x, y, z (three words) |
| 0x31c | 64 | noise table A (32 words) |
| 0x35c | 64 | noise table B (32 words) |
| 0x12a2 | 512 | period table, 256 words, indexed by 2*note |
| 0x1392 | 272 | pattern pointer table: 68 longs, offsets from 0x2a2e |
| 0x14a2 | 64 | sample pointer table: longs, start of sample i then start of i+1 |
| 0x14e2 | 12 | pitch-sequence pointer table: 3 longs, offsets from 0x284e |
| 0x14ee | 16 each | waveform table; wave number w is at 0x14ee + 16*w, 8 words |
| 0x25ee | 64 each | wave-sequence records |
| 0x262e | 16 each | instrument records |
| 0x2864 | 16 each | subsong table, offsets from the track base 0x28d4 |
| 0x28d4 | | track data |
| 0x284e | | pitch-sequence data |
| 0x2a2e | | pattern data |
| 0x3110 | | sample data (sample pointers are offsets from here) |

The image areas at 0x292 and 0x2d2, and the noise state at 0x312, hold the
module file's own bytes before the first init; the first play ticks emit those
bytes unchanged (see section 7.4).

The period table has 256 entries and is used as stored; the first entries are
0x3580, 0x3280, 0x2fa0, 0x2d00, 0x2a60, and the value written to PER is the
table word itself. Note numbers used below are always byte values.

### 2.1 Instrument record (16 bytes, index i at 0x262e + 16*i)

| byte | meaning |
| --- | --- |
| 0 | high nibble: speed of the pitch effect (vibrato phase length / pitch-sequence step delay); low nibble: sound mode (0 wave sweep, 1 sample, 2 chunk streaming, 3 wave sequence) |
| 1 | start volume |
| 2 | sustain target level (the level the attack climbs to) |
| 3 | release target level |
| 4 | high nibble attack rate, low nibble release rate |
| 5 | high nibble attack delay, low nibble release delay |
| 6 | high nibble times 4 = vibrato delay in ticks; low 3 bits = vibrato depth shift |
| 7 | pitch effect: 0 none, 1 vibrato, 2 pitch sequence |
| 8-9 | wave position at note start (mode 0); 0xfff selects noise |
| 10-11 | wave position minimum (signed word) |
| 12-13 | wave position maximum (signed word) |
| 14 | wave position step |
| 15 | sample index (modes 1, 2) or wave-sequence record index (mode 3) |

### 2.2 Wave-sequence record (64 bytes)

Sixteen words of wave numbers followed by sixteen words of note offsets
(period-table index offsets). A wave word of 0xffff is a jump: the next word
times 2 is the new position in the record (the position is a byte offset into
the first half). A wave number of 0xfff selects the noise tables.

### 2.3 Samples

A sample is the data between its start pointer and the next entry of the
sample pointer table. It is either an IFF 8SVX file image or a raw block. In
both cases the first 0x68 bytes are skipped when playing (for 8SVX that is the
header; raw blocks are treated the same way). A sample is 8SVX when the long
at sample offset +8 is 0x38535658. From an 8SVX header only two numbers are
used: the low word of the one-shot length (sample offset +0x16) and the low
word of the repeat length (sample offset +0x1a), both in bytes.

### 2.4 Subsong table, tracks, patterns, pitch sequences

* Subsong table: 16 bytes per subsong, four longs (voices 0..3), each an
  offset from 0x28d4 to that voice's track. Subsong count =
  (0x28d4 - 0x2864) / 16 = 7: the track data base is the end of the table.
* Track: a byte stream, section 5.1.
* Pattern: a byte stream, section 5.2. Patterns are addressed by number
  through the pattern pointer table.
* Pitch sequence: a byte stream, selected by number 0..2, section 6.5.

### 2.5 Title and length

The title is the info text starting at offset 0x10 (it runs until the zero
long; newlines in it are converted to line breaks). The module has no
length field; subsong lengths are not stored and the song ends when voice 0's
track loops (section 8). The reference host reports the info text as module
info and nothing as title.

---------------------------------------------------------------------------

## 3. Host interface (what the 68k wrapper does)

The play tick is called from an interrupt-like timer at 70940 cc per tick
(50 Hz), so the tick period of the write logs is exactly 70940 cc.

Selecting subsong N makes the host call init with 0, then 1, ..., then N
(so N+1 init calls), each of them preceded by a stop call. The stop call
writes DMACON = 0x000f, then PER = 0 and VOL = 0 for each of the four
channels (long writes to PER/VOL, logged as two lines at one clock, 8 cc
apart per channel), then DMACON = 0x000f again 22 cc later. The reference
logs therefore contain several such 10-write groups before the first play
tick; they are host behaviour, not part of the replay, and a comparison tool
should start at the first tick that carries a channel-0 LC write.

The wrapper patches the module image in three places, and these three effects
are part of what must be reproduced:

1. The driver's own hardware copy is replaced: per channel the wrapper
   writes LC, LEN, PER, VOL itself with the fixed timing of section 9.1.
2. The driver's wait-for-raster-line loop (it waits until the beam passes a
   deadline) is replaced by a fixed delay of 10 CIA-B TOD "edges" (scanline
   counter edges). See section 9.3 for the exact behaviour.
3. The driver's assignment that restarts a voice's track at its loop point is
   replaced by a routine that signals SONG END when the voice index is 0.

SongEnd is also signalled when all four voice-active flags are 0 at the start
of a tick. After a SongEnd the wrapper re-initialises the same subsong
(subsongs 4, 5 and 6 of the measured module are short and loop this way). The
reference host stops the run shortly after SongEnd for subsongs that end
(subsong 0 ended at 41.1 s); the replay itself continues.

The TOD register of CIA-B is used by the wrapper (item 2). In the reference
runs each read of the CIA-B TOD low byte returned floor(colour clock / 227),
modulo 256, i.e. it advances once per scanline (227 cc).

---------------------------------------------------------------------------

## 4. Per-voice state (vocabulary)

State exists per voice v = 0..3 unless marked global. The names below are the
spec's own.

Timing/sequencing:
* `active` (bool), `speedReload`, `speedCnt`, `lenCnt`, `lenReload`
* `repeatNote` (rest/repeat counter), `tie` flag
* `trackPos`, `patPos`, `transpose`, `instBase`, `trackRepeat`
* `inst` (current instrument number), `noteVal` (note after transpose),
  `newNote` flag, `waveOffset` (set by pattern token 0xf8)
* globals: `retrig` (4-bit mask of voices restarted this tick), `retrigPrev`
  (mask of the previous tick), two bytes `follow1`, `follow2` (the bytes that
  follow the pattern token in the track at the last fetch)

Envelope: `level` (attack level), `sustain` (target, also the decaying
level in the release phase), `releaseTarget`, `attackRate`, `releaseRate`,
`attackDelay`, `releaseDelay`, `attackCnt`, `releaseCnt`, `phase` (0 attack,
1 release).

Fade (set by the track token 0xfd, per voice): `fadeOn`, `fadeCap` (initial
0x40), `fadeCnt`, `fadeRate`, `fadeReset` flag.

Pitch: `period` (current), `basePeriod`, `slideOn`, `slideStep`,
`slideFrom` (note), `phaseCnt` (shared counter of vibrato phase /
pitch-sequence delay), `pitchSpeed`, `vibDelay`, `vibShift`, `vibDelta`,
`pitchMode` (0 none, 1 vibrato, 2 pitch sequence), `psPos`, `psSel`.

Sound: `mode` (the instrument's byte 0, only its low nibble selects the mode),
`wavePos` (word), `waveMin`, `waveMax`, `waveStep`, `waveDir`, `sampIdx`,
`chunkLen` (word), `chunkCnt` (word), mode-3: `wsPos` (position in the wave
sequence record, a byte offset), `wsLast`, `wsPrev`.

Register shadow (section 7): LC, LEN, PER, VOL per voice.

---------------------------------------------------------------------------

## 5. Streams

### 5.1 Track stream (one per voice and subsong)

At a "fetch" (section 6.2) the track is read at `trackPos`. Let t be the
current token.

1. t = 0xfe, only as the first token: the voice becomes inactive. Fetch ends.
2. t = 0xfd, only as the first token: a fade is started for ALL FOUR voices:
   `fadeOn` = 1, `fadeCap` = 0x40, `fadeCnt` = 0, `fadeRate` = the byte after
   the token. `trackPos` advances by 2 and the next byte is the new token.
3. If (t & 0xf0) = 0xf0 (one time only): `speedReload` and `speedCnt` are set to
   (t & 0x0f); advance one byte.
4. Then repeat the following group until a pass consumes no instrument-base
   token:
   * if (t & 0xc0) = 0xc0: `transpose` = t & 0x3f; advance and reload t
   * if (t & 0x80): `trackRepeat` = t & 0x3f; advance and reload t
   * if (t & 0x70) = 0x70 or (t & 0x60) = 0x60 (that is 0x60..0x7f): `instBase`
     = t & 0x1f; advance and reload t; run the group again
5. The token now at `trackPos` is a pattern number (t & 0x7f, 0..67).
   `follow1` and `follow2` are the two bytes after it in the track.
   The pattern is parsed from the voice's `patPos` (section 5.2). `trackPos`
   is not advanced here; it advances only when the pattern ends (5.3).

### 5.2 Pattern stream

Starting at `patPos` the tokens are interpreted in this order of test. Each
consumed token advances `patPos` by its length (the single exception is
section 5.3).

| token | action |
| --- | --- |
| 0xfe note speed | slide: `slideOn` = 1; `slideFrom` = (first byte + `transpose`) & 0xff; `period` = period table[2*`slideFrom`]; `slideStep` = second byte; 3 bytes consumed; the following byte is then handled as the note (note start with the slide active) |
| 0xfd n | pitch-sequence select: `psSel` = n; 2 bytes |
| 0xfc | hold: `lenCnt` = `lenReload`; no note start; go to 5.3 |
| 0xfb hi lo | chunk length: `chunkLen` = hi*256 + lo (mode 2); 3 bytes |
| 0xfa / 0xf9 | tie on / tie off (`tie` = 1 / 0); 1 byte |
| 0xf8 n | `waveOffset` = n (added to the wave-sequence note offset, mode 3); 2 bytes |
| 0xc0..0xf7 | instrument: `inst` = (t & 0x3f) + `instBase`; 1 byte |
| 0x80..0xbf | note length: `lenReload` = t - 0x7f (rows); 1 byte |
| 0x60..0x7f | repeat: `repeatNote` = (t - 0x5d) & 0x3f; 1 byte |
| 0xff | end of pattern (never reached directly; see 5.3) |
| anything else (0..0x5f) | note: note start (6.4) |

Tokens that do not start a note are followed immediately by the next token.

### 5.3 After a note (or hold)

After a note start or hold:

* If `repeatNote` is not 0, decrement it and stop: the pattern position is not
  advanced, so the same note is started again at the next fetch.
* Otherwise advance `patPos` by 1. If the byte at `patPos` is not 0xff, stop.
* The pattern has ended: `patPos` = 0. If `trackRepeat` is not 0, decrement it
  and stop (the pattern repeats).
* Otherwise `trackPos` advances by 1. If `follow1` is not 0xff, stop.
* Track loop: `trackPos` = `follow2`; `transpose` = 0; `instBase` = 0;
  `fadeOn` = 0; `fadeCnt` = 0; `fadeReset` = 1. For voice 0 this is also where
  SONG END is signalled (wrapper patch 3). Note that this happens when the
  LAST note of the last pattern STARTS, not when it finishes.

`fadeReset`, when set, is cleared at the next fetch of that voice, which also
sets `fadeOn` = 0 and `fadeCap` = 0x40.

---------------------------------------------------------------------------

## 6. The tick

### 6.1 Order of work in one tick

1. If all four `active` flags are 0: song end (nothing is written).
2. Write the previous tick's images (section 7).
3. `retrig` = 0.
4. Process voices in the order 3, 2, 1, 0 (6.2 to 6.8).
5. Noise regeneration check (6.9).
6. Wait of 10 raster edges (9.3).
7. Write DMACON (section 7).
8. `retrigPrev` = `retrig`; the shadow image is copied to the last-written
   image.

### 6.2 Voice step

For an inactive voice: set shadow PER and VOL to 0 and write DMACON = (1 << v)
(bit 15 clear, i.e. a DMA-off) right away, between the other work. Nothing else
changes.

For an active voice:
* `speedCnt` -= 1. If it is not 0, go to 6.3 (a "plain" tick).
* Otherwise `speedCnt` = `speedReload`, `lenCnt` -= 1. If `lenCnt` is not 0 go
  to 6.3 (a "row" tick).
* Otherwise this is a FETCH: if `fadeReset` is set, clear it and set
  `fadeOn` = 0, `fadeCap` = 0x40. Clear `slideOn`. Parse the track (5.1) and
  pattern (5.2). A fetch tick does nothing else for the voice: no envelope,
  no mode processing.

### 6.3 Per-tick parameter reload

Every non-fetch tick first reloads from the instrument `inst` (the
instrument number & 0x7f): `waveMin`, `waveMax`, `waveStep`, `attackRate`
(byte 4 high nibble), `releaseRate`, `releaseTarget` (byte 3), `attackDelay`,
`releaseDelay` (byte 5 nibbles), `pitchSpeed` (byte 0 high nibble).
Then, if `newNote` is set (the first tick after a note start):
clear `newNote`; `psPos` = 0; `waveDir` = 0; `sustain` = instrument byte 2;
`attackCnt` = `releaseCnt` = 0; `pitchMode` = instrument byte 7;
`phaseCnt` = 0; `vibDelay` = (byte 6 high nibble) * 4; `vibShift` = byte 6 & 7.

Fade (if `fadeOn`): if `fadeCnt` differs from `fadeRate`, increment `fadeCnt`;
otherwise `fadeCnt` = 0 and, if `fadeCap` is not 0, decrement `fadeCap`.

### 6.4 Note start

(Reached from 5.2.) Set `newNote` = 1, `phase` = 0, `chunkCnt` = 0.
`noteVal` = (note + `transpose`) & 0xff. `basePeriod` = period table
word at byte index (2*`noteVal`) & 0xff. If `slideOn` is 0, `period` =
`basePeriod`. `lenCnt` = `lenReload`. Load from the instrument `inst`
(index without masking): `mode` = byte 0, `level` = byte 1, `wavePos` =
bytes 8-9, `waveDir` = 0, `sampIdx` = byte 15. Shadow PER = `period`;
shadow VOL = byte 1, but if `fadeOn` and `fadeCap` (as a signed byte) is less
than that volume (signed), VOL = `fadeCap`.

If `tie` is set, stop here (go to 5.3): no LC/LEN change and no restart.
Otherwise the LC/LEN setup by mode (low nibble of `mode`):

* mode 0: wave number `wavePos`: if 0xfff, LC = noise table A, LEN = 0x40;
  else LC = wave table + (wavePos << 4) (the shift result is taken as a
  sign-extended 16-bit offset), LEN = 8.
* mode 1: sample start S, next-sample pointer E. If the sample is not 8SVX:
  LC = S + 0x68, LEN = (E - 2 - (S + 0x68)) / 2 (words). If 8SVX: LC = S + 0x68,
  LEN = (one-shot + repeat) / 2.
* mode 2: LC = S + 0x68, LEN = `chunkLen` / 2.
* mode 3: record R of the wave-sequence table, entry 0: wave number w and
  note offset o (first entries of the two halves): LC/LEN as in mode 0 using
  w (no sign extension quirk), shadow PER = period table[2*o], `period` =
  that, `wsPos` = 2, `wsLast` = `wsPrev` = w.

Any other value of the low nibble (4..15) behaves as mode 1.
Finally `retrig` |= (1 << v), then 5.3.

### 6.5 Envelope (non-fetch ticks, after 6.3)

Attack phase (`phase` = 0): if `attackCnt` = `attackDelay`: `attackCnt` += 1
and nothing else happens. Otherwise `attackCnt` = 0; if `sustain` =
`level`, the phase becomes release (1); else `level` = (`level` +
`attackRate`) & 0xff and the volume output is `level`.

Release phase (`phase` = 1): if `releaseCnt` = `releaseDelay`: `releaseCnt` =
0; if `releaseTarget` = `sustain`, the volume output is `releaseTarget`; else
`sustain` = `sustain` - `releaseRate` (clamped to 0 when negative) and the
volume output is the new `sustain`. Otherwise `releaseCnt` += 1.

"Volume output v": shadow VOL = v; but if `fadeCap` (signed byte) < v
(signed byte), then shadow VOL = (((inst & 0x7f) >> 4) << 8) | `fadeCap`
(the upper byte is a leftover of the original; it only matters for
comparing logs). VOL is not written on ticks where no output event happens;
the shadow keeps its value.

### 6.6 Sound mode (after the envelope)

Mode 0 (wave sweep): `waveDir` 0 (up): if `waveMax` (signed) <= `wavePos`
(signed): toggle `waveDir`, leave LC/LEN unchanged and continue with 6.7; else
`wavePos` += `waveStep`. `waveDir` 1 (down): if `waveMin` (signed) >=
`wavePos` (signed): toggle `waveDir`, leave LC/LEN unchanged and continue with
6.7; else `wavePos` -= `waveStep`.
After a move: if `wavePos` = 0xfff, LC = noise table A, LEN = 0x40; else
LC = wave table + (`wavePos` << 4) (not sign-extended here), LEN = 8. Then
6.7.

Mode 1 (sample): LC and LEN are set EVERY tick. Non-8SVX: LC = E - 4, LEN = 1.
8SVX: if repeat length (low word) <= 4: LC = E - 4, LEN = 1; else
LC = S + 0x68 + one-shot, LEN = repeat / 2. Then 6.7.

Mode 2 (chunk streaming, synthetic only): the sample (start S', end E; for
8SVX S' = S + 0x68, else S'= S). With C = `chunkLen` and k = `chunkCnt`: if
the 16-bit value (E - S' - C) is greater than the low 16 bits of (C * k):
LC = S' + C*k, LEN = C / 2, `chunkCnt` = k+1. Otherwise the stream has ended:
`level`, `sustain`, `releaseTarget` = 0, shadow VOL = 0 and the low nibble of
`mode` is cleared (the voice continues as mode 0). Then 6.7.

Mode 3 (wave sequence, synthetic only): `wsPrev` = `wsLast`. At `wsPos` read
the wave word w and the offset word o; a w of 0xffff is a jump (new `wsPos` =
next word * 2, mask 0xffff) and the read is repeated. `wsLast` = `wavePos` =
w. If w = 0xfff: LC = noise table A, LEN = 0x40; else LC = wave table +
(w << 4), LEN = 8, and if `wsPrev` was 0xfff, `retrig` |= (1 << v). Shadow
PER = period table[2 * ((o + `waveOffset`) & 0xff)]. `wsPos` = (`wsPos` + 2)
& 0x1f. Mode 3 does NOT run 6.7.

### 6.7 Pitch processing (after modes 0, 1, 2)

If `slideOn` (synthetic only for all slide cases except where noted):
* if `slideFrom` >= `noteVal`: target = period table[2*`noteVal`]; `period` +=
  `slideStep`; shadow PER = `period`; if target (signed) <= `period` (signed)
  the slide is done, else stop.
* else: `period` -= `slideStep`; shadow PER = `period`; if target (signed) >=
  `period` (signed) the slide is done, else stop.
* done: `slideOn` = 0; shadow PER and `period` = `basePeriod`.
Otherwise by `pitchMode`:
* 2 (pitch sequence): if `phaseCnt` is not 0, decrement it and stop. Else
  `phaseCnt` = `pitchSpeed`. Read the byte b at `psPos` of sequence `psSel`:
  0xff: `psPos` = next byte and re-read; 0xfe: `pitchMode` = next byte and
  stop; else n = `noteVal` + b (mod 256) if b < 0x19 or b >= 0x80, otherwise n
  = b; shadow PER = period table[2*n]; `psPos` += 1.
* 1 (vibrato): if `vibDelay` is not 0, decrement it and stop. Let n =
  `noteVal`, delta = period[n-1] - period[n] (16-bit), lim = (2*`pitchSpeed`)
  & 0xff, ph = `phaseCnt`. If ph < lim: d = delta >> `vibShift`;
  `period` = `period` + d - (d >> 1). Else if ph >= (2*lim) & 0xff:
  `period` = period[n], `phaseCnt` = 0, shadow PER = `period`, stop. Else:
  `period` = `period` - d + (d >> 1) with d as above. Then shadow PER =
  `period`, `phaseCnt` = ph + 1.

### 6.8 Row ticks

A row tick (6.2) processes 6.3 to 6.7 exactly like a plain tick; "row" and
"plain" differ only in the timing cost (section 9.2).

### 6.9 Noise regeneration

After all voices: if any voice's `wavePos` (the word, for modes 0 and 3 the
current wave number) equals 0xfff, checked in the order voice 0, 1, 2, 3
(first match is enough), the noise tables are regenerated once:

* The toggle word (state area) is inverted; if its top bit is then set, table B
  is filled, else table A.
* The table (32 words) is filled from the last word to the first: x = 3x + z + y
  (mod 65536), then y += 1, z += 2, and the new x is stored.

Initial x and y are read from the module file (offsets 0x312, 0x314) at the
very first init and then persist across inits; z is set at EVERY init to the
VHPOSR word that the CPU reads (high byte = vertical beam position low 8
bits, low byte = horizontal). Init runs the generator twice (table B, then table A). The seeds seen in the reference run (7 inits) were 0x1830, 0x453d,
0x7266, 0x9fab, 0xcd29, 0xfba5, 0x295c, which reproduced the audio data the
host fetched for noise waves. They depend on host start timing; they do not
affect register writes. The noise length of 0x40 words covers both tables.

---------------------------------------------------------------------------

## 7. Hardware writes

### 7.1 Register images

Each voice owns 16 bytes in the shadow image: LC (long), LEN (word), PER
(word), VOL (word). At the end of every tick the whole 64-byte shadow image
is copied to the last-written image. At the beginning of the next tick the
LAST-WRITTEN image is written to the hardware (so the hardware always trails
the processed state by one tick).

### 7.2 Write order within a tick

DMACON clear of the previous retrigger mask (value `retrigPrev`, bit 15
clear), then for each channel 0, 1, 2, 3 in turn: LC (one long write, logged as
two word lines, high word first), then LEN and PER (one long write, two lines),
then VOL (one word write). Voice processing follows, with direct DMACON
writes for inactive voices (6.2). The last write of the tick is DMACON =
0x8200 | `retrigPrev` (set bits); this happens after the wait (9.3).
Both DMACON writes use `retrigPrev`, the mask of the previous tick; the
mask of the current tick becomes `retrigPrev` at the end of the tick.
Initial `retrigPrev` is 0.

### 7.3 Values of LC

LC values are module-relative: the host's load address (logged in the log
header) plus the offset of the data in the module. Values in the first
ticks (7.4) are literal.

### 7.4 First ticks

Init does not touch the register images; they are part of the module data.
Tick 1 therefore writes the last-written image exactly as it is in the file,
with the original game's literal LC addresses. A new init has speed counter 4
and length counter 1, so the first fetch (the first note start) happens in
tick 4; its shadow values are copied at the end of that tick and written in
tick 5. Ticks 1 to 4 of the real module therefore carry the file's literal
values (LC not module-base relative); from tick 5 on LC values are module-base
relative. When comparing, accept the literal image value for those first ticks.

### 7.5 Init

Init(subsong s) does: copy the subsong table entry (16 bytes) into the
voices' track-base offsets; set z (6.9) and run the noise generator twice
(table B first, then table A); zero the whole state area; then for each voice 3, 2, 1, 0:
`active` = 1, `speedReload` = 6, `speedCnt` = 4, `lenCnt` = 1, `lenReload` = 1,
`fadeCap` = 0x40, all other state 0. Initial `retrig` = `retrigPrev` = 0.
Init writes nothing.

---------------------------------------------------------------------------

## 8. Song end

* Voice 0's track loop (5.3), at note start of the last note of its last
  pattern.
* All four voices inactive at the start of a tick (checked before any write).
* After either, the wrapper re-initialises the subsong; nothing in the player
  needs to know the end.

---------------------------------------------------------------------------

## 9. Timing

The tick is called every 70940 cc. Let C be the clock of the tick call
(C = clock of the first write of the tick - 362).

### 9.1 Fixed write offsets (cc after C)

| write | offset |
| --- | --- |
| DMACON (clear) | 362 |
| ch0 LC / LEN+PER / VOL | 400 / 426 / 526 |
| ch1 LC / LEN+PER / VOL | 546 / 574 / 674 |
| ch2 LC / LEN+PER / VOL | 694 / 722 / 817 |
| ch3 LC / LEN+PER / VOL | 837 / 865 / 965 |

These were identical in all 11664 measured ticks. The two lines of a long
write have the same clock.

### 9.2 Voice processing time and the first TOD read

The processing of the voices starts at about C+1056 cc and the time at
which the wait loop (9.3) takes its first TOD reading is
X = 2*C + F cycles (F in CPU cycles, 1 cc = 2 cycles). F is a sum of fixed
base and event costs, fitted to the traces; it is an approximation of
instruction timing, not exact cycle counts, but it reproduces the measured
last-read time of 11637 of 11659 real ticks exactly (22 ticks out of by a
cycle or two).

F = 7052 + sum of the event costs below (cycles). An event is counted once per
occurrence, per voice. "Plain tick" has cost 0.

Voice step:
* row tick (speed counter hits zero, length not): +81
* fetch tick: -660 (and add the fetch events below)
* newNote first tick (6.3): +328
* fade active: counter increment +81; counter reset with decrement +120;
  counter reset with cap already 0 +102

Envelope: attack step +150; attack end (phase change) +55; release hold +112;
release step +175 (+4 when clamped to 0); volume clamped by fade cap +4.

Mode: mode 0 +28 (down move +10, direction flip -121, noise wave +712); mode 1
(8SVX with repeat) +63; mode 2 chunk +173; mode 3 +179 (jump +193, noise +696,
retrigger +24).

Pitch: pitch sequence +20 (jump +87, stop +195, relative +277, absolute +252);
vibrato +49 (up +247, down +254, reset +222); slide add +81, slide subtract
+82, slide done +66.

Fetch events: fadeReset handling +189; fade token +137; speed token +101;
transpose token +83; repeat-count token +74; instrument-base token +222;
pattern lookup +511; pattern tokens: slide -81, chunk length +50, tie token
+174, wave offset +241, instrument +260, note length +273, repeat +372;
note start +1130 (+47 if fade active; mode 2 +50; mode 3 +61; mode 0 wave
-104; 8SVX mode 1 +60; tie -347); end-of-note: repeat counter active -55,
pattern repeat +71, next track step +116, track loop +332.

Per tick: noise regeneration +1408 when the first voice checked (voice 0) hits;
other positions of the hit were not separated by the fit. An inactive voice -934.

The event costs were fitted jointly (the values are not independent
measurements) so use them as a set. Costs of events never seen are 0.

### 9.3 The wait

Starting at X (a cycle count) the wrapper takes ten TOD edges. A "read" returns
floor(floor(t / 2) / 227) at cycle t. One edge wait is: read at time r1 (value
V); the second read at r1 + 16; further reads every 25 cycles until the value
differs from V (the detecting read). The next edge wait starts with a read 35
cycles after the detecting read. After ten edge waits, the final DMACON write
(0x8200 | mask) happens at floor((t_last + 87) / 2) cc, where t_last is the
cycle time of the last detecting read.

So the end of the tick depends on F through the raster phase. The value of F
can be off by a cycle without error most of the time: if the F error moves
the last detecting read across a line boundary the result changes by whole
scanlines (227 cc).

### 9.4 Off voices (synthetic only)

When voice 3 is inactive, its direct DMACON write falls at C+1178 cc; each
further consecutive inactive voice writes 122 cc after the previous one. Mixed
cases (active voices between inactive ones) were not measured. In the real
module the voices switch off only at the end (when song end follows).

---------------------------------------------------------------------------

## 10. Known gaps

See README.md.
