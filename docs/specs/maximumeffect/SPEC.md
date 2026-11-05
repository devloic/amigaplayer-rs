# Maximum Effect (Jonathan Scarcliffe and Alastair Dukes, 1993) replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the Amiga game-music format found in the game 'Allo 'Allo! (Alternative Software, 1993), file
name prefixes `max.` (song data) and `smp.` (sample bank), as played by the reference (the Eagleplayer
"Maximum Effect", an adaptation of the game's own driver). This is a description of observed
behaviour (write logs, beam-read traces, module data) in my own words. It contains no instructions or
addresses of the original. "cc" is a colour clock (1 cc = 2 cycles of the 68000 the reference runs on).
Numbers written 0x.. are hexadecimal. All multi-byte values in the files are big-endian.

## 1. Files, detection, information

### 1.1 Two files

A module is a pair: the song file `max.NAME` and the sample bank `smp.NAME` in the same directory.
The reference loads the bank like this: the file name must start with `max.` (letters in either case); the first three characters are replaced by `SMP` and the rest of the name is kept
(`max.title` gives `SMP.title`). If that file cannot be loaded, or the name does not start with `max.`,
the file `SMP.set` in the same directory is tried. If neither loads, the module fails.

The reference host starts the replayer without any further module data: the player holds only the two
tables of section 7 and its own code.

### 1.2 Detection (the song file only)

The song file is accepted when all of these hold (a "pointer" is a 32-bit value):

1. The 32-bit value at offset 0 is at least 15 (it is the word at 0 followed by the word at 2).
2. The pointer P1 at offset 4 is either zero, or it is positive, even, not larger than the file size,
   and P1 - 2 is a non-zero multiple of 18.
3. Each of the three pointers at offsets 8, 12 and 16 (P2, P3, P4) is positive, or zero (a negative value,
   one with bit 31 set, is refused); if non-zero it must be even, not larger than the file size, and
   the 32-bit value found 6 bytes before the place it points to must be zero (it is the end marker of
   the preceding track list, 2.3).
4. At least one of P1..P4 is non-zero. (Because of 2, a non-zero P1 alone suffices.)

Nothing else is checked. The sample file is not checked by detection; it is checked on load (3.2).

### 1.3 What the reference shows

- Sub-song count: (P1 - 2) / 18 (P1 as in 1.2; for a valid file this is the number of song entries, 2.2). Sub-songs are numbered from 0.
- No title and no author string exist in the files. The reference shows the player's own name.
- Voices: the number of set bits among the low four bits of the song's mask byte (2.2).
- "Length" (display only): if the song's first voice pointer is zero, 0; otherwise the number of entries
  (not counting the end marker) in the first voice's track list.
- Samples: the number of sample records (word at offset 0 of the bank plus one).
- No song length in ticks is stored. The song end is reported while playing (section 6).

## 2. Song file

All pointers in the file are byte offsets from the start of the file. At start-up the reference adds
the load address of the file to every pointer that is going to be followed (see 2.5). A port keeps them
as offsets.

### 2.1 Overall layout

    offset 0   word   number of songs minus one (D)
    offset 2   song table: (D+1) entries of 18 bytes
    then       track lists (6-byte entries), then the pattern streams

(The detection and the sub-song count assume that the first track list starts right after the song
table, so that P1 = 2 + 18 x songs; the synthetic files are built that way.)

### 2.2 Song entry (18 bytes)

| offset in entry | size | meaning |
|---|---|---|
| 0 | byte | priority (unused by the replay path) |
| 1 | byte | mask: bits 0..3 = song voice 0..3 present; bit 4 = "play once" (5.3); other bits unused |
| 2 | long | pointer to the track list of song voice 0 (0 = none) |
| 6 | long | track list of song voice 1 |
| 10 | long | track list of song voice 2 |
| 14 | long | track list of song voice 3 |

### 2.3 Track list

A track list is an array of 6-byte entries followed by an end marker.

| offset in entry | size | meaning |
|---|---|---|
| 0 | long | pointer to a pattern stream (non-zero) |
| 4 | word | number of events to read from the stream before the next entry is taken |

The end marker is a 6-byte entry whose first long is 0; its word at offset 4 is the **restart offset**: a
byte offset from the start of this list to the entry at which playback of this voice continues when the
end is reached (0 = from the first entry; a multiple of 6 in the synthetic files).

### 2.4 Pattern stream

A stream is a sequence of 16-bit words, read in order. An **event** is the group of words
read between two duration words:

    [note word] [command word [argument bytes]]...  duration word

- A word with bit 15 clear is a **command word**: bits 8..14 = command number, bits 0..7 = the command's
  byte operand (5.5). Some commands take extra bytes that follow it in the stream (always an even
  number, so the stream stays word aligned).
- A word with bit 15 set that comes *first* in an event is the **note word**. A word with bit 15 set
  that comes after the first word (after a note word or after command words) is the **duration word**
  and ends the event. So an event without a new note must start with a command word (command 0 with
  any operand is a no-operation and is used for pure rests); an event that is just one word with
  bit 15 set is read as a note word and swallows the next event's first word as its duration.
- Note word: bits 8..14 = sample number (0..127) (index into the sample bank's table); bits 1..7 =
  note index n (0..35, period table of section 7; larger values read outside the table); bit 0 =
  "keep volume": when 0 the voice's volume is loaded from the sample record, when 1 it is left as it is.
- Duration word: 0x8000 + d. The event lasts d+1 ticks (the tick that reads it and d more).

The number of events a list entry covers (word at offset 4 of the entry) is only a counter: the
stream pointer simply continues through the stream; the next list entry's stream pointer is used when
the counter has run out, even if the stream has more data.

### 2.5 Sharing

The reference adds the load address to the pointers of each track list once per pointer it follows when
the file is started. A track list pointed at by two voices or two songs would have its stream pointers
offset twice and break; no file does this. A port that treats pointers as offsets need not care.

## 3. Sample bank

### 3.1 Layout

    offset 0   word   number of samples minus one
    offset 2   sample table: 16 bytes per sample
    then       the sample data (8-bit signed), starting right after the table

| offset in record | size | meaning |
|---|---|---|
| 0 | long | start of the sample (offset from the start of the bank) |
| 4 | word | length in words |
| 6 | word | default volume (normally 0..64) |
| 8 | long | start of the loop part (offset from the start of the bank) |
| 12 | word | length of the loop part in words |
| 14 | word | unused |

As with the song file, the reference adds the bank's load address to the two offsets of every record at
start-up; the values written to the audio registers are these absolute addresses.

### 3.2 Checks on load

The bank must be at least as large as (start of the last record's sample + 2 x its length); the check
uses the first record's start as the end of the table. If not, the reference reports "module too short".

## 4. Starting a sub-song

Start of sub-song s (0-based). The reference host does the following, in this order (several
sub-songs are stepped through, see 9.2): song state is cleared (voice records are NOT cleared, only the
fields set below), then:

1. Take the entry of song s. Let m be its mask byte. Let k be the number of set bits among bits 0..3.
2. Voice assignment: the present song voices, in order of voice number 0..3, are assigned to the
   hardware channels 0, 1, ..., k-1 in that order (song voice with the lowest number goes to channel 0,
   and so on). For each assigned channel: track list = that voice's pointer, list position = 0,
   remaining event count = 0, tick counter = 0x8000 (so that the first tick reads an event).
3. **Active channels.** The channels that are processed in a tick are not channels 0..k-1: they are the
   top k of the four channels, {4-k, ..., 3}. A channel produces output only if it is both assigned
   (has a song voice) and processed:

   | k | processed channels | assigned channels | channels that play |
   |---|---|---|---|
   | 4 | 0 1 2 3 | 0 1 2 3 | 0 1 2 3 (song voices 0..3) |
   | 3 | 1 2 3 | 0 1 2 | 1 2 (the 2nd and 3rd present song voices) |
   | 2 | 2 3 | 0 1 | none |
   | 1 | 3 | 0 | none |
   | 0 | none | none | none |

   The player therefore plays correctly only songs with four voices. A processed channel that has no
   data (its record untouched: tick counter 0 in a fresh run) does nothing at all in a tick: its counter
   goes negative and, having no effect, it writes nothing (5.1). (After 32 768 ticks its counter would
   wrap to a positive value and it would read garbage; not specified.) Songs with k < 4 never report
   the song end (section 6). Observed with synthetic songs of 1, 2 and 3 voices.
4. Song-end flags (section 6): the four flags are set for the song voices present (flag i for song voice
   i, bit i of the mask), cleared for the absent ones.
5. The mask byte is kept for the "play once" test.
6. Volume balance and voice-mask values of the reference host are at their neutral values (64, 64, all
   voices on); the volume register value written is the voice volume AND 0x3FF (5.6).

Register state that survives from a previous sub-song of the same host run is irrelevant to a port that
starts at a given sub-song; see 9.2 for what the reference's log looks like before the first tick.

## 5. The tick

The reference calls the player once per tick. Ticks are 70 940 cc apart (50 Hz). In one tick:

1. the active channels are processed in order 0, 1, 2, 3 (5.1..5.7);
2. the DMA-enable word is written (5.8);
3. the loop-part registers of all four channels are written (5.8).

A "write" below means a write to a custom register: channel c has the base 0xDFF0A0 + 0x10 c with LCH
at +0, LCL at +2, LEN at +4, PER at +6, VOL at +8; DMACON is 0xDFF096. A 32-bit address write
(LC) is two 16-bit writes in the order LCH, LCL, which carry the same colour clock.

### 5.1 Voice step

Each processed channel has these values: track list pointer, list position, stream pointer,
remaining event count, tick counter (16 bit), effect (none or one of four), four effect parameter bytes,
base period, current period (16 bit), volume (16 bit), loop address (32 bit), loop length (16 bit).

1. Tick counter := tick counter - 1 (16 bit, wrapping).
2. If bit 15 of the counter is set (negative) this is an **effect tick**:
   - no effect set: nothing else happens (no register write);
   - otherwise: run the effect step (5.7), then do the **output** (5.6).
3. Otherwise this tick starts a **new event** (5.2..5.5).

### 5.2 New event

1. Clear the effect (none).
2. If the remaining event count is not 0, go to 5.4 at the current stream pointer.
3. Otherwise take the next list entry: read the long at the list position in the track list.
   - Non-zero (a stream pointer): stream pointer := it.
   - Zero (end marker): if the mask has bit 4 set the voice stops (it is removed from the active
     set; no register write, now or later). Otherwise: list position := the marker's restart offset;
     stream pointer := the long at the new list position; the song-end test for this channel is done
     (section 6); continue.
   - Remaining event count := the entry's word at offset 4; list position := list position + 6.
4. Go to 5.4.

(The marker's pointer found at the restart offset is expected to be non-zero.)

### 5.3 Play-once songs

A song whose mask has bit 4 set stops each voice when its list ends (5.2). No song-end report is
generated for such songs. Tick calls continue; they only write the loop registers (5.8).

### 5.4 Reading the event

Read a word w from the stream.

- Bit 15 clear: it is a command; execute it (5.5), then continue reading words (as below).
- Bit 15 set: it is a **note word**: start the note (5.4.1), then continue reading words.

Continue reading: read a word; while bit 15 is clear execute it as a command; the first word with
bit 15 set is the **duration word**: tick counter := that word; remaining event count :=
remaining event count - 1; then do the **output** (5.6) and the voice step ends.

#### 5.4.1 Note start

Let sample record r = sample number of w, n = note index.

1. If bit 0 of w is 0: volume := the sample record's default volume (offset 6).
2. Current period := period table[n].
3. Write DMACON := (1 << c): channel c's DMA is switched off.
4. Mark channel c for DMA start at the end of the tick (5.8).
5. Write LC := bank address + record's sample start (two 16-bit writes), then LEN := record length (word).
6. Remember for the end of the tick: loop address := bank address + record's loop start (offset 8);
   loop length := record's loop length (offset 12).

(No period or volume is written here; the output step 5.6 follows after the duration word.)

### 5.5 Commands

Command number = bits 8..14 of the word; operand B = bits 0..7. "Next bytes" are read from the stream
immediately after the command word, one byte after the other.

| cmd | effect |
|---|---|
| 0 | none |
| 1 | CIA-A port A bit 1 set (a read-modify-write of the byte at 0xBFE001; the log shows 0x02 written). Switches the audio filter / LED state on the host; no Paula effect |
| 2 | the same bit cleared (the log shows 0x00 written) |
| 3 | volume := B (8-bit value, not limited to 64) |
| 4 | vibrato, starts at phase 0: see below |
| 5 | vibrato, keeps the phase |
| 6 | effect := volume slide; slide parameter := B |
| 7 | effect := portamento to a note: target offset := B, then two next bytes: step, slide parameter |
| 8 | effect := arpeggio: offset 0 := B, then two next bytes: offset 1, offset 2; arpeggio position := 0xFF |
| 9 and up | undefined (the original jumps into code that is not a command); do not occur |

Each voice has four parameter bytes A, B, C, D that the effects share (they are *not* cleared when a
new event clears the effect, and a command may read a byte that an earlier command of another
kind wrote):

| effect | A | B | C | D |
|---|---|---|---|---|
| vibrato | slide | phase | speed | depth shift |
| volume slide | slide | | | |
| portamento | slide | target offset | step | |
| arpeggio | offset 0 | offset 1 | offset 2 | position |

Writes by the commands: command 4: B := 0, C := operand, D := first next byte, A := second next byte,
and the base period := current period; command 5: the same without B := 0; command 6: A := operand;
command 7: B := operand, C := first next byte, A := second next byte; command 8: A := operand,
B := first next byte, C := second next byte, D := 0xFF. In a fresh run all four bytes are 0.

Vibrato (commands 4 and 5): effect := vibrato; the parameters as above (the phase of command 5 is
whatever byte B holds). Then **one vibrato step is run at once** (5.7): it changes the current period
and possibly the volume; no register is written. The effect remains set and runs on the following
effect ticks.

Portamento (command 7): effect := portamento (parameters as above); then **one portamento step is run at once** (5.7).
Volume slide (6) and arpeggio (8) do not run a step at once (command 8 only sets the effect).

The portamento target and the arpeggio offsets are byte offsets into the period table (even, at most 70;
larger values read outside the table and are not specified). A command with extra bytes takes them from
the stream before the next word is read.

### 5.6 Output (period and volume)

Done after the duration word of a new event and on every effect tick whose effect is set.

1. Write PER := current period (16 bit).
2. Write VOL := ((volume x 64) AND 0xFFFF) >> 6, i.e. the volume AND 0x3FF. (The reference scales the
   volume by the host balance 64 / 64; with the values the volume is unchanged for volume < 1024.)

The order is PER then VOL, always in this order.

### 5.7 Effect steps

All arithmetic is 16-bit; a "signed byte" is a byte as -128..127.

Volume slide (also the last part of vibrato and portamento): let s := parameter A as a signed byte.
If s = 0 nothing happens (the volume is not clamped). Otherwise v := volume + s (16 bit); if v is
negative (bit 15 set) volume := 0; else if v > 0x40, volume := 0x40; else volume := v.

Vibrato step: index := (B AND 0x7E); entry := sine table[index / 2] (section 7, 16-bit signed);
shifted := entry shifted left arithmetically by D (only the low 6 bits of D count; a count of 16 or more gives 0); delta := shifted >> 7
(arithmetic shift of the 16-bit value); current period := base period + delta (16 bit);
B := ((B AND 0x7E) + C) AND 0xFF (the phase byte keeps the masked value, so bit 0 and bit 7 of B are lost
after each step). Then the volume slide step (parameter A).

Portamento step: t := period table[B / 2]; step := C as a signed byte. If step >= 0:
period := current period + step; if period > t (signed 16-bit compare) period := t. If step < 0:
period := current period + step; if period < t period := t. Then the volume slide step.

Arpeggio step: D := D + 1 (byte); if D >= 3, D := 0; current period := period table[X / 2] where
X = A, B or C for D = 0, 1, 2. No volume slide. (After command 8, D = 0xFF, so the first step uses A.)

### 5.8 End of the tick

1. Write DMACON := 0x8000 OR (the bits of the channels that started a note in this tick). (A write of
   0x8000 alone happens when none started.)
2. For channels 0, 1, 2, 3 in this order (also channels that are inactive or have no data): write LCH,
   LCL := loop address, then LEN := loop length. A channel that has not started a note yet has
   loop address 0 and length 0 (so zeros are written); the values stay in force until the next note
   start of that channel.

The effect of 1 and 2 on Paula: the DMA of a channel that started a note is switched on after its start
registers were written; Paula takes LC/LEN at that moment; the loop registers written afterwards are
what the channel repeats from. Between 1 and 2 the reference waits for raster lines (section 8.4).

## 6. Song end

Four flags, one per hardware channel, are kept (set at sub-song start from the mask, 4.4). When a
channel takes the restart branch of 5.2 its flag is cleared; when this leaves all four flags clear
the song-end report is made to the host and the flags are set again as at start. The report is a
call into the host, after which the reference's own stopping of the player is host behaviour (it
stops a few ticks later, 0 to 4 ticks in the logs).

Songs with fewer than four voices never clear all flags (4.3); play-once songs (5.3) never report.

## 7. Tables

Period table (36 values, note index 0..35; this is the ProTracker finetune-0 table of three octaves):

    856 808 762 720 678 640 604 570 538 508 480 453
    428 404 381 360 339 320 302 285 269 254 240 226
    214 202 190 180 170 160 151 143 135 127 120 113

Sine table (64 signed 16-bit values, index 0..63): for i = 0..31 the value is floor(255 x sin(pi x i / 32)),
and the entries 32..63 are the negatives of the entries 0..31 (so the table has the values 0, 24, 49,
74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, ... for i = 0..17).
Only even phase values reach it (the index is (phase AND 0x7E) / 2).

## 8. Timing

### 8.1 Clock

A tick is called at colour clock t0; successive calls are 70 940 cc apart. The first call is 70 940
after the end of the host's start-up (see the README for the start-up durations measured; in the
reference runs the first call came at colour clock 75 233 to 86 464). The beam position in the
reference is derived from the same clock as the log: line = floor(c / 227) (the vertical position
byte the reference reads is this number, modulo the frame; only a change of the number matters).

Cycle model: a counter T starts at 0 at t0. A step adds its cycle cost. The colour clock of a register
write or beam read made by a step is t0 + floor(T / 2), with T *including* the cost of the step that
makes it (write steps below are marked "W"; for LCH and LCL, which are one 32-bit store, both writes
have the same clock). All costs are fixed numbers (they do not depend on the branch taken).

### 8.2 Tick skeleton

| step | cycles |
|---|---|
| tick entry up to the first channel test | 300 |
| per channel c = 0..3: the "channel active" test | 25 |
| ... if the channel is active: its set-up (c = 0 / c = 1, 2, 3) | 42 / 46 |
| ... if active: the voice step (8.3) | see 8.3 |
| after channel 3 | 52 |
| start of the end-of-tick part | 56 |
| raster wait A (8.4) | see 8.4 |
| DMACON write (W) | 20 |
| call of the second wait | 18 |
| raster wait B (8.4) | see 8.4 |
| per channel c = 0..3: LCH/LCL write (W) | 28 |
| ... LEN write (W) | 20 |
| after channel 3's LEN | 16 |
| end of the tick (no writes) | 168 |

### 8.3 Voice step costs (cycles)

Channel-dependent values, channel c = 0, 1, 2, 3:

| name | c=0 | c=1 | c=2 | c=3 |
|---|---|---|---|---|
| SetAll (after a note's LEN write) | 139 | 170 | 201 | 209 |
| VolPrep (from the PER write to the VOL write, inclusive of it) | 177 | 194 | 201 | 228 |
| VolRest (after the VOL write) | 141 | 172 | 203 | 211 |
| SongEnd test | 77 | 104 | 127 | 140 |

Common costs:

| step | cycles |
|---|---|
| counter decrement and test | 25 |
| effect tick: effect-pointer test | 39 |
| ... no effect set: end of the voice step | 16 |
| ... effect set: call 16, then the effect (below), then 10, then the output |  |
| new event: start | 45 |
| ... remaining count = 0: list-entry fetch | 67 |
| ... ... stream pointer zero (end marker): test | 33, then either of: |
| ... ... ... play-once: stop the voice (the voice step ends) | 48 |
| ... ... ... restart: 41, then 74, then SongEnd test (c), then 21, then (only if the song ends) 68, then 44 | |
| ... ... block count and list advance (after a normal entry, and after a restart) | 38 |
| first word of the event: read and test | 33 |
| note word: decode | 57 |
| ... volume loaded (bit 0 clear) | 22 |
| ... period lookup | 22 |
| ... DMACON-off write (W) | 16 |
| ... mark channel | 4 |
| ... LCH/LCL write (W) | 26 |
| ... LEN write (W) | 22 |
| ... call of SetAll | 18, then SetAll (c) |
| ... store loop values | 52 |
| each further word of the event: read and test | 17 |
| duration word: store | 44 |
| output: PER write (W) | 20 |
| ... then VolPrep (c) (ends with the VOL write (W)) | |
| ... then VolRest (c) (the voice step ends) | |

The song-end report (the 68) rebases the clock: let T_n be the counter right after the 68; the host call returns
after 16 more cycles, and every clock after that point is
t0 + floor(T_n / 2) + 8 + floor((T - T_n) / 2) (instead of t0 + floor(T / 2)). This changes the stamps of
the rest of that tick by +8 cc (and may change the rounding by 1 cc).

Commands (paid for each command word: first the 60 for the dispatch, then 10, then):

| cmd | cycles after the dispatch and 10 |
|---|---|
| 0 | 0 |
| 1 | 24 including the CIA write (W, at the end of the 24), then 10 |
| 2 | the same as 1 |
| 3 | 22 |
| 4 | 16, then 106 (this includes the call of the vibrato step), the vibrato step (below), then 10 |
| 5 | 106, the vibrato step, then 10 |
| 6 | 46 |
| 7 | 86 (includes the call), the portamento step, then 10 |
| 8 | 94 |

The command-word cost 60 + 10 is paid in addition to the "first word of the event" 33 (first word) or
"each further word" 17 of the word that held the command.

Effect steps (cycles; the volume slide step is part of vibrato and portamento):

| step | cycles |
|---|---|
| volume slide, slide parameter 0 | 37 |
| volume slide, parameter non-zero, result negative | 82 |
| volume slide, parameter non-zero, result in 0..0x40 | 95 |
| volume slide, parameter non-zero, result above 0x40 (clamped) | 113 |
| vibrato step | 136 + volume slide |
| portamento step, step byte >= 0: base 84, plus 14 if clamped to the target | + 22 + volume slide |
| portamento step, step byte < 0: base 84, plus 4 if clamped to the target | + 22 + volume slide |
| arpeggio step | 113, plus 4 when the position wrapped to 0 |

(The portamento step's base is 67 plus the comparison 17 for step >= 0 or 17 for step < 0, i.e. 84 either
way; the clamp adds 14 or 4.)

### 8.4 The raster wait

The reference waits for the beam before the DMA enable and again before the loop registers, each wait is
the same routine:

1. 28 cycles of entry.
2. Nine rounds. Each round: a beam read costing 16 cycles (it reads line L = floor(clock / 227) of the
   clock stamped at the end of that read); then repeated polls, each poll = 16 cycles for the read
   followed by 9 cycles (the clock of the read is the clock at the end of the 16); polling stops at the
   first poll whose line differs from L; then 10 cycles.
3. 44 cycles of exit.

So each wait takes at least 28 + 9 x (16 + 25 + 10) + 44 = 531 cycles and ends in about nine raster lines
(each round ends in the first poll after a line boundary). Both waits are the same. Beam reads here are
stamped like writes (t0 + floor(T / 2) with T after the 16 cycles of the read).

### 8.5 Examples (from the logs)

First tick of a four-voice song, first write (DMACON off of channel 0): 300 + 25 + 42 (channel 0
test and set-up) + 25 + 45 + 67 + 38 + 33 + 57 + 22 + 22 + 16 = 692 cycles, i.e. the write is 346 cc
after the call.

Gaps (cc) inside a note start: DMACON-off to LCH/LCL 15 (4 + 26 cycles), LCL to LEN 11, LEN to PER =
(18 + SetAll(c) + 52 + 17 + 44 + 20) / 2 (145 for channel 0, 180 for channel 3, when the note word
is followed directly by the duration word), PER to VOL = VolPrep(c) / 2 (88, 97, 100/101, 114).
Loop-part writes: 10 cc between a channel's LCH/LCL and its LEN, 14 cc between its LEN and the next
channel's LCH/LCL.

## 9. Reference-host behaviours visible in the logs

### 9.1 Song end

After the song-end report the reference host stops after a few ticks (0 to 4 in the synthetic logs);
the log simply ends. Nothing is written by the host at that point in the logs.

### 9.2 Start of a run

For sub-song 0 the log begins with the first tick. For a sub-song s > 0 the host runs the start-up of
sub-songs 0, 1, ..., s in turn (each: song state started as in section 4; between two of them the player's own stop routine runs),
and the log begins with s groups of writes, one per stop routine, before the first tick: DMACON :=
0x000F, then VOL := 0 for channels 0, 1, 2, 3 (spacing 8, 6, 6, 6 cc). The first tick follows 70 940 cc
after the end of the last start-up.

### 9.3 CIA writes

The two LED/filter commands (5.5) appear in the log as writes at 0xBFE001 (values 0x02 and 0x00).
