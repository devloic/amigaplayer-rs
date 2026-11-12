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
below 32 BPM it plays at 32; at 0 BPM no tick runs. The NotePlayer call moves by up to about 1100
colour clocks with the player's own time. Speed (ticks a row) starts at 6.

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
8Voices-NotePlayer genie does not play. `SPM.teinteremix22` gets the sides, `SPM.antarctica` the centre
(its channels 4-7 are silent under DeliTracker). The genie plays a lone voice on Paula channel
`voice % 4` at the voice's own period and mixes the voices that share a channel.

## 8. Known differences of the port

- Channels 4-7 always play, left/right as 0-3 (section 7).
- The mix is the host amplifier's NotePlayer genie mix (8 bits, 14477 Hz, wrapping), also for a lone
  voice.
- A single-sample bank plays; a jump to or past the end of the position list ends the song and starts
  again at position 0, with no stray patterns.
- A note with no instrument ever given plays nothing (no change bits), where the original triggers an
  empty sample.
