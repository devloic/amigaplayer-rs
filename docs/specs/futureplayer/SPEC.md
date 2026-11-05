# Future Player (Amiga, `fp.*`) replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the Amiga "Future Player" module format by Paul van der Valk (1988-89), played by the UADE
Eagleplayer "Future Player" (module prefix `fp`, files named `fp.<name>`). The reference modules are
the two Hybris tunes (title and in-game) from the public corpus, plus three synthetic modules built
for this spec.

This is a behaviour description derived from write logs and execution traces of the 68k reference
and from reading the player's data handling. It contains no code of the original. Cycle numbers in
section 8 are the 68000 cycle counts that the reference machine charges (2 cycles = 1 colour clock, "cc").

Status (checked against the reference logs with a private model of this behaviour; see README for how it was built):

| item | result |
|---|---|
| 2 real modules, 7 subsong runs, about 326 000 register writes | value, order, colour clock of every write equal the reference; addresses equal up to one constant load offset per run |
| 3 synthetic modules, 18 subsong runs, about 17 000 register writes | same (one run ends during pre-run and is checked on its two writes) |
| paths never executed by any reference run | listed in README |

## 1. File format

### 1.1 Container

A module is a loadable Amiga executable ("hunk" file) with exactly one code hunk. The file starts
with the big-endian words: `0x000003F3`, `0` (no resident library names), `1` (one hunk), `0`, `0`
(first and last hunk number), then the hunk size in 32-bit words with its memory-type flags in the top
byte (a non-zero top byte, normally `0x40` = chip memory, is required), then `0x000003E9` (code hunk) and
the code hunk size in words, then the hunk image, then relocation records (`0x000003EC` with
lists of 32-bit offsets, ending with a zero count) and the end marker `0x000003F2`.

The hunk image is the module. Every pointer stored in the image is an offset from the start of the
image (as stored in the file, before relocation); the loader adds the load address. All "pointer"
fields below are such offsets, and all integers are big-endian. An open replayer can ignore the
relocation records and treat pointers as image offsets, keeping the image in memory as a byte array;
only the values written to the Paula address registers depend on the load address
(section 6.9).

The image contains no replay code: the driver is the 68k program supplied by the Eagleplayer, and the
image is data. (The first four bytes of the image are a short stub that is never run by the player.)

### 1.2 Detection

All of these hold, with offsets from the start of the file:

- the long word at 0 is `0x000003F3`;
- the byte at offset 20 (the memory-type byte of the hunk size) is non-zero;
- the long word at offset 32 (first word of the image) is `0x70FF4E75`;
- the eight bytes at offsets 36 to 43 (image offsets 4 to 11) are the ASCII text `F.PLAYER`;
- the long word at offset 64 (image offset 32, the pointer of the first song-table entry) is non-zero.

Files with the prefix `fp.` that do not satisfy these are not Future Player modules.

### 1.3 Header and song table (image offsets)

| offset | size | meaning |
|---|---|---|
| 0 | 4 | stub (`0x70FF4E75`) |
| 4 | 8 | text `F.PLAYER` |
| 12 | 4 | pointer to the song name, NUL-terminated ASCII |
| 16 | 4 | pointer to the author name, NUL-terminated |
| 20 | 4 | pointer to a free-text information string, NUL-terminated (may contain line feeds) |
| 24 | 4 | "load size" in bytes (informational) |
| 28 | 4 | "calculated size" in bytes (informational) |
| 32 | 8 per entry | song table: pointer to a song record (4 bytes), then a 4-byte parameter word; the table ends at the first entry whose pointer is 0 |

- Number of subsongs = number of entries in the song table. Subsongs are numbered from 0.
- Song title = the string at the name pointer; author = the string at the author pointer; the
  information string is shown by the reference as "special info". The two corpus modules carry the titles "Hybris title" and "Hybris ingame".
- The parameter word of an entry: bits 0-15 = pre-run count P (section 3), bits 16-23 = fade-in step
  F (section 4.2); bits 24-31 are unused.
- The song table of the corpus modules has 1 entry (title) and 6 entries (in-game; entries 2 to 5 share one
  song record and differ only in the pre-run count: 0, 64, 128, 224).
- Song length: the format stores none. Playing time is only known by running the replay until the
  song-end event (section 7); looping tunes never end (the title module's single subsong runs more than
  200 seconds).

### 1.4 Structures

Offsets are bytes from the start of the structure.

**Song record**

| offset | size | meaning |
|---|---|---|
| 8, 12, 16, 20 | 4 each | pointers to the track records of voices 0 to 3 (hardware channels 0 to 3); 0 = voice not used |
| 24 | 1 | speed: `S = value AND 7`; if that is 0 then 8 |
| 25 | 1 | timer byte `T` |

Other bytes are not used by the replay.

**Track record** (one per voice; calls and jumps also use track records): byte offset 8: pointer
to the first byte of the command stream.

**Command stream** (section 5): a byte stream of items; each item is a note item or a command item.

**Sound record**: offset 8 = pointer to a wave-set record (section 1.4.1), offset 12 = pointer to the
parameter block (section 1.4.2).

**Wave-set record**: byte at offset 8 = mode (0 = single waveform, non-zero = waveform list), long at
offset 10 = pointer (to a waveform record in mode 0, to a list header in list mode). The pointer field
is at an odd word position (it is a 32-bit value at offset 10, unaligned relative to the record start).

**List header**: the entries start at offset 8. Each entry is 6 bytes: pointer to a waveform record
(4), duration in ticks (1), flag (1). The list ends at the entry whose pointer is 0.

**Waveform record**:

| offset | size | meaning |
|---|---|---|
| 8 | 4 | pointer to the waveform data (signed 8-bit samples; must be in chip memory when loaded) |
| 12 | 2 | length in bytes (the value written to the length register is this divided by 2, i.e. in words) |
| 16 | 1 | flags: bit 0 = "synth" waveform (address modulators and half length are used), bit 1 = one-shot (after the start the voice is pointed at the silent buffer) |
| 17 | 1 | signed transposition added to the note index |

**Modulator table**: offset 8 = pointer to signed or unsigned data bytes, offset 12 = word with the
number of data bytes.

**Arpeggio table**: from offset 8, a sequence of bytes (semitone offsets) terminated by the byte `0x80`.

#### 1.4.2 Parameter block (pointed to by the sound record)

All fields are bytes unless stated; values are offsets in the block:

| offset | meaning |
|---|---|
| 8 | volume scale (0..255) |
| 14..17 | detune of voices 0..3 (signed bytes, added to the period) |
| 18 | attack step A and start level |
| 19 | attack target level (peak) |
| 20 | decay step |
| 21 | decay target = sustain level |
| 22 | slope byte (second decay: values below 0x80 rise by that amount, 0x80 and above fall by value AND 0x7F) |
| 23 | slope target = final level |
| 24 | release step |
| 25 | retrigger level mode (0 or non-zero) |
| 26 | pointer (4 bytes) to period modulator 0's table, 0 = off |
| 30 | period modulator 0 shift |
| 31 | period modulator 0 delay |
| 32 | period modulator 0 flag |
| 33 | period modulator 0 negate |
| 34 | pointer (4 bytes) to period modulator 1's table, 0 = off |
| 38, 39, 40, 41 | period modulator 1 shift, delay, flag, negate |
| 42 | pointer (4 bytes) to address modulator 2's table, 0 = off |
| 46, 47, 48 | address modulator 2 shift, delay, flag |
| 50 | pointer (4 bytes) to address modulator 3's table, 0 = off |
| 54, 55, 56 | address modulator 3 shift, delay, flag |

(The offsets are decimal.)

#### 1.5 Data kept in the player

The module does not carry the period table. It is 97 16-bit values, index 0 to 96 (index 0 is the
lowest note). File `data/periods.bin` holds them as big-endian words; values:

7232 6848 6464 6096 5760 5424 5120 4832 4560 4304 4064 3840 3616 3424 3232 3048 2880 2712 2560 2416 2280 2152 2032 1920 1808 1712 1616 1524 1440 1356 1280 1208 1140 1076 1016 960 904 856 808 762 720 678 640 604 570 538 508 480 452 428 404 381 360 339 320 302 285 269 254 240 226 214 202 190 180 170 160 151 143 135 127 120 113 107 101 95 90 85 80 76 72 68 64 60 57 53 50 47 45 42 40 38 36 34 32 30 28

Indices above 96 are read from whatever follows the table in the player and are not
specified; the corpus never uses them.

Default objects (used when a voice has no sound yet, the arpeggio is reset, or a waveform list is
empty): a default arpeggio table whose entries are `0x00`, `0x80`; a default sound whose parameter
block is all zero (so all volumes are 0) and whose wave set is single-waveform mode pointing to a
default waveform record with data pointer = the player's silent 4-byte chip buffer, length field 2
(one word), flags 0, transposition 0. The silent buffer is the "dummy" address below.

## 2. Replay model

- The player runs one **tick** per timer interrupt. The interval between tick starts is exactly
  `5 * (256*T + 1)` colour clocks, where `T` is the timer byte of the subsong (section 1.4). It never
  changes during a song.
- A **beat** happens every `S` ticks (`S` = speed, section 1.4). On a beat tick the sequencer of
  each voice runs before the sound processing of the voices.
- Four voices map one-to-one to the Paula channels 0 to 3. Voice `i` uses channel registers at
  `0xDFF0A0 + 16*i` (address, high word at +0 and low word at +2; length at +4; period at +6; volume at
  +8) and DMA enable bit `1 << i` of the DMA control register `0xDFF096`.
- All register writes are 16-bit writes. The 32-bit address is written as two 16-bit writes (high word
  to +0, low word to +2) at the same colour clock; this document says "write address" for the pair.
- Per-voice state (all start at 0 unless stated in section 3):

| name | meaning |
|---|---|
| running / ended | voice state; an ended voice does nothing and is silent |
| sound | the current sound record; the default sound at the start |
| note | current note index (8 bit), changed only by non-zero notes |
| gate | note value of the last note item; 0 = key released |
| trigger | set for one tick when a note item starts a new note |
| duration | beats left in the current item (8 bit counter) |
| stream pointer | position of the next item |
| call stack | up to 8 levels: saved return pointers, loop start pointers, loop counters; level index (in steps of 1) |
| transpose 1, transpose 2 | bytes set by two commands, added to the note index |
| arpeggio table, arpeggio index | current table (default at the start) and index (16 bit) |
| portamento speed | 16 bit, 0 = off |
| last period | the period of the previous tick before modulation and detune (16 bit; 0 at the start) |
| level, phase | envelope level (0..255) and envelope phase 0..3 |
| detune | signed byte; see command 1 |
| modulator index / delay | for the four modulators of the sound: index (16 bit; bit 15 set = stopped) and delay counter |
| list state | current waveform record, remaining ticks, next list entry, loop entry, "started" flag |
| master | fade-in master level 0..255 (shared by all voices) |

## 3. Start-up of a subsong

The host calls the player's initialisation with the subsong number. The player then:

1. Silences hardware (the host wrapper writes, see LOGS.md; these writes are not part of the
   replayer proper).
2. Sets master to 0 for all voices. If `F` (fade-in step of the song-table entry) is non-zero, fade-in is
   active; otherwise master is set to 255 at the end of start-up. Voice timing and `S`, `T` are taken
   from the song record.
3. For all four voices: ended, sound = default sound, arpeggio = default, arpeggio index 0, portamento
   0, level 0, detune 0, both transposes 0, duration = 1, master = 0.
4. For every voice whose track pointer in the song record is non-zero: state running; stream pointer =
   the track's stream start; call stack level 0 with that stream start as return pointer. Voices
   with pointer 0 stay ended (and count as already finished for the song-end test, section 7).
5. The beat counter is set to 1 (so the first tick is a beat).
6. **Pre-run**: for `P` rounds, the sequencer step (section 5) is run for voices 0, 1, 2, 3 in this
   order. During pre-run, command 13 does nothing. No register writes happen during pre-run except
   the voice-end writes of section 5.2 (and the song-end event can fire, which ends the run before
   the first tick).
7. If fade-in is not active, master = 255 for all voices.

The first tick starts one timer period after the host's initialisation finishes plus a small host
cost (LOGS.md, "first tick").

## 4. Tick

Order of events inside one tick:

1. Interrupt entry (cost `tick_in` in section 8).
2. Fade-in step (4.2).
3. Decrement the beat counter. If it is 0: reload it with `S` and run the sequencer step of voices 0,
   1, 2, 3 in this order.
4. Run the sound processing of voices 0, 1, 2, 3 in this order (section 6).
5. Interrupt exit.

### 4.2 Fade-in

Active only if `F` is non-zero, from tick 0 until it finishes. At the start of each tick while active:
`n = F + master`; if `n > 255`, set master = 255 and end the fade; otherwise master = `n` for all voices.
So the first tick already uses master = `F`. There is no fade-out.

## 5. Sequencer

Run once per voice per beat. A voice that is ended does nothing.

1. Clear the voice's trigger flag; set the per-call command counter to 0.
2. Duration = (duration - 1) modulo 256. If it is not 0, stop.
3. Read items from the stream pointer onward until a note item is read:

**Command item**: first byte has bit 7 set. The command number is the low six bits of the byte
(bit 6 ignored); the following byte is the operand `op`. Both bytes are always consumed; any parameters
follow. First, the command counter is decremented modulo 256 (so the first command of a call makes it
255); if it reaches 0 (the 256th command of one call), the sequencer step stops at once without
changing the stream pointer or the duration (duration stays 0, so the voice then waits 255 beats
and parses the same items again). Otherwise the command is executed and the next item is read at the
position it leaves.

**Note item**: first byte has bit 7 clear. The first byte is the note value `n`; the second byte is `d`.
- If `n != 0`, the voice's note = `n`. The gate of the voice = `n` (so `n = 0` releases the key).
- If bit 7 of `d` is set (tie): duration = `d AND 0x7F`; no new note is started.
- Otherwise duration = `d` and, if `n != 0`, the trigger flag is set (a new note is started in the same
  tick's sound processing).
- The stream pointer is set to the byte after `d`, and the sequencer step ends.

### 5.1 Commands

Parameters follow the operand byte. "Pointer" = 4 bytes, image offset. The stream position after the
command is given.

| cmd | parameters | behaviour |
|---|---|---|
| 0 | none | if the call stack level is above 0, go down one level and continue at the saved return pointer of that level; otherwise end the voice (5.2) |
| 1 | pointer to a sound record | the voice's sound = that sound. All four voices' detune values are replaced by the signed bytes at offsets 14 to 17 of this sound's parameter block (voice `i` gets byte `14 + i`), not only this voice's. This voice's four modulator indices and delay counters are cleared, and its waveform-list "started" flag is cleared |
| 2 | pointer to an arpeggio table | arpeggio table = that table (index unchanged) |
| 3 | none | arpeggio table = default, index = 0 |
| 4 | 16-bit word | portamento speed = word (0 turns portamento off) |
| 5 | none | nothing |
| 6 | pointer to a track record | call: store the position after the pointer as the return pointer of the current level; if the level is below 7, go up one level (at level 7 the level stays and the next call overwrites the same return slot); set the new level's loop counter to 1; continue at the stream start of the track record |
| 7 | pointer to a track record | jump: continue at the stream start of the track record (no return pointer stored) |
| 8 | none | loop start: loop start position of the current level = position after the operand; loop counter of the current level = `op` |
| 9 | none | loop end: loop counter of the current level = (counter - 1) modulo 256; if it is non-zero, continue at the loop start position of the current level, otherwise continue after the command |
| 10 | none | continue at the loop start position of the current level, unconditionally (the original tests a flag that nothing ever sets) |
| 11 | none | transpose 1 = `op` |
| 12 | none | transpose 2 = `op` |
| 13 | none | end the voice at once (ignores the call stack); no effect during pre-run |
| 14 | none | nothing |

Command numbers above 14 are not specified.

### 5.2 Ending a voice

The voice is marked ended. If it was running, it becomes silent: write the DMA control register with
the voice's channel bit (clear bit, no 0x8000), then run the song-end test (section 7). If it was already
silent nothing is written. The sequencer step stops.

## 6. Sound processing of one voice

Costs written as `[name]` are in cycles (section 8). The cycle counter `C` is 0 at the start of the
tick; a register write that is marked WRITE happens at colour clock `tick start + floor(C/2)` where
`C` includes every charge listed above that WRITE (the write's own cost is charged first). Steps are
in the order the reference executes them.

### 6.1 Entry

1. Charge `[pr_a]`.
2. If the voice is ended: charge `[pr_ended]`; nothing else happens for this voice in this tick.
3. Charge `[pr_b]`, then `[pr_c]`.

### 6.2 New note (trigger flag set)

If the trigger flag is set:

1. Charge `[tr_a]`. WRITE DMA control register = channel bit (stops the channel).
2. Charge `[tr_b]`. Arpeggio index = 0; waveform-list "started" flag cleared; envelope phase = 0.
3. Level: if parameter byte 25 is non-zero: level = parameter byte 18, charge `[lv_set]`. Otherwise
   charge `[lv_cmp]`, and if level is not greater than parameter byte 18, set level = byte 18 and charge
   `[lv_set]`.
4. For each modulator `j` = 0, 1, 2, 3 (flag bytes 32, 40, 48, 56; delay bytes 31, 39, 47, 55):
   charge `[mi_a_j]`; if the flag byte is not equal to 1: modulator index = 0, delay counter = delay
   byte, and charge `[mi_b_j]`.

(The new-note flag itself is cleared in section 6.8.)

### 6.3 Choosing the waveform record W

Charge `[ty_a]`. Let `mode` = byte 8 of the sound's wave-set record.

**mode != 0, waveform list**: charge `[l_a]`.
- If the "started" flag is clear: charge `[l_first]`. Look at the first list entry (list header offset 8).
  If its pointer is 0: `W` = default waveform record, charge `[l_empty]`, and stop here (the flag stays
  clear, so this is repeated every tick). Otherwise: loop entry = first entry, current waveform = the
  entry's pointer, remaining ticks = the entry's duration byte, next entry = first entry + 6,
  started flag = set, charge `[l_load]`.
- `W` = current waveform. Charge `[l_dec]`. Remaining = (remaining - 1) modulo 256. If it is 0: charge
  `[l_adv]`; entry = next entry; if that entry's pointer is 0, entry = loop entry and charge `[l_wrap]`;
  charge `[l_rd]`; current waveform = the entry's pointer, remaining = its duration byte, next entry =
  entry + 6; if the entry's flag byte is non-zero, loop entry = this entry and charge `[l_flag]`; charge
  `[l_end]`. (The newly read entry is shown from the next tick on.)

**mode = 0, single waveform**: charge `[s_a]`; `W` = the waveform record pointed to by the wave-set record.

### 6.4 Period

1. Charge `[base]`. `B` = (byte 17 of `W`, plus note, plus transpose 1, plus transpose 2) modulo 256.
2. If portamento speed is 0 (arpeggio mode): charge `[arp_a]`. `d` = byte at (arpeggio table offset 8 +
   arpeggio index). If `d` = 0x80: `d` = the first byte of the table, index = 0xFFFF, charge `[arp_wrap]`.
   Then index = (index + 1) modulo 65536 (so after a wrap the first entry is used twice in a row); charge
   `[arp_b]`. Period `P` = table entry number `(B + d) AND 0x7F` of the period table (section 1.5).
3. If portamento speed is not 0: charge `[por_a]`. Target `T` = table entry `B AND 0x7F`; `C0` = last
   period. If `T >= C0`: charge `[por_up]`; `X = C0 + speed`; if `X > 65535`: `X = T`, charge `[por_clamp]`;
   otherwise charge `[por_up_ok]` and, if `X >= T`: `X = T`, charge `[por_up_clamp]`, then `[por_clamp]`.
   If `T < C0`: charge `[por_dn]`; `X = C0 - speed`; if `X < 0`: `X = T`, charge `[por_clamp]`;
   otherwise charge `[por_dn_ok]` and, if `X <= T`: `X = T`, charge `[por_clamp]`. Finally charge
   `[por_end]`; `P = X`. (The arpeggio index does not advance while portamento is on.)
4. Last period = `P` (before modulation and detune); charge `[per_save]`.
5. Period modulators 0 and 1 (table pointers at parameter bytes 26 and 34; modulator 1 starts with a
   charge of `[m1_pre]`; if a modulator's pointer is 0 it does nothing and costs nothing further).
   For modulator `m` (costs `[m0_*]` or `[m1_*]`): charge `[m_a]`. If the delay counter is not 0: decrement
   it, charge `[m_delay]`, and finish. Otherwise charge `[m_b]`; if the modulator index has bit 15 set,
   finish. Otherwise: `x` = signed byte at (data pointer + index); charge `[m_c]`; if the negate byte
   (33 or 41) is non-zero: `x = -x`, charge `[m_neg]`; charge `[m_d]`; `x = x << (shift byte AND 63)`,
   where a count of 16 or more gives 0, kept as a signed 16-bit value (shift bytes 30 and 38);
   `P = (P + x) modulo 65536`; index += 1; if index >= the word count of the table: charge `[m_wrap]`,
   index = 0, and if bit 7 of the flag byte (32 or 40) is set: index = 0xFFFF, charge `[m_stop]`;
   finally charge `[m_e]`.
6. Charge `[det]`; `P = (P + detune) modulo 65536` (detune is the voice's signed detune byte).
   WRITE period register = `P`. Then charge `[per_post]`, `[setper_i]` (voice `i`), `[w_a]`.

### 6.5 Waveform address and length

1. List mode: the record `W2 = W`, and the register update is always done; charge `[w_list]`.
   Single mode: charge `[w_single]`; `W2` = the waveform record of the wave-set record; if the trigger flag
   is set the update is done, otherwise charge `[w_chk]` and the update is done only if bit 0 of byte 16 of
   `W2` is set.
2. If the update is done: charge `[go_a]`. `A` = pointer at offset 8 of `W2`; `L` = word at offset 12
   divided by 2 (words). If bit 0 of byte 16 of `W2` is set ("synth" waveform): `L = L / 2` (integer),
   charge `[sync_a]`, then the two address modulators:
   - Modulator 2 (table pointer at parameter byte 42; nothing happens if it is 0): charge `[a2_a]`; if
     its delay counter (byte 47 at start) is not 0: decrement it and charge `[a2_delay]`; otherwise charge
     `[a2_b]` and, if its index has bit 15 clear: charge `[a2_c]`; `v` = unsigned byte at (data pointer +
     index); `v = v << ((shift byte 46 + 1) AND 63)` (0 if the count is 16 or more, truncated to 16
     bits); `A = A + v`; index += 1; if index >= word count: charge `[a2_wrap]`, index = 0, and if bit 7
     of flag byte 48 is set, index = 0xFFFF and charge `[a2_stop]`; charge `[a2_e]`.
   - Charge `[a3_pre]`. Modulator 3 (pointer at byte 50; nothing if 0): charge `[a3_a]`; while its delay
     counter is not 0: decrement it and charge `[a3_delay]` then `[a3_loop]` (the whole delay is used up in
     this one tick); then charge `[a3_b]`; if its index has bit 15 clear: charge `[a3_c]` and do as
     modulator 2 with shift byte 54 and flag byte 56, using `[a3_wrap]`, `[a3_stop]`; finally `[a3_e]`.
   Then: charge `[len_a]`, WRITE length register = `L`; charge `[addr_a]`, WRITE address = `A`; charge
   `[addr_b]` and `[settwo_i]` (voice `i`).

### 6.6 Volume

Charge `[vol_a]`, `[vol_b]`, `[vol_pre_i]`. WRITE volume register = `V`, then charge `[vol_post_i]`, where
`V = (bits 16 to 23 of ((level * parameter byte 8) * master)) >> 2` (a 6-bit value, 0 to 63). The level
used is the one from before this tick's envelope step. The reference's host volume/balance scaling is the
identity at its default settings and is ignored.

### 6.7 Envelope step

Charge `[env_a]`. Then:
- gate = 0 (released): charge `[rel_a]`; level = level - parameter byte 24; if level < 0: level = 0, charge
  `[rel_floor]`.
- otherwise charge `[env_g]` and act on the phase:
  - phase 0 (attack): charge `[ph0_a]`. If byte 18 equals byte 19: do the decay step below (the phase stays 0).
    Otherwise charge `[ph0_b]`; level += byte 18; if level > 255: level = byte 19, phase = 1, charge `[ph0_to1]`;
    otherwise charge `[ph0_c]` and, if level >= byte 19: level = byte 19, phase = 1, charge `[ph0_to1]`.
  - phase 1: charge `[ph1_a]` and do the decay step.
  - phase 2: charge `[ph2_a]` and `[ph2_b]`; `r` = byte 22. If `r < 0x80`: charge `[ph2_up]`, level += `r`; if
    level > 255: level = byte 23, phase = 3, charge `[ph2_up_end]`; otherwise charge `[ph2_up_ok]` and, if level
    >= byte 23: level = byte 23, phase = 3, charge `[ph2_up_end]`. If `r >= 0x80`: charge `[ph2_dn]`, level -=
    (`r AND 0x7F`); if level < 0: level = byte 23, phase = 3, charge `[ph2_dn_end]`; otherwise charge `[ph2_dn_ok]`
    and, if level <= byte 23: level = byte 23, phase = 3, charge `[ph2_dn_end]`.
  - phase 3: charge `[ph3_a]` and `[ph3_b]`; level = byte 23.
  - decay step: charge `[dec_a]`; level -= byte 20; if level < 0: level = byte 21, phase = 2, charge
    `[dec_end]`; otherwise charge `[dec_ok]` and, if level <= byte 21: level = byte 21, phase = 2, charge
    `[dec_end]`.

### 6.8 Waiting for the beam and starting the channel

1. Wait for the raster (6.10).
2. Charge `[after_dw]`.
3. If the trigger flag is set: charge `[on_a]`; WRITE DMA control register = `0x8000 + channel bit` (starts
   the channel); clear the trigger flag; charge `[on_b]`; the voice is done.
4. Otherwise charge `[nt_a]`; in single-waveform mode charge `[os_a]` and, if bit 1 of byte 16 of the
   waveform record is set (one-shot): charge `[os_len]`, WRITE length register = 2; charge `[os_addr]`,
   WRITE address = the silent buffer (the player's 4-byte chip buffer). Then charge `[end_proc]`.

### 6.9 Address values

The values written to the address registers are `load address + image offset` (plus the modulator sums
above), or the silent buffer address. The silent buffer lies in the player's own chip memory, so its value
is a host property; in the logs of this spec it appears as the high word 0x0079.

### 6.10 Raster wait

Charge `[dw_a]` and `[dw_b]`. Then 9 times: charge `[dw_it]` and note `L0` = `floor(clock / 227)` where
`clock` is the absolute colour clock (`tick start + floor(C/2)`) after that charge; then repeat: charge
`[dw_rd]`, compute `L1` = `floor(clock / 227)`, charge `[dw_cmp]`, and stop repeating when `L1 != L0`.
After the repeat loop charge `[dw_it_end]`. After the 9 iterations charge `[dw_c]`. (The beam line
advances every 227 colour clocks; the wait lasts from the current line until 9 line changes have been
seen.) This wait makes the voice's remaining writes depend on the absolute clock: a tick start phase
that differs by a few colour clocks mod 227 changes the clock of every later write.

## 7. Song end

Four "voice alive" flags: set for voices that have a track, clear for voices without one. When a voice
ends (5.2, only when it was running): clear its flag; if all four flags are now clear, the song has
ended: all four flags are set again and the host's song-end routine is called. Costs: `[se_a]`, plus
`[se_cmp0]` to `[se_cmp_i]` summed over voices 0 to `i` (the comparisons made to find the voice), plus
`[se_clr_i]`, `[se_b]`, and, if the song ended, `[se_fire]`, then `[se_c]`.
In the reference runs the host stops the run at the end of the tick in which the song end fires; that
last tick still writes everything its remaining voices write. A song whose voices never all end plays
forever (the title module).

## 8. Timing

### 8.1 Clock model

- The reference machine charges each executed instruction a fixed number of cycles that depends only
  on the instruction kind and addressing mode, never on operand values: a multiplication always costs 54,
  shifts and counted loops have fixed costs, a conditional branch has one fixed cost. One colour clock (cc) is 2 cycles. A write that is made by an instruction is
  stamped with `start + floor(C / 2)` where `C` includes the cost of that instruction itself and of all
  instructions executed before it in the tick. All costs below are those fixed values, summed per
  behavioural step; they are listed in 8.3.
- Tick `k` starts at `K0 + k * (5 * (256*T + 1))` colour clocks. The tick interval has no jitter (over
  10 000 ticks). `K0` is a host value (LOGS.md): the first tick starts one tick interval plus about 500 cc
  after the last register write of the host's start-up routine.
- The write clock inside a tick depends on `K0` modulo 227 because of the raster wait (6.10).

### 8.2 Order of charges

**Tick** (section 4): `[tick_in]`, `[tick_a]`; if fade-in is active: `[fade_chk]`, `[fade_a]`; then if
`F + master > 255`: `[fade_end]` and `[setmaster]`, master = 255, fade ends; otherwise `[fade_step]` and
`[setmaster]`, master = `F + master`; then `[tick_b]`; decrement the beat counter; on a beat: `[beat]`, and
for voices 0 to 3: `[seqcall_i]` followed by that voice's sequencer step; then for voices 0 to 3:
`[proccall_i]` followed by that voice's sound processing (section 6); finally `[tick_c]` and `[tick_out]`.

**Sequencer step** (section 5): `[seq_a]`; if the voice is ended: `[rts]` and stop. Otherwise `[seq_b]`;
(trigger cleared, command counter cleared, duration decremented); if the duration is still non-zero:
`[rts]` and stop. Otherwise `[seq_c]`, then per item: `[seq_item]`;
- command item: `[cmd_hdr]`; if the counter reached 0: `[cmd_abort]` and stop; otherwise `[cmd_disp]`, then
  the command's own charge below, then `[cmd_after]` (not charged if the command ended the voice);
- note item: `[note_a]`; if `n != 0`: `[note_set]`; `[note_b]`; then `[dur_tie]` for a tie or `[dur_trig]`
  otherwise, and stop.

Command charges: 0: `[k0_a]`, then either `[k0_pop]` (level above 0) or the voice end; 1: `[k1]`; 2: `[k2]`;
3: `[k3]`; 4: `[k4]`; 5: `[k5]`; 6: `[k6_a]`, `[k6_inc]` only if the level was below 7, `[k6_b]`; 7: `[k7]`;
8: `[k8]`; 9: `[k9_a]` then `[k9_loop]` if the loop is taken or `[k9_exit]` if not; 10: `[k10_a]`, `[k10_jmp]`;
11: `[k11]`; 12: `[k12]`; 13: `[k13_a]` then `[k13_pre]` in pre-run or the voice end; 14: `[k14]`.
Voice end: `[end_a]`; if the voice was running: `[end_b]`, WRITE (DMA control register = channel bit),
`[end_c]`, the song-end test (section 7); then `[end_d]`. The stream stops there.

### 8.3 Cost table (cycles)

| name | cycles | name | cycles | name | cycles |
|---|---|---|---|---|---|
| `tick_in` | 246 | `pr_b` | 20 | `sync_a` | 49 |
| `tick_out` | 168 | `pr_c` | 23 | `a2_a` | 53 |
| `tick_a` | 21 | `tr_a` | 24 | `a2_delay` | 26 |
| `fade_chk` | 30 | `tr_b` | 85 | `a2_b` | 21 |
| `fade_a` | 21 | `lv_set` | 20 | `a2_c` | 93 |
| `fade_step` | 28 | `lv_cmp` | 33 | `a2_wrap` | 29 |
| `setmaster` | 184 | `mi_a_j` | 25/25/25/25 | `a2_stop` | 8 |
| `fade_end` | 42 | `mi_b_j` | 36/36/36/36 | `a2_e` | 12 |
| `tick_b` | 29 | `ty_a` | 37 | `a3_pre` | 25 |
| `beat` | 24 | `l_a` | 21 | `a3_a` | 53 |
| `tick_c` | 16 | `l_first` | 45 | `a3_delay` | 26 |
| `seqcall_i` | 30/30/30/30 | `l_empty` | 18 | `a3_loop` | 78 |
| `proccall_i` | 30/30/30/30 | `l_load` | 100 | `a3_b` | 21 |
| `seq_a` | 23 | `l_dec` | 41 | `a3_c` | 93 |
| `seq_b` | 63 | `l_adv` | 37 | `a3_wrap` | 29 |
| `seq_c` | 16 | `l_wrap` | 16 | `a3_stop` | 8 |
| `seq_item` | 17 | `l_rd` | 57 | `a3_e` | 12 |
| `cmd_hdr` | 29 | `l_flag` | 40 | `len_a` | 12 |
| `cmd_abort` | 36 | `l_end` | 26 | `addr_a` | 12 |
| `cmd_disp` | 68 | `s_a` | 16 | `addr_b` | 18 |
| `cmd_after` | 10 | `base` | 77 | `vol_a` | 196 |
| `note_a` | 9 | `arp_a` | 59 | `vol_b` | 28 |
| `note_set` | 12 | `arp_wrap` | 20 | `vol_pre_i` | 156/179/202/225 |
| `note_b` | 17 | `arp_b` | 60 | `vol_post_i` | 50/50/50/40 |
| `dur_tie` | 64 | `por_a` | 55 | `env_a` | 21 |
| `dur_trig` | 68 | `por_up` | 21 | `rel_a` | 39 |
| `rts` | 16 | `por_up_ok` | 13 | `rel_floor` | 26 |
| `k0_a` | 23 | `por_up_clamp` | 10 | `env_g` | 21 |
| `k0_pop` | 83 | `por_clamp` | 4 | `ph0_a` | 33 |
| `end_a` | 49 | `por_dn` | 21 | `ph0_b` | 25 |
| `end_b` | 40 | `por_dn_ok` | 13 | `ph0_c` | 35 |
| `end_c` | 18 | `por_end` | 4 | `ph0_to1` | 46 |
| `end_d` | 16 | `per_save` | 53 | `ph2_a` | 17 |
| `k1` | 364 | `m0_a` | 53 | `ph2_b` | 21 |
| `k2` | 40 | `m0_delay` | 26 | `ph2_up` | 25 |
| `k3` | 56 | `m0_b` | 21 | `ph2_up_ok` | 33 |
| `k4` | 32 | `m0_c` | 39 | `ph2_up_end` | 46 |
| `k5` | 16 | `m0_neg` | 4 | `ph2_dn` | 33 |
| `k6_a` | 73 | `m0_d` | 55 | `ph2_dn_ok` | 33 |
| `k6_inc` | 4 | `m0_wrap` | 29 | `ph2_dn_end` | 36 |
| `k6_b` | 74 | `m0_stop` | 8 | `ph1_a` | 26 |
| `k7` | 44 | `m0_e` | 12 | `ph3_a` | 26 |
| `k8` | 76 | `m1_a` | 53 | `ph3_b` | 30 |
| `k9_a` | 39 | `m1_delay` | 26 | `dec_a` | 37 |
| `k9_loop` | 34 | `m1_b` | 21 | `dec_ok` | 33 |
| `k9_exit` | 16 | `m1_c` | 39 | `dec_end` | 46 |
| `k10_a` | 21 | `m1_neg` | 4 | `dw_a` | 18 |
| `k10_jmp` | 46 | `m1_d` | 55 | `dw_b` | 28 |
| `k11` | 28 | `m1_wrap` | 29 | `dw_it` | 16 |
| `k12` | 28 | `m1_stop` | 8 | `dw_rd` | 16 |
| `k13_a` | 25 | `m1_e` | 12 | `dw_cmp` | 9 |
| `k13_pre` | 16 | `m1_pre` | 25 | `dw_it_end` | 10 |
| `k14` | 16 | `det` | 24 | `dw_c` | 44 |
| `se_a` | 32 | `per_post` | 18 | `after_dw` | 21 |
| `se_cmp_i` | 33/33/33/33 | `setper_i` | 79/110/141/149 | `on_a` | 24 |
| `se_clr_i` | 22/26/26/16 | `settwo_i` | 95/126/157/165 | `on_b` | 32 |
| `se_b` | 21 | `w_a` | 49 | `nt_a` | 37 |
| `se_fire` | 88 | `w_list` | 14 | `os_a` | 41 |
| `se_c` | 44 | `w_single` | 37 | `os_len` | 16 |
| `pr_a` | 44 | `w_chk` | 27 | `os_addr` | 20 |
| `pr_ended` | 27 | `go_a` | 63 | `end_proc` | 16 |

Per-voice costs have four values for voices 0 to 3. `[se_cmp_i]`, `[se_clr_i]` and the vol/setper/settwo
costs depend on the voice number as shown. `[setmaster]` includes the loop over the four voices.
`[se_fire]` includes the 32 cycles charged for the call into the host's song-end routine.

## 9. Host settings that are ignored

The reference player has per-voice mute switches and left/right volume scaling (host controls), and a
fade-out path that no module can reach. At the host's default settings (all
voices on, full volume) these do nothing visible and are not reproduced. The reference player's
start-up silences the audio channels and clears the voices' volume registers; see LOGS.md for the host
writes.

