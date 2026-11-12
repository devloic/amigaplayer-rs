# Digital Sound Creations: behaviour specification

The sample sequencer of Andrew Bailey and David M. Hanlon's game music: Populous (`han.gmusic`,
`han.gwords`, The Final Frontier's `han.gwords`), Fusion (`han.Fusion`) and Enlightenment
(`han.Druid_music`, `han.Druid_game`). Not Digital Sonix & Chrome (`dsc.*`), the same authors'
other format.

**Provenance.** Drawn up from black-box observation only of the reference, the Wanted Team
Eagleplayer "Digital Sound Creations", run on this project's 68000 engine: the audio.device
requests it makes (the engine's OS trace), the times of its host calls (CheckIO, AbortIO, BeginIO,
the song-end call), the Paula register write log, and how all of these change when bytes of the
song, the kit or the bank are changed (edited copies, run again). The player's code was not
disassembled, decompiled or read, and no byte of it is used; the Wanted Team source archive was
not opened. Section numbers are referred to from `src/replay/digitalsoundcreations.rs`.

## 1. Files

* Populous: one file holding the song and its samples (section 2).
* DTRK: the song `han.<name>` (section 3.1), the kit whose name the song's header gives (3.2), and
  the bank `smp.<name>` (3.3), all in the song's directory. The reference loads both companions.

Both kinds share the grid: 16384 bytes, 64 patterns of 4 tracks of 64 rows, the byte of pattern
p, track k, row r at `p*256 + k*64 + r`. A byte c is a note of instrument `c - 0x41` (a byte
subtraction) when that is below 104; any other byte (space, `!`, 0x00, 0xA9, 0xF0 were probed)
is an empty row, with the same time as a space. Bytes above `Z` are used (Fusion: 0x84).

## 2. Populous layout

| offset | contents |
|---|---|
| 0 | 64 words: the pattern lengths in rows |
| 0x80 | word N, then N (pattern word, count word) sequence entries |
| | word 0, word 0x4000 (the grid's length) |
| | the grid (16384 bytes) |
| | word: entry count (104 in every file), then 8-byte entries: period word, length word (bytes), volume byte, sample byte, loop-offset word |
| | word: sample count S, then S 8-byte records: length word, loop word (0: the sample loops), an offset long |
| | long: data length, then the sample data |
| end | 2 more bytes (the file is exactly this long) |

N = 0 (`han.gwords`) plays pattern 0 once. The samples lie one after the other from the data's
start, by their length words; the offset longs are not used (in `han.gwords` they leave 6-byte
gaps that the reference does not follow). A request (section 4) of entry e: the sample's address,
e's length, e's period and volume. A sample number at or past S is not probed; the port takes
it as no sample (section 4), as DTRK's past its table.

Tick: 70940 colour clocks (50 Hz). A row every 10 ticks, the first on the tenth tick.

## 3. DTRK

### 3.1 The song

`FORM` length `DTRK`, then chunks (not padded): `THDR` (43 bytes in the corpus), `BODY` (the
grid, 16384), `SEQN` (3-byte entries: pattern word, count byte), `MLEN` (64 pattern-length words).
THDR starts with the kit's name, NUL-terminated. The tempo T is 9 bits: bit 8 is bit 7 of THDR
byte 37, bits 0-7 are THDR byte 40; the other header bytes probed (34 to 3E of the file) change
nothing. The tick is `5 * ((7499855 / (T + 30)) - 176)` colour clocks (integer division), exact for
every probed T from 128 to 511 (28 probes; the constant lies in [7499842, 7499868)); below T = 84
the true value passes 65535, and the reference's periods there are 1 to 4 more than this mod
65536 (not in the corpus). Fusion T = 432 (80285 cc), Druid_music 399 (86530), Druid_game 329
(103570). A row every 4 ticks, the first on the fourth.

### 3.2 The kit

`FORM` length `DKIT`, `KHDR` (4 bytes; its first byte, 0x68, is not the entry count: an edited
0x50 changes nothing), `BODY`: 44-byte entries: name (16), 16 bytes, rate long, length long
(bytes), volume byte, sample number byte (1-based into the bank; 0 is the first too), loop word.
Bytes after the BODY are junk. Entry 103 is ":No Sample" in the corpus kits (sample 0, length 0)
and is played as such when a row names it (a request of length 0).

The request's period is the 68000 `DIVU` of 3579545 by the rate's low word: rate 75536 plays as
10000 (period 357); a quotient above 65535 leaves the dividend's low word (rate 32: period
0x9E99). Rate 0 stops the reference with a division by zero. Every corpus pair (75) fits.

The loop word non-zero makes the note loop (section 4); the loop request's address and length are
junk in the reference (high words from the length), and never reach Paula.

### 3.3 The bank

The 8SVX FORMs in file order; sample i's data is its FORM's BODY data. In `smp.Druid_game` the
eighth FORM's length stops 4 bytes before the ninth: the reference's table then has an empty slot
there (sample number 9 plays nothing, 10 plays the ninth FORM). A sample number past the table
(16 or 20 of Fusion's 15): no sample. Sample 0xFF of Fusion plays a junk address.

## 4. Notes and requests

One audio.device request a track, on channel k (unit `1 << k`). A row's note on track k:

1. CheckIO. If the request is still held (playing, in the registers, or queued) and the note's
   volume is below the request's volume (the last note's): skipped, nothing more.
2. AbortIO (a held request is aborted; DMA off if it was playing).
3. If the instrument has no sample: nothing more.
4. CMD_WRITE: the sample's address and length, the period and volume, `ADIOF_PERVOL`, 1 cycle.
5. If the instrument loops (Populous: its sample's loop word 0; DTRK: its loop word non-zero): a
   second CMD_WRITE on the same request, from the loop offset, cycles 0. It queues behind the
   first and is never taken; its cycles 0, read when the pass starts, make the whole sample repeat
   with no end.

The engine's audio.device: a write to an idle channel puts the sample's pointer and length in the
registers and writes DMA off, DMA on at the call (the channel's interrupt request is dropped between
the two: an idle channel leaves one set, and the device must not take it for the pass's start). When the engine next looks at the channels and
Paula has raised the channel's interrupt (after each output sample, vertical blank and tick, with
Paula as far as it ran: it runs only at register writes and at those moments), the pass starts:
period and volume are written; a one-cycle request leaves the registers, and at the next
interrupt (the sample started over) DMA is switched off and the request replied.

## 5. Sequence

A global play counter c, starting at 0 and never reset. At a pattern's last row (after the row's
notes): c += 1; if c < the entry's count the pattern repeats; else the next entry, or after the
last the song ends (the reference's song-end call), and no row is played after. A pattern length
0 acts as one row. Probed with Fusion count bytes and Populous count words.

## 6. Timing

The first tick comes one tick period after the song start.

## 7. Time within a tick

Each tick is the reference's code between host calls, timed in 68000 cycles; the engine's clock
moves by `floor(cycles / 2)` colour clocks from one host call to the next. Fitted exactly over all
42,112 call-to-call gaps of the six songs (cycles):

| part | cycles |
|---|---|
| tick without a row, start to end | 444 |
| row tick start to track 0 | 681 |
| empty track 0, 1, 2, 3 (3: to the row's end) | 241, 241, 261, 184 |
| CheckIO of a skipped note to the next track | 568, 568, 588, 510 |
| CheckIO to AbortIO: request not held / held | 108 / 182 |
| AbortIO to CMD_WRITE | 1276, 1366, 1466, 1508 |
| CMD_WRITE to the next track | 499, 499, 518, 442 |
| first to second CMD_WRITE of a loop (tracks 0, 1 seen; 2, 3 extrapolated) | 1262, 1352, (1452, 1494) |
| second CMD_WRITE to the next track (tracks 2, 3 extrapolated) | 478, 478, (497, 421) |
| row's end: next entry / pattern repeats | 266 / 216 |
| song end: to the song-end call, then to the tick's end | 42, 390 |
| no sample: AbortIO to the next track, track 0 (other tracks assumed the same less) | `AbortIO to CMD_WRITE + CMD_WRITE to next - 127` |

## 8. Choices of the port (not observed)

A pattern number above 63 plays 64 empty rows; a row past 63 reads on in the grid (the next
track's rows), past the grid nothing; rate 0 takes the overflow period; a short kit or entry table
reads zeros; a bank gap of any size gives one empty slot.
