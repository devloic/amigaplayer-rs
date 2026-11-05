# Art & Magic (`aam.*`) replayer: behaviour specification

*Sanitised for publication: behaviour only.*

Scope: the sound driver used by the Amiga game "Agony" (music by Jeroen Tel; four modules
`aam.level_1_and_6`, `aam.level_2`, `aam.level_3_and_5`, `aam.level_4` in the archive), as played
by the Eagleplayer "Art And Magic" (UADE name `ArtAndMagic`, prefix `aam`). Documented from
observation of the reference 68k player run through the host with `DELI_WRITE_LOG` and
`DELI_DMA_LOG`, and from the module files themselves. A private model written from this text
reproduces every Paula register write (address, value, order) and every write's colour clock
(relative to the tick, and absolute) of the four real modules and of 34 synthetic modules, and
every audio DMA word of two modules (LOGS.md).

The format is unusual: **the replay code lives inside the module.** The Eagleplayer is only a thin
wrapper: it recognises the file, relocates a block of pointers, patches two places of the module's
code (a volume hook and a song-end hook), calls the module's entry routine once, and calls the
module's play routine once per tick. A replayer does not need the module's code (this spec
describes what it does) but needs its *data* (tables, streams, samples), laid out as in section 2.

All multi-byte values in the module are big-endian. "Offset" means file offset. Hexadecimal values
are written `0x..`. Register names are the Amiga hardware names (AUDxLC, AUDxLEN, AUDxPER, AUDxVOL,
DMACON; x is the channel number).

## 1. What the music is

Three Paula voices (hardware channels 1, 2 and 3; channel 0 is never touched) play 8-bit samples
taken from IFF 8SVX files embedded in the module. A sample is started once per note as a one-shot
part followed by a looping part (the 8SVX one-shot / repeat split). The driver rewrites each voice's
period and volume every tick (50 Hz), restarts samples on note events, and offers instruments with
a volume envelope (attack ramp or decay ramp), a software vibrato, an arpeggio (list of semitone
offsets) and a portamento ("slide") event. Each voice has its own song data: an *order list* (which
pattern to play next, with transposition, instrument bank offset and repeat count) and *streams*
(event sequences, one per pattern).

There is one subsong. Playback loops forever; the host's song-end notification happens when the
voices' order lists wrap (10.3).

## 2. File format

### 2.1 Layout

In file order: the driver code (about 0x700 bytes, containing a few constant tables); the pointer
tables (order-list table, stream table, arpeggio table); arpeggio lists and the instrument table;
order lists and streams; a 2-byte "initial stream"; the voice records (0x40 bytes per voice,
zero in the file: the driver's working memory); the wave table (12 bytes per sample, room for only
two); the sample section (a sequence of IFF 8SVX files). The file ends with the 4 ASCII bytes
`EXIT`, which also terminate the sample section.

### 2.2 Detection, songs, title, length

A file is an Art & Magic module when both hold (the reference player's check):

- the 4 bytes at offset 0x1A are `EXIT` (0x45584954);
- the last 4 bytes of the file are `EXIT`.

Nothing else is checked. One subsong (the host reports subsongs 0..0). The module has no title;
the 8SVX `NAME` chunks hold sample names (for example `l1.filmmegabass1`) and `ANNO` holds
`Protracker 1.1B`. The length is the time to the first order-list wrap (10.3): tick index 3528
(70.56 s), 3456 (69.12 s), 2880 (57.60 s) and 6048 (120.96 s) for level_1_and_6, level_2,
level_3_and_5 and level_4.

### 2.3 Two driver versions

Version **B** is a module whose 4 bytes at offset 0x20E are `01 02 04 08` (level_3_and_5); every
other module is version **A** (level_1_and_6, level_2, level_4). Behaviour is identical except:

- B writes 0x0FF0 to COLOR01 (0xDFF182) on every tick on which a voice is in attack-envelope mode
  (8.5);
- B's code takes 14 fewer cycles in the event entry and 2 more in two other places (section 11,
  costs given as "A / B");
- the table positions differ (2.4).

### 2.4 Locating the tables

All table positions are given by signed 16-bit displacement words at fixed offsets of the module;
table offset = (offset of the word) + (value of the word). Offsets of the displacement words:

| table | A | B |
|---|---|---|
| sample section start (first `FORM`) | 0x0C | 0x0C |
| wave table | 0x14 | 0x14 |
| voice records (voice 0 first) | 0xD4 | 0xD4 |
| order-list table | 0xE2 | 0xE2 |
| initial stream | 0xFE | 0xFE |
| instrument table | 0x15E | 0x15E |
| DMA-bit table (bytes 01 02 04 08) | 0x190 | 0x190 |
| period table | 0x3A0 | 0x3A2 |
| stream pointer table | 0x474 | 0x478 |
| arpeggio pointer table | 0x4E6 | 0x4EA |

Resulting table offsets:

| module | ver | order-list table | stream table | arp table | instruments | period table | DMA bits | voice records | initial stream | wave table | samples |
|---|---|---|---|---|---|---|---|---|---|---|---|
| level_1_and_6 | A | 0x766 | 0x78E | 0x90E | 0x993 | 0x6A6 | 0x6A2 | 0xCBA | 0xCB7 | 0xDBA | 0xDD2 |
| level_2 | A | 0x766 | 0x78E | 0x90E | 0x993 | 0x6A6 | 0x6A2 | 0xDAE | 0xDAC | 0xEAE | 0xEC6 |
| level_3_and_5 | B | 0x76C | 0x794 | 0x894 | 0x919 | 0x6AC | 0x20E | 0xE8E | 0xE8A | 0xF8E | 0xFA6 |
| level_4 | A | 0x766 | 0x78E | 0x90E | 0x993 | 0x6A6 | 0x6A2 | 0xD5A | 0xD56 | 0xE5A | 0xE72 |

The order-list table (5 entries of 8 bytes) is followed directly by the stream table (96 entries in
A, 64 in B), then the arpeggio table (32 entries of 4 bytes); arpeggio list data and the instrument
table follow it.

### 2.5 Pointer tables and their relocation

The module stores pointers as "stored value = K + offset" (K is a constant of the module; the
wrapper adds `load address - K` to every pointer). A port working with offsets needs only K: the
last (32nd) entry of the arpeggio table is a pointer to the start of the instrument table (offset
I; it sits 9 bytes before I), so K = (that stored long) - I.

Pointers are exactly:

- the **order-list table**: five entries of 8 bytes, `long pointer; word speedA; word speedB`.
  Voice v (1..3) uses entry v; entries 0 and 4 are unused. `pointer` points to the voice's order
  list; `speedA`, `speedB` are the step lengths (5.3). All four modules have speedA = speedB = 2;
- the **stream pointer table**: consecutive longs, entry p = the stream of pattern p (entry 0
  is a dummy);
- the **arpeggio pointer table**: 32 consecutive longs, entry a = arpeggio list a (unused entries
  point at the instrument table).

No other long in the module is a pointer. In the logs the module is loaded at address 0x0045E0
(a host choice; AUDxLC values are 0x45E0 + offset).

### 2.6 Sample section

From the sample-section offset the module holds N IFF 8SVX files back to back (N = 5 or 6 in
these modules), each with exactly this chunk order and with chunk sizes used literally (no pad
bytes):

`FORM`, long size, `8SVX`, `VHDR`, long size (0x14), 20 bytes of VHDR body, `NAME`, long size,
name bytes, `ANNO`, long size, annotation bytes, `BODY`, long size, sample bytes.

`EXIT` follows the last file. The first long of the VHDR body is the one-shot length in bytes, the
second long the repeat length in bytes; the rest of the VHDR, NAME and ANNO are not used.

The module's entry routine converts the section in place:

- a **pool** pointer starts at the sample-section start (the address of the first `FORM`);
- for each file in order: (a) a 12-byte entry is appended to the wave table (2.7); (b) the BODY bytes
  are copied byte by byte to the pool and the pool pointer advances by the body length. The pool
  never overtakes the read position.

### 2.7 Wave table

One 12-byte entry per sample, written at the wave-table offset in file order:

| bytes | value |
|---|---|
| 0..3 | address of the sample's first byte in the pool (load address + offset) |
| 4..7 | address of the loop start = start + one-shot length' |
| 8..11 | total length in bytes = one-shot length' + repeat length' (a length, not an address) |

If the VHDR repeat length is zero then one-shot length' = VHDR one-shot length - 2 and repeat
length' = 2 (a two-byte loop at the end); otherwise they are the VHDR values.

The file has room for only two entries (24 bytes) before the sample section begins; further
entries are written **over the first bytes of the pool**, that is over the beginning of sample 0
(12 bytes per entry beyond the second). The order of operations is: entry k is written, then
body k is copied. So entry k (k >= 2) overwrites bytes that body 0 already occupies. This is real behaviour: the reference's audio DMA words equal this model's memory word for
word (426 484 words in level_2, 408 745 in level_3_and_5). An implementation that plays from
separate sample buffers must reproduce the overwritten bytes.

### 2.8 Instrument table

Instrument i is the 8-byte record at (instrument table) + 8*i. The number of records is whatever
the order lists and streams reference (the real tables hold 6 records; the synthetic modules use
16).

| byte | meaning |
|---|---|
| 0 | base volume, signed byte (normally 0x40) |
| 1 | vibrato: high nibble = depth shift, low nibble = half-period in ticks; 0 = no vibrato |
| 2 | vibrato delay in steps (8.0) |
| 3 | flags: bit 6 = arpeggio on, bit 7 = filter pulse at note start (10.4) |
| 4 | detune: signed byte subtracted from the period in every period write |
| 5 | envelope rate: the envelope counter moves by (value + 1) per tick |
| 6 | envelope mode: 0 = none; 1..127 = decay begins when the note has this many steps left or fewer; bit 7 set = attack ramp |
| 7 | sample (wave table entry) number |

### 2.9 Order lists, streams, arpeggio lists, initial stream

**Order list** (one per voice, start = the `pointer` of its order-list table entry): a sequence of
entries, each one of:

- `0xFF, n`: loop: continue at the entry n bytes after the start of the voice's order list (this
  also triggers the song-end notification, 10.3);
- optional prefixes, then a pattern number: `0x80 + t` transposition t (0..127, kept until
  changed); `0x70 + b` instrument bank b (0..15), added to instrument numbers selected in streams;
  `0x60 + r` repeat count r (0..15): the pattern is played r + 1 times in total; then the pattern
  number p (0..0x5F) which selects stream table entry p. Prefixes may only appear in the order
  0x80, 0x70, 0x60; each is optional; without 0x60 the repeat count is 0.

**Stream**: a sequence of events ended by `0xFF` (end of pattern: continue with the next order-list
entry) or `0xFE` (end of voice). Event grammar in section 6.

**Arpeggio list** (entry a of the arpeggio table): bytes; `0xFF, n` = continue n bytes after the
list start; `0xFE` = stop the arpeggio; any other byte is a semitone offset (signed byte) added to
the note.

**Initial stream**: every voice starts with its stream position on a 2-byte stream whose first byte
is 0xFF (so the first event fetch reads the first order-list entry). A port can start every voice
in the "end of pattern" state.

### 2.10 Period table

96 words at the period-table offset, index = note number 0..95 (the vibrato also reads index + 1).
Identical in all four modules:

```
32245 30439 28726 27114 25593 24155 22800 21521 20313 19172 18096 17081
16122 15219 14363 13557 12797 12078 11400 10760 10156  9586  9048  8541
 7181  6779  6398  6039  5700  5380  5078  4793  4524  4270  4031  3805
 3591  3389  3199  3019  2850  2690  2539  2397  2262  2135  2015  1902
 1795  1695  1600  1510  1425  1345  1270  1198  1131  1068  1008   951
  898   847   800   755   713   673   635   599   566   534   504   476
  449   424   400   377   356   336   317   300   283   267   252   238
  224   212   200   189   178   168   159   150   141   133   126   119
```

Values go to AUDxPER unchanged (periods above 0x7FF occur). Note numbers outside 0..95 read
whatever bytes follow the table; the real modules never do this.

## 3. Voice state

Voices v = 1, 2, 3. Fields (named by function; all unsigned unless stated):

- `active` (byte): 0 = voice finished, writes nothing at all any more;
- `listPos`: read position in the order list; `streamPos`: read position in the stream;
- `pattern4`: current pattern number * 4 (word); `repeatLeft` (byte);
- `transpose`, `instBank` (bytes), from the order list;
- `stepCount` (word) and `stepToggle` (word): see 5.3; `rowCount` (byte): steps left in the note;
  `rowLen` (byte): the note's length in steps;
- `note` (byte, used as signed): current note number including transposition; negative = "no
  note sounding", effects are skipped;
- `inst` (word): instrument record offset = instrument number * 8; `wave` (word): wave table
  offset = sample number * 12;
- `period` (32 bits): high word is the period, low word the fraction;
- `baseVol` (word): the volume of the current note (see 6.3), also the volume override set by the
  0xFC prefix; `envVol` (word): the envelope volume actually used;
- `master` (word) = 0x40, never changes (3.1);
- `phase` (byte): 0 = running; 2 = note just started, DMA still to be enabled; 1 = DMA enabled,
  loop still to be set;
- `attack` (flag), `envCounter` (word), `decayLimit` (word, 0xFFFF = none), `flags` (byte, copy of
  instrument byte 3);
- vibrato: `vibThreshold` (word, 0xFFFF = never), `vibShift`, `vibCount`, `vibDir` (bytes);
- slide: `slideTrigger` (word, 0xFFFF = none), `slideMul` (byte), `slideTarget` (byte);
- arpeggio: `arpSel` (arpeggio number * 4), `arpPos`.

### 3.1 Inert machinery

The record also holds a fade-out mechanism (rate and accumulator that would scale `master` and
finally stop the voice). The rate is zero from initialisation and nothing the Eagleplayer does
ever sets it, so `master` stays 0x40. It is ignored here.

## 4. Initialisation (before the first tick)

1. The wrapper relocates the pointers (2.5), patches the module's code (the two hooks, section 10)
   and calls the entry routine.
2. The entry routine converts the sample section (2.6, 2.7), writes the value 0 four times to the
   word register AUD0DAT (0xDFF0AA) (colour clocks 0, 17, 33 and 50 relative to the first write; the
   first four lines of the log), then initialises voices 1, 2, 3.
3. The host initialises voices 1, 2, 3 once more (same values, no register is written).
4. Ticks follow (section 12 for their timing).

Voice initialisation: `listPos` = pointer of the voice's order-list table entry; `streamPos` =
initial stream; `master` = 0x40; `active` = 1; `rowCount` = 1; all other fields 0 (the fields
that matter are rewritten by the first note start before they are read). Note that on the first
tick `inst` is 0 (instrument 0 supplies the detune byte) and `period` = 0, `envVol` = 0, so the
first AUDxPER and AUDxVOL writes of every voice are 0.

## 5. The tick

The play routine runs voices 1, 2, 3 in this order, each in full. A voice with `active` = 0 does
nothing. Otherwise:

### 5.1 Period and volume (every tick)

1. AUDxPER := (high word of `period`) + (the negated detune byte of the current instrument, as a
   signed byte, sign-extended), 16-bit wrap.
2. AUDxVOL := p, where p = ((`master` * `envVol`) mod 65536) >> 6, scaled by the wrapper's gain
   (10.1): ((p * gain) mod 65536) >> 6. With the default gain 64 this is p. Not clamped.

### 5.2 Phase

- If `phase` = 2: `phase` := 1; DMACON := 0x8000 | bit(v), with bit(v) from the DMA-bit table
  (2, 4, 8 for v = 1, 2, 3). The channel's DMA starts and latches the pointer and length written
  by the note start.
- Else if `phase` = 1: `phase` := 0; AUDxLC := loop start address (wave table bytes 4..7; two
  word writes, high then low, same colour clock); AUDxLEN := (repeat length') / 2 where repeat
  length' = total length - one-shot length' = wave table bytes 8..11 minus (bytes 4..7 minus
  bytes 0..3).
- Else nothing.

### 5.3 Step counter

`stepCount` (16 bit, starts at 0) is decremented by one. If it did not wrap below 0, go to 5.4.
On a wrap:

1. `stepToggle` := NOT `stepToggle` (16 bit); `stepCount` := `speedB` if bit 1 of `stepToggle` is
   1, else `speedA` (from the voice's order-list table entry). The first reload after start uses
   `speedB`, the next `speedA`, and so on alternately; a step therefore lasts speed + 1 ticks.
2. `rowCount` := `rowCount` - 1 (byte wrap). If it is not zero, go to 5.4. If it is zero, the next
   event is fetched (section 6) and this voice's tick ends there (no effects in an event tick).

### 5.4 Effects

If `note` is negative the voice's tick ends. Otherwise section 8 runs.

## 6. Events

An event starts at `streamPos`. The grammar is a fixed chain of optional prefixes of decreasing
byte value; the first byte that does not fit ends the chain.

Fetch, step by step (b = the current byte; "read" advances `streamPos`):

1. `slideTrigger` := 0xFFFF.
2. Read b. If b is not 0xFB, write DMACON := bit(v) (switches the voice's DMA off). If b is 0xFB (a
   "tie"), write nothing and read the next byte as b.
3. If b = 0xFF (end of pattern): fetch the next pattern (section 7), then read the first byte b of
   the new stream (no tie test, no DMA-off write).
4. If b = 0xFE: `active` := 0; the event ends and the voice never writes again.
5. If b = 0xFD: go to 6.2.
6. `baseVol` := 0. If b = 0xFC: `baseVol` := the next byte sign-extended to a word; read the next
   byte as b; if b = 0xFD go to 6.2; if b >= 0xFB do not consume it (back up one byte) and go to
   6.3.
7. If b >= 0xF0 (0xF0..0xFA): a **rest**: `rowLen` := b & 15; `note` := 0xFF (negative);
   `rowCount` := `rowLen`; no note start; the event ends. (The envelope volume is frozen where it is
   and the sample keeps playing.)
8. If b >= 0xC0: `inst` := (((b & 0x3F) + `instBank`) as signed byte) * 8; read the next byte as b;
   if b = 0xFD go to 6.2; if b >= 0xC0 back up one byte and go to 6.3.
9. If b >= 0x80: `rowLen` := b & 0x3F; read the next byte as b; if b = 0xFD go to 6.2; if b >=
   0x80 back up one byte and go to 6.3.
10. If b >= 0x60: `arpSel` := (b & 0x1F) * 4; read the next byte as b; if b >= 0x60 back up one byte
    and go to 6.3.
11. `note` := b + `transpose` (byte add). Go to 6.3.

An event may therefore consist of prefixes only (no note byte); the note start still happens with
the old `note` (and with the new `rowLen`, `inst` etc.).

### 6.2 Slide event (0xFD)

Three bytes follow: g, n1, n2. `slideMul` := (g >> 4) + 1; `slideTrigger` := (`rowLen` - (g & 15) +
1) as a signed byte sign-extended to a word; `note` := n1 + `transpose`; `slideTarget` := n2 +
`transpose`. Then 6.3. (`baseVol` is **not** cleared on this path when 0xFD is the first byte of the
event, step 5: the retriggered note keeps the previous note's volume; after another prefix the
clearing of step 6 has already happened.) The slide itself is done by the effects (8.1) once
`rowCount` <= `slideTrigger`.

### 6.3 Finish: note start

`streamPos` is stored (after the consumed bytes). The note start is:

1. `wave` := (instrument byte 7) * 12. AUDxLC := wave table bytes 0..3 (two word writes, high then
   low, same colour clock); AUDxLEN := (wave table bytes 8..11) / 2 (words; the whole sample).
2. If `baseVol` = 0, `baseVol` := the instrument's volume byte sign-extended. `envVol` := `baseVol`.
3. `period` := period table[`note` as signed byte] in the high word, fraction 0.
4. `phase` := 2; `attack` := 0; `decayLimit` := 0xFFFF.
5. With m = instrument byte 6: if m is negative (bit 7): `attack` := 1, `envVol` := 0,
   `envCounter` := 0x200. If m = 0: nothing further (`envCounter` keeps its old value). If m is
   positive: `decayLimit` := m + 1, `envCounter` := 0x200.
6. Filter pulse: if instrument byte 3 has bit 7, write 0x00 to CIA-A port A (0xBFE001); then, in
   every case, write 0x02 to it (10.4).

Then `rowCount` := `rowLen`.

## 7. Next pattern (order-list fetch)

`repeatLeft` := `repeatLeft` - 1 (byte). If it was non-zero (no wrap): the same pattern again:
`streamPos` := stream table entry `pattern4`/4. Otherwise (it wrapped to 0xFF), read from `listPos`:

1. Read e. If e = 0xFF: song-end notification (10.3); read the loop offset n; `listPos` := (order
   list start of this voice) + n; read e there.
2. If e >= 0x80: `transpose` := e & 0x7F; read e.
3. If e >= 0x70: `instBank` := e & 15; read e.
4. `repeatLeft` := 0. If e >= 0x60: `repeatLeft` := e & 15; read e.
5. `pattern4` := (e as signed byte) * 4; `listPos` := the position after e.
6. `streamPos` := stream table entry `pattern4`/4.

## 8. Effects

`rem` = `rowCount`. Effects run in the order 8.0, then slide **or** vibrato (8.1, 8.2), then
arpeggio (8.3), then attack (8.5), then decay (8.4).

### 8.0 First effect tick of a note

If `phase` = 1 (the tick on which 5.2 just changed 2 into 1):

- if instrument byte 1 is non-zero: `vibThreshold` := (`rowLen` - instrument byte 2 + 1) as a
  signed byte sign-extended; `vibShift` := byte1 >> 4; `vibCount` := ((byte1 & 15) + 1) >> 1;
  `vibDir` := 0. Otherwise `vibThreshold` := 0xFFFF;
- `flags` := instrument byte 3;
- if `flags` bit 6: `arpPos` := arpeggio list pointer for `arpSel`/4.

(If this tick happens to be an event tick, effects do not run and this block is skipped.)

### 8.1 Slide

If `rem` <= `slideTrigger` (signed 16-bit compare; 0xFFFF = -1 means never):

1. d := (period table[`note`] - period table[`slideTarget`]) as a signed 16-bit value; d := d *
   `slideMul` * 1024 (32-bit signed product, wrap); `period` := `period` - d (32-bit).
2. Arrival: if the subtraction wrapped below zero (d as unsigned > old `period`): arrived when the
   new period high word (signed) is >= period table[`slideTarget`]; if it did not wrap: arrived
   when the new high word is <= that value.
3. On arrival: `period` := period table[`slideTarget`] * 65536; `note` := `slideTarget`;
   `slideTrigger` := 0xFFFF; `vibCount` := ((`vibShift` & 15) + 1) >> 1; `vibDir` := 0. (The
   vibrato counter is reloaded from the depth shift, not the half-period: a quirk of the driver.)

Vibrato (8.2) is skipped on the ticks on which the slide test is true.

### 8.2 Vibrato

If `rem` > `slideTrigger` (no slide this tick) and (`rem` - `vibThreshold`) as a 16-bit difference
has its bit 15 set (that is `rem` < `vibThreshold` as signed numbers; 0xFFFF never matches):

1. `vibCount` := `vibCount` - 1 (byte). If the byte became negative: `vibDir` := NOT `vibDir` and
   `vibCount` := instrument byte 1 & 15.
2. a := (period table[`note`] - period table[`note`+1]) mod 65536; a := (a * 2) mod 65536;
   s := (a << 16) arithmetically shifted right by `vibShift` (count mod 64); if bit 7 of
   `vibDir` is set, s := -s. `period` := `period` + s (32-bit).

So the period accumulates a triangle wave around its value; it is reset only by a note start, an
arpeggio step or the end of a slide.

### 8.3 Arpeggio

If `flags` bit 6: read x at `arpPos` (advance). If x = 0xFF: read the next byte n, position :=
(arpeggio list for `arpSel`/4) + n, read x there (advance). If x = 0xFE: clear `flags` bit 6 (the
position is not stored). Otherwise `period` := period table[(x + `note`) as signed byte] * 65536 and
`arpPos` := the position after x.

### 8.5 Attack

If `attack`: (version B: write COLOR01 := 0x0FF0 now.) `envCounter` := `envCounter` - (instrument
byte 5 + 1) (16 bit). If it wrapped below zero: `attack` := 0, `envVol` := `baseVol`, then the decay
test follows in the same tick. Otherwise `envVol` := f(0x200 - `envCounter`) and the effects of this
tick end (no decay test), where f(c) = ((((c * `baseVol`) mod 2^32) >> 8) mod 65536) >> 1.

### 8.4 Decay

If `attack` = 0 and (`rem` - `decayLimit`) as a 16-bit difference has bit 15 set (`rem` <
`decayLimit`; 0xFFFF never): `envCounter` := `envCounter` - (instrument byte 5 + 1) (16 bit). If it
wrapped below zero: `decayLimit` := 0xFFFF, `envVol` := 0. Otherwise `envVol` := f(`envCounter`).

## 9. Register writes of a tick, in order

For each active voice v (hardware channel v, registers at 0xDFF0A0 + 0x10*v: LC +0, LEN +4, PER
+6, VOL +8):

1. AUDxPER, AUDxVOL;
2. (a) DMACON (phase 2: DMA on), or (b) AUDxLC (two word writes) and AUDxLEN (phase 1), or nothing;
3. if an event is fetched: DMACON (DMA off) unless tied; then, if a note starts, AUDxLC (two word
   writes), AUDxLEN, one or two CIA writes. An event tick has no effects, hence no later write;
4. otherwise, on an effects tick of a version B voice in attack mode: COLOR01 := 0x0FF0.

A voice with `active` = 0 writes nothing. The long write of AUDxLC appears in the log as two
consecutive word lines (0xDFF0x0 then 0xDFF0x2) with the same colour clock; AUDxLEN is written
about 20 colour clocks later.

## 10. Reference-host behaviours visible in the output

### 10.1 Volume and balance

The wrapper replaces the module's final volume write by a scaling step; with p as in 5.1 the
written value is ((p * gain) mod 65536) >> 6, where `gain` is a per-voice byte: voices 1 and 2 use
(master volume * right balance) >> 6, voice 3 uses (master volume * left balance) >> 6. With the
default volume 64 and balances 64 the gain is 64 and the value is unchanged (all logs). A port
applies its own volume and balance the same way.

### 10.2 CIA reads

A read of 0xBFE001 returns 0 in the reference host, so the CIA writes are 0x00 (instrument bit 7)
and 0x02.

### 10.3 Song end

Every time a voice executes the loop entry (0xFF in an order list) the wrapper calls the host's
song-end notification. In the four modules all three voices loop on the same tick (tick index
3528, 3456, 2880, 6048, the first tick being 0). The host tests for song end once per chunk of
0.1 s of audio (about 5 ticks), so the reference logs end 0 to 3 ticks after that tick.

### 10.4 Filter pulse

An instrument with flag bit 7 makes the note start write 0x00 and then 0x02 to CIA-A port A. On
hardware that makes bit 1 (the audio filter / LED) low and then high again within a few
microseconds; the net state is unchanged. No instrument of the four modules has the flag.

## 11. Timing

### 11.1 Clock

Ticks are called every 70 940 colour clocks (verified over about 7 000 ticks). `H_k` is the colour
clock at which the host starts tick k (12). All costs are 68000 cycles, 2 per colour clock. The
reference host counts a **static cost per instruction**: a short conditional branch counts 9 and a
long one 11 whether taken or not; a multiplication counts 54 plus its operand cost; a register
shift whose count is in a register counts like a shift by 4. Hence all costs below depend only on
the path taken.

For tick k keep a cycle counter T, 0 at `H_k`. A register write is stamped `H_k + floor(T/2)`,
T being the value after the step that contains the write (the numbers are chosen so that each
write is "at the end of its step"). The two words of a long write get the same stamp. The only
exception is the song-end notification (11.9).

### 11.2 Tick skeleton

T = 258 when the play routine begins its first voice (130 for the wrapper's entry, 128 for the
routine's own entry); before each voice's routine another 22. Nothing is written after the last
voice's routine; the tick's remaining cycles (return paths) do not matter.

### 11.3 Voice routine

All numbers are the same in A and B unless two are given.

| step | cycles | write at the end |
|---|---|---|
| call (22) | | |
| head: locate the record, test `active` | 113 | |
| inactive: return | 16; the routine ends | |
| period computation and write | 100 | AUDxPER |
| volume computation, hook and write | 12 + 62 + 212 = 286 | AUDxVOL |
| return from the hook | 44 | |
| first phase test | 25 | |
| phase 2: DMA-on | 62, then 10 | DMACON on (at the end of the 62) |
| phase not 2: second phase test | 25 | |
| phase 1: loop pointer | 104 | AUDxLC |
| phase 1: loop length | 66 | AUDxLEN |
| step counter decrement and test | 25 | |
| (no wrap: continue at "no event", below) | | |
| wrap: toggle, reload, row decrement and test | 103 | |
| (`rowCount` non-zero: continue at "no event") | | |
| `rowCount` zero: event: call 18, event (11.4), return 16; the routine ends | | |
| no event: test `note` | 21 | |
| `note` negative: return | 16; the routine ends | |
| `note` >= 0: call 18, effects (11.7), return 16; the routine ends | | |

### 11.4 Event (all of 6)

| step | cycles |
|---|---|
| entry: clear `slideTrigger`, get byte, compare with 0xFB | 73 / 59 |
| b is not 0xFB: DMA-off write | 38 (write at the end), then 10 |
| b is 0xFB: read next byte | 8 |
| compare with 0xFF | 17 |
| b is 0xFF: call 18, next pattern (11.5), reload stream byte 24 | |
| compare with 0xFE | 17 |
| b is 0xFE: end | 32 (the event ends) |
| compare with 0xFD | 17 |
| b is 0xFD (first byte): slide body 164, then 11.6 "finish" |  |
| clear `baseVol`, compare with 0xFC | 33 |
| 0xFC: read volume and next byte, compare with 0xFD | 51 |
| after 0xFC, b is 0xFD: slide body 164, finish |  |
| after 0xFC, b is not 0xFD: compare with 0xFB | 19; if b >= 0xFB: back up 8, finish |
| compare with 0xF0 | 17 |
| b >= 0xF0: rest | 62, then 36 (row count copy, return); no note start |
| compare with 0xC0 | 17 |
| b >= 0xC0: instrument select, read next, compare with 0xFD | 75; if 0xFD: slide body 164, finish; else compare with 0xC0 (17), if >= 0xC0 back up 8, finish |
| compare with 0x80 | 17 |
| b >= 0x80: length, read next, compare with 0xFD | 47; if 0xFD: slide body, finish; else compare with 0x80 (17), if >= 0x80 back up 8, finish |
| compare with 0x60 | 17 |
| b >= 0x60: arpeggio select, read next, compare with 0x60 | 55; if >= 0x60 back up 8, finish |
| note byte: add transposition and store | 34 |
| finish: store stream position, call note start | 34, then note start (11.6) |
| after note start: copy row length, return | 36 |

### 11.5 Next pattern (includes its own return)

| step | cycles |
|---|---|
| repeat counter decrement and test | 25 |
| repeat pending: reload pattern 22, stream pointer fetch and return 54 | |
| else: read list byte, compare with 0xFF | 37 |
| e is 0xFF: 4 + 80 (to the notification, 11.9) + 64 (rest of the hook) | then 42 |
| compare with 0x80 | 17; if e >= 0x80: 28 |
| compare with 0x70 | 17; if e >= 0x70: 28 |
| clear repeat count, compare with 0x60 | 33; if e >= 0x60: 28 |
| store pattern number and list position | 36 |
| stream pointer fetch and return | 54 |

### 11.6 Note start (including its return)

| step | cycles | write at the end |
|---|---|---|
| wave offset, AUDxLC | 152 | AUDxLC |
| AUDxLEN | 40 | AUDxLEN |
| test `baseVol` | 21 | |
| `baseVol` = 0: take the instrument's volume | 30 | |
| volume stores, period lookup, phase, instrument byte 6 test | 157 / 159 | |
| mode: negative 38 + 16 = 54; zero 9; positive 9 + 16 + 16 = 41 | | |
| test flag bit 7 | 27 | |
| bit 7 set: first CIA write | 40 | CIA 0x00 |
| second CIA write | 40 | CIA 0x02 |
| return | 16 | |

### 11.7 Effects (the call 18 and the return 16 are in 11.3)

In order; each line is added if its condition holds.

| step | cycles | condition |
|---|---|---|
| test `phase` = 1 | 27 | always |
| first-tick set-up | 43 | `phase` = 1 |
| vibrato set-up / no vibrato | 152 / 16 | `phase` = 1, instrument byte 1 non-zero / zero |
| flags copy and arpeggio test | 49 | `phase` = 1 |
| arpeggio set-up | 54 | `phase` = 1 and arpeggio flag |
| row count load, fade test | 37 | always |
| slide trigger test | 21 | always |
| vibrato test | 21 | not slide |
| vibrato counter | 25 | vibrato active |
| half-period boundary | 70 | vibrato active and counter went negative |
| vibrato step | 114, then 15 (negative step) or 9 (positive), then 24 | vibrato active |
| slide step | 212, then 9 | slide |
| slide, no wrap | 35 (+10 if arrived) | slide, subtraction did not wrap |
| slide, wrap | 35 | slide, subtraction wrapped |
| slide arrival | 138 | slide arrived |
| slide end | 10 | slide |
| arpeggio test | 25 | always |
| arpeggio read | 41 | arpeggio flag |
| arpeggio jump | 70 | x was 0xFF |
| arpeggio end test | 17 | arpeggio flag |
| arpeggio ended / step | 30 / 82 | x was 0xFE / otherwise |
| attack test | 25 | always |
| B only: COLOR01 write | 16 (write at the end) | attack |
| attack: call 18, rate lookup and subtract | 18 + 67 | attack |
| attack wrapped: return 16, test 9, end of attack 36, then continue with the decay test | | attack and wrapped |
| attack not wrapped: store 12, return 16, test, negate and add 21, call 18, volume 110, return 16, branch 10, final return 16; effects end | | attack and not wrapped |
| decay test | 21 | not attack, or attack just ended |
| decay: call 18, rate lookup and subtract 67 | | decay active |
| decay wrapped: return 16, test 9, clear 32 | | decay active and wrapped |
| decay not wrapped: store 12, return 16, test 9, call 18, volume 110, return 16, branch 10 | | decay active and not wrapped |
| final return | 16 | always (except attack not wrapped, which has its own) |

### 11.8 Plain tick

For orientation: a tick on which nothing but the plain sustain runs (no phase change, no step
wrap, effects with nothing to do): voice 1's AUDxPER at colour clock 0, AUDxVOL at +143, voice 2's
AUDxPER at +444, its AUDxVOL +587, voice 3's AUDxPER +888, AUDxVOL +1031 (relative to the first
write of the tick).

### 11.9 Song-end hook and re-based clock

The song-end hook costs 4 + 80 cycles up to the notification and 64 after it. At the notification
the host re-bases its clock: from then on stamp = floor(T_n/2) + floor((T - T_n)/2), where T_n is T
at the notification (after the 4 + 80). Only a tick containing a loop entry is affected and by at
most one colour clock.

## 12. Start phase

Let I be the colour clock of the first line of the log (the first AUD0DAT write). Then `H_0` = I +
72 592 and `H_k` = `H_0` + 70 940 * k, for all modules (the first AUDxPER write of tick 0 is at T =
493, colour clock I + 72 838). The absolute value of I depends on the host's start-up and on the
module (the entry routine's copying time).

## 13. What the real modules do and do not exercise

The four real modules use: every prefix and event type except the arpeggio select, the rest
(0xF0..0xFA), the slide (0xFD) and the voice end (0xFE); ties, volume override, instrument banks,
transpositions, repeats, attack envelopes (version A), plain sustain. Vibrato, slide, arpeggio,
decay, rests, voice end, the filter pulse and the version B colour write only occur in the
synthetic modules (LOGS.md), where they all agree with the reference.
