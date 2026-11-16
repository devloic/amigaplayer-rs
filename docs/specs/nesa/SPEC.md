# NESA (A/NES PRO `.nsa` APU logs): behaviour spec

Scope: the NESA files that the Amiga NES emulator A/NES PRO saves. A NESA file is a log of the
writes a NES game made to the APU sound registers (`$4000`–`$4017`), with a delay before each group.
This text gives the file layout and how DeliTracker's DeliPlayer `NESA_note` ("NESA v1.23") turns
the log into NotePlayer voices, so that an open replayer can be written from it.

Method: black box only. `NESA_note` (PlayerStore) ran in this project's 68000 host, alone in a
player directory (`--players DIR --eagleplayers /nonexistent`). What was observed: the DeliTracker
calls it made, the NoteStruct it filled in each tick (the host's built-in NotePlayer,
`RUST_LOG=amigaplayer_rs::notes=trace`), the memory it allocated, the Paula writes and the audio
words fetched through the default `4Voices-NotePlayer` genie (`DELI_WRITE_LOG`, `DELI_DMA_LOG`), and
the output of about 1 300 NESA files built by hand (one header field, one register, one delay
changed at a time) and 36 random ones (section 9). The player and the A/NES programs were not
disassembled, decompiled or read. No code of either is in this text. Of A/NES PRO only its
documentation text files were consulted; everything below comes from the files and the
observations. The NES facts used for comparison come from the
NESdev wiki.

The test files are the project's rips of public-domain NES homebrew by Shiru (FamiTone2 demo songs,
the cc65 example 8 music), `dl-modules/nesa/*.nsa` in the scratch tree; their sources and licences
are in `dl-modules/anes/SOURCES.md` and `dl-modules/nesa/SOURCES.md`.

Numbers: `$` is hexadecimal; multi-byte fields are big-endian. A "tick" is one call of the player's
interrupt routine. "cc" is a colour clock (3 546 895 per second).

## 1. Reference behaviour in brief

| item | finding |
|---|---|
| timing | no `SetTimer` call: DeliTracker's default 50 Hz timer (70 940 cc). One record delay unit is one tick, so the 60 Hz NES frames of the rips play 6/5 slow (the FamiTone2 songs end after 57.5 s) |
| init | `InitPlayer` allocates `$240` bytes (exec `AllocMem`), builds the waveforms (section 6), calls `dtg_GetListData` for the module |
| subsongs | one (min 0, max 0) |
| output | NotePlayer only (NoteStruct, section 7); no Paula writes of its own |
| song end | signalled through `dtg_SongEnd` at the end of the data, then the song restarts (section 3.4) |
| name | NESA3 files: none from the file (no text of the header is shown). NESA2 files: see section 2.1 |

## 2. File layout

| offset | size | meaning |
|---|---|---|
| 0 | 4 | `NESA` (the only thing the player's check looks at; a stream without this header is not taken) |
| 4 | 1 | version: `$23` (`#`, "NESA3") plays; `$22` ("NESA2") is taken but see 2.1; any other value: `InitPlayer` fails ("InitPlayer of NESA failed") |
| 5 | 1 | `$0F` in the rips; no effect seen (every value tried) |
| 6 | 1 | `$60` in the rips; bits 4–6 = which optional 32-byte header blocks follow (see below); bit 7 and bits 0–3 no effect |
| 7 | 1 | no effect seen |
| 8 | 4 | no effect seen |
| 12 | 4 | length in bytes of the record data (the trailer is not counted) |
| 16 | 32 × k | k optional header blocks, k = number of bits set among bits 4–6 of byte 6 |
| 16 + 32 k | length | record data (2.2) |
| after | 3 | `FF FF FF` in the rips; never read by the player (the end is found by the length) |

Start of the record data = 16 + 32 × popcount(byte 6 & `$70`). Checked with headers of 16, 48, 80
and 112 bytes. In the rips k = 2 and the two blocks (`$10`–`$4F`) each hold `F8 6C 5C 00` and seven
`FE 56 44 00` (probably an empty title and comment as saved by A/NES; their meaning was not needed).

### 2.1 NESA2

With byte 4 = `$22` the player initialises, takes the module name from the text at offset `$10`
(a rip with its byte 4 changed showed `øl\`, the bytes of the first header block), and then never
produced any voice change or song end with any data tried. No real NESA2 file was available. A
replayer may refuse NESA2 (open question 10.1).

### 2.2 Records

The record data is a sequence of records:

    record = delay byte, then one or more (value, code) byte pairs

| byte | meaning |
|---|---|
| delay | ticks to wait before the *next* record is applied (0 = in the same tick) |
| value | byte written to the register |
| code | bits 0–4: register number r, the write goes to `$4000 + r`; bit 5: another pair follows in this record; bits 6–7: no effect |

The rips write the registers in ascending order, as the NES code did, and FamiTone2 rewrites most
registers every frame; nothing in the player depends on that (random orders were tested).

Delay `$FF` is just 255 ticks; it does not end the song.

## 3. Sequencer

State: a read position p (from 0 at the start of the record data), a tick counter c, and an
"end next" flag.

### 3.1 Start

At init (and at every restart, 3.4) all register copies and channel state are cleared (section 5
lists what is not), p = 0, and the first delay is read: if the length is 0 the "end next" flag is
set and c = 1; otherwise c = the byte at p and p advances.

### 3.2 Each tick

1. The per-tick counters run (section 5.5, in that order).
2. c is decreased by 1. While c is 0:
   - if "end next" is set: the song ends (3.4) and nothing else happens this tick;
   - else the pairs of the record at p are applied in order (section 4), then, if p ≥ length,
     "end next" is set and c = 1; otherwise the next delay byte is read into c (a 0 makes the loop
     apply the next record in the same tick).

So the record that follows a delay d is applied d ticks after the one before it, a record with
delay 0 is applied together with the next one, and the first record is applied in tick
(first delay − 1) counting the first call as tick 0. In the rips (first delay 2) tick 0 has no
change at all and tick 1 has the first record. The end comes one tick after the last record.

### 3.3 Length beyond the file

Not probed (open question 10.3). A replayer should stop at the end of the file as it would at
the length.

### 3.4 Song end

In the tick of the end: every voice gets a volume change to 0 (change flags `$10`), DeliTracker is
told (`dtg_SongEnd`), and the state is reset as in 3.1 (this reads the first delay again). The
first record is then applied (first delay) ticks later, so the period of the song is (sum of
delays + 1) ticks. In the FamiTone2 rips the end is at tick 2875 and again at 5751; in cc65
example 8 at 2696 and 5393.

## 4. Register writes

Every write stores the value in a 32-entry register copy. What else happens:

| register | effect |
|---|---|
| `$4000` / `$4004` | pulse 1 / 2 update (5.1) |
| `$4001` / `$4005` | none (sweep is not emulated) |
| `$4002` / `$4006` | pulse update, but only if that pulse's volume nibble (`$4000` / `$4004` bits 0–3) is not 0 |
| `$4003` / `$4007` | the pulse's length reload value is set to LENP[value >> 3] (always, even when the rest of the write is ignored); then a pulse update, only if the volume nibble is not 0 |
| `$4008` | the triangle's linear value is set to value & `$7F` (5.3); no update |
| `$4009` | none |
| `$400A` | triangle length counter = LEN[`$400B` copy >> 3]; triangle update |
| `$400B` | triangle level = 46; triangle length counter = LEN[value >> 3]; triangle update |
| `$400C` | noise update |
| `$400D` | none |
| `$400E` | noise period = 500; noise update |
| `$400F` | noise length counter = LEN[value >> 3]; noise update |
| `$4010`–`$401F` (incl. `$4015`, `$4017`) | none (no DMC, no channel enable, no frame counter) |

A voice whose registers were not written in a tick, and whose counters did nothing (5.5), gets no
change flags in that tick.

Length tables (index = value >> 3):

    LEN  = 10 254 20  2 38  4 80  6 160  8 60 10 14 12 26 14
           12  16 24 18 48 20 96 22 192 24 72 26 16 28 32 30
    LENP = LEN with entry 1 = 2 instead of 254

These are the NES length-counter values except entry 4 (38, not 40: measured as 19 ticks for all
three channels) and, for the pulses only, entry 1 (runs out after one tick, where the noise and
triangle take 127).

## 5. Channels

Each "update" below fills the channel's voice (section 7) and sets change flags, which are ORed
within the tick (two updates in one tick, the first audible and the second silent, give flags `$1C`
with volume 0). Flags: `$10` volume, `$08` frequency, `$04` repeat.

An *audible* update sets flags `$1C`: repeat = the channel's waveform, period, volume.
A *silent* update sets flag `$10` and volume 0; the waveform and period of the voice are left as
they were.

### 5.1 Pulse 1, pulse 2 (voices 0, 1)

n = volume nibble (`$4000` bits 0–3), t = (`$4003` bits 0–2) × 256 + `$4002`, duty = `$4000` bits
6–7 (the registers `$4004`–`$4007` for pulse 2).

Update:

- if n ≠ 0 and t ≠ 0: audible, waveform = duty waveform d (6), period 4 t, volume n + 23
  (1 → 24 … 15 → 38); the length counter is loaded with the reload value (`$4003` write, 4);
- else silent; the length counter is left as it is;
- in both cases: the fade level = n + 23 if n ≠ 0, else 0, and the fade is stopped.

Bit 4 (constant volume / envelope) is ignored: the volume is always the nibble.
Bit 5 (length halt) is used, see 5.5.

The reload value starts at 0 and the length counter at 0, so a pulse played without any `$4003`
write fades at once (5.5).

### 5.2 Triangle (voice 2)

t = (`$400B` bits 0–2) × 256 + `$400A`.

Update: audible if t ≠ 0 and the triangle length counter > 0: waveform triangle, period 4 t, volume
= the triangle level; else silent.

The triangle level is 0 after init or restart and 46 after any `$400B` write. A file that writes
only `$400A` therefore plays the triangle at volume 0 (flags `$1C`, volume 0) until a `$400B`
write.

### 5.3 Triangle linear value and fade

The linear value (`$4008` & `$7F`, 0 at start) works unlike the NES linear counter: it is not
reloaded by `$400A`/`$400B`, it counts in steps of 4, and when it runs out it starts a slow fade of
the triangle level, the *triangle fade*, which once started stays on for the rest of the playback
(it survives song restarts; presumably a new `InitPlayer` clears it, not probed). See 5.5 for the per-tick
steps. Effects seen:

| `$4008` (bit 7 clear) | result after a `$400A`/`$400B` start |
|---|---|
| 0 to 3 | nothing |
| 4, 5 | volume 0 after 1 tick, then 45, 44, … 1, 0, one step per tick |
| 8, `$10`, `$20`, `$40`, `$7F` | volume 0 after 2, 4, 8, 16, 32 ticks, then the same ramp |
| bit 7 set | nothing (no counting) |

Once the fade is on, every later `$400B` write sets the level back to 46 and it ramps down again by
1 a tick (the voice shows 46, 45, 44, …); a `$400A` write meanwhile is an audible update with the
current level (e.g. `$1C` volume 27).

### 5.4 Noise (voice 3)

n = `$400C` bits 0–3 (bits 4–7 ignored; no envelope, no halt).

Update: audible if n ≠ 0 and the noise length counter > 0: waveform noise, period = the noise
period, volume 4 n (4 … 60); else silent.

The noise period is 0 after init or restart and 500 after any `$400E` write; the value written
(rate, mode) does not matter. A noise started without a `$400E` write since the (re)start is
reported with period 0.

The noise length counter is loaded only by `$400F` writes. It cannot be halted.

### 5.5 Per-tick counters

At the start of each tick, before any record, in this order:

1. Pulses (each): if `$4000` bit 5 is clear: if the length counter > 0 it is decreased by 2; then
   if it is ≤ 0 the fade starts. If the fade is on and the fade level > 0: level −1, and the voice
   gets flag `$10` with that level as volume (37, 36, … 0 for nibble 15). The fade does not touch
   the waveform or period; a voice that was silent (t = 0) also gets these volume changes.
2. Noise: if the length counter > 0 it is decreased by 2; on reaching 0, flag `$10`, volume 0.
3. Triangle length: if the counter > 0 and `$4008` bit 7 is clear, it is decreased by 2; on reaching
   0, flag `$10`, volume 0, but only if the triangle level is > 0.
4. Triangle fade step: if the triangle fade is on and the level > 0: level −1, flag `$10` with the
   level as volume.
5. Triangle linear value: if `$4008` bit 7 is clear and the value ≥ 4: value −4; if it is now < 4
   and the triangle fade is not yet on, the fade is switched on and the voice gets flag `$10`,
   volume 0, in this tick (the level itself is not changed until the next step 4).

Expiry times after a load in tick k: the counter reaches 0 in tick k + LEN/2 (pulse: the fade's
first step is in that tick). Rewriting `$400A`, `$400B`, `$400F`, or a pulse update that is
audible, reloads the counter, so FamiTone2 music (which rewrites them every frame) never expires.

### 5.6 Initial and restart state

Cleared at init and at each restart: all register copies, the three length counters, both pulse
reload values, the pulse fade levels and fade flags, the triangle level, the linear value, the noise
period, the read position. Not cleared by a restart: the triangle fade flag (5.3), the voices'
waveform and period fields.

## 6. Waveforms

The player builds them in its `$240`-byte allocation A (signed 8-bit samples):

| offset | bytes | repeat length (words) | content |
|---|---|---|---|
| A + 0 | 8 | – | zeros (not used as a waveform) |
| A + 8 | 8 | 4 | duty 0: `7F` ×7, `80` |
| A + 16 | 8 | 4 | duty 1: `7F` ×6, `80` ×2 |
| A + 24 | 8 | 4 | duty 2: `7F` ×4, `80` ×4 |
| A + 32 | 8 | 4 | duty 3: `7F` ×2, `80` ×6 |
| A + 40 | 16 | 8 | triangle: `00 1F 3F 5F 7F 5F 3F 1F 00 E0 C0 A0 80 A0 E0 00` |
| A + 56 | 512 | 256 | noise (below) |

The four duty waves are 87.5, 75, 50 and 25 % high (`7F`): the NES duties 12.5, 25, 50 and 75 %
(the last being 25 % inverted) with the polarity inverted, which sounds the same. Periods are 4 t for 8 samples (pulse) and 16
samples (triangle): a pulse tone has the frequency 3 546 895 / (8 × 4 t) = NES rate / 16 t
approximately, the triangle one octave lower, as on the NES.

The noise table, identical in every run (the same bytes seen in each allocation; no generator
rule was recognised in it, so it is given as data). It is output data of the player, observed in
guest memory (`fixround/nesa/ref/waveforms_568.bin`, offsets 56..567):

    cc 99 32 63 ee f4 8f 1f 3e 7c d4 a8 51 a1 43 ae 5d b9 89 7f fd fb f6 08 0f 1e 3c 77 ee b8 88 11
    21 42 84 a4 48 90 21 6a d4 53 a6 4c 98 dc 21 41 82 1a 33 4f 9e 59 b1 39 72 e4 c8 7b f6 ec 45 8a
    2a 53 10 4a 94 3e 26 4b 2e 5c 25 20 40 7f a7 4e 9c e4 ef fa f5 eb 2e 5b b6 6c 3e 7c 9d 3a 74 e8
    d0 a0 41 ea d5 ab ae 5d b9 0a 14 94 29 52 a3 ee dc b8 71 4c c3 86 0c 2e 77 d3 a7 77 ed 81 03 05
    0a 14 27 27 aa ad 5a b3 7e fc e3 c7 a7 4e 81 03 b0 88 11 22 d9 b2 50 a0 5d b9 49 92 11 21 41 81
    02 03 ef de d8 d8 b1 63 c6 8d b3 12 24 64 e0 18 30 60 26 4c 98 30 b6 c6 76 eb ee dc a3 23 dc b8
    5c b8 71 7d f9 f2 e5 e2 eb d7 d5 16 2c 58 97 44 87 f7 5a b3 66 cb c0 5a b3 bf a4 48 35 6a d4 93
    27 69 d1 f9 f3 e7 ce 9d 93 26 4c ed da b5 6a 41 d8 4a 93 7e 65 b0 60 bf e8 aa 54 a7 4e b1 63 c5
    f1 e3 ed c6 b3 83 ab b1 0f 1e d2 3b 62 c4 25 49 6a 6c d8 1d 10 20 27 f4 11 21 eb 32 fe fd d2 bb
    63 ab f2 0d f0 e1 c3 86 0c bd 7b f6 18 2f 77 ee dd bb 76 ec d9 da db e1 c3 87 77 ed da b4 ce b6
    6c d7 16 2b 56 53 a5 4a 70 e0 1d d0 f7 5b b6 6c d7 af 5e 52 cd 9a 4f 0b 16 2c 57 c8 90 20 66 cb
    bd 7b f5 eb d6 ac 59 b2 8f 1f 3d a2 6d 00 a9 7d fa f5 eb d6 82 04 6f 44 b2 64 5d 27 35 00 aa bc
    11 22 2b 2c 57 99 32 7b f5 eb d7 85 0b 01 29 69 d1 a3 9f 3e 62 5d e6 25 4a fa 8e 1c 0f 1e a3 46
    32 7d fa 50 a0 96 16 2c 57 ad 5b 9f da b4 c4 a1 42 69 6e 05 09 12 3a 5e bb 77 58 ca 2e 5b 0f 1e
    22 43 31 61 c2 85 a3 72 e4 e3 c7 e5 77 ed da 8f 3b 4d 99 9b 4c 98 16 2c 7f fd a1 43 2e 5c b8 cd
    80 5a 5f bd 7a 90 21 41 82 2a 7e a5 65 e6 24 48 76 95 d6 ac 59 cc 42 5d ba d1 c8 90 c5 35 6a 00

(This table is the one item here that is a copy of the original's data rather than a description;
it is 512 bytes of generated noise. If the project's policy wants it replaced, any 512-byte noise
loop sounds alike; bit-exact checks against the reference then cover voices 0–2 only.)

The noise pitch never changes (period 500, 256-word loop); `$400E` does not select a rate.

## 7. NoteStruct

The player's NoteStruct (DeliTracker NotePlayer interface):

| field | value |
|---|---|
| +0 | pointer to the first of 4 channel structs |
| +4 | flags `$00020232` |
| +8 | max frequency 28 867 (`$70C3`) |
| +12 | max volume 64 |

Channels (voice 0..3 = pulse 1, pulse 2, triangle, noise): stereo word `$8001` (left) for voices 0
and 3, `$7FFF` (right) for voices 1 and 2. Only the repeat fields are used: repeat start (+24) =
the waveform address, repeat length (+28, in words: 4 pulse, 8 triangle, 256 noise), period word
(+32), volume word (+36, 0..64 scale, the values of section 5 as they are). The sample fields stay
0; the change byte (+11) uses `$04` repeat, `$08` frequency, `$10` volume only. Before the first
audible update the repeat fields are 0.

## 8. Output through the default genie (for bit-exact rendering)

What `4Voices-NotePlayer` made of it, as observed in the write and fetch logs:

- At start: DMACON `$000F` off, ADKCON `$00FF`, INTREQ/INTENA `$0780`, all volumes 0; per channel a
  pair of 96-byte buffers (`$30` words, e.g. `$ADE328`/`$ADE388` for channel 0), period `$80`; DMA
  on.
- Each audio interrupt fills the channel's next buffer with 96 bytes of the current repeat loop.
  The read position runs on modulo the loop length across buffers and ticks (noise offsets 0, 192,
  384, 64, …); a repeat change (even the same waveform again) does not reset it; a new waveform
  appears from the next filled buffer. Before the first repeat the buffers are zero.
- Paula period = 3 546 895 / round(3 546 895 / P) (integer division), not below 122: 124 → 123,
  500 → 499, 1024 → 1023, 2028 → 2027, 8188 → 8191; P ≤ 120 → 122. Same rule as
  `docs/specs/robhubbard2/SPEC.md` §9.
- Volume: written as the NoteStruct value.
- In a tick, writes happen inside the player's interrupt: for each voice with a frequency change,
  period then volume (+142 cc), next voice +227 cc; a voice with a volume change only: volume +320
  cc after the previous volume; voices without change: no writes. The first write of the tick is
  about 5 400–5 900 cc after the tick start, depending on the player's work (the very first tick
  +609 cc). The dependence on the record content was not fitted (open question 10.4).

## 9. Checks a replayer can use

A Python model written from sections 3–6 (`fixround/nesa/tools/model.py` in the scratch tree)
reproduces the reference NoteStruct (change flags, waveform, period, volume of all four voices,
every tick) with no difference over:

- `ft2demo_danger_streets.nsa`, `ft2demo_after_the_rain.nsa` (2879 ticks each, one song end) and
  `cc65_example8.nsa` (2699 ticks);
- all of about 1 280 hand-built probe files (header fields, every register, delays 0/1/255, length
  0, all 32 length indices per channel, halts, the linear value, restarts);
- 36 random streams (random registers, values and delays, up to 400 records, several restarts).

Reference values (tick 0 = first call):

| file | tick | voices 0 / 1 / 2 / 3 (flags, waveform, period, volume) |
|---|---|---|
| danger streets | 0 | no change on any voice |
| danger streets | 1–32 | all `$10` volume 0 |
| danger streets | 33 | `$10` 0 / `$10` 0 / `$1C` tri 2028 46 / `$1C` noise 500 44 |
| danger streets | 36 | noise `$10` 0 (its only `$400F` ran out) |
| after the rain | 34 | `$1C` d1 1136 28 / `$1C` d2 4560 31 / `$1C` tri 1136 46 / `$1C` noise 500 32 |
| after the rain | 35 | `$1C` d1 1136 28 / `$1C` d2 4560 28 / `$1C` tri 2276 46 / `$1C` noise 500 16 |
| cc65 example 8 | 34 | `$10` 0 / `$10` 0 / `$1C` tri 2028 46 / `$10` 0 (this file never writes `$400F`: its noise never sounds) |
| all | end | danger streets and after the rain 2875, cc65 2696: all four `$10` 0 |

Reference logs (scratch tree, `fixround/nesa/ref/`): `<file>.voices.txt` (per tick and voice:
change flags, waveform offset, period, volume), `<file>.write.log` (Paula writes with cc),
`<file>.dma.log`, `<file>.genie.wav` (60 s through the genie), `waveforms_568.bin`.

## 10. Open questions

1. NESA2 (byte 4 = `$22`): nothing ever played; the layout of NESA2 data is unknown (no real NESA2
   file; A/NES PRO itself could make one in the emulator rig).
2. Byte 6 was only seen to give the number of header blocks; what A/NES stores in the blocks and in
   bytes 5, 7–11 (PAL/NTSC, title, comment?) is unknown. No PAL rip was made.
3. A length larger than the file: not probed.
4. Paula write timing within a tick against the record content (section 8) was not fitted.
5. `$4008` values 1–3 (no effect seen) and the exact linear-value arithmetic are inferred from the
   observed expiry times (step 4, stop below 4); every probe and random file agrees with that rule.
6. The noise table is the player's data (section 6).

## Errata (found while implementing, from black-box runs; 2026-11-16)

The sequencer (sections 3-6) was exact: a model from it alone matched all three voice
dumps on every tick. Section 8 (Paula write timing) was not fitted when this spec was
written; the implementation (`src/replay/nesa.rs`, refcmp rows 2008-2010 IDENTICAL)
measured:

1. The host's `--player` name is `NESA`, not `NESA_note`.
2. The first write of a tick is not at about 5 400-5 900 colour clocks: it is at the
   player's time + 505 + 104 (frequency change) or + 197 (volume only), about 4 200 cc on
   quiet ticks and up to about 9 500 on busy ones.
3. The genie divides (utility.library) for all four voices every tick, changed or not: 369,
   321 or 192 cc per voice (frequency change, volume only, no change), + 2 when the voice's
   period is 0.
4. From the last write to the end of the tick: 160 cc after a frequency change, 161 after a
   volume-only change.
5. A period of 0 is written as PER 0 (not raised to 122).
6. Audio interrupt handler: LOC written 63 cc after entry; INTREQ cleared after 367 cc
   (silence), 1263 (pulse), 846 (triangle), 478-550 (noise, by read position); exit 30 cc
   later.
7. The interrupt raised when DMA first starts is cleared by the start-up's INTREQ write and
   never handled.
8. The start-up's write times and its end set the DMA phase (Paula's line grid restarts
   there); see the implementation's start-up model.
9. The very first record of a playback costs 25 cc more than the same record after a restart.
