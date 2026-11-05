# Mark II Sound-System: behaviour specification

*Sanitised for publication: behaviour only.*

Format: modules of the Mark II Sound-System (editor by Andreas Scholl and Darius Zendeh), played
by the Eagleplayer "MarkII" ("Mark II player module V1.2, Cachet Software, adapted by Eagleeye"),
internal player name `Mark_II`, file name prefixes `mk2.` and `mkii.` (the files in the archive
carry the suffix `.mk2`), slug `markii`.

This spec is written for a port that **runs the 68000 sound driver contained in each module on
the repo's 68000 core** and reproduces what the Eagleplayer adds around it, in the manner of the
Riff Raff and Benn Daglish SID ports. All numbers were checked against the reference 68k player
run through the host with DELI_WRITE_LOG. A private model built from this text reproduces every
register write (address, value, order and colour clock) of all 24 real modules known for 60 s,
and of six of them up to and past the song end (LOGS.md).

## 0. Summary

A Mark II module is a plain 68000 program image (no hunk header, no relocation): the driver's code
at offset 0, followed by a parameter block, the driver's tables, the song data and the samples.
The driver is position independent (every reference into the module is PC-relative); it writes
Paula directly. It has one entry point, offset 0, with a command in the low byte of D0.

The Eagleplayer:

1. checks the module (1.3) and identifies which of three driver builds it is by comparing fixed
   code words at fixed module offsets (1.4);
2. replaces five or six instructions of the driver with calls into itself (display bookkeeping,
   volume scaling, the song-end report): the **sites** (section 5);
3. calls the entry point once with the start command and then once per tick (section 4);
4. reports the song end when the driver's song position wraps (section 6).

A port does the same: it runs the driver's instructions on the 68000 core with the repo's cycle
table and treats every site as a host trap with the behaviour and cost of section 5. Notes,
envelopes, slides and sequencing are whatever the driver's own code does; section 8 describes
them for orientation only.

All values are big-endian. "Module offset" is an offset from the first byte of the module file.
Cycles are 68000 cycles of the cycle table; colour clocks (cc) are the host's time unit; one
colour clock is two cycles. D0..D7 and A0..A7 are the 68000's registers; they are named only where
the host and the driver exchange values, and where a site must leave a register as the replaced
instruction would.

## 1. File

### 1.1 Layout

- Offset 0: the driver code. All known modules begin with the bytes `48 E7 00 F0 41 FA`; the word
  at offset 6 is a displacement `X` such that **P = 6 + X** is the start of the driver's
  parameter block (P = 0x2BC, 0x364 or 0x358 for the three builds of 1.4).
- P + 0x00: the driver's live state (saved pointers, counters, flags) as the editor left it when the
  module was saved, followed by these fields that the driver reads:

| offset from P | size | meaning |
|---|---|---|
| 0x26 | 4 | offset of the record table, counted from the song table (P + 0x256) |
| 0x2A | 4 | offset of the sample data, counted from the song table |
| 0x2E | 2 | number of song rows `R` |
| 0x30 | 2 | record size in bytes `RS` (4 bytes per step: steps per row `S` = RS / 4) |
| 0x32 | 0x1C0 | sample table: 8-byte entries, long offset into the sample data at +0, length in words at +6 |
| 0x1F2 | 0x64 | period table: 50 words |
| 0x256 | ... | song table (8 bytes per row), then the record table, then the samples |

The state words are part of the file and are **not** reset by the start command. In particular
the driver's gate/row alternation flag (section 4.3) comes from the file: of the 24 known modules,
four (movement_oldskool, defcon1-contact, defcon1-promotiondemo, psychotic) were saved with it set,
so their first tick is a row tick; the others start with a gate tick. A port must load the file
unchanged and run it in place.

### 1.2 Placement

The module is run where the host loaded it. Sample pointers written to Paula are absolute
(module address + offsets), so they depend on that address. In the reference the module file is
at **0x0045E0** (the `# module` line of every log).

### 1.3 Detection (the player's check)

Scanning words of the file from offset 0, with one shared budget of 288 words for steps 2 and 3:

1. the word at offset 4 must be 0x41FA;
2. find the first word 0xE742, scanning from offset 0 (count the words examined including the hit:
   `nE`);
3. continue right after it to the first word 0x41FA (`nF` words including the hit); let `Q` be the
   offset just after that word;
4. from `Q`, examine at most 24 words for the word 0xD1FA (`nD` words including the hit); let `Q2`
   be the offset just after it;
5. from `Q2`, scan backwards (first examined word at `Q2 - 2`) at most 16 words for 0x41FA (`nB`
   words including the hit).

If a step does not find its word the file is rejected. (The player has a second branch for a
file without 0xD1FA in step 4 that accepts it when the word at `Q + 2` is 0x3770; no module takes
it, and such a file fails 1.4 anyway: treat it as rejected.)

**Check cost** (used in 4.1): `C_check = 87 + 31 * (nE + nF + nD) + 33 * nB` cycles. Known
modules: build A nE 184, nF 1, nD 9, nB 3 (6200 cycles); builds B and C nE 238, nF 1, nD 9, nB 3
(7874 cycles).

### 1.4 Driver build and sites

The player then compares fixed words at fixed module offsets, trying three tables in order; the
first table whose every entry matches gives the build. `data/markii_sites.bin` holds the three
tables: for each, a byte with its entry count, then per entry: the module offset (word), the
three code words expected there (three words), and a kind byte (1 LEN, 2 LOC, 3 LOCCMP, 4 PER,
5 VOL, 6 PERTAB, 7 END; section 5). Every entry of the matching table is a **site**: six bytes
(three words) of driver code that the port replaces by a trap.

| build | table | sites (module offset: kind) | known modules |
|---|---|---|---|
| A | 1 | 0x174 LEN, 0x186 LOC, 0x1D4 PER, 0x274 VOL, 0x0E4 END | quadlite-wabb, quadlite-mark ii is blue, mattgraymix, movement oldskool |
| B | 2 | 0x0FE PERTAB, 0x1E0 LEN, 0x1F4 LOCCMP, 0x258 PER, 0x304 VOL, 0x144 END | the 18 others (defcon1-*, theend, psychotic, hotel, pixeldots, ...) |
| C | 3 | 0x0FE PERTAB, 0x1E0 LEN, 0x1F2 LOC, 0x24C PER, 0x2F8 VOL, 0x144 END | atron, jtg |

A file that matches no table is not played (the player's InitPlayer fails): include the table
match in detection. Within a build the driver code of all known modules is the same (the four
build-A files differ only in two equivalent instruction encodings).

### 1.5 Metadata

- No title, author or comment: the title is the file name.
- One subsong (0). The player declares no subsong range; a request for another subsong plays
  subsong 0 (checked: identical log).
- Voices: 4.
- Duration: the song end is reported after `2 * R * S` ticks counted from the first tick
  (`2 * R * S - 1` when the first tick is a row tick, 1.1); that is `R * S / 25` seconds, for
  example quadlite-wabb 48 rows x 32 steps = 61.44 s.

## 2. Time model

As in the repo's other hosted-driver ports:

- Each host call starts a count of cycles at a start clock `T` (its epoch).
- Before an instruction executes, its table cycles (`m68k_rc::cycles::cycles` of the instruction
  decoded with the 68000 decode model) are added to the count; the clock is then
  `T + floor(count / 2)`, and a store made by that instruction happens at that clock. The half
  cycle stays in the count.
- A site adds its cycles to the same count; its store happens at the count reached at the listed
  offset (section 5).
- The call ends at `T + floor(count / 2)` with the final count.

The CPU is a 68000 core; each call starts with all registers 0 except those named, SR 0x2000 and
a private stack (the driver pushes and pops four address registers).

## 3. Loading

Nothing is copied or relocated. The player's own changes to the module are the sites (the trap
replaces the six bytes; a port need not write anything there, the driver never reads its own code
words). No hardware address fix-up, no sample pointer fill.

## 4. Start-up and ticks

### 4.1 Timeline

With `S0` = the clock at which the host's detection starts (the reference's clock 100, the
engine's `START_PHASE`):

1. `I = S0 + floor(C_check / 2) + floor(C_init / 2)`, where `C_init` is the player's InitPlayer
   cost for the build: **A 1526, B 1966, C 2485 cycles**. (Known modules: I = 3963 for build A,
   5020 for B, 5279 for C.)
2. **Start call** at `I`: 32 cycles, then the entry point runs with D0 = 0xFFFFFFFF, D1 = 0,
   A0 = module address; the call ends when the driver returns. Its end is `E`.
3. **Ticks**: the first at `E + 70940`, then every 70940 cc (the default 50 Hz timer; the player sets
   no tempo).

Reference values: quadlite-wabb E 7563, movement_oldskool 7551, atron 9107, defcon1-contact 8838,
theend 8821, psychotic 8829 (the first tick 70940 later).

### 4.2 One tick

At the tick clock a call starts at count 0: 264 cycles, then the entry point runs with D0 = 0,
D1 = 1, A0 = module address; after it returns, 168 cycles. The bus clock is left at the end.

### 4.3 What the driver does on these calls (for checking)

- Start (D0 low byte negative): it turns DMA of the four voices off, resets its song position,
  voice order and fade state, writes the first step of row 0 on the four voices (through the
  sites), and turns DMA on (DMACON 0x820F). It writes no volume clear.
- Tick (D0 low byte 0): ticks alternate between a gate tick and a row tick by a flag in the
  driver's state (1.1). A gate tick writes DMACON with the mask of voices to retrigger collected
  by the previous row (bit 15 clear: those voices stop), and in builds B and C then updates the
  period of every voice with an active slide (PERTAB site). A row tick advances one step (and the
  song row when the record's steps are used up), writes the four voices (length, location,
  period, volume, through the sites), then DMACON 0x820F.
- A command byte 1..0x7F (fade with step D1) exists in the driver; the player sends it only when
  it stops (with D1 = 1, then writes volume 0 on the four voices and DMACON 0x000F). It is not
  part of playback.

## 5. Sites

When execution reaches the first word of a site, the port performs the site's behaviour, adds
its cycles to the count, and continues at the site offset + 6. "Voice" in the cost tables is
chosen by the voice register base the site works on: 0xDFF0A0, 0xDFF0B0, 0xDFF0C0 give voices 0,
1, 2; **any other value** (0xDFF0D0, and also the bad base of section 6) costs as voice 3.

| kind | behaviour (registers as the driver passes them) |
|---|---|
| LEN | the word at address A0 + D2 (low word, sign-extended) + 6 is written to A3 + 4 (the voice's length register); no register changes |
| LOC (A, C) | A0 = A0 + D2 (32-bit); the long A0 is written to A3 (location high and low, one long store: two log lines with the same clock); the low word of D2 = the low word of D1 |
| LOCCMP (B) | the long A0 is written to A3; then the condition codes are set as by comparing the low byte of D1 with 0x32 (the driver branches on that result right after the site) |
| PER | the low word of D1 is written to A3 + 6 (period); D2 = 0 |
| VOL | `v = (low word of D2 & 0x7F) * F >> 6` is written to A3 + 8 (volume); A1 = A1 + 2. `F` is the player's per-voice volume, 64 for all voices (the host never changes it), so v = D2 & 0x7F |
| PERTAB (B, C) | A3 points at a voice's period register. The low word of D1 becomes the word at A1 + 2 x (low word of D1) (index sign-extended; the upper word of D1 kept); that word is written to A3. The cost's voice is chosen by A3 - 6 |
| END | the song end is reported (section 6); then the word at module offset `site + 6 + K` is cleared, where `K` is the second code word of the site's entry (the displacement of the replaced instruction), and A0 is left pointing at that word |

After the sites other than LOCCMP the driver does not test the condition codes. Costs (cycles
from reaching the site):

| kind | store at | total, voice 0 / 1 / 2 / 3 |
|---|---|---|
| LEN | 76 | 169 / 192 / 215 / 205 |
| LOC | 58 | 147 / 170 / 193 / 183 |
| LOCCMP | 50 | 143 / 166 / 189 / 179 |
| PER | 50 | 135 / 158 / 181 / 171 |
| VOL | 231 / 254 / 277 / 267 | 311 / 334 / 357 / 347 |
| PERTAB | 80 | 169 / 192 / 215 / 205 |
| END | report at 52 | 116 |

## 6. Song end

The END site sits where the driver, having counted `R` song rows, resets its song position to
row 0 and clears its own status word. The song then continues from row 0 without a pause (the
music loops); the report is the only song-end event. The reference's offline render stops at the
end of the 0.1 s chunk that holds the report, so each full-length log holds the rest of that
chunk.

**The cleared word is not the driver's status word.** The replaced instruction clears the word at
`site + 2 + K`; the player clears the word 4 bytes further on, `site + 6 + K`, which in all three
builds is the second entry of the driver's table of voice register offsets (the entry that the
driver adds to 0xDFF000 for one of the four voice slots; which voice uses that slot depends on the
voice-order swaps of section 8). From the row written in the same tick as the report (row 0
again), that voice's writes go to
0xDFF000 + 0, 4, 6, 8 (location to 0xDFF000/0xDFF002, length to 0xDFF004, period to 0xDFF006,
volume to 0xDFF008), not to Paula, so the voice keeps playing its last sample at its last period
and volume. In all six full logs this is voice 2. A port reproduces this by doing what the END
site says; the writes to those addresses appear in the write log and must be made (they are
harmless on the bus of this host), and the comparison of the full logs includes them.

Report clocks in the reference: quadlite-wabb 217935559 (61.44 s), movement_oldskool 108900767
(30.70 s), atron 510777423 (144.01 s), defcon1-contact 177004454 (49.90 s), theend 735515057
(207.37 s), psychotic 994233245 (280.31 s).

## 7. Volume, balance, voices

The player installs its volume and balance functions only when its InitPlayer succeeds; the
reference host never calls them (no such call in any trace), so the per-voice factor `F` of the
VOL site stays at its initial 64. Should a host apply them: `F` for voice v would be the host's voice volume
(0..64) and the VOL site writes `(D2 & 0x7F) * F >> 6`. Not exercised in any log.

## 8. What the driver does (information only)

For orientation; the port runs the code. A song row holds, for each of the four voices, a record
number byte and a note byte. A record has `S` steps of 4 bytes: sample byte (bit 7: slide), base
note index, volume (bit 7: voice-order swap), flags (bit 0 set: tie, no retrigger; bit 7: slide or
detune direction; bits 1-5: slide amount). The note byte's low 5 bits are added to or (bit 7)
subtracted from the base index; bit 5 starts a volume ramp up and bit 6 a ramp down, each counting
one unit per row up to 64. The period is taken from the period table; with the slide bit the
previous period is moved by the record's amount instead (build B also treats base index 0x32 as
"keep the period"), and in builds B and C the gate tick moves sliding voices further. The volume is
the record volume minus the fade amount and the ramp (at least 1). The voice-order swap exchanges
the register offset and DMA bit of the voice with its neighbour, persistently. The main
differences from the older Darius Zendeh driver (repo spec `dariuszendeh`), which is played
data-only from tables located through code constants: Mark II modules carry their song length
and table offsets in a parameter block, report a song end, have the ramps, the slide flags,
the voice-order swap, the slide update on gate ticks (B, C) and the note sign bit, and use none
of the Darius Zendeh detection strings; the two players do not accept each other's modules.

## 9. Validation

A private model, written alongside this text and checked to follow it, runs the module's driver
on the repo's 68000 core (68000 decode model, standard cycle table), implements sections 1 to 6
and nothing else. Compared with the reference write logs: all 24 known modules for 60 s (33 001 to
33 025 writes each; two of them across their song end), and quadlite-wabb, movement_oldskool,
atron, defcon1-contact, theend and psychotic up to the end of the render after their song end
(16 961 to 154 241 writes): every write equal in address, value, order and colour clock;
`tools/replay-cmp.py` reports "no difference" for the full logs. The PERTAB site occurs in those
logs only in atron (build C, 48 times); the build-B PERTAB path was never reached in any log.
