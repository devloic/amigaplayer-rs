# David Hanney replayer: behaviour spec (module format, replay, Paula writes, timing)

> **Sanitised for publication.** This copy was reviewed before it was added to the repository. Removed or rewritten: scratch paths, the archive location of the corpus and references to private notes only; the spec text was already behaviour-only. Numbers, formulas and behaviours are unchanged. References to `logs/` files, scripts and traces name data that is not shipped (see `docs/specs/README.md`).

Scope: the "David Hanney" player (UADE `DavidHanney`, prefix `dh.`, Eagleplayer "David Hanney";
the two are the same player). Everything here is behaviour observed from write logs, DMA logs,
execution traces and black-box runs of the 68k reference. No code, labels or addresses of the
original appear in this file.

Validation status of the model described here (a private re-implementation that follows this
spec only):

| what | result |
|---|---|
| register values and write order, 8 real modules (Tearaway Thomas), 20 s each, about 82 000 writes | 0 differences |
| same, 6 synthetic modules (random effects/notes, speeds 0, 3, 5, 8, notes up to 130, note value 2), about 40 000 writes | 0 differences |
| absolute colour-clock time of every write (section 8), the same 14 modules, about 120 000 writes | 0 differences |
| synth-wave contents changed by the "pulse sweep" (section 6.8) vs. the DMA log of the reference, about 360 000 fetched words | 0 differences |

"Validated" for a feature means a log with that feature exists (LOGS.md lists which). Features
that no log exercised are marked UNVERIFIED.

## 1. Module file format

A module is a chunk container, all numbers big endian:

    "DSNG"                       4 bytes magic
    chunk*                       repeated; a chunk is  id (4 bytes), length (u32), data (length bytes)
    0 (u32)                      terminator (an id of zero ends the list)

Chunks present in every known file, in this order:

| id | length | content |
|---|---|---|
| `SEQU` | 256 | the song order: 256 bytes, each a block number; only the first `songlen` are played |
| `INFO` | 10 | five u16: `songlen` (number of order entries played), a second value that equals `songlen` in all files and is never used by the replayer, `speed` (see 3), two zero words |
| `BLK` + byte n | 2 + 12 * rows | block (pattern) number n; the chunk id is the three characters `BLK` followed by the byte n (0, 1, 2 ...). Data: u16 `rows`, then `rows` rows of 12 bytes |

A row is four voices of three bytes: `note`, `instrument`, `effect`. Voice 0 first.

All 8 known files have 3 to 5 blocks of 64 rows (chunk length 770). There is no title, author,
instrument, sample or song-name data in the file: the instruments and the three PCM samples are
part of the replayer (section 4). One file is one song (one subsong).

### 1.1 Detection

The reference accepts a file when, walking the chunk list from the start, the chunks follow each
other in exactly this order: `DSNG` magic, a chunk `SEQU`, a chunk `INFO`, a chunk `BLK`+0, a
chunk `BLK`+1 (the first four chunk ids are compared, the length of each of the first three is
used to find the next chunk; nothing else is checked: no size, no `songlen` range). A file with
only one block is therefore rejected. The prefix `dh.` is only the file-name convention of the
UADE/Eagleplayer lists; detection does not look at the name.

### 1.2 Song length / duration

Ticks per row = `speed` + 1. Song length in ticks = sum over the first `songlen` entries of
`SEQU` of (rows of the referenced block) * (`speed` + 1). One tick is one video frame: the
reference calls the player every 70940 colour clocks (exactly 50.0 Hz at the PAL 3 546 895 Hz
colour clock). Examples: title (speed 4, 3 blocks x 64 rows) 960 ticks = 19.2 s;
jingles (speed 5, 4 x 64) 1536 ticks = 30.72 s.

### 1.3 Song end

When the last row of the last order entry (entry `songlen - 1`) is loaded, the replayer calls the
host's "song end" function, then wraps to entry 0 and keeps playing (the replayer itself never
stops). The call happens in the tick on which that last row is loaded, i.e. 4 ticks (speed 4) or
`speed` ticks before the last row's ticks are over. The reference host then stopped the
rendering a few ticks later: 3 ticks after the song-end tick in 12 of the 14 files (title: song end
in tick 956, last tick played 959), but 0 ticks after in the two synthetic files with speed 0 and
speed 8 (the rule is the host's and was not worked out). That tail is host behaviour, not player
behaviour. The block-switch also happens in this tick (section 5.4).

## 2. Replayer state

Four voices, each with its own record; a global set: master-volume table, `speed`, row counter
(tick countdown), current order position, current block, current row, "frame stamp" (a byte that
counts ticks and wraps at 256), an instrument bank (section 4).

Per voice: hardware channel (voice i drives Paula channel i); `state` (idle / start-pending /
running); the row's `note`, `instrument`, `effect`; arpeggio table row (12 offsets) and arpeggio
step (0..11); portamento speed (0 = off), portamento target period, current period; vibrato
delay counter and vibrato phase byte; envelope level (byte), envelope stage index (0, 2, 4, 6),
envelope countdown, envelope speed (default 2); retrigger countdown; base period index.

## 3. Initialisation (what the host sees)

1. The replayer builds the master-volume table (section 4.4), copies the instrument bank
   (section 4.1) and sanitises it (4.2).
2. Order position 0, tick countdown 0 (so row 0 of the first block is loaded in the very first
   tick), `speed` = INFO word 2, row 0.
3. Writes on the first call, all identical in all 14 modules (absolute colour clocks from reset,
   with the reference harness's start phase): DMACON clear of all four channels (value 0x000F)
   at 9601493, then volume 0 of channel 0, 1, 2, 3 at +105, +327, +549, +771 clocks after
   the DMACON write (9601598, 9601820, 9602042, 9602264). Nothing else is written in that call.
4. The first tick call starts at 9677798 + 30 * (k - 2) where k is the index, in the file's chunk
   list (counting `SEQU` as 0), of the chunk of the first block played (`SEQU[0]`): k = 2 for
   block 0, 3 for block 1, ... (Jingles/unused: 9677798; title: 9677828; woodland: 9677858.)
   The 30 clocks per chunk are the cost of the replayer's chunk walk (section 8.3).

The sanitising and the init host calls do not write further registers.

## 4. Data that is part of the replayer, not of the module

IMPORTANT for the implementer: all instruments and the three PCM samples are inside the original
player and are the same for every module. They are NOT described by the module and NOT given
in this spec (they are player bytes; see README for how the lead can supply them). What this
section gives is their structure and meaning.

### 4.1 Instrument bank

16 records of 0x5E (94) bytes, numbered 0..15; the row's `instrument` byte indexes this bank
(values above 15 are UNVERIFIED, the reference would read outside the bank).

| offset | size | field |
|---|---|---|
| 0x00 | u16 | type: 0 = synthesised wave; 1, 2, 3 = PCM sample 1, 2, 3 (any value >= 3 behaves as 3) |
| 0x02..0x09 | 8 | unused |
| 0x0A | 32 | wave: 32 signed bytes, the looped waveform of a synth instrument |
| 0x2A | 16 | vibrato table: 16 signed bytes |
| 0x3A | 8 | envelope: 4 stages of (level, step) byte pairs |
| 0x42 | u8 | vibrato rate (0 = no vibrato) |
| 0x43 | u8 | vibrato delay in ticks |
| 0x44 | u8 | vibrato depth (multiplier) |
| 0x45 | u8 | pulse-sweep rate (0 = off) |
| 0x46 | u8 | pulse-sweep lower bound (wave byte index) |
| 0x47 | u8 | pulse-sweep upper bound |
| 0x48 | u8 | wave length in words (the Paula length written) |
| 0x49 | u8 | vibrato phase wrap constant |
| 0x4A..0x59 | 16 | unused |
| 0x5A | u8 | pulse-sweep position |
| 0x5B | u8 | pulse-sweep direction (+1 or -1 as a signed byte) |
| 0x5C | u8 | pulse-sweep countdown |
| 0x5D | u8 | pulse-sweep "last processed frame" stamp |

In the shipped bank instruments 0, 1, 2 are the sample types 1, 2, 3 (all other fields zero) and
3..15 are synth types. All waves are 32 bytes max, `wave length` is 1..16 words.

### 4.2 Sanitising at start (synth instruments only: type 0)

Applied once to all 16 records at song start (not at each note), in this order:
wave length > 16 becomes 16; phase wrap constant > 16 becomes 16; sweep bounds >= 32 become 31;
the four envelope levels >= 64 become 63; if lower bound > upper bound the two are swapped; if
the sweep position is outside [lower, upper] it becomes lower; if the sweep countdown is larger
than the sweep rate it becomes 0; if the sweep direction is neither +1 nor -1 it becomes +1.
The bank is not re-initialised when the song wraps or at a note: the sweep position, direction,
countdown, the wave bytes already modified by the sweep, and the stamp carry over.

### 4.3 PCM samples

Three 8-bit signed samples, used by instrument types 1, 2, 3. Lengths in words: 691, 213, 1110
(1382, 426, 2220 bytes). The Paula length written for a sample note is that word count.

### 4.4 Master-volume table

64 entries t[0..63]. With master volume M (the host's volume, 64 in the reference) and
m = M - 1 (m = 0 if M = 0, and only the low 6 bits are used): t[i] = ceil(i * m / 63). For M = 64
the table is the identity (t[i] = i). All volume writes go through this table.

### 4.5 Period table (format data)

69 periods, index 0 is the lowest note. Index i+12 is the period of index i halved (twelve notes per
octave); values: 6848, 6464, 6096, 5760, 5424, 5120, 4832, 4560, 4304, 4064, 3840, 3616, 3424, 3232,
3048, 2880, 2712, 2560, 2416, 2280, 2152, 2032, 1920, 1808, 1712, 1616, 1524, 1440, 1356, 1280,
1208, 1140, 1076, 1016, 960, 904, 856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 452, 428,
404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135
(index 36 = 856 = ProTracker C-1). Notes above 70 (index above 68) are outside the format's defined range; the reference then reads other
player data as periods and that is not specified here. No known module has a note above 68.

### 4.6 Arpeggio table (format data)

Effect 3 selects row p (0..13; rows 14 and 15 are all zero, as is row 0) of 12 offsets in
semitones; the step counter walks 0..11 and wraps:

    0: 0 0 0 0 0 0 0 0 0 0 0 0
    1: 6 5 4 3 2 1 1 2 3 4 5 6
    2: 0 4 7 0 4 7 0 4 7 0 4 7
    3: 0 5 9 0 5 9 0 5 9 0 5 9
    4: 0 3 8 0 3 8 0 3 8 0 3 8
    5: 12 11 10 9 8 7 6 5 4 3 2 1
    6: 1 2 3 4 5 6 7 8 9 10 11 12
    7: 0 0 12 12 0 0 12 12 0 0 12 12
    8: 6 5 4 3 2 1 6 5 4 3 2 1
    9: 1 2 3 4 5 6 1 2 3 4 5 6
    10: 0 5 7 0 5 7 0 5 7 0 5 7
    11: 0 5 8 0 5 8 0 5 8 0 5 8
    12: 0 4 7 0 4 7 0 4 7 0 4 7
    13: 0 3 7 0 3 7 0 3 7 0 3 7

## 5. Tick processing

The host calls the player once per tick. Within one call:

1. The frame stamp is incremented (byte, wraps).
2. For voices 0, 1, 2, 3 in this order the voice step runs (5.1).
3. The tick countdown is decremented. If it did not underflow (it was >= 1), the call ends.
   If it underflowed (it was 0): it is reloaded with `speed`; so a row is processed every
   `speed` + 1 ticks, the first one in the first tick. Row processing (5.3, 5.4) follows.

### 5.1 Voice step

1. State handling for the voice:
   - idle (state 0, also 12): nothing.
   - "start pending" (state 4, set by a PCM-sample note, 6.3): write DMACON set for the voice
     (0x8000 | channel bit), then write the channel's length register = 1 word, state becomes idle.
     (The sample's location/period/volume were already written when the note was triggered.)
   - "running" (state 8, set by a synth note, 6.2): the running step (section 6).
   - state 16 (only set by an API entry the host never calls) is treated as: becomes running
     next tick. UNVERIFIED, never occurs.
2. Retrigger countdown: if it is non-zero it is decremented; when it reaches zero the
   voice's row effect is forgotten (effect byte := 0) and the voice's current note is decoded
   again as if the row had just been loaded (5.3), i.e. a note with the same note/instrument
   is triggered again with no effect (this is effect 4, section 7).

### 5.2 Order of writes in a tick

Voices are processed in index order, voice step first (5.1), then (only on a row tick) the row
phase (5.3) for voices 0..3. So within a tick the writes are: for each voice in order its step
writes (running: period, volume, DMACON-set; start-pending: DMACON-set, length 1; an effect-4
retrigger that fires writes its note-trigger writes right there, after the voice's own step
writes and before the next voice's step), then, on a row tick, the trigger writes of the voices 0..3
that have a note. A trigger writes DMACON-clear, location, length (synth), or DMACON-clear, location,
length, volume, period (PCM). A voice triggered by a synth note writes nothing else in that
tick: its first running step is in the next tick. A PCM voice writes DMACON-set and length 1 in the
next tick and nothing in later ticks.

### 5.3 Row load and decode (row tick)

The row to read is the current row of the current block; the 12 bytes are distributed to the
four voices (note, instrument, effect each). Then each voice 0..3 in order is decoded:

1. If note, instrument and effect are all zero: nothing happens (portamento, arpeggio and everything
   else keep running).
2. Otherwise portamento speed is set to 0 (portamento ends), then the effect nibble pair
   (command = high nibble, parameter p = low nibble) is handled:
   - command 1 (portamento, section 7): set speed p + 1, target = period-table entry indexed
     by the note value itself (NOT note - 2!), and the decode of this voice ENDS (no note is
     triggered, a note byte in the same cell is ignored).
   - command 2: envelope speed := p.
   - command 3: arpeggio row := row p of 4.6.
   - command 4: retrigger countdown := (`speed` + 1) >> 1.
   - command 0 or >= 5: nothing.
3. Note handling: note 0: nothing more. Note 1: DMACON-clear for the voice, state idle (a note
   off; the Paula channel stops). Note 2: nothing observable (see README). Note n >= 3: base
   period index = n - 2, current period = period table [n - 2], instrument = bank[instrument]. 
   Then the note triggers (6.2 or 6.3).

### 5.4 End of row: advancing

After the four voices are decoded: if the current row is not the last row of the block, the
row counter is incremented. If it is the last row: order position + 1; if it equals `songlen` the
song-end call is made (1.3) and the position becomes 0; the block of the new order entry is
selected, the row becomes 0. In the very tick of the last row nothing else changes; the new
block's row 0 is read at the next row tick (`speed` + 1 ticks later).

## 6. The running step of a synth voice

A running step happens in every tick after the trigger tick, for as long as the voice is
running (a synth voice never stops by itself; only note 1 or a new note changes it). Order:

### 6.1 Period

If the portamento speed is 0 (arpeggio mode): index = base index + arpeggio offset
(low byte add: the base index is below 256 in practice) at the arpeggio step, period =
table[index]; then the arpeggio step advances (wrapping from 11 to 0). The step advances
every tick, also when the arpeggio row is the all-zero row, and is not reset by notes.

If the portamento speed is non-zero: the current period moves toward the portamento target by
speed per tick and clamps on the target (never overshoots); if it already equals the target
nothing changes. While it is still moving the envelope stage index is reset to 0 (the envelope
is restarted from stage 0 on every tick of movement, the level byte is kept). The current period
is the value used (arpeggio is not applied).

Vibrato (only if the instrument's vibrato rate is non-zero): while the vibrato delay counter
is non-zero it is decremented and nothing is added. Afterwards: period += vibrato_table[phase]
(signed) * depth; then phase := phase - rate (byte), and if that subtraction borrowed
(phase < rate before), phase := phase + 2 * wrap constant (byte). The delay counter is
loaded from the instrument at note trigger, the phase is reset to 0 at note trigger. The
vibrato is added to the value that is written; the stored current period (portamento) is not
changed by it.

Then the period register is written.

### 6.2 Synth note trigger

Writes, in this order: DMACON-clear (channel bit), location = the instrument's wave, length =
the instrument's wave-length field (words). The voice becomes running. Envelope level 0, stage 0,
envelope countdown 0 (so the envelope acts in the first running step), vibrato delay and phase
reset as above. No volume/period write in the trigger tick.

### 6.3 PCM note trigger (instrument types 1..3)

Writes in this order: DMACON-clear, location = sample start, length = sample word count,
volume = table[63] (63 at master volume 64), period = the note's period. State "start pending":
see 5.1. No envelope, vibrato or portamento ever applies to a PCM voice (its running step does not
exist). The sample's length register is set to 1 word after the DMACON-set, making the loop after
the first pass a single word of the sample's start.

### 6.4 Envelope

A countdown byte is decremented every running step; when it underflows it is reloaded with the
envelope speed (2 at start of a voice, changed by effect 2) and one envelope step runs;
otherwise nothing changes. One envelope step: let target = 4 * level byte of the current stage
(byte arithmetic, level <= 63), step = step byte of the current stage, cur = level register
(0..252). If cur >= target: cur := cur - step, but not below the target (cur = target when
cur - target < step). If cur < target: cur := cur + step, but not above the target. After that, if
cur equals target and the stage index is not 6, the stage index advances by 2; at stage 6 it stays
(sustain at the final stage's level). The volume written afterwards is
table[cur >> 2].

### 6.5 Volume write, 6.6 Pulse sweep, 6.7 DMACON-set

After the period write and the envelope step, the volume register is written. Then the pulse sweep
of the instrument (6.8) runs, then DMACON-set for the voice is written (every running tick, a
redundant write).

### 6.8 Pulse sweep of the wave (instrument rate non-zero)

The wave bytes of the instrument record are modified in place; Paula fetches the new bytes
through DMA, which is what makes the sound change. Once per tick per instrument (an instrument
shared by two voices is processed only by the first voice that reaches it, tracked by the frame
stamp): the sweep countdown is decremented; if it underflowed it is reloaded with (rate - 1)
and a sweep step runs; otherwise nothing. A sweep step: if the direction is -1: store 0x3F at
wave[position]; if the position equals the lower bound the direction flips, else the position is
decremented. If the direction is +1: store 0xC0 at wave[position]; if the position equals the
upper bound the direction flips, else the position is incremented.

## 7. Effects (row column 3)

| cmd | meaning |
|---|---|
| 0 | none (also 5..15) |
| 1 p | portamento: speed p + 1, target = period-table[note]; the cell's note is not triggered. Note the target index is the note value itself while a normal note uses note - 2: a portamento to note n ends 2 semitones above the pitch of a normal note n (observed in the synthetic logs, matches) |
| 2 p | envelope speed := p (ticks between envelope steps minus one; 0 = a step every tick). Persists until another effect 2 |
| 3 p | arpeggio row p (4.6). Persists (the table row and the step counter are not cleared by later rows) |
| 4 p | the note of this cell is triggered normally and triggered again after (`speed` + 1) >> 1 ticks (p ignored). The retrigger counts from the tick after the row tick |

Effects 2 and 3 act together with a note in the same cell. Effect 1 stops the arpeggio (portamento
speed non-zero means arpeggio mode is off) until the next decoded cell sets the speed back to 0
(any non-empty cell does).

## 8. Timing: colour-clock position of every write

The reference gives each write a colour-clock time. The following model reproduces all of them
exactly (0 differences over 120 000 writes in 14 modules).

### 8.1 Tick origin

Tick k (k = 0 for the first tick) starts at T0 + 70940 * k (T0 from section 3.4). Writes are
positioned relative to a running origin:

- The origin starts at the tick start.
- For each write: the write time = origin + floor(C / 2) where C is a cycle count (68000 cycles,
  two per colour clock) = the sum of the "pair cost" (8.2) for (previous write kind, this write
  kind) plus the costs (8.3) of every event that took place since the previous write in the tick.
  After the write, origin := write time.
- Exception: after a DMACON-set write ("dmaon") the host call does not return at once: origin :=
  (floor(write time / 227) + 10) * 227, i.e. the start of the video line (227 clocks per line) 10
  lines after the line that contains the write (the call spans 2053..2267 clocks). All other
  writes return immediately (origin := write time).
- The location write consists of two halves (high word, low word) that are logged at the same
  clock; the kind "loc" below is that pair.
- Kinds: `dmaon` = DMACON write with bit 15 set, `dmaoff` = DMACON write without it,
  `loc`, `len`, `vol`, `per`; the pseudo-kind `start` = tick start (no previous write).

### 8.2 Pair costs (cycles; the cost of the code between the previous write and this one in the
baseline path of that situation, including the host call overhead of both)

| previous > this | cycles | typical situation |
|---|---|---|
| start > per | 439 | first running voice at tick start |
| start > dmaon | 302 | a PCM voice's start-pending step |
| start > dmaoff | 633 | row tick, first write is a note trigger |
| per > vol | 267 | running step |
| vol > dmaon | 253 | running step end |
| dmaon > per | 464 | next running voice |
| dmaon > dmaon | 327 | next voice is a start-pending PCM voice |
| dmaon > len | 242 | start-pending: length 1 after DMACON-set |
| dmaon > dmaoff | 658 | first note trigger after the running voices |
| len > dmaoff | 674 | next note trigger |
| len > per | 480 | next voice's running step, after a start-pending step |
| len > dmaon | 343 | start-pending step after a start-pending step |
| per > dmaoff | 678 | note trigger after a PCM trigger |
| per > per | 483 | running voice after PCM trigger/period |
| dmaoff > dmaoff | 686 | note-off or consecutive triggers |
| dmaoff > loc | 182 | trigger |
| loc > len | 210 | trigger |
| len > vol | 190 | PCM trigger |
| vol > per | 186 | PCM trigger |
| dmaoff > per | 492 | note-off followed by a running step |

Other pairs did not occur in any log. (`start > per` etc. are for the tick where nothing was
written earlier.)

### 8.3 Event costs (cycles added to the next write; gauge: baseline paths cost 0)

These are relative to a baseline path (arpeggio mode, no vibrato, envelope not stepping, no
pulse sweep, synth note, plain effect); positive or negative offsets. One term is added each time
the event occurs between two writes.

Voice step (each voice, in order):

| event | cycles |
|---|---|
| voice idle | 90 |
| no retrigger pending | 33 |
| retrigger counting down (not yet zero) | 58 |
| retrigger fires (adds also the decode terms of 5.3) | 66 |
| running, arpeggio mode | 0 |
| arpeggio step wraps 11 -> 0 | 4 |
| portamento: equal to target | -50 |
| portamento moving, still above/below target after the step | 18 / 18 |
| portamento reaches target from above | 31 |
| portamento reaches target from below | 22 |
| vibrato off | 0 |
| vibrato delay counting | 47 |
| vibrato active, phase did not wrap | 154 |
| vibrato active, phase wrapped | 186 |

Envelope (added between the period write and the volume write):

| case | cycles |
|---|---|
| not stepping this tick (countdown not yet underflowed) | 0 |
| stepping, level moves by the full step (target not reached or exactly reached), stage unchanged because it is not exactly on target after the move | 133 |
| stepping, a full step lands exactly on the target, stage advances (stage < 6) | 163 |
| same, but stage is already 6 (no advance) | 147 |
| stepping, the step would pass the target so the level is clamped to the target (from above), stage advances | 177 |
| same, stage 6 | 161 |
| stepping from below, level moves by the full step, target not reached | 133 |
| stepping from below, the step would pass the target (clamped), stage advances | 167 |
| same, stage 6 (never seen; inferred) | 151 |

"From above" means the level was at or above the target before the step, "from below" lower.

Pulse sweep (added between the volume write and DMACON-set):

| case | cycles |
|---|---|
| instrument has no sweep | 0 |
| already processed this frame for this instrument | 33 |
| countdown running (no step) | 73 |
| step, position moves (either direction) | 193 |
| step, direction turns (either direction) | 183 |

Row phase (added once per row tick before the first decode write; blocks are looked up by
walking the chunk list):

| event | cycles |
|---|---|
| a row is loaded | 723 + 60 * k, k = number of chunks in front of the current block's chunk (SEQU = 0, INFO = 1, BLK0 = 2 ...) |
| voice decode: cell empty | 89 |
| voice decode: cell not empty | 0 plus the following |
| effect 0 (none/5..15) | 0 |
| effect 1 | 265 (nothing more for that cell) |
| effect 2 | 79 |
| effect 3 | 213 |
| effect 4 | 103 |
| note 0 | 181 (then nothing) |
| note 1 | -237 (relative to a synth note trigger) |
| note >= 3, synth instrument | 0 |
| note >= 3, PCM sample 1 / 2 / 3 | -39 / -6 / 14 |
| PCM trigger: between DMACON-clear and location; and between location and length | -7 each |

The tail of a tick after its last write is not observable and is not modelled. The cost of the
song-end host call and of the block switch is therefore also not needed.

All numbers above were obtained by exact integer fitting of 499 distinct segment types and then
verified by predicting every write of the 14 logs (0 differences); the fit is not unique (the
individual terms are only known up to the sums that matter), so use the numbers together.

## 9. Differences between module files (what to expect)

Tick structure is identical for all files. Files differ in speed (4 or 5), block count, order
and which instruments/effects they use. No file uses effect 1, 2 or 4 or notes 1 or 2; those are
validated only through the synthetic modules in logs/syn.
