# Dave Lowe: Eagleplayer-side behaviour (round 3)

> **Sanitised for publication.** This copy was reviewed before it was added to the repository. Removed or rewritten: 68k mnemonics and register-level descriptions of the patched sites and stub effects (the module search keys are kept as hex words, the cycle costs as numbers), an absolute address inside one module's data, and the scratch paths in LOGS.md. Numbers, formulas and behaviours are unchanged. References to `logs/` files, scripts and traces name data that is not shipped (see `docs/specs/README.md`).

Scope: the part of the original player that is NOT the module's own 68000
routines. Round 3 target: with `DELI_DL_CPU=1` (module routines run on the
repo's 68k core), reproduce the Paula write log in sequence AND in time.
Validated result (4 s, 45 UNCLEART files): 100% sequence and 100% time
(56399 / 56399 minus the two older_* files, which are another, smaller
player and not covered). 30 s: 100% as well, including song end. All numbers
come from the logs listed in LOGS.md. Method: behaviour only; no original
code is reproduced here. Opcode words below are search keys for module
shapes (they describe the module's bytes, which the implementer already has).

Units: cc = colour clock = 2 CPU cycles. Costs below are CPU cycles ("cyc")
using the repo's static table (conditional branch 8/9 regardless of taken, absolute call 20,
relative call 18, return 16, no-op 4). Event time = epoch + floor(cycles/2).

## 0. Core idea (what to build)

The Eagleplayer does not emulate Paula stores in a table; it PATCHES THE
MODULE'S OWN CODE. Wherever the module stores to a Paula register
(volume, period, location, length, and the "list" bookkeeping around
channel restart), the EP overwrites the instruction with
an absolute subroutine call into its own stub (opcode word 4EB9 + 32-bit address), padded with no-op words to the original
length. The stub performs the store itself (through its own scaling code),
maintains shadow state, and costs time. Every excess cycle in the baseline
log comes from these stubs, not from timers.

Recommended design: patch the module image in guest memory exactly as
described in section 1, then treat each such call to a stub address as a host
trap (a reserved address window) that applies the stub's effect on
registers/Paula/EP variables and charges the stub's cycle cost for the
(rule, channel class, path). The 68k core then reproduces garbage-flow cases
(e.g. rule 2 below) by itself. Cycle numbers in 3. are TOTALS charged for
the replaced site: call (20) + trailing no-ops (4 each) + stub body + return (16),
i.e. what the site costs instead of the native instruction(s).

## 1. Patching: where and what

Range: from `play` (module header word at +16) up to the subsong table
(header +44), or `play + 900` bytes when the table pointer is 0.
Rules are applied one rule at a time, in order R0..R12, each scanning the
range word by word (even addresses). On a hit the site is replaced by
a call to stub k, the remaining words of the match become no-ops, and the scan
resumes after the match. Matches of earlier rules are not re-matched by
later ones. (Observed per-file site lists: logs/sites/*.txt, format
`site <rule> <module offset> <words>`.)

Module shapes (search keys, big-endian words; `-` = any word):

| Rule | Shape (words) | Meaning in module | Patched? executed in corpus |
|---|---|---|---|
| R0 | 9068 003e 6400 0008 3039 - - 3540 0008 6000 | 10-word volume special | never executed |
| R1 | 3540 0008 2149 0012 | volume store, then the list-pointer store that follows | many builds ("form 1") |
| R2 | 3540 0008 6000 | volume store followed by a long branch | only Time Scanner |
| R3 | 3142 0006 3542 0006 | period store (shadow copy + hardware) | many |
| R4 | 3140 0006 3540 0006 3159 0002 | period store plus the post-incremented word read that follows | many |
| R5 | 24a8 0026 3568 002a 0004 | location long + length word | many |
| R6 | 3540 0008 3559 0008 2149 0012 | two consecutive volume stores (the second from the post-incremented pointer), then the list-pointer store | only SF2 |
| R7 | 249b 3553 0004 | location and length stored through the post-incremented pointer | never executed |
| R8 | 3559 0008 2149 0012 | volume store from the post-incremented pointer, then the list-pointer store ("form 2") | most builds |
| R9 | 3179 - - 0042 (4 words, absolute long source) | word store of an absolute-address source into the voice block at offset 66 | 21 builds |
| R10 | 2168 001e 001a 2268 001a 58a8 001a | list restart + counter chain | all |
| R11 | 58a8 001a 2251 | list pointer advance | all |
| R12 | 3159 0002 5368 0002 | list word read, negative = restart | all |

R1/R2 are alternatives (R1 first). R6 is tested before R8 so SF2's pair is
consumed as one site (R6 is the only rule whose stub does two volume stores).
"Form 1" modules = those whose sites are R1 (+R2/R6 variants); "form 2" =
R8. Detection is purely by which shapes occur in the range; no header flag.
R9's match requires the move source to be an absolute long address (words
2..3 arbitrary) and the destination to be the 66-offset field.

Before patching, the EP "init-player bra fix-up" runs: scan from play+380
for the first occurrence of `3540 0008` or `3559 0008`. If it is `3540 0008`
and the word at +4 is `6000` (a long branch), add 0x6002 to the word at +6, i.e. the
long branch's 16-bit displacement word is altered so that the branch becomes
a short one (the patched R2 stub sits where a volume store used to
be). Only Time Scanner takes this branch; without the fix-up it falls into
garbage and the log is wrong; with it, it is exact.

Not patched (corpus-wide): everything outside the range, the older_* files.

## 2. Volume store stub (R1, R2, R6, R8)

Written value = ((v & 0x7f) * Mv & mask) >> 6 with the default master
Mv = 0x40, mask = 0xffff, so effectively `v & 0x7f` (this fixes the After
Burner value mismatch at write index 40: the module writes a value with
bit 7+ set; the unmasked value was the repo's bug). After the stub d0 holds
the scaled value as a full long (upper word zero); the original d0 high word
is not preserved. Non-Paula a2 (Time Scanner uses a pseudo-channel block
inside its own data as the channel for volume): the Paula store is skipped, and d0 =
(d0 & 0xffff0000) | (d0 & 0x7f).
The EP's own volume bookkeeping stores to unrelocated absolute low
addresses: it has NO effect on module state, so ignore it. R1/R2/R6 stubs
also perform the module's store of the list pointer (the a1 value) into the voice block at offset 18. R8's stub reads the word at (a1), a1 += 2, scales it, stores it.

Channel classes by a2: ch0..3 = dff0a0/b0/c0/d0, "ch4" = any other a2.
Cost depends on class because the stub walks a per-channel shadow update;
ch4 costs like ch3 in shadow routines but skips the Paula store.

## 3. Cycle tables (total charged for the site, replaces native cost)

Order: ch0 / ch1 / ch2 / ch3 / ch4.

| Rule | Patched total | Native (unpatched) |
|---|---|---|
| R1 | 354 / 402 / 440 / 475 / 361 | 28 |
| R2 | 350 / 398 / 436 / 471 / 357 | 22 |
| R3 | 189 / 220 / 251 / 259 / 259 | 24 |
| R4 | 185 / 216 / 247 / 255 / 255 | 40 |
| R5 | 346 / 408 / 470 / 486 / 486 | 44 |
| R6 | 726 / 822 / 898 / 968 / 740 | 44 |
| R8 | 386 / 434 / 472 / 507 / 393 | 32 |
| R9 | 56 (all) | 24 |
| R11 | 153 on match, 99 otherwise | 36 |

Time of the Paula store inside the site (measured from the site's first
cycle, including the call, 20):

| Rule | ch0 / ch1 / ch2 / ch3 | native |
|---|---|---|
| R1 | 193 / 210 / 217 / 244 | 12 |
| R3, R4 (period) | 44 | 24 |
| R5 loc | 72 | 24 |
| R5 len | 197 / 228 / 259 / 267 | 44 |
| R6 vol1 | 193 / 210 / 217 / 244 | 12 |
| R6 vol2 | 529 / 594 / 639 / 701 | 28 |
| R8 | 213 / 230 / 237 / 264 | 16 |

(R2 follows R1's stub: same store time, total 4 less; ch4 has no store.)

"+32 on ch0 period writes in 21 builds" = R9: a module shape
(a word store of an immediate or absolute source into the voice block at offset 66) that sits on the ch0 path right before the
period write. Native 24 cyc becomes 56 cyc: +32. Criterion from module bytes:
the 4-word pattern `3179 xxxx xxxx 0042` occurs in the patch range; if it
occurs, every execution costs +32. (It is not channel specific in itself;
it appears only on the ch0 path in these builds, hence the observation.)
Present in 21 of the 45 builds; absent otherwise. Count per file in
logs/sites/.

### 3.1 Restart chain (R10 and R12-negative)

The restart of a channel's list (when the list ends) executes the module's
list-restart code plus a counter chain in the stub. Totals incl. the call and the
4 trailing no-ops, rows ch0..ch4, columns:
[counter was zero, counter nonzero, end-of-chain with effect==0,
end with effect!=0 and sub==1, end with effect!=0 and sub!=1].

| ch | zero | nonzero | fx==0 | fx!=0,sub==1 | fx!=0,sub!=1 |
|---|---|---|---|---|---|
| 0 | 241 | 263 | 392 | 461 | 417 |
| 1 | 268 | 294 | 423 | 492 | 448 |
| 2 | 291 | 317 | 446 | 515 | 471 |
| 3 | 314 | 330 | 459 | 528 | 484 |
| 4 | 293 | 293 | 422 | 491 | 447 |

"End of chain" columns apply on the call where the four counters are all zero
after the update (song end, section 5). Other calls use the first two
columns by the counter's pre-update value. R12 (list word read): positive
word costs 85 (stub 61 + call/return); negative word runs the restart chain at
(R10 value - 1). R11 charges 153 when a2 equals the "longest-voice marker"
(sec 5), 99 otherwise.

## 4. Start-up and tick glue

Tick spacing 70940 cc. Tick wrapper: 256 cyc before the play routine runs.
Start-up chain (first write time), spec dl unchanged: T = 392 +
(ceil(c_player_init/2) - 1) + floor(c_song_init/2); `FIXUP_EXTRA = 0` is
validated (Raffles, F-15, Beneath all exact with 0). The "tick-1 front extra
(~+97 cc)" and the "recurring +27..+58 cc front extras" are NOT separate
constants. They are exactly the sum of the site costs inside the tick before
the first Paula write: restart chains (R10: +173..+246 cyc over native),
R9 (+32), and the first period/volume stubs. Recurring +27 cc (+54 cyc) =
R11 matching the longest-voice marker (Power Drift ticks 81 and 161, After
Burner ticks 65/129/193 are exactly the ticks in which the marker voice
advances its list). No tick-count logic is needed; reproduce R11's match
condition and the extras fall out.

The master DMACON write is the module's own immediate store (no EP
adjustment). Time Scanner's effect request is written into the module
variable (header +24 pointer, minus 6) BEFORE the module init, with a
song-init term of 37 cyc (subsong 1) / 21 (subsong >1).

## 5. Song end and subsongs

Four counter bytes (one per channel) start at 0xfe each (reset in the
song-init). Every restart (R10, or R12 with a negative word) on a known
channel (ch0..3) increments that channel's counter if nonzero; a zero
counter stays zero. When all four bytes are zero after the update: counters
:= 0xff x4, the host SongEnd callback runs (+16 cyc for its return), and the
replayer's song-end flag is set at the end of that tick. If the effect
header word (+76) is nonzero and the subsong is 1, the effect request is
rewritten into the module variable. ch4 restarts do not touch counters.
The "longest voice" marker used by R11 is recomputed at song-init over
subsong entry n only (the last voice with the maximum length).

Subsong switching (engine does N x change_subsong(+1) for `--subsong N`):
1. Stop call: the module's stop routine (header +20) runs with 37 cyc EP
   overhead, duration floor(cycles/2). With no stop routine (header +20 = 0)
   the default stop costs 121 cyc = 60 cc; its writes are DMACON 0x000f at
   53 cyc and volumes 0 on ch0..3 at 69/81/93/105 cyc.
2. Two fixed calls of 118 cc each (the VOLUME and BALANCE tag calls).
3. Song-init p = 1605 + 29*SL + 16*SU + 254*(n-1) + 21*ts cc (+16 more when
   n == 1), where SL and SU are sums over list entries 1..n of the entry
   length L and the number of restarts U for that entry (U restarts per
   entry), and ts is the Time Scanner term of section 4.
   Counters are reset to 0xfe, longest marker recomputed over entry n,
   the sub number written into the module variable first.
4. The module's own init runs; the first tick follows 70940 cc later.

## 6. Validation (details in LOGS.md)

| Test | Result |
|---|---|
| Baseline (repo davelowe_cpu.rs) 4 s | 56200 / 56399 sequence, 32665 time (58%) |
| Round 3 model 4 s, 45 files | 100% sequence, 100% time |
| Round 3 model 30 s, 45 files | 432811 writes seq/time exact incl. song end |
| `--subsong` 1,2,3 | exact except Betrayal ingame (1 write off by 1 cc), F-15 step 1, Midwinter 2 step 2 |
| older_* (2 files) | not modelled (different player) |
