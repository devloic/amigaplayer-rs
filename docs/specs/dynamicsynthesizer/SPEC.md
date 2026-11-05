# Dynamic Synthesizer (Chris Huelsbeck, 1987-88) - behaviour specification

*Sanitised for publication: behaviour only.*

Player name: Dynamic Synthesizer. Module prefix: `dns.` (companion sample file prefix `smp.`).
Author string used by the player: Chris Huelsbeck. The format has no title text and no
per-module author text; the song name to display is simply the player name.

Everything below was measured from two real modules (the title tune and the in-game tune of
one game, 8 and 5 subsongs) by running the reference 68k player through the host with
DELI_WRITE_LOG, and checked by a private model (see README). Numbers in this file that are
timings are 68000 CPU cycles unless marked "cc" (colour clocks, 1 cc = 2 cycles).

## 1. Module files

A song is two files: `dns.NAME` (the module) and `smp.NAME` (raw 8-bit sample data). The
player maps the prefix `dns.` to `smp.` for the sample file name (case-insensitive on the
three letters, so `Dns.` / `DNS.` also map). The sample file is loaded into chip RAM; every
sample reference in the module is an offset into it (section 4.3). The host chooses where the
sample file lives; in the reference logs it is at 0x71A8 (title) and 0x8D68 (in-game), so a
comparator must not treat the base as part of the format.

The module itself is a complete, position-dependent 68000 program (the game's own driver plus
its data). It is linked for address 0x1F400: a pointer stored in the file maps to file offset
`pointer - 0x1F400`. The port does not run it; it interprets the data tables described below.

### 1.1 Detection

The file starts with five consecutive 16-bit words equal to 0x6000, each followed by a
16-bit displacement that is non-negative and even (these are the five entry slots, at file
offsets 0, 4, 8, 12, 16). At the address obtained by adding the fifth displacement (stored at offset 18) to the
address of that displacement word (offset 18) there must be the word 0x23C8, then a 32-bit operand, then the
word 0x4E75. In both known modules the slots are: tick (offset 0), init (4), sound effect
(8), start-song (12, D0 = subsong number), set-sample-base (16). Only the data tables are
needed by a port.

### 1.2 Subsong count

The subsong table (section 2) is five parallel arrays of 16-bit words, each array 0x20 bytes
after the previous one. The first array holds the start line of each subsong. The number of
subsongs is the smallest n >= 1 such that entry n and entry n+1 of the first array are equal
(capped at 32). Title module: 8; in-game module: 5. Subsongs are numbered from 0.

### 1.3 Song length / end

There is no stored duration. The song-end event (the wrapper's SongEnd) is raised in the tick
in which the sequencer loads its final line (the line whose index equals the subsong's "end
line", section 2) and so wraps back to the loop line. In the reference logs of long runs the
host stops about two ticks after that tick. Writes before and after the wrap are unaffected
(the sequence just loops). A length estimate is therefore the number of ticks from tick 0 up to
and including the tick that loads the end line; compute it by running the sequencer
(section 3) without emitting writes.

## 2. Tables inside the module

All offsets are file offsets. These offsets differ per module and were taken from the two real
modules; a third module needs them located (see README, "unknowns").

| table | title | in-game | content |
|---|---|---|---|
| period table | 0x0D64 | 0x1950 | 48 words, then reads run on into the sequence data (see below) |
| sequence lines | 0x0DC4 | 0x19B0 | 24 bytes per line |
| pattern pointer table | 0x1324 | 0x28B8 | 32-bit link addresses, entry i is pattern i+1 |
| subsong table | 0x1244 | 0x27D8 | five arrays, 0x20 bytes apart |
| instrument table | 0x1FD4 | 0x39C0 | 0x60 bytes per instrument |
| volume boost shift | 0 | 2 | see 4.3 |

Period table values (identical in both modules for the first 48 entries):
1710 1614 1524 1438 1357 1281 1209 1141 1077 1017 960 908 856 810 764 720 680 642 606 571
539 509 480 454 428 404 381 360 340 320 303 286 270 254 240 227 214 202 191 180 170 160 151
143 135 127 120 113. The table is read as 64 words indexed by `note & 0x3F`; entries 48-63 are
simply whatever 32 bytes follow it in the file (they are the first bytes of the sequence
lines; do not special-case them, read them from the file).

Subsong table, for subsong s, using word index s in each array:

| array (offset from table) | meaning |
|---|---|
| +0x00 | start line index |
| +0x20 | loop line index (line to continue at after the end line) |
| +0x40 | end line index (last line played before SongEnd) |
| +0x60 | speed: ticks per row (>= 1) |
| +0x80 | track mute mask: low byte of the word; bit t set = track t (0-5) is skipped |

Sequence line (24 bytes): six 4-byte entries, one per track t = 0..5:
byte 0 = pattern number (0 = track empty on this line, else pattern pointer table entry
number-1), byte 1 = transpose added to notes, byte 2 = instrument offset added to the
instrument number, byte 3 unused.

Pattern data (at the pointed-to link address minus 0x1F400): 16-bit big-endian words, one
per row, terminated by the word 0xFFFF. Row word:

| value | meaning |
|---|---|
| 0x0000 | rest: nothing happens, the previous note keeps playing |
| 0x8000 | note off for this track |
| other | high byte = note, low byte: high nibble = volume boost, low nibble = instrument |

Instrument record (0x60 bytes, offsets in bytes; words are 16-bit big-endian, "s" = signed):

| offset | size | meaning |
|---|---|---|
| 0x00 | long | sample start offset (into the sample file) |
| 0x06 | word | sample length in words (value for AUDxLEN) |
| 0x08 | long | loop start offset (into the sample file) |
| 0x0E | word | loop length in words |
| 0x18 | word | attack interval (ticks per envelope step) |
| 0x1A | s word | attack delta |
| 0x1C | s word | start volume |
| 0x1E | s word | attack target volume |
| 0x20 | word | decay interval |
| 0x22 | s word | decay delta |
| 0x24 | word | jump-on-release flag (1 = on release the volume jumps to the sustain level first) |
| 0x26 | s word | sustain target volume |
| 0x28 | word | release interval |
| 0x2A | s word | release delta |
| 0x4A | word | period detune added to the table period |

## 3. Replay: one tick

The player is called once per frame, 50 Hz, exactly 70940 cc apart (the wrapper's "100 Hz"
remark is not honoured; verified: one call per 70940 cc). A tick does, in this order:

1. Tempo: a counter (initially 1) is decremented; when it reaches 0 it is reloaded with the
   subsong speed and a ROW is processed (3.1). So tick 0 is a row, and rows follow every
   `speed` ticks.
2. Only after the first note has ever started (the "active" state, set by any note start and
   never cleared) the following are executed every tick, in this order, otherwise the tick
   produces no register writes at all:
   a. one write of the value 2 to 0xBFE001 (CIA-A port A);
   b. four volume writes, voices 0,1,2,3 (3.3);
   c. the volume envelope of voices 0..3 (3.4, no writes);
   d. the DMA state machine of voices 0..3 (3.5).

### 3.1 Row processing

For tracks 0..5 in order: the track is skipped if its mute bit is set or its pattern number on
the current line is 0. Otherwise the row word at the current row position is read. If the
word after it is 0xFFFF, the line is flagged "last row" (this is checked for every
non-skipped track before looking at the row word; the flag is only ever set). Then:
- row word 0: nothing more for this track.
- row word 0x8000: the track's play state is cleared; if the track currently owns a voice,
  that voice is released (3.2).
- any other word: if the track's play state says it is playing, its voice is released first
  (3.2); the play state is set to playing; a note is started (3.2).

After all six tracks the row position advances by one word. If the flag "last row" is set,
the next sequence line is loaded (3.6).

Note start values: boost = high nibble of the low byte of the row word; instrument = (low nibble of the low byte + the line's instrument offset) & 0xFF; note =
(high byte + the line's transpose + 1) & 0xFF.

### 3.2 Voice allocation, release and note start

The player has four hardware voices (voice v uses the Paula channel v registers) but six
tracks. Each track remembers which voice it owns (or none). At start of the subsong all six
tracks are recorded as owning voice 0 and not playing, all voices have age 0xFFFF; those
initial conditions only matter for a note-off in a track that never started a note (it then
releases voice 0).

Each voice has an "age" word. A voice is chosen for a new note like this: a rotating start
voice r (initially 0) is used; voices are visited in the order r, r-1, r-2, r-3 (modulo 4);
the visited voice replaces the current best only if its age is strictly greater than the
best age so far (the best starts at 0; if none exceeds it the start voice r is used). After
the choice r is advanced by one voice (modulo 4) and the chosen voice's age is set to 0.
Any track (of the six) that owned the chosen voice loses it; the new track owns it.

Ages: for every voice whose current volume is 0, the age is set to 0xFFFF each tick; for the
others the age is incremented (16 bit) each tick, and if the voice's envelope phase is not
"release" the top bit of the age is cleared. Releasing a voice sets the top bit of its age
(so released voices are preferred victims).

Release of the voice owned by a track: the track now owns nothing; if the voice's "held" flag
is set it is cleared first; then the release envelope starts (3.4): if the instrument's
jump flag is 1 the voice volume is set to the sustain level; the target volume becomes 0, the
step counter and its reload value the release interval, the delta the release delta, and the
phase "release".

Note start on voice v with instrument I and note n:
- instrument fields are copied from the table; the voice is marked held;
- writes, in this order: DMACON (0xDFF096) with value 1<<v (DMA of that channel off);
  AUDxLC high word, AUDxLC low word (a long write: the high and low halves carry the same
  timestamp), both = sample base + sample start offset; AUDxLEN = sample length word;
  then the period write AUDxPER;
- envelope: phase "attack"; step counter and reload = attack interval; delta = attack delta;
  current volume = min(start volume + boost, 0x40); target = min(attack target + boost,
  0x40) where boost = volume boost nibble << shift (shift 0 title, 2 in-game);
- period = table[n & 0x3F] + detune (16-bit wrap); values below 0x71 are written as 0x71;
- the DMA state is set to 3 (3.5).

### 3.3 Volume writes

Four writes AUDxVOL for voices 0..3 with the voice's current volume reduced to its low 7 bits (value & 0x7F; volumes are signed
16-bit in the envelope, a negative or oversized value is masked, e.g. -32768 is written as 0).

### 3.4 Envelope (per voice, voices 0..3 in order; no register writes)

If the volume is 0: age = 0xFFFF and nothing else happens. Otherwise: age incremented (and
top bit cleared unless in release). If phase is 0 (idle/sustained): nothing more. Otherwise
the step counter is decremented (16 bit); while it is not zero nothing more. When it reaches
zero it is reloaded and the volume gets the delta added; if the delta is >= 0 and the volume
is still below the target, or the delta is negative and the volume is still above the target,
the step ends. Otherwise the target is reached: volume = target and
- phase release: phase becomes 0;
- phase decay: the release start of 3.2 is performed unless the voice is held (a held voice
  simply stays at the sustain level and the step repeats);
- phase attack: target = sustain level, counter/reload = decay interval, delta = decay delta,
  phase becomes decay.
Phases: 3 attack, 2 decay (towards sustain), 1 release, 0 idle.

### 3.5 DMA state machine (voices 0..3 in order)

Each voice has a state, 0 at start. A note start sets 3. Each active tick: state 3 becomes 2
(no write); state 2: write DMACON with 0x8200 | (1<<v) (turn the channel on) and state becomes
1; state 1: write AUDxLC (high, low, same timestamp) = sample base + loop start offset and
AUDxLEN = loop length, state becomes 0. State 0: nothing.

### 3.6 Next sequence line

Row position is reset to the first word and the "last row" flag cleared; the six entries of
line `L` are loaded (L = current line index), then L is incremented; if the new L is greater
than the subsong's end line it is replaced by the loop line (and SongEnd is raised, 1.3).
At the start of a subsong, before tick 0, L = start line and one line is loaded exactly like
this.

## 4. Host behaviour seen in the reference logs

### 4.1 Subsong initialisation groups

Subsong 0 has no init writes. Subsong s > 0 begins the log with s identical groups of 21
writes (the player's init was called s times): DMACON 0x000F; AUDxVOL = 0 for channels 0..3;
AUDxLC = sample base (high word, low word) for channels 0..3; AUDxLEN = 1 for channels 0..3;
AUDxVOL = 0 for channels 0..3. Offsets in cc from the group start for the 21 writes: 0, 8,
14, 20, 26, 82, 82, 100, 100, 118, 118, 136, 136, 146, 156, 166, 176, 186, 196, 206, 216
(the LC writes of one channel share a timestamp). Group g starts at G0 + 1572 g. After the
groups the tick sequence of section 3 begins (fresh state).

### 4.2 Timing of the tick grid

Tick k (k >= 1) begins at C(k) = C1 + 141880 (k-1) cycles; tick 0 begins at C0 and
C1 = C0 + 141891 (tick 0 is 11 cycles early relative to the grid). For subsong s: C0(s) =
C0(0) + 3144 s and G0 = floor(C0(0)/2) - 70920 (cc). A write whose offset into the tick is S
cycles is stamped (log value, in cc) floor((C(k) + S) / 2). C0(0) as measured with the module
named by a short relative path: 26093223 (title), 26266735 (in-game); it depends on the path
string length and the module size, so align on the first write rather than hardcode it.

### 4.3 Offsets S inside a tick

Row ticks (tempo reload) start with S = 374. Then each track adds:
- empty pattern: 128; muted: 95;
- rest: 276 (+16 if this row is flagged last);
- note off: 218 if the track owned no voice (stolen), else 161 + REL(v) (+16 if last), where
  REL(v) is the release cost of voice v: v0 534, v1 553, v2 572, v3 582;
- note start (S1 = cost before the first write): 1769 + 16 (last row) + 4 (in-game module)
  + release cost (0 if the track was not playing; 227 if it was playing but its voice was
  stolen; else REL(old voice)) + 52 for each strictly-greater comparison made while choosing
  the voice (0-4) + 18 for each track that owned the chosen voice. Writes: DMACON at S+S1;
  AUDxLC (both halves) at +132; AUDxLEN at +152 (from DMACON); AUDxPER after a further
  1148 + V(v) + 8 (c1 + c2 + p) where V = 0, 93, 186, 210 for v = 0..3, c1/c2 = 1 if start/
  target volume before clamping exceeded 0x40, p = 1 if the period was clamped. The track then
  adds S1 + 152 + (that period time) + 132.
After the six tracks: +148, and +371 if a new sequence line was loaded. On the tick that wraps
the final line add about 221 more cycles at this point (measured from long runs; 221 or
222, rounding in the log hides the half cycle).
Non-row ticks start at S = 401.
Then, at that S: BFE001 write at S; volumes at S+242, S+551, S+881, S+1252 (voices 0..3).
With S4 = S + 1252, the envelope phase ends at S4 + 413 + sum of per-voice envelope costs:
volume 0: 67; phase 0: 118; counter still counting: 192 (168 in release phase); step ended
without reaching target: 294 (release) or 318; target reached: 373 (release), 439 (decay),
540 (attack). Then the DMA machine: start at that time + 12; for each voice add 26 first, then
a state 2 write happens 137 later, or state 1 writes happen 217 (LC) and 237 (LEN) later; then
add 8 plus the state cost (state 0: 39, 3: 78, 2: 185, 1: 265).

## 5. Quirks

- The "last row" test looks one row ahead of the row being played: the line ends after the
  row whose following word is 0xFFFF.
- Notes 48-63 read the period "table" past its end (section 2).
- A voice stays at its sustain level while held; only a release (new note on the same track or
  note-off) lets it fade.
- Ticks before the first note produce no writes; this was not observed in either real
  module, where tick 0 has a note (timing of such ticks is unverified).
