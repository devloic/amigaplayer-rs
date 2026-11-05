# Blade Packer replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the Amiga music format "Blade Packer" by Tord "Blade" Jansson (c) 1991-96, file name prefix `uds.`
(song file) with a companion `smp.` file (samples), as played by the reference (the Eagleplayer
"Blade Packer"; the replayer inside it was taken from the game Obsession, Unique Development Sweden, 1995).
This is a description of observed behaviour (register-write logs of the reference 68k player, module data)
in my own words. It contains no instructions or addresses of the original. "cc" are Amiga colour clocks
(1 cc = 2 CPU cycles). Numbers written 0x.. are hexadecimal.

What the format is: a packed ProTracker-like song. A module has a position list, a pool of 64-row tracks
(patterns of one channel) and a pool of distinct note cells (note, sample, command, parameter) that tracks
refer to by index; samples are in a separate file. The replayer is a tracker with its own command set (a
ProTracker superset/variant: commands 0-F, E sub-commands), driven at the 50 Hz CIA-timer tick of the
host, plus an output stage that tracks the logical Paula playback position of each channel and
reprograms Paula only when a note starts. A value model written from this description reproduces the
Paula register writes (register, value, order) of the reference for 94 real runs (5 modules, every subsong, 38 816
ticks) and 605 synthetic runs (571 865 ticks) with no differing tick except the undefined cases of section 11;
section 10 gives the write timing in cc (exact in structure, within +-9 cc per write on 99.9 % of 86 000 writes).

## 1. Files, detection, loading

### 1.1 Song file (`uds.NAME`)
Accepted when the first four bytes are 53 8F 4E 47 and the fifth byte is 2E (the string `S`, 0x8F, `NG`, `.`).
The Eagleplayer name is "Blade Packer", author "Tord 'Blade' Jansson", prefix `UDS.`. There is no title and no
author string in the file; show the file name.

### 1.2 Sample file (`smp.NAME`)
The host loads a second file from the same directory: the module's file name with its first three characters
(`UDS`, any case) replaced by `SMP` (upper case in the reference; a port should match the name
case-insensitively, the files in the archive are lower case). If the prefix is not `UDS.` the load of the
second file fails. The song file is the module; the sample file is loaded as the "additional file".

### 1.3 Module sanity checks (what makes the reference refuse a file)
- the 16-bit word 0xDD48 must occur at an even offset (see 2.2) and the word 0xD8F1 after it (see 2.3);
- the computed song size (2.5) must not exceed the size of the file;
- the sample file must start either with the bytes `SPLS` or with the 32-bit value 0x000001F4 (3.1).
Otherwise the reference reports a corrupt module / module too short.

## 2. Song file layout

All multi-byte values are big endian.

### 2.1 Header (8 bytes)
| offset | meaning |
|---|---|
| 0..4 | detection bytes (1.1) |
| 5 | track entry type: the character `B` (0x42) = one byte per row in a track; any other value (observed `W`, 0x57) = one 16-bit word per row. |
| 6 | voice count: the character `4` (0x34) = 4 tracks per position; any other value (observed none; intended `8`) = 8 tracks per position. In the 8-track layout only the first four tracks of every position are played; tracks 5-8 are read and ignored (they still take part in the subsong scan 2.6). |
| 7 | restart position: the position the song returns to when it reaches the end of the list. The value 0x7F means 0. |

Written below: `E` = 1 for type `B`, 2 for type `W` (bytes per track row); `V` = 4 or 8 (tracks per position).

### 2.2 Position list
From offset 8: positions of V bytes each, one byte per track = track number. The list ends at the first
16-bit word equal to 0xDD48 found at an even offset when scanning words from the start of the file (the first
two bytes of a would-be position equal to DD 48 therefore terminate the list). Number of positions
(song length) = (offset of that word - 8) / V.

### 2.3 Track pool
Directly after the 0xDD48 word. Track number n starts at pool start + n * 64 * E and holds 64 rows;
each row is one cell index (a byte for `B`, a word for `W`). The pool ends at the first even-offset word equal to
0xD8F1 (found by scanning words from the start of the pool). Number of tracks = (offset of the 0xD8F1 word -
offset of the pool start) / (64 * E).

### 2.4 Cell dictionary
Directly after the 0xD8F1 word. Cell k is 4 bytes at dictionary start + 4k:

| byte | meaning |
|---|---|
| 0 | note: 0 = none, otherwise an even number 2..72 (twice the 1-based note index into the period table of 6.2) |
| 1 | sample number, 0 = none; otherwise 1-based index into the sample table (3.1) |
| 2 | command times four (0, 4, 8 ... 60); the command number is this byte / 4. Values that are not a multiple of 4 are not defined. |
| 3 | command parameter |

The number of cells is 1 + the highest cell index that any track row refers to (the reference finds it by
scanning every word of the pool; for `B` it compares both bytes of each word, for `W` the word).

### 2.5 Song size
End of dictionary = dictionary start + 4 * number of cells. This is the "module size" reported and the size
saved for the song file; it must not exceed the loaded file size.

### 2.6 Subsongs
A module has several subsongs; the song list is shared. Subsong 0 starts at position 0. The other start
positions are found by scanning the position list in order: for each position (all V tracks of it), look at
every row of every track of that position and at the command byte (byte 2) of the cell it refers to; as soon as
one of them is 0x2C (command B) or 0x20 (command 8) the number (position index + 1) is appended to the table
and the scan continues with the NEXT position (the rest of the current position is not looked at). Start
position table = [0] followed by those numbers. Number of subsongs = the count of appended numbers, or 1 if
there were none. Subsong n (0-based) starts at table entry n. Note that the table has one entry more than
there are subsongs; the last appended number is never a start position of a subsong. (Corpus: Options 1
subsong, Desert_Run 20, Aquatic_Adventure 25, Balls_and_Bats 25, X-ile_Zone 23.) The reference host starts a
subsong n by running the init for subsongs 0, 1, ... n in turn (see 9.3).

### 2.7 Information shown by the reference
Subsongs (2.6), voices (4 or 8), song length = number of positions, patterns = number of tracks (2.3), samples =
number of table entries (3.1) whose start and end offset differ, sample size, song size (2.5), loaded size.
No title.

## 3. Sample file layout

### 3.1 Table
The file has either the 4 bytes `SPLS` followed by the table, or the table at offset 0. The table consists of
entries of 16 bytes; the number of entries is (the first 32-bit value of the table) / 16, rounded down
(the first value is the start offset of the first sample = table size + 4; all five corpus files have the value
500 = 0x1F4 and 31 entries). Entry n (0-based; sample number n+1):

| offset | meaning |
|---|---|
| 0 | start offset of the sample (32-bit) |
| 4 | end offset (32-bit) |
| 8 | loop length in bytes (32-bit); 0 = no loop |
| 12 | default volume (byte, 0..64) |
| 13 | finetune (byte, 0..15; 8..15 = -8..-1 as in ProTracker) |
| 14, 15 | zero in all files |

Data address of an offset o: (address of the first byte of the table) + o - 4. For the `SPLS` form this equals
(address of the file) + o; for the headerless form (address of the file) + o - 4. Sample data is 8-bit signed.
A sample is played from its start offset to its end offset (length = (end - start) / 2 words); if the loop
length is not zero the loop is the last `loop length` bytes before the end offset (loop start = end - loop
length), otherwise the channel goes silent after the end (section 8). Offsets in the corpus can exceed 64 KiB
(up to 170 KiB), so the 16-bit length register of Paula can wrap for samples of 128 KiB and more (not seen).
Unused table entries have start = end (empty). Sample numbers above the table size are not defined.

## 4. Overview of the replay

The reference calls one routine per tick, 70 940 cc apart (about 49.95 Hz; the host timer, not the video
frame). The routine does two things in this order:

1. the **output stage** (section 8): writes Paula registers from the state left by the previous tick's
   sequencer run, and advances the logical channel positions;
2. the **sequencer** (sections 5-7): advances time, executes rows and commands, and updates the state the
   next tick's output stage will use.

Consequence: a note whose row starts in tick t is first audible (period, volume, address) in the Paula
writes of tick t+1. The first tick (tick 0) writes the initial state.

## 5. Sequencer state

Global: position pointer (the position that will be loaded next), row counter (rows left in the current
pattern, 63 after a position is loaded), tick counter, speed (ticks per row minus 1; initially 5), pattern delay counter,
repeat flag, restart position (header byte 7), subsong start position, a pending jump flag, and for every
track slot (8 slots) a row pointer into its current track. For each of the 4 played channels: a record
(period, slide speed, volume-slide up/down, tone-portamento target and speed, arpeggio list, remembered note,
vibrato position/speed/depth/last output, tremolo position/speed/depth/base volume, finetune, selected sample,
sample offset position, "new note" flag, "fresh" flag, saved cell for retrigger/delay), and for each of the 4
"handler slots" a current per-tick action (default: none) and the vibrato and tremolo waveform choices
(default: sine).

Per output channel the sequencer owns: volume (byte), "logical start address", "logical end address" and "loop
length" (see section 8). The sequencer never writes Paula registers.

### 5.1 Start of a subsong (the first sequencer run after init)
If the pending jump flag is set (it is set by init to the subsong's start position, and by command 8), the sequencer first does
this and nothing else before the row logic: all four volumes := 0, row counter := 0, tick counter := 0,
position pointer := start of the position list + start position * V; flag cleared. It then proceeds with the
normal tick logic (so the first row of the subsong starts in the same run).

### 5.2 Tick logic
- If the tick counter is not 0: decrement it; this is a **mid-row tick** (6.3).
- If it is 0: this is a **row tick**:
  1. If the pattern delay counter is not 0: if the "repeat flag" is clear (first repeat) then every track row
     pointer moves back by one row (E bytes) and the repeat flag is set; the pattern delay counter is
     decremented; the row below is executed again with its note and sample fields cleared (the cell's
     note and sample become 0; command and parameter are kept). The position/row counters do not change.
  2. Otherwise (delay counter 0): if the repeat flag is set, the row pointers move forward by one row again;
     the repeat flag is cleared. Then, if the row counter is not 0 it is decremented; if it is 0 the next
     position is loaded: if the two bytes at the position pointer are DD 48 the pointer is set to the restart
     position (start of list + restart * V) and **song end** is signalled (9.1); then the V track numbers at the
     pointer give the row pointers (pool start + track * 64 * E), the position pointer advances by V, and the
     row counter := 63.
  3. Tick counter := speed.
  4. For channels 0..3 in order: read the row from the channel's row pointer (E bytes: a cell index); the
     pointer advances by E unless the repeat flag is set; fetch the cell; cell command = byte 2 / 4; apply the
     repeat mask (fields cleared as in step 1) ; execute the command's row action (6.4) with the full 32-bit cell.
  5. Channel handler slots: see 6.5 (the slot index advances once per executed row action, except command 8).
- After either kind of tick the channel periods are selected (6.6).

Timing: a row lasts speed + 1 ticks (6 with the initial speed); the row action runs in the first, handlers in
the following ones. Pattern position: rows are numbered 0..63; row k of a freshly loaded position is executed with the row counter at
63 - k.

## 6. Replay rules

### 6.1 Note start (used by almost every command)
Given a cell (note n, sample s, ...) for channel c:
1. vibrato output := none; the "fresh" flag := set (used by command 7).
2. if n != 0: the channel's remembered note := n.
3. if s != 0: select sample s (header s-1 of the table): remembered sample := s; the channel's sample offset position
   := the header's start offset; volume := the header's default volume (raw byte, may exceed 64 only through command C);
   finetune := the header's finetune byte.
4. if n != 0: the "new note" flag := set; logical start address := data address of the offset position (3.1);
   logical end address := data address of the header's end offset; loop length := the header's loop length; channel
   period := period table (6.2) [finetune][remembered note].
Notes: (a) the offset position is set only when a sample number is given, so a note without a sample number after
command 9 restarts from the 9xx position (offsets are not reset); (b) a sample number without a note changes the volume and
the sample used by later notes but does not retrigger; (c) volume is not reset by a note without a sample.
(d) A note on a channel that never had a sample selected is not defined (the reference reads memory at address 0).

### 6.2 Period table
Index = finetune * 36 + (note / 2) - 1, finetune 0..15 (note is even, 2..72). It is the standard ProTracker table
of 16 finetune rows x 36 periods (the file `data/periods.bin` holds all 576 words). Finetune 0:
856 808 762 720 678 640 604 570 538 508 480 453 428 404 381 360 339 320 302 285 269 254 240 226 214 202 190 180
170 160 151 143 135 127 120 113. Rows 1-7 are the ProTracker finetune +1..+7 rows, rows 8-15 are -8..-1.
Finetune bytes above 15 are not defined. Periods of the engine are clamped to 0x6C..0x38B by every slide
that clamps (below).

### 6.3 Mid-row ticks
Ticks 1..speed of a row: for each handler slot 0..3 in order, run the slot's per-tick action (6.4) on channel
index = slot index. A slot with no action does nothing.

### 6.4 Commands (row action; per-tick action). x = parameter >> 4, y = parameter & 15, p = parameter.
"Slot" = the handler slot written (6.5). "NS" = note start (6.1) with the full cell.

- **0 (arpeggio)**: if p = 0: slot := none; NS. Otherwise NS, slot := arpeggio, and build a list of up to three
  periods from table row r, base index i0 = r * 36 + n/2 - 1 (n = the channel's remembered note after NS; for n = 0 the
  index is -1 which reads the player word in front of the table, observed 0x71AE): r is **not** the finetune; it is
  the byte at offset 17 of the sample's table entry, i.e. bits 16..23 of the NEXT entry's start offset (value 0, 1 or 2
  in the corpus; a quirk of the original, reproduced here). A list entry "+k" means table[i0 + k] (the table is
  linear, so it spills into the next finetune row). Cases:
  - p treated as signed byte >= 0x10 (p = 0x10..0x7F) with y != 0: list = [base, +x, +y];
  - p = 0x10..0x7F with y = 0: list = [+x, base] (see the order below);
  - p < 0x10 (y only) or p >= 0x80 (negative as signed): list = [base, +y] (for p >= 0x80, x is ignored even if y = 0);
  where base = table[i0]. Per-tick action: the list is walked with a cursor that starts at the SECOND entry on the first
  tick; after the last entry it wraps to the first. Each tick the entry at the cursor becomes the current arpeggio period,
  and the cursor advances. Thus for [base,+x,+y]: ticks 1,2,3,4,.. give +x, +y, base, +x ...; for [base,+y]: +y, base, +y ...;
  for the x-only case the list holds [+x, base] and the cursor starts on `base`: ticks give base, +x, base, +x ...
  The current arpeggio period replaces the channel period in the Paula period write on mid-row ticks (6.6) but not on the row tick.
- **1 (slide, period down) / 2 (slide, period up)**: NS; slide speed := p; per tick: period := period - speed
  (clamped to >= 0x6C) for 1, period + speed (clamped to <= 0x38B) for 2.
- **3 (tone portamento)**: no NS (no retrigger, remembered sample/note unchanged). If n != 0: target := table row 0
  [n / 2 - 1] (finetune ignored). If p != 0: tone speed := p. If s != 0: volume := that sample's default volume (the
  remembered sample is NOT changed). vibrato output := none; fresh flag := set. Per tick: if period < target: period +=
  speed, limited to the target (never above it); otherwise period -= speed, limited to the target (never below it).
- **4 (vibrato)**: remember the vibrato output; NS; restore the remembered vibrato output; if the new-note flag is
  set: vibrato position := 0, vibrato output := none, new-note flag := clear. If p != 0: vibrato speed := x, depth := y.
  Per tick: with pos = vibrato position (0..63): offset = (wave[pos] * depth) >> 7 (wave from 6.7, byte 0..255);
  value = period + offset if pos < 31, else period - offset; clamp to 0x6C..0x38B; vibrato output := value;
  pos := (pos + speed) & 63. The vibrato output replaces the period in the Paula write (6.6).
- **5 (tone portamento + volume slide)**: volume slide down := y, up := x; NS (a note here DOES retrigger); per tick:
  the tone portamento step of 3 followed by the volume slide step of A.
- **6 (vibrato + volume slide)**: down := y, up := x; then exactly as 4 (remember output, NS, restore, new-note logic; parameter
  is NOT split into speed/depth: speed and depth stay as set by an earlier 4); per tick: vibrato step then volume slide step.
- **7 (tremolo)**: let F = the fresh flag. If F is clear (a previous row was also a 7 and nothing cleared it): NS; if the cell has
  a note the tremolo position := 0; if it has a sample the tremolo base volume := current volume. If F is set: tremolo position :=
  0; NS; tremolo base volume := current volume. Then fresh flag := clear; if p != 0: tremolo speed := x, depth := y.
  Per tick: pos = tremolo position; delta = ((wave[pos] * depth) & 0xFFFF) >> 6; v = base volume + delta if pos < 31 else
  base volume - delta (byte arithmetic); if v as a signed byte is above 64 then v := 64; if v is negative then v := 0; volume := v;
  pos := (pos + speed) & 63.
- **8 (restart)**: signal song end (9.1); set the pending jump flag to the subsong's start position (the jump takes effect at the START of the
  next sequencer run: 5.1, which also silences all volumes); NS. This command does NOT consume a handler slot (6.5).
- **9 (sample offset)**: NS; position := p * 256 + the sample header's start offset; if position >= the header's end offset then position := the header's
  start offset; the channel's offset position := position; if the cell has a note, the logical start address := data address of position.
- **A (volume slide)**: down := y, up := x; NS; per tick: volume := clamp(volume + up - down, 0, 64).
- **B (position jump)**: row counter := 0; position pointer := start of list + p * V (no range check; p must be a valid position);
  signal song end (9.1) every time; NS. The jump takes effect at the next row tick.
- **C (set volume)**: NS; volume := p (raw byte, no clamp to 64; the Paula volume write is the raw value).
- **D (break)**: p = 0: row counter := 0 (next row tick loads the next position); NS. p != 0: row counter := 63 - p (byte arithmetic); the next
  position is loaded immediately (position pointer: if the two bytes there are DD 48 it becomes the restart position and song end
  is signalled; the V row pointers := pool start + track*64*E + p*E); NS. Because the row counter is then 63 - p and the next row tick
  decrements it first, rows p .. 62 of the new pattern are played; row 63 is never played. p is plain binary, not BCD.
  Tracks of the same row that come after the D channel (higher channel numbers) already read their cells from the new pattern's row p.
- **E, sub-command x of p (y = low nibble)**:
  - 0, 3, 6, 8, F: NS only;
  - 1: NS; period := min(period + y, 0x38B);  2: NS; period := max(period - y, 0x6C);
  - 4: choose the vibrato waveform of the current handler slot (6.5): y = 0 sine, y = 1 triangle ("ramp", 6.7), otherwise square; vibrato output := none; NS;
  - 5: NS; finetune := y; then a second note start with only the note field of the cell (re-pitches and re-triggers with the new finetune if the cell has a note);
  - 7: choose the tremolo waveform of the current handler slot (same coding); NS;
  - 9 (retrigger): remember the cell; NS; per tick t (1,2,..): if y != 0 and t mod y = 0: NS with the remembered cell (t = ticks since the row tick);
  - A: NS; volume := volume + y (byte arithmetic; if the result as a signed byte is above 64 then 64);
  - B: NS; volume := volume - y; if the result is negative (byte) then 0;
  - C (cut): NS; y = 0: volume := 0 now; y != 0: a counter := y, decremented each tick, when it reaches 0 volume := 0;
  - D (delay): remember the cell; NS with the note field cleared (the sample number, if any, takes effect now); per tick t: if t = y (y != 0) and
    the remembered cell has a note: NS with the full remembered cell. y = 0 never starts the note;
  - E (pattern delay): NS; if the repeat flag is clear: pattern delay counter := y (6.4 / 5.2 step 1 repeats the row y times).
- **F (speed)**: speed := (p - 1) & 0xFF and the tick counter of the current row := the new speed too (the new speed applies to the rest of this
  row); then NS. So a row lasts p ticks; there is no tempo command; p = 0 gives 256 ticks per row.

### 6.5 Handler slots
The array of per-tick actions has four slots. During the row tick a running slot index starts at 0; every row action
except command 8 stores its action (or "none") in the current slot and advances the index. So normally slot i belongs
to channel i. After a command 8 on channel k the following channels' actions land one slot earlier (channel k+1's action in slot k, ...) and the last slot keeps its
old action; mid-row ticks then apply slot i's action to channel i, i.e. to the wrong channel (observed and reproduced). Waveform
choices (E4, E7) are per slot and persist; all slots start as sine.

### 6.6 Period selection (every tick, end of the sequencer run)
For each channel c, the "period slot" p_c written by the sequencer for the next output stage is:
- on a mid-row tick, if handler slot c holds the arpeggio action: the current arpeggio period;
- otherwise if the channel's vibrato output is not "none" (bit 15 clear): the vibrato output; otherwise the channel period.
Row ticks use the channel period or vibrato output (never the arpeggio period).

### 6.7 Waveform tables for vibrato and tremolo (64 bytes each)
- sine: the half sine 0 24 49 74 97 120 141 161 180 197 212 224 235 244 250 253 255 253 250 244 235 224 212 197 180 161 141 120 97 74 49 24, repeated twice;
- triangle ("ramp"): 0 8 16 ... 248 (32 values, step 8) then 255 248 240 ... 8 (32 values);
- square: 64 times 255.
(`data/waves.bin` holds the three tables in this order.)

## 7. Volumes and master volume
The volume written to Paula is (volume * V) / 64 with V = (balance * master volume) / 64 for the side of the hardware voice (voices 1 and 4 left,
voices 2 and 3 right); with the reference host's defaults both sides are 64 so the written value equals the channel volume (up to 255 after
command C). The host's voice-mask bits only force a voice to 0 and are not part of the replay.

## 8. Output stage (first part of every tick)

### 8.1 Logical channels
There are five logical channels: the four music channels and a fifth that no music command ever uses (it serves as a sound effect
channel in the game). Each has: period (long), volume, logical position (address), logical end, loop length, and the address last programmed.
Initial values (set by init): period 0x11C, volume 0, position = the address of the player's own silent word (a zero word in
chip memory, referred to as SILENT; its address is chosen by the host, 0x7E6138 in the logs), end = SILENT + 2, loop length 0, last programmed = (unchanged from before: see 9.3; zero after a cold start).

Hardware voice for logical channel: 0 -> voice 1 (AUD0), 1 -> voice 2 (AUD1), 2 -> voice 4 (AUD3), 3 -> voice 3 (AUD2).
(Channel 2 plays on AUD3 and channel 3 on AUD2.)

### 8.2 A tick's writes, in order
Let T be the set of channels whose logical position differs from the last programmed address ("started"). Rules of the fifth channel (8.3) can add voice 1.
Write 1: DMACON := bits of the started voices (bit 0 = voice 1 ... bit 3 = voice 4), bit 15 clear (stops those voices). Always written, also when 0.
Then for hardware voices 1,2,3,4 in order: AUDxPER := period slot, then AUDxVOL := volume (always, every tick), taken from the logical channels
0, 1, **3**, **2** respectively (voice 3 plays channel 3, voice 4 plays channel 2). While the fifth channel has borrowed a voice (8.3) that voice
gets the fifth channel's period and volume instead.
Wait ("wait 1", 10.2).
Then for each started voice in voice order (1..4): AUDxLC high word, AUDxLC low word (same colour clock), AUDxLEN :
  address = position with bit 0 cleared; length (words) = (end - address) / 2 truncated to 16 bits.
Write 2: DMACON := 0x8000 | the same bits (starts them). Wait ("wait 2").
Then for each started voice in voice order: second-phase address and length:
  if loop length != 0: address = end - loop length, length = (loop length & 0xFFFF) / 2;
  otherwise address = SILENT, length = 1.
Write 3: DMACON := 0x8000 | the same bits.
A voice that is not started gets no address or length writes. Period and volume writes happen for all four voices on every tick, in the order PER, VOL.

### 8.3 Fifth channel and the voice-1 quirk
On the very first output stage the fifth channel is "started" (its position differs from "last programmed" 0), so voice 1 is added to the started
set with the fifth channel's data (address SILENT, length 1, period 0x11C, volume 0, i.e. the same as the other voices' initial values).
On the second tick the fifth channel's position equals SILENT, which releases that "borrowed" voice: voice 1's last programmed address is cleared, so
voice 1 is **always reprogrammed on the second tick** from its logical position (SILENT if nothing started, or a started note's sample), even though it
did not change. Neither effect occurs again (no music command touches the fifth channel).

### 8.4 Advance of the logical positions (last step of the output stage)
For each of the five channels: step = 70938 / period (integer division, treated as a 16-bit quotient; if the period is 0 the step is 70938)
[periods < 3 are not defined]. position += step. If position < end: keep. Otherwise, if loop length != 0: subtract the loop length until position < end;
otherwise position := SILENT and end := SILENT + 2. Then last programmed := position. (It is the estimate of where Paula is reading after one tick, assuming
50 Hz and the PAL clock; only used to decide when a channel needs reprogramming: it is not meant to be audio-accurate.)
A new note (6.1 step 4, command 9) writes a new logical start position; the next output stage sees it differs from "last programmed"
and starts the voice. A channel whose sample ended (non-looping) therefore stays silent without writes; a started note replaces SILENT by a real address.
The step uses the period slot value of that logical channel, i.e. the same value that was written to Paula in this tick (arpeggio and vibrato values included).

## 9. Init, subsong start, end

### 9.1 Song end
A song-end notification is sent to the host: (a) when a position load finds DD 48 (position list ended) and the pointer goes to the restart
position; (b) every time command B executes; (c) every time command 8 executes; (d) command D with p != 0 when the next position is the terminator.
The reference host used offline stops the run about two ticks after the first notification; in the host's normal mode it continues with
the next subsong (9.3). The player itself keeps running (restart position / jump).

### 9.2 Init of a song (module load)
Read the header; locate the track pool and dictionary (2.2-2.4); wave tables := sine for all slots; row counter, tick counter := 0; speed := 5;
repeat flag clear; pattern delay 0; the restart pointer; all channel records zero.

### 9.3 InitSound (start of subsong n)
Writes, in order (cc after the previous write): AUD0VOL (0xDFF0A8) := 0; AUD1VOL := 0 (+10); AUD2VOL := 0 (+10); AUD3VOL := 0 (+10); CIA-A port A (0xBFE001) := 0x0002 (+12; the host's port reads
0, the player sets bit 1); DMACON := 0x000F (+10). Then the logical channels are initialised (8.1) and the pending jump flag gets the subsong's
start position (the subsong table entry, 2.6) which also becomes the position that command 8 returns to. "Last programmed" addresses
and the fifth-channel borrow state are NOT reset by init (a subsong started without reloading the module keeps them: if a channel had been
left on SILENT nothing is restarted on the first tick of the new subsong).
First tick (tick 0): its interrupt time is 71 385 cc after the DMACON write that ends init (see 10.1).

### 9.4 EndSound (stop)
DMACON := 0x000F; then (+8, +6, +6, +6) AUD0VOL, AUD1VOL, AUD2VOL, AUD3VOL := 0.

### 9.5 The reference host's subsong stepping
Observed with the offline run: to start subsong n the host performs n+1 init/stop cycles (subsong 0 .. n) separated by about 130 740 cc, and plays only the last.
After a song end during playback the host stops (offline) or starts the next subsong with an init/stop cycle (no state of the module is reloaded).

## 10. Timing in colour clocks
All times are cc of the reference host's clock, from the logs.

### 10.1 Tick times
The interrupt of tick k is at T0 + 70 940 * k. The first Paula write of a tick (the first DMACON write of 8.2) happens at T0 + 70 940 k + extra, with
extra = 0 for a tick that starts no voice. For ticks that start voices: extra = floor(76.5 * s + 30 * l) where s = number of started voices and l = number of
those whose sample has a loop (loop length != 0). Tick 0 adds 120 (fifth channel); tick 1 adds 30 (release of the borrowed voice).
T0 = (the init DMACON write) + 71 385. (First write of tick 0 = init DMACON + 71 811.)

### 10.2 Inside a tick (offsets from the previous write)
- period/volume writes after the first DMACON write: +56 (PER1; 56..58), +82 (VOL1; 82..84), +24 (PER2; 19..24), +86 (VOL2), +23 (PER3; 23..24), +81 (VOL3), +24 (PER4; 23..24), +86 (VOL4). Total from the first write to the last volume write: 462 (range 462..465).
- Wait 1 ends (the next write is the first address write, or write 2 if no voice is started) at a time T with
  T = 227 * ceil((t_vol4 + 1853) / 227) + p1, t_vol4 = time of the last volume write; p1 = 112 if no voice started, else 63, 78, 93, 108 for first started voice 1, 2, 3, 4. Accuracy +-9.
  (The wait is the player polling the raster line register until it has changed 9 times; lines are 227 cc in the host's clock.)
- Address group 1 (per started voice, in voice order): LC high and LC low at the same cc; LEN +10; next started voice's LC: +51 for the adjacent voice, +14.5 for every unstarted voice skipped; after the last started voice v: write 2 at +40 for v = voice 4, +14.5 for every voice after v.
- Write 2 to wait 2: the next write is at 227 * ceil((t_write2 + 1847) / 227) + p2, p2 = 110 if no voice started, else 61, 77, 92, 106 for the first started voice 1..4. (+-9)
- Address group 2: LEN +10 after LC; next started voice +22.5 (adjacent) + 14.5 per skipped voice; write 3 at +12 after the last LEN of voice 4 (+14.5 per voice after it).
- A write sequence of tick with no started voice: DMACON (0), 8 PER/VOL writes, DMACON (0x8000) after wait 1, DMACON (0x8000) after wait 2.

### 10.3 Model quality
Predicted write times from these formulas equal the logged times exactly for 73 % of 86 113 writes in the five real modules, within +-1 for 80 %, within +-3 for 88 %, within +-9 for 99.9 %; the remainder is
jitter of the chip-bus contention and of the 9-iteration polling loop that the reference host emulates.

## 11. Undefined / not reproduced
- Arpeggio (0) or note start on a channel that never had a sample selected, or arpeggio before the first note: the reference reads memory outside the data tables; value is host dependent (0x71AE observed for the period read in front of the table).
- Sample numbers above the sample table's size, finetune bytes above 15, command bytes that are not multiples of 4, position jumps/breaks to positions beyond the list, and a first position pair equal to DD 48: not defined.
- Second tick quirk (8.3) and the run of init cycles (9.5) are host-state dependent in the sense that a restarted module keeps its "last programmed" addresses: reproduced in the model for the observed restarts.
