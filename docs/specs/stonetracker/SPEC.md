# StoneTracker (`SPM.*` song, `SPS.*` samples) replayer: behaviour spec

*Behaviour only, from black-box observation.*

Scope: modules played by the Eagleplayer "StoneTracker" (EP 2.06: "StoneTracker player module 1.6
(29 Dec 95)", replay "Stone Player V1.98", DELIRIUM) and the DeliPlayer `DeliStone_Note`. Both are
`_note` players: they never write Paula, they fill an 8-channel NoteStruct and call DeliTracker's
NotePlayer, which mixes.

Method: the player was run in this project's 68k host on the two modules of the archive
(`SPM.antarctica`, `SPM.teinteremix22`) and on several hundred synthetic modules built once the layout
was known, and the NoteStruct (change bits, sample, repeat, period, volume, stereo position of each
channel) was read at every call of the NotePlayer; timer periods were measured from the call times.
No code of the player was read, disassembled or copied. `src/replay/stonetracker.rs` follows this text.

Status of the port against the original (NoteStruct of every tick):

| run | result |
|---|---|
| `SPM.antarctica`, 60 s (3081 ticks) | every channel change equal (change bits, sample start and length, repeat start and length, period, volume) |
| `SPM.teinteremix22`, 60 s (3081 ticks) | every channel change equal |
| 166 synthetic modules (one or a few rows of each effect and parameter range, flow, 8 channels) | 164 identical; the 2 left are in section 8 |

## 1. Hosting

The player opens `stonepacker.library` and calls its offset -30 before it loads the sample file; when
the call returns 0 it fails with "could not load additional files". The library is not part of the
host; its result is a handle the player only keeps and passes back to -36 (in a0; it never reads or
writes the handle's memory), so the host's stub answers -30 with a zeroed block of its own (src/os.rs
`stub_call`). The player then loads the sample file with plain `Open`/`Read`, calls the library at -36
and plays. The
sample file is `SPS.` followed by the rest of the name for a song named `SPM.<rest>` (any case), else
`SPS.` followed by the whole name; only that one name is tried.

## 2. Song file (`SPM`), big-endian

| offset | size | content |
|---|---|---|
| 0x00 | 4 | `SPM\x02` |
| 0x04 | 32 | module name, NUL padded |
| 0x24 | 2 | number of songs (subsongs 1..n) |
| 0x26 | 2 | number of patterns - 1 |
| 0x28 | 2 | 1 in both files (0 breaks playback, 2 and 3 change nothing seen) |
| 0x2A | 2 | rows per pattern (64) |
| 0x2C | 4 each | absolute offsets: one info block, then each song block, then each pattern |

Info block: a tag list (u16 tag, u32 value, ..., 0) and text; playback does not need it.

Song block:

| offset | size | content |
|---|---|---|
| 0x00 | 30 | song name |
| 0x1E | 1 | unknown (no effect seen) |
| 0x1F | 1 | tempo in BPM |
| 0x20 | 1 | channels (8; with 4, channels 4-7 stay unused; 2 crashes the player) |
| 0x21 | 1 | 0xFF: an extra position list follows the channel lists |
| 0x22 | 2 | number of positions `npos` |
| 0x24 | 2 x npos x channels | channel-major pattern lists: channel c plays pattern `list[c][pos]` |
| after | 2 x npos | extra list (zeros in the files; other values hang the player) |

A pattern holds one channel's rows. It is a stream of two-byte units `b0 b1`:

- `b0 = 0xFF`: `b1 + 1` rows pass; the first of them is the row being built, if it has units;
- otherwise `k = b0 & 0x7F`: `k <= 0x24` is a note `k` (1..36, 0 = none) with instrument `b1`
  (0 = none); `k >= 0x25` is effect `k` with parameter `b1`; bit 7 of `b0` ends the row.

All patterns of both files decode to exactly 64 rows ending at the next block.

## 3. Sample file (`SPS`)

`SPS\x02`, u16 count, then `count` 32-byte entries, then the sample data. Entry: name (8), u32 start,
u32 length (bytes), u32 repeat start (bytes, from the data's start, not the sample's), u32 repeat
length (bytes, 0 = no repeat), u16 volume (0..64), i16 finetune, u32 zero. Offsets and lengths may be
odd. The player reads `start + length` of the last entry bytes of data; with a single entry it reads
none (that sample is silent).

## 4. Timing

The player programs CIA-B as a one-shot itself each tick. The tick period is
`5 * (floor(1789773 / bpm) - 42)` colour clocks (checked at 32, 33, 45, 64, 77, 125 and 200 BPM);
below 32 BPM it plays at 32; at 0 BPM no tick runs.

The player is exact: the CIA-B interrupts of an undisturbed tick are 71380 colour clocks apart at
125 BPM (it measures the time since the interrupt itself and shortens the next timer by it). In the
reference (the player under the 8Voices-NotePlayer genie) a tick is often later: the genie's audio
interrupt handlers (the writes of its channel set-up, each audio interrupt acknowledged 3000 to
3400 colour clocks after the last) hold the timer interrupt back until the handler that straddles
the expiry has finished, and the next period then counts from the late interrupt. Measured over
12 to 20 s on `SPM.teinteremix22` and `SPM.antarctica` at 60, 100, 125, 150 and 200 BPM: mean tick
longer than the formula by 1543 / 2000 (125), 1323 / 627 (60), 4286 / 1128 (100), 1466 / 970 (150),
2333 / 3132 (200) colour clocks (0.4 to 7 %; 2.2 % and 2.8 % at 125 BPM, which is the drift the
port had). The delays follow the genie's own interrupt timing (a chain of one handler a channel,
3000 to 3400 colour clocks apart, started by each note), so a tick's lateness is neither a function
of the tick's changes nor of the tempo. The port gets it by playing the genie's handlers with their
measured durations (section 9), which hold its ticks back as the genie's hold the original's. The
NotePlayer call moves by up to about 1100 colour clocks with the player's own time. Speed (ticks a
row) starts at 6.

The first tick sets every channel to period 256 and volume 0 (change bits FREQ | VOL) and plays row 0
of position 0.

## 5. Periods and notes

Period of note `n` (1..36) at finetune `f` (-16..15, eighths of a semitone):
`round(856 * 2^(-(8 (n - 1) + f) / 96))`, never below 113. This equals all 1152 measured periods
(32 finetunes x 36 notes). Finetunes of +-16 and beyond give unrelated values; -32 and -128 act
as -16.

NoteStruct change bits: 0x02 sample, 0x04 repeat, 0x08 period, 0x10 volume, 0x20 trigger.

- note with instrument: sample, repeat, period, volume (the instrument's), trigger (0x3E);
- note without instrument: the last instrument's sample, period, trigger (0x2A): the volume stays;
- instrument without note: volume (the instrument's) and repeat, no trigger (0x14);
- an instrument above the count: the note and instrument are ignored (the effects act);
- a note when no instrument was ever given: a trigger of an empty sample at a stray period (silent).

## 6. Effects (`k`, parameter `p` = `xy`)

"Ticks" are the ticks of the row, 0 the first. Unless said, an effect changes nothing on tick 0.

| k | effect |
|---|---|
| 0x25 | arpeggio: tick % 3 = 0 the period, 1 / 2 the note `x` / `y` semitones above the first note of the finetune's row whose period is not above the current one; period bit every tick > 0, also for `p` = 0; no memory |
| 0x26 | slide up: period - p (16 bits, wrapping), raised to 113 if below; every tick > 0, bit set also for 0 |
| 0x27 | slide down: period + p, at most 961 |
| 0x28 | tone portamento: a note is not triggered (with an instrument: repeat, period and volume bits); speed `p` (0 keeps the last); target the note at the (new) instrument's finetune; moves by the speed to the target, stops there (no bit once there); a row without a note also moves on tick 0 |
| 0x29 | vibrato: `x` speed, `y` depth (0 keeps each); offset `(wave(pos) * depth) >> 7` added to the period, then pos += speed (mod 64); not on tick 0; the position is not reset by a note |
| 0x2A | 0x28 going on, with the volume slide `p` (also on tick 0 of a row without a note) |
| 0x2B | 0x29 going on, with the volume slide `p` |
| 0x2C | tremolo: as the vibrato on the volume, `>> 6`, kept in 0..64 |
| 0x2D | nothing seen |
| 0x2E | sample offset `p * 256`: start moves on, length shrinks; an offset at or past the length plays 2 bytes from the start; no memory |
| 0x2F | volume slide: `x` up, else `y` down, ticks > 0; `p` = 0 nothing |
| 0x30 | position jump to `p`, row 0 (the break row with 0x32); to `npos` or past it the player reads past the lists (stray patterns; one probe ended the song at once, another played stray data and did not end) |
| 0x31 | volume `p` (at most 64) |
| 0x32 | pattern break to row `p` (not BCD; past the last row: 0) |
| 0x33 | the note is not triggered: repeat, period and volume bits (with an instrument); `p` != 0 sets the new period, 0 keeps the old one |
| 0x34 | 0: the song ends and stops; below 0x20 speed; else BPM |
| 0x35 | (LED) nothing seen |
| 0x36 / 0x37 | fine slide up / down by `p` on tick 0 (bit also for 0) |
| 0x38 | glissando on (`p` != 0): the tone portamento's output snaps to the first period of the row not above it |
| 0x39 | vibrato waveform: 0 sine (ProTracker's), 1 ramp (16 pos, pos - 64 from 32), 2 square (255 / -256), others 0 |
| 0x3A | finetune `p` (0..15) for this note; above 15 ignored |
| 0x3B | pattern loop (ProTracker's E6x) |
| 0x3C | tremolo waveform (as 0x39) |
| 0x3D | nothing seen |
| 0x3E | retrigger every `p` ticks (tick > 0): sample and trigger bits |
| 0x3F / 0x40 | fine volume up / down on tick 0 (bit also for 0) |
| 0x41 | cut: volume 0 at tick `p`; `p` = 0: no trigger, repeat, period and volume (0) |
| 0x42 | note delay `p` (0 or not below the speed: none): tick 0 period, volume 0 (and repeat with an instrument), volume bit each tick, then at tick `p` sample, volume and trigger |
| 0x43 | pattern delay: the row lasts `p` more rows |
| 0x44-0x47 | nothing seen |
| 0x48 | volume scale: `(vol * p + 32) >> 6`; `p` > 64 ignored |
| 0x49 | volume bit every tick |
| 0x4A | fade: on every tick t from 0, volume = v0 - round((t + 1) p v0 / 64), not below 0 (v0 the volume at the row's start); `p` = 0 sets the volume bit every tick |
| 0x4B | volume bit on tick 0 |
| 0x4C | one fade step on tick 0: volume - round(p * volume / 64) |
| 0x4D, 0x4E, 0x50-0x54, 0x59 | nothing seen |
| 0x4F | song end (playback goes on) |
| 0x55 / 0x56 | retrigger a semitone up / down: with `p` (0 < p < speed) the note is delayed as by 0x42 and retriggered at tick `p` a semitone up / down (sample, period, volume, trigger); without a note and no delay: at once on tick 0; with a note and no delay: a plain note |
| 0x57 / 0x58 | tone portamento (speed `p`, 0 keeps) to a semitone above / below the note, or above / below the current period on a row without a note; period bit on tick 0 |

Vibrato sine: `0 24 49 74 97 120 141 161 180 197 212 224 235 244 250 253 255 253 250 244 235 224 212
197 180 161 141 120 97 74 49 24`, negated for positions 32..63.

No effect restores the period or the volume when it stops: the last output stays until a new one.

Flow: at the end of a row, a pending jump or break moves there; otherwise the next row, and after the
last row the next position. After the last position the song ends and starts again at position 0.

## 7. Voices and stereo

Channels 0-3 get the stereo positions left, right, right, left (0x8001 / 0x7FFF). Channels 4-7 get
the same only when a byte the player reads outside the module during the first tick (at a fixed
address plus the song block's file offset) is negative; otherwise the centre (0x8000), which the
8Voices-NotePlayer genie does not play (silent).

That byte lies in the sample file, which the host loads after the module: it is the sample file's
byte at file offset `song + 14086 - max(0, round8(len) - 1024)`, `song` the song block's file
offset and `len` the module's length (probed with modules of 144 to 12144 bytes and with
two to six samples: the number of samples does not matter, the module's length does only above
1024 bytes, by its length rounded up to 8; the byte is found by bisection with a sample file of
zeros up to a point and 0x80 from it: voices 4-7 audible or silent). Negative (0x80 or above): the
sides; else the centre. Both archive modules agree (`SPM.antarctica` 6060 bytes, song block 424:
byte 9474 = 0x04, the centre; `SPM.teinteremix22` 824 bytes, song block 154: byte 14240 = 0xD6, the
sides). Past the end of the sample file the byte is not known (the port assumes the centre). `SPM.teinteremix22` gets the sides, `SPM.antarctica` the centre
(its channels 4-7 are silent under DeliTracker). How the genie plays the voices: section 9.

## 8. Known differences of the port

- The genie's handlers and the tick take the reference's most frequent times (section 9); the
  reference's take a few hundred colour clocks more at some effects, and its Paula fetches fall up
  to a raster line (227 colour clocks) away from the port's, set by when the host started it. So a
  tick's change can reach a buffer the port fills one handler earlier or later (a volume slide one
  step apart for one buffer).
- Where the rule of section 7 reads outside the sample file the port plays the centre (silent).
- A single-sample bank plays; a jump to or past the end of the position list ends the song and starts
  again at position 0, with no stray patterns.
- A note with no instrument ever given plays nothing (no change bits), where the original triggers an
  empty sample.

## 9. The 8Voices-NotePlayer genie (v2.10), measured

Measured as a black box from the reference's Paula register writes (`DELI_WRITE_LOG`), Paula's DMA
fetches (`DELI_DMA_LOG`) and the NoteStruct at each NotePlayer call, on both archive modules (20 s)
and synthetic modules (one or two voices, sides byte set or not, periods 113 to 961, volumes, one-shot
and looped samples of odd and even lengths). Colour clocks throughout.

Buffers. Each Paula channel plays a double buffer of 192 bytes (LEN 0x60). The channel's audio
interrupt (a buffer started) runs a handler that writes PER, VOL 10 later, then LC of the other half,
which it fills, and acknowledges the interrupt (INTREQ) last. The PER and VOL written are those of the
buffer filled by the handler before (the one now starting). An interrupt the channel raises while its
own handler runs is lost: the acknowledge clears it (the next comes a buffer later).

Periods. The host gives the genie a voice's frequency `round(3546895 / per)`; the genie's period is
`floor(3546895 / f)`.

Mode A (voices 4-7 centred): Paula channel `k` plays voice `k` alone: PER the voice's genie period,
VOL 64, each byte `(s * vol) >> 6` (arithmetic), one source byte a buffer byte. A voice that is off
gives zeros.

Mode B (sides): channel 0 plays voices 0 and 3, channel 1 voices 1 and 2, channel 2 voices 5 and 6,
channel 3 voices 4 and 7. PER is the smaller of the two voices' genie periods (a voice that is off
counts as period 256), VOL the larger volume `vmax`; each voice adds `(s * (vol * 64 / vmax)) >> 6`,
the sum wrapping at 8 bits. A voice at the channel's period copies one byte a buffer byte from the
first; the other steps by `floor((PER << 16) / per) + 1` in 1/65536 of a byte, the fraction added
before each byte. `vmax` 0: VOL 0; with both voices off the buffer is zeros, else the voices play
on unscaled (`s` itself; their positions move as at any volume) and are not heard at VOL 0.

Samples. Starts are rounded down to even, lengths up to even (a one-shot of 601 bytes plays 602); a
repeat of length 0 is silence after the sample.

Handler times (PER to acknowledge, the most frequent): mode A 5824 for a voice that is off, 4645 at
volume 64, else about 5549 + 0.894 per period unit (5662 at 127, 5702 at 170, 5778 at 254, 5854 at
340, 6090 at 604); mode B 3296 silent, 5822 at the channel's period, 6291 when a voice is resampled,
4743 for voices playing at volume 0 (LC at 246).
Each channel's first handler is shorter: 142 (mode A), 191 (mode A, voice off), 147 (mode B), 96
(mode B silent). LC comes 352 after PER in mode A, 347 in mode B, 174 in mode B silent. An interrupt
raised while a handler or the tick runs is taken 68 after its acknowledge (the next handler's PER);
an interrupt that finds nothing running has its PER 100 later.

Ticks. A tick held back by a handler starts 82 after its acknowledge; ticks are otherwise the
player's own period apart from the last start. From the tick's start to the NotePlayer call and to
the tick's end: mode A 1417 and 2238, at a row 1788 and 2609; mode B 1689 and 2874, at a row 2151 and
3336 (ticks with effects take up to a few hundred more); the next handler's PER is 86 after the
end. The first tick comes after the genie's set-up: DMA off, the four channels on a zero buffer
(LOC, LEN 0x60, PER 128, VOL 0 at 2834 + 47c and 8 apart, from the set-up's first write), DMA and
audio interrupts on at 3369, the tick 593 later; mode B's channel writes and DMA come 652 later. The
first tick's NotePlayer call and end: 5349 and 7098 (mode A), 5448 and 7993 (mode B).
