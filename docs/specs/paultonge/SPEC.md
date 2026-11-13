# Paul Tonge replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the Amiga music of Paul Tonge as used by the game whose set is in `modules/` (song files `PAT.<name>`,
sample banks `SMP.set` / `SMP.<name>`), played by the reference: the UADE/Eagleplayer player "Paul Tonge"
(file `Paul_Tonge`, prefix `pat`), a thin wrapper that loads the game's own 68k music driver from a third file
(`WantedTeam.bin`) and runs it once per timer tick. This spec describes what that driver does, as observed in
register-write logs, call timing and the module data, in my own words. It contains no instructions or addresses of
the original. "cc" = Amiga colour clocks; "cy" = CPU cycles = half colour clocks.

Two small tables live in the driver, not in the modules; their exact values are in `data/` (see 3 and README).

A value model written from this text reproduces the reference write sequence (register, value, order) exactly
for all nine available songs to their song end (one song only up to the point where it starts executing undefined
data, see 7). Section 6 gives the timing; with it the model also reproduces every write time and every call
return time of the reference exactly (LOGS.md).

## 1. Files, detection, information

### 1.1 The set
A song is played from three files in one directory:
- `PAT.<name>`: the song (the module the user opens);
- the sample bank: `SMP.<name>` if that file exists (the `PAT` prefix, matched case-insensitively and followed by a
  dot, replaced by `SMP`), otherwise `SMP.set`. In the available set, `PAT.title` uses `SMP.title` and
  `PAT.TUNE1`..`PAT.TUNE8` use `SMP.set`. A module whose name does not start with `PAT.` gets `SMP.set`.
- the driver file (`WantedTeam.bin`). A port does not need it, apart from the two tables in `data/`.

### 1.2 Detection (on the song file)
- big-endian word at 0 equals 12 (0x000C);
- for each of the words at 2, 4, 6 (call it o): o must not have bit 15 set and must be even; o = 0 is accepted and
  skipped; otherwise let e = the word at offset o; e must be positive as a signed word (1..0x7FFF), and the byte at
  offset e - 1 must be 0x80 or 0x8F.
(The word at o is the first pattern offset of that voice's position list, and patterns are stored back to back, each
ending with 0x80 or 0x8F, so the byte before a pattern ends the previous one.)

### 1.3 Information shown by the reference
- one subsong (no subsong selection); no title or author strings in the files (author shown: "Paul Tonge");
- voices: the number of non-zero words among the four header offsets (words 0, 2, 4, 6);
- length: take the first non-zero word among the words at 2, 4, 6 (if all three are zero, the word at offset 12);
  subtract 12, halve, subtract 1; a result of 0 is shown as 1. (Display only.)
- samples: the sample bank's entries up to the first entry whose first long is 0; each sample is shown at
  bank + entry offset with length 2 x the entry's word count, volume 64.

## 2. Module data

All values big-endian unless stated. Offsets are from the start of the song file.

### 2.1 Header (11 bytes used)
| offset | size | meaning |
|---|---|---|
| 0, 2, 4, 6 | word | position-list offset of voice 0, 1, 2, 3 (Paula channel = voice); 0 = voice unused |
| 8 | word | offset of the instrument table (may be odd: it is read bytewise) |
| 10 | byte | initial speed (ticks per row) |
The first list always starts at 12, directly after the header.

### 2.2 Position lists
A voice's list is a sequence of words, each the offset (from the song start) of a pattern. A word 0 marks the end;
the word after the 0 is the offset of the list position to continue from (normally the list's own start, so the
list loops). Lists are read on demand (2.4 command 0x80); a list is not required to have a terminator: if it has
none, reading simply continues into whatever follows (see 7).

### 2.3 Instrument table
12-byte records, instrument n at table offset 12 n:
| byte | meaning |
|---|---|
| 0 | sample number (entry in the sample bank) |
| 1 | transposition (signed, added to every note of this instrument) |
| 2 | instrument volume 0..15 |
| 3 | attack: low nibble = number of steps, high nibble = step size |
| 4 | decay: low nibble = number of steps, high nibble = step size |
| 5 | delays: high nibble = attack step interval, low nibble = decay step interval (ticks) |
| 6 | release point (rows before the note's end, see 5.6) |
| 7 | release: high nibble = step interval (ticks), low nibble = step size |
| 8 | vibrato delay (rows) |
| 9 | vibrato depth (period units per tick) |
| 10 | vibrato half-period (ticks) |
| 11 | unused |

### 2.4 Pattern bytes
A pattern is a byte stream read when a voice's row counter runs out (5.3). Byte b:
- b < 0x80: a note (5.4). It ends the read.
- 0xE0..0xFF: note length = b - 0xDF rows (1..32), for this and the following notes; continue reading.
- 0xC0..0xDF: select instrument b - 0xC0 (5.5); continue reading.
- 0xB0..0xBF: instrument volume = b - 0xB0 (replaces the value loaded from the instrument; continue).
- 0xA0..0xAF: "chord" selection: no audible effect (the driver stores a pointer and sets a flag that nothing reads);
  continue. Only 0xA0 is valid; no module uses these.
- 0x80..0x99: commands, below. Arguments follow the command byte.
- 0x9A..0x9F: undefined (the driver jumps through bytes past the end of its command table into its own code).

| byte | args | effect |
|---|---|---|
| 0x80 | - | next pattern: take the next word of the voice's position list (on 0: song-end bookkeeping, then jump to the restart offset that follows the 0 and take that word); continue reading at the new pattern; also clears the voice's pitch-slide-at-note-start flag (0x88) and the chord flag |
| 0x81 | - | glide up: +1 note per row (5.7) |
| 0x82 | - | glide down: -1 note per row |
| 0x83 | p, r | release point = p; release interval = high nibble of r (both the counter and its reload), release step = low nibble |
| 0x84 | - | chord off (no audible effect) |
| 0x85 | d, lo, hi, rate, n | pitch slide now: slide flag on; slide delay = d; step word = hi*256+lo (sign-magnitude, 5.8); rate counter = rate reload = rate; slide count = n |
| 0x86 | d, lo, hi, rate, n | pitch slide on release: "slide on release" flag on; same parameters as 0x85 (the slide flag itself is not set here) |
| 0x87 | - | "slide on release" flag off |
| 0x88 | olo, ohi, lo, hi, n | pitch slide from each note start: offset word = ohi*256+olo (added to the period at every note start), step word = hi*256+lo, count reload = n; slide delay, rate counter and rate reload = 0; note-start-slide flag on |
| 0x89 | - | note-start-slide flag off |
| 0x8A | d, depth, half | vibrato delay = d, depth = depth, half-period = half (as in the instrument) |
| 0x8B | v | master volume = v (normally 16, see 5.2) |
| 0x8C | - | tie: row counter = note length; the read ends; nothing else happens to this voice in this tick |
| 0x8D | - | rest: envelope level = 0, attack and decay step counts = 0, row counter = note length; the read ends; nothing else happens to this voice in this tick |
| 0x8E | t | transposition = t (signed) |
| 0x8F | - | stop song (5.10) |
| 0x90 | word | position jump: the word (at an even address in the reference, which reads it as a word) is a list offset; continue as 0x80 from that list position |
| 0x91 | - | voice off: voice inactive, envelope level 0, AUDxVOL = 0 (written raw, not scaled), DMACON = voice bit (clear); the read ends |
| 0x92 | s, c | fade: fade speed = s, fade counter = c (5.9) |
| 0x93 | - | instrument volume + 1 (not above 15) |
| 0x94 | - | instrument volume - 1 (not below 0) |
| 0x95 | s | speed = s |
| 0x96 | - | LED filter off: CIA-A port A bit 1 set (read-modify-write) |
| 0x97 | - | LED filter on: CIA-A port A bit 1 cleared (read-modify-write; the reference log records the byte write 0x00 to 0xBFE001) |
| 0x98 | t | start tune t on the next tick: replaces the song by a tune built into the driver; no module uses it; not specified |
| 0x99 | - | stop on the next tick (as 0x8F one tick later); no module uses it |

Commands used by the available modules: 0x80, 0x83, 0x85, 0x8C, 0x8D, 0x8F, 0x92, 0x93, 0x94, 0x95, 0x97 plus
length, instrument and volume bytes. 0x81, 0x82, 0x86..0x8B, 0x8E, 0x90, 0x91, 0x96 are specified from the driver but
not exercised by any reference log.

### 2.5 Sample bank
A table of 12-byte entries at the start of the bank file, terminated by an entry whose first long is 0:
| bytes | meaning |
|---|---|
| 0..3 | long: offset of the attack (first) part from the bank start |
| 4..5 | word: its length in words |
| 6..9 | long: offset of the loop part |
| 10..11 | word: loop length in words |
Samples that do not loop point their loop part at a short silent block shared by all of them (offset 0x15C, 8 words
in both banks). Offsets may be odd (Paula ignores bit 0 of a location; the writes carry the odd value).
Addresses written to AUDxLC are absolute: bank base + offset.

## 3. Driver tables (data/)

- `data/volumes.bin`: 17 big-endian words. Entries 0..14 are the volume curve indexed by the scaled envelope level
  (5.2): 0, 2, 3, 4, 6, 8, 10, 13, 17, 22, 28, 35, 40, 51, 64. Entries 15 and 16 are what the driver reads when the
  index is 15 or 16 (it has no bound check; they are words of the driver, not volumes: entry 15 is above 1024,
  entry 16 below 64). Index 15 occurs (level 15 at master
  volume 16); index 16 occurs once per voice before the first note (5.1).
- `data/periods.bin`: 128 big-endian words, the period for note index 0..127 (index = (note + transposition) AND
  0x7F). Indices 0..35 are a three-octave table, 856 (C) down to 113 (B). The driver reads indices 36..127 from the
  bytes that follow its table (its own code); the reference songs use indices up to 53 (36, 38, 40, 44, 46, 47, 52,
  53 occur). The file holds the values the reference reads, including index 52, a word that the wrapper patches at
  load time (the file holds the patched value). Indices 49, 50, 51 and 117, 118, 119 are the wrapper's patch (an absolute address of
  the host's code) and are not reproducible; the file holds the driver's unpatched bytes there. No module uses them.

The port embeds entries 0..14 of the curve and 0..35 of the period table only (the rest are driver code words)
and reads the last entry for a larger index (volume 64, period 113).

## 4. State

Global:
- start request (1 when the song is started; 5.10), playing flag (0);
- speed (ticks per row) and speed counter;
- master volume (word; 16 after a start);
- fade speed and fade counter (bytes);
- two accumulators, cleared every tick: interrupt bits (INTENA set-word of every voice that started a note this tick)
  and DMA bits (DMACON set-word of those voices);
- instrument table offset, module, bank.

Per voice v (channel v): active flag; period word; envelope level (byte; 0..15 in use); position-list pointer;
pattern pointer; flags: slide-active, note-start-slide, vibrato direction, slide-on-release (and the dead chord flag);
row counter, note length, current note; attack steps/size, decay steps/size; instrument volume; glide (0, +1, -1);
release point, release step, release-active; attack/decay/release delay counters and their reloads; slide step,
note-start offset, slide count and its reload, slide delay, rate counter and reload; vibrato depth, counter,
period (= 2 x half-period), delay; transposition; sample attack pointer/length, loop pointer/length.
Voice constants: INTENA set-word 0x8080, 0x8100, 0x8200, 0x8400; DMACON set-word 0x8201, 0x8202, 0x8204, 0x8208;
channel bit = low bits of those.

Initial values (before the first start; the driver's static state): every voice inactive, period 0, envelope level
16, note length 4; all other fields are overwritten before use by every available module (each voice's first pattern
selects an instrument before its first note). Master volume 0, speed 0, fade off.

## 5. Replay

### 5.1 One tick
The play routine is called once per timer tick (5.11). In this order:
1. Write DMACON = DMA-bit accumulator (as is: 0 when no note started last tick, else 0x8200 | channel bits, e.g.
   0x820F); then INTREQ = interrupt accumulator AND 0x0780; then INTENA = interrupt accumulator (as is: 0 or
   0x8000 | bits). Clear both accumulators. (So the notes started in a tick get their DMA and their audio interrupt
   switched on at the start of the next tick.)
2. Start check: if not playing and the start request is non-zero: take the request and clear it; request >= 2 stops
   the song (5.10); request 1 starts it:
   - playing = 1, fade off, master volume = 16;
   - for each voice whose header offset is non-zero: position pointer = the place of the list's second word (the
     next entry to read), pattern pointer = song start + the list's first word, flags cleared, vibrato depth 0, glide 0, transposition 0, row counter 1,
     active = 1 (voices with offset 0 are left as they are);
   - instrument table = song + header word 8; speed = header byte 10; speed counter = 1;
   - write INTENA = 0x0780 (all four audio interrupts off), then ADKCON = 0x00FF (the wrapper's replacement of the
     driver's interrupt-vector setup).
3. If not playing: if no voice is active, the tick ends here; otherwise the speed counter is set to 1 (every tick is a
   row tick; does not happen with the available modules).
4. speed counter = speed counter - 1 (byte). The tick is a "row tick" if it is now 0.
5. Voices 0, 1, 2, 3 in this order (5.2).
6. Fade (5.9).
7. If the speed counter is 0, reload it with the speed.

### 5.2 One voice
Nothing at all if the voice is inactive. Otherwise:
1. Write AUDxPER = period (the value computed in earlier ticks).
2. Volume: index = ((envelope level AND 0xFF) x master volume, kept to 16 bits) >> 4; value = `volumes[index]`
   (data/volumes.bin). The reference wrapper then scales: out = ((value x gain) AND 0xFFFF) >> 6, AND the voice
   mask (0xFFFF, or 0 for a voice muted by the host), gain = left volume (channels 0 and 3) or right volume (channels
   1 and 2), each = (balance x master volume of the host) >> 6, 64 by default. With gain 64 this is the identity for
   values below 1024; `volumes[15]` becomes 0x228. Write AUDxVOL = out twice (two writes, same value).
3. Not a row tick: envelope (5.6), then the slide step for the note-start slide if that flag is set (5.8), then the
   slide step again if the slide-active flag is set (both can run in one tick), then vibrato (5.7). Done.
4. Row tick: row counter = row counter - 1 (byte). If it reaches 0: read the pattern (2.4) until a note, tie, rest,
   voice-off or stop ends the read; nothing else happens to the voice in this tick. Otherwise: glide (5.7), release
   check (5.6), then envelope, slides and vibrato as in 3.

### 5.3 Rows
A voice reads its pattern only when its row counter runs out; the counter counts rows (row ticks), so a note lasts
note length x speed ticks. Length bytes, instrument and volume bytes and most commands are read in the same read as
the next note.

### 5.4 Note start
Note byte n:
1. current note = n; the pattern pointer moves past the byte.
2. Period: i = (n + transposition) AND 0x7F; period = `periods[i]`. If the note-start-slide flag is set: period =
   period + offset word (16-bit wrap) and slide count = its reload. If the slide-on-release flag is set: the
   slide-active flag is cleared.
3. attack steps/size and decay steps/size from the attack and decay bytes; envelope level = instrument volume; row
   counter = note length; release-active = 0; vibrato counter = vibrato period >> 1 (byte); (chord index reset).
   The attack/decay/release delay counters are NOT reset (they keep counting across notes; 5.6).
4. Writes, in this order: AUDxLC = attack pointer (one 32-bit write: the log shows the high and low word at the same
   time), AUDxLEN = attack length; accumulators |= the voice's INTENA and DMACON set-words; INTENA = voice bit
   (0x80 << v, a clear write); DMACON = channel bit (1 << v, a clear write); INTREQ = interrupt accumulator AND
   0x0780 (the bits of every voice that started a note so far in this tick).
The new period and volume are written at the start of the voice's processing in the next tick (5.2).

### 5.5 Instrument selection (0xC0 + n)
All voice flags are cleared (slide-active, note-start-slide, vibrato direction, slide-on-release). From record n
(2.3): sample entry s = byte 0 -> attack pointer = bank + long 0, attack length = word 4, loop pointer = bank +
long 6, loop length = word 10 of entry s; transposition = byte 1 (replaces any 0x8E value); instrument volume = byte
2; attack byte = byte 3; decay byte = byte 4; attack delay counter = attack reload = high nibble of byte 5; decay
delay counter = decay reload = low nibble of byte 5; release point = byte 6; release delay counter = release reload =
high nibble of byte 7, release step = low nibble; vibrato delay = byte 8, depth = byte 9, vibrato period = (2 x byte
10) AND 0xFF.

### 5.6 Envelope and release
Each envelope call does exactly one of:
- attack steps > 0: attack delay counter - 1 (byte); if not 0: done. Else counter = attack reload, attack steps - 1,
  s = attack size + level (byte); if s < 16 level = s (else unchanged).
- else decay steps > 0: decay delay counter - 1; if not 0: done. Else counter = decay reload, decay steps - 1,
  level = level - decay size, or 0 if that would go below 0.
- else release-active: release delay counter - 1; if not 0: done. Else counter = release reload, level = level -
  release step, or 0 below 0. (No step limit: it runs until the next note.)
- else nothing.
A reload of 0 makes the next interval 256 ticks (the counter is decremented from 0).

Release check (row ticks without a pattern read, after the glide): if release point = row counter (after the
decrement) and the next unread pattern byte is not 0x8C (tie): release-active = 1, glide = 0, and slide-active =
slide-on-release flag (set if that flag is set, cleared otherwise).

### 5.7 Glide and vibrato
Glide (row ticks without a pattern read): if glide is not 0: current note = (note + glide) AND 0xFF and the period
is recomputed as in 5.4 step 2 (including the note-start slide offset/count and the slide-on-release rule).

Vibrato (every tick where the envelope runs): it runs if the last pattern byte read was 0x8C (tie), or if (note
length - row counter) (byte) >= vibrato delay (unsigned). When it runs and depth is not 0: if the direction flag is
set: period = period + depth, counter - 1, and when the counter reaches 0 the flag is cleared and counter = vibrato
period; otherwise period = period - depth, counter - 1, and at 0 the flag is set and counter = vibrato period. The
direction flag is only reset by an instrument selection, so it carries over from note to note. The changes
accumulate in the period (a triangle around the note) until the next note start recomputes it.

### 5.8 Slide step
Used for both the slide-active flag and the note-start-slide flag: if slide count = 0: nothing. Else if slide delay
> 0: slide delay - 1. Else if rate counter > 0: rate counter - 1. Else: slide count - 1, rate counter = rate reload,
and the period moves by the step word: bit 15 set = subtract (step AND 0x7FFF), else add (16-bit wrap). The
slide-active flag's count is set only by commands 0x85/0x86, so such a slide runs once; the note-start slide's count
is reloaded at every note start.

### 5.9 Fade
After the voices, every tick (row or not): if fade speed is not 0: fade counter - 1 (byte); when it reaches 0:
counter = fade speed, master volume - 1; when the master volume reaches 0 the song stops (5.10).

### 5.10 Stop, song end, restart
- Stop (command 0x8F, fade end, or a start request >= 2): playing = 0, start request = 1 (the wrapper turns the
  driver's "stop" into "restart on the next tick"), write DMACON = 0x000F, every voice inactive, fade off, and the
  wrapper reports song end. The remaining voices of that tick do nothing (they are inactive). The next tick (if the
  host keeps playing) starts the song again (5.1 step 2).
- Song end by the position lists: each voice that is in use (non-zero header offset) is marked when it reads the 0
  terminator of its list (2.2); when all voices in use are marked, song end is reported and the marks are cleared.
  The song itself continues (the lists loop).
- The reference host stops 2 to 3 ticks after the tick in which song end is reported (that tick and the following
  ones run normally; after a stop the next tick restarts the song, so the logs end with the song's first ticks).

### 5.11 Audio interrupt handler
Each started note enables its channel's audio interrupt at the start of the next tick (5.1 step 1). When the host
dispatches an audio interrupt (the engine calls the handler for each raised channel, lowest first): let S =
INTREQ AND INTENA AND 0x0780 at that moment. If S does not contain the calling channel's bit, nothing happens (an
earlier call of this dispatch round already served it). Otherwise, for each channel v = 0..3 whose bit (0x80 << v)
is in S: AUDxLC = loop pointer (32-bit write), AUDxLEN = loop length. Then INTREQ = S and INTENA = S (both clear
writes). The loop part therefore replaces the attack part once Paula has latched the attack part.

The request comes from Paula, not from the driver. Two cases occur in the reference:
- the usual one: the channel's DMA is switched on at the start of a tick, Paula raises the request at the channel's
  first DMA fetch, and the handler runs as soon as the tick routine returns, serving every channel started in the
  previous tick in one call;
- a race: the tick writes DMACON (on) and, 18 cc later, INTREQ (clear) for the same channels. If a channel's first
  DMA fetch falls inside those 18 cc, its request is cleared again; the interrupt stays enabled and is only raised
  when the attack part has played to its end (Paula's next block start). The handler then runs between ticks for
  that channel alone. With exact write times (6) a port gets this from the emulated Paula automatically.

## 6. Timing

### 6.1 Clock rule
- The play routine is called once per timer tick; in the reference the calls are exactly 70 940 cc apart (the
  host's standard 50 Hz player timer). The audio interrupt handler is called by the host when Paula raises an
  enabled audio interrupt (5.11): at the moment the tick call returns if the request came up during the tick, or at
  the request time otherwise.
- Within one call, the driver's work is counted in CPU cycles (cy) from the call time E. Every step in the tables
  below adds a fixed number of cycles; a register write happens at E + floor(S / 2) cc, where S is the cycle count
  including the step that ends with that write. The call returns at E + floor(S_end / 2).
- Host calls (the four interrupt-vector installations of the start tick, the song-end report) take no time but
  drop an odd half cycle: S is rounded down to even at that point.
- Costs do not depend on data values (multiplications and branches have fixed costs in the reference).
- The 32-bit AUDxLC writes show both words at the same time.

### 6.2 Tick
| step | cy | ends with |
|---|---|---|
| call to first write | 443 | DMACON |
| | 36 | INTREQ |
| | 24 | INTENA |
| no start: start check, sound-effect check, speed count | 202 | (voices follow) |
| start (5.1 step 2): head | 223 | |
| start: each voice in order 0..3, used / unused | 227 / 67 | |
| start: remainder | 84 | INTENA = 0x0780 |
| start: to the first vector installation (host call) | 124 | |
| start: to each of the next three host calls | 66 | |
| start: after the last host call | 108 | ADKCON = 0x00FF |
| start: speed count etc. | 136 | (voices follow) |

Each voice, in order 0..3:
| step | cy | ends with |
|---|---|---|
| voice call | 26 | |
| inactive voice: | 37 | voice done |
| active voice: to the period write | 73 | AUDxPER |
| to the first volume write, channel 0/1/2/3 | 432 / 480 / 518 / 553 | AUDxVOL |
| to the second volume write, channel 0/1/2/3 | 137 / 168 / 199 / 207 | AUDxVOL |
| after the volume writes | 53 | |
| row tick: row counter | 25 | |
| (row tick with a pattern read: see 6.4) | | |
| row tick without read: glide, then release check | 6.3 | |
| envelope, note-start slide, slide, vibrato | 6.3 | |
| return | 16 | voice done |

After voice 3: fade off 55 / fade counting 80 / fade step 125 (fade step: estimated); then speed counter reload
345 or no reload 325; the call returns. (A fade that reaches master volume 0 stops the song instead, 5.10.)

### 6.3 Per-voice maintenance costs (each includes its own call overhead)
| part | case | cy |
|---|---|---|
| envelope | nothing to do | 97 |
| | attack, delay counting | 80 |
| | attack step stored / sum >= 16 (not stored) | 169 / 157 |
| | decay, delay counting | 101 |
| | decay step / clamped to 0 | 182 / 186 |
| | release, delay counting | 122 |
| | release step / clamped to 0 | 187 / 191 |
| note-start slide | flag clear | 59 |
| | flag set | 43 + slide step |
| slide | flag clear | 59 |
| | flag set | 43 + slide step |
| slide step (5.8) | count 0 | 37 |
| | delay counting | 74 |
| | rate counting (estimated) | 95 |
| | period step down / up (up: estimated) | 160 / 152 |
| vibrato | delay not reached | 104 |
| | runs (elapsed >= delay) | 88 + vibrato body |
| | runs (last byte was a tie) | 43 + vibrato body |
| vibrato body | depth 0 | 37 |
| | period up / up and turn | 111 / 151 |
| | period down / down and turn | 111 / 151 |
| glide | glide 0 | 55 |
| | glide step (estimated) | 77 + period computation |
| period computation (5.4 step 2) | base | 118 |
| | note-start slide flag set (estimated) | + 56 |
| | slide-on-release flag set (estimated) | + 20 |
| release check | release point not reached | 67 |
| | reached, next byte is a tie | 88 |
| | reached, released / with slide-on-release (estimated) | 165 / 185 |

### 6.4 Pattern read
Each byte read costs 19 cy plus its own cost:
| byte | cy | ends with / continues |
|---|---|---|
| note | 334 + period computation | AUDxLC |
| | 40 | AUDxLEN |
| | 241 / 272 / 303 / 311 (channel 0/1/2/3) | INTENA |
| | 40 | DMACON |
| | 36 | INTREQ |
| | 16 | voice done |
| length 0xE0..0xFF | 47 | next byte |
| instrument 0xC0..0xDF | 678 | next byte |
| volume 0xB0..0xBF | 83 | next byte |
| chord 0xA0..0xAF (estimated) | 178 | next byte |
| command 0x80..0x99 | 110 + command | |

Commands (after the 110):
| command | cy |
|---|---|
| 0x80 next pattern | list read (below) |
| 0x90 position jump (estimated) | 48, then list read |
| 0x81, 0x82, 0x8E | 26 each (estimated) |
| 0x83 | 100 |
| 0x84, 0x87, 0x89 | 30 each (estimated) |
| 0x85 | 126 |
| 0x86 / 0x88 | 136 / 158 (estimated) |
| 0x8A / 0x8B | 66 / 38 (estimated) |
| 0x8C tie | 52, voice done |
| 0x8D rest | 100, voice done |
| 0x8F stop | 60, DMACON = 0x000F, then the song-end report (host call) |
| 0x91 voice off (estimated) | 64, AUDxVOL = 0; 36, DMACON; 16, voice done |
| 0x92 | 42 |
| 0x93 / at 15 | 53 / 27 |
| 0x94 / at 0 | 47 / 31 |
| 0x95 | 26 |
| 0x96 / 0x97 | 40, CIA-A port A write; 10 (0x96 estimated) |

List read (0x80, 0x90): 85, plus 20 for voice 0, plus 17; then
- non-zero entry: 155, continue reading the new pattern;
- zero entry: mark the voice, 136 / 173 / 206 / 229 (voice 0/1/2/3); or, when this completes the song end: 192 /
  229 / 262 / 285, host call, 44; then 213 (jump to the restart offset) and the list read starts again (85 [+20] +
  17 ...).

### 6.5 Audio interrupt handler
From the call: 188 cy; then for each channel 0..3: served: 47 -> AUDxLC (32-bit), 20 -> AUDxLEN; not served: 19.
Then 16 -> INTREQ, 16 -> INTENA, 148 -> return.

### 6.6 What the engine sees
Ticks with nothing written still cost time (the four voices' maintenance), so the tick's return time varies with the
voices' state; it matters because pending audio interrupts are served at that moment. Reference values: first write
221 cc after the call; ticks return 1 263..7 482 cc after the call (mean 3 120; the start tick is the longest); a
handler call returns 246 / 270 / 294 / 318 cc after it is called when it serves 1 / 2 / 3 / 4 channels, and its
first write comes 117 cc after the call when channel 0 is served.

## 7. Undefined behaviour, limits
- Period indices 36..127 read driver bytes (3); indices 49..51 and 117..119 are host addresses.
- Volume index above 16 (master volume above 16 via 0x8B, or level above 15) reads further driver bytes; does not
  occur.
- A position list without a terminator runs on into whatever follows: in `PAT.TUNE8` voice 3's list (one entry)
  continues into pattern bytes, which give a pattern offset beyond the end of the song file; the reference then
  reads that voice's "pattern" from the sample bank (which the host placed 0x810 bytes after the song's start) and
  meets command 0x9E after 864 ticks, which jumps into the driver's code. Only the first 864 ticks of that song are
  defined.
- Commands 0x98 (built-in tune), 0x9A..0x9F: not specified.
- The sound-effect part of the driver (a second voice set started by the game) is never triggered by the player.
