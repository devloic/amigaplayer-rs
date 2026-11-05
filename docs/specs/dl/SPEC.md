# Dave Lowe (`dl.*`, original driver): when the reference starts, and how its ticks are spaced

> **Sanitised for publication.** This copy was reviewed before it was added to the repository. Removed or rewritten: instruction-level wording of the volume-store search windows (kept as the 4-byte window constants), the reference to another spec file, and the description of the module init routine's structure. Numbers, formulas and behaviours are unchanged. References to `logs/` files, scripts and traces name data that is not shipped (see `docs/specs/README.md`).

Scope: `src/replay/davelowe.rs`, the `dl` flavour (UNCLEART hunk files, 45 in the corpus, plus the 2 "DAVELOWE" files `dl.older_*`).
The `New` layouts (dln A/B) are done and are not covered. Everything below is a behaviour description with formulas in
observable quantities (module-file fields, colour clocks "cc", 68000 CPU cycles "cyc"; 1 cc = 2 cyc). It contains no code, no
addresses of the original and no tables copied from it. All numbers come from the reference's execution traces and write logs
(commands in `LOGS.md`; the `logs/` data files it names are not shipped). The spec team examined the original to learn *which* module quantities to
count; none of it is reproduced here.

Status of the evidence: the start-time model (sections 2 to 6) reproduces the measured start of all 45 UNCLEART files **exactly
(45 of 45, 0 cc error)**, and a model refit without 12 held-out files predicts those 12 exactly (section 8). The in-tick part
(section 9) is a measurement-based description with an invariant part and a per-file part; it is **not** a complete model (section 10).

## 1. What the reference does between "start" and the first tick

The engine starts the Eagleplayer at a frame-aligned clock `A` (clock = 100 mod 355255; A = 8526220 in the engine's own
log clock, i.e. `100` in the refcmp logs, whose origin is `A - 100`). The Eagleplayer then runs three pieces of work back to back
(each starts at the clock the previous one returned), and the module's own driver code runs inside the last two:

1. a **check** call: 126 cc, always.
2. the **player-init** call (loads/prepares the module, copies its header, counts the subsongs, finds and patches the module's
   volume-write code so that the Eagleplayer sees every register write, then calls the module's optional sample fix-up).
   Duration 2186 to 32786 cc depending on the module (section 4).
3. four small calls: 12, 18, 118, 118 cc, always.
4. the **song-init** call (clears its own state block, scans the order lists of subsong 1 to find the longest, then the module's own
   init routine runs: the 21-write Paula reset burst, a state-clearing loop, per-voice state set-up). Duration 1827 to 15642 cc (section 5).

`T` = the clock at which call 4 returns minus `A`. **The first tick call starts exactly 70940 cc after that return, for all 47
files, with no exception** (`logs/measure_T.tsv`). Ticks are then exactly 70940 cc apart for as long as measured (149 ticks, every file).
So the file-dependent start offset is `T`, and nothing else delays the first tick: the "delay from the end of init to the first tick
varies by about +-150 cc" seen in earlier analysis is not a timer effect; it is the tail of the module's init *after* the Paula
reset (section 6), which differs by build. The measured reset-to-first-tick distance is 72972 to 74354 cc (table in section 6).

Measured range of `T`: 3193 (IK+) to 38776 (Frontier II), 45 files; mean about 21 000. The port's single constant
(`DL_INIT_CLOCKS` 21093) is right only for Line of Fire (T = 21193): the port-minus-reference clock difference at the first tick
of a file is `21193 - T(file)` to within the in-tick noise (checked on 5 files: California Games 2: expected -207, seen -209;
Count Duckula: expected +313, seen +311; Midwinter 2: expected -3367, seen -3369; Altered Beast: expected +4946, seen +4975;
Line of Fire: 0, seen 0..-5). **So the per-file start constant is `T(file) - 100`** (the 100 is the same phase offset that makes
Line of Fire's 21093 correspond to its T of 21193).

## 2. The model of T

All durations are CPU cycles of the reference's 68000 (the engine's own cycle table) converted to cc by the rounding of
section 3. The cycles of the two big calls are:

    c_player_init =  1191
                   + 76  x steps
                   + 72  x subs
                   + 20  x bra
                   - 25  x zterm
                   - 2   x [subs = 0]
                   + 395 x midwinter
                   + (X + 20) x [initplayer present]

    c_song_init   =  1605
                   + 29  x L
                   + 16  x U
                   + 37  x timescanner
                   - 290 x [no subsong table]
                   + M

with the module quantities of section 4, the per-module sample fix-up cost `X` (section 5.2) and the per-build module-init cost `M`
(section 5.1). The sum of everything is

    T = 392 + (ceil(c_player_init / 2) - 1) + floor(c_song_init / 2)       (cc)

where 392 = the check call (126) + the four small calls (12 + 18 + 118 + 118) and the two rounding forms are exact, not
approximate (section 3). Checked on all 45 UNCLEART files: 45 of 45 exact (`logs/model.py`; per-file components in
`logs/T_components.tsv`).

## 3. Rounding (why `ceil(c/2) - 1` for one call and `floor(c/2)` for the others)

The reference engine converts CPU cycles to cc per call: a call that contains no host (OS-library/Eagleplayer-host) function
call advances the clock by `floor(cycles / 2)`; a call that does contain host function calls advances it by `ceil(cycles / 2) - 1`.
Exactly one call of the start-up work contains host calls (the player-init call; it contains three: a library call, the module
patcher and a Deli host function). This rule reproduced the duration of all 906 calls inspected (the start-up chains and the first
ticks of all 47 files) with no exception. A port that works in half-cc can simply add `c/2` and subtract 1 cc once; the whole
spread of the rounding is at most 1 cc.

## 4. The module quantities (read from the module file; `dl.*` file offset = hunk offset + 0x20)

Hunk offsets in the module header used below (play routine at hunk offset 16, subsong
table at 44, sample-info table start/end at 28/32, init-player routine at 40, special-effect number at 76, load size at 60).

* `steps` (scan length). The player searches the module's own play routine for the place where it writes the volume register:
  starting **380 bytes after the start of the play routine**, it looks at every even byte position in turn (a 4-byte window) until
  the window equals one of two 4-byte patterns: 0x35400008 (the 16-bit volume store into the channel register block at offset 8,
  value taken from the first data register; "form 1") or 0x35590008 (the same store taking its value from a post-incremented
  word pointer; "form 2"; the second form never hit in the corpus). `steps` is the
  number of windows looked at, the matching one included: 10 (IK+) to 838 (Frontier II). Cost: 76 cyc per window.
  The 380 and the two forms were found by matching `steps` read from the trace with the module bytes for all 45 files.
* `bra` = 1 if the first form matched and the instruction right after the matched store is an unconditional branch with a 16-bit
  displacement (the player then rewrites that branch's displacement, 20 cyc). 0 for Beneath a Steel Sky (2), Frontier II, ISS, IK+.
* `subs` (the subsong count the player computes): 0 if there is no subsong table (table offset 0: IK+); otherwise 1 plus the number
  of following 16-byte table entries whose first long is at least 256 (stopping at the first entry whose first long is below 256).
  Cost: 72 cyc per counted subsong. 1 to 30 in the corpus.
* `zterm` = 1 if the table scan stopped on an entry whose first long is **0**, 0 if it stopped on a non-zero value below 256
  (a zero stopper is 25 cyc cheaper). 
* `L`, `U`: for the **first table entry (subsong 1)**, take its four order-list pointers (voices 0..3). For each voice count the
  non-zero 32-bit words before the first zero word, add 1 (the terminator): `n_v`. `L` = sum of the four `n_v` (cost 29 cyc each).
  `U` = the number of voices (in order 0..3) whose `n_v` is **greater than or equal to** the largest `n` so far (starts at 0, so
  voice 0 always counts); cost 16 cyc each.
* `midwinter` = 1 for the module whose load-size field (hunk offset 60) is 77554 with 17 subsongs (Midwinter 2): the player
  applies a one-off table fix-up costing 395 cyc. 0 otherwise.
* `timescanner` = 1 if the special-effect number (hunk offset 76) is 38 (Time Scanner): +37 cyc.
* `initplayer present` = the header's init-player routine offset (hunk offset 40) is non-zero (Beneath a Steel Sky intro and
  game_complete, F-15 Strike Eagle II, Raffles).

Values for every file are in `logs/T_components.tsv`. The definitions were validated by reading the same quantities from the
traces (loop counts) and from the module bytes: they agree on all 45 files.

## 5. The two per-module costs that the header does not predict

### 5.1 `M`: the module's own init routine (cycles)

The module's init has a fixed set-up part (including the 21-write reset burst, measured below) followed by a state-clearing pass over a block
of longwords. Its cost is `M = S + 22 x N` cyc (ISS: 30 per iteration; IK+ has several loops): `N` iterations
(205 in most builds), `S` the straight-line part. `S` and `N` are properties of the *build of the driver* inside the hunk, not of the
song. Measured classes (the 45 files):

| M (cyc) | S | N | files |
|---|---|---|---|
| 5664 | 1154 | 205 | Altered Beast, Bangkok Knights, Betrayal title, Darius, Galaxy Force 2, Games Summer Edition (archery, bars, closing, cycling, diving, hammer, hurdles, opening, village), Garfield, Ghostbusters 2 (ingame, jingle, title), Power Drift, Turbo Out Run |
| 5400 | 1154 | 193 | After Burner, Time Scanner |
| 5420 | 1174 | 193 | Raffles, Soldier of Light |
| 5648 | 1138 | 205 | Games Summer Edition pole, rings |
| 5876 | 1366 | 205 | Betrayal ingame |
| 6009 | 1499 | 205 | Street Fighter 2 |
| 6081 | 1571 | 205 | Beneath a Steel Sky intro, game_complete |
| 6095 | 1585 | 205 | Black Hornet, California Games 2, Count Duckula 2, ESWAT, F-15, Midwinter 2, Night Shift, Railroad Tycoon, Summer Camp, Summer Games |
| 6113 | 1603 | 205 | Line of Fire |
| 3934 | 920 | 137 | Frontier II |
| 4213 | 1221 | 136 | MicroProse Formula One GP |
| 2340 | 300 | (3 x 25, 6, 7) | IK+ |
| 2236 | 376 | 62 (30 cyc/iter) | ISS |

No header-only rule that predicts the class was found (the obvious distances `init - stop-offset` = 200 and `play - init` do not
separate all classes: Games pole/rings differ from the other Games files by 1 to 3 instructions; Betrayal ingame from Betrayal
title). **Implementation advice:** key `M` on the same row key the port already uses for the per-build flags (`ROWS`: hunk longs,
init, play, end offsets), measured per row from the table; default 5664 for a module not in the table (33 of 45 files have the
default flags). The class also fixes the pre-reset prefix of the module init (section 6).

### 5.2 `X`: the sample fix-up routine of the four "init-player" modules (cycles, added with 20 cyc of call overhead)

The module-supplied routine walks the sample records: measured 2853 (Beneath game_complete), 3356 (Beneath intro), 2984
(F-15) and 866 (Raffles) cyc. For the two Beneath files the routine is the same code and the difference is the number of records
processed: 23 iterations (game_complete) and 28 (intro), 3 of each of which take a longer "special" path; the extra 5 ordinary
records cost 503 cyc, about 100 cyc each (not an integer per record, so the per-record cost depends on the record). F-15's loop has
49 iterations of about 60 cyc, Raffles' 13 of about 42 cyc plus a straight-line part. Only these four values are known; they are
module constants, not predicted from the record count.

## 6. When the 21-write Paula reset burst happens

The burst (DMACON 000f, LOC/LEN/VOL/PER of the four channels, 21 writes, `dff096` first) is the first thing the module's init
does. Clock of its first write, relative to `A`:

    burst = 392 + (ceil(c_player_init/2) - 1) + floor(P/2) + p

where `P` = `c_song_init - M` (the Eagleplayer-side cycles of the song-init call, i.e. the part that depends on `L`, `U`) and
`p` the module's pre-reset prefix in cc: **20 for the classes with S = 1154..1174 and 1138 (5400, 5420, 5648, 5664), 71 for 6081,
73 for 6009 and 6113, 74 for 6095, 5876 and 4213**. Frontier II, ISS, IK+ and the two `older_*` files have **no reset in the init**
(their first writes are in the first tick, or five volume-zero writes in the init of the older ones). The measured burst clocks are
579..31200 (`first_write_rel_A` in `logs/measure_T.tsv`).

The module's init finishes `T - burst` cc after the burst: 2032 to 2983 cc for the UNCLEART modules with a burst (most common
2812: the 205-iteration clearing loop is 2255 cc of that); so the first tick starts `70940 + T - burst` = 72972 to 73923
after the burst (74353 for the older files). That is the "reset to first tick" distance. It depends only on the build class.

## 7. Two flows that differ

### 7.1 The `older_*` (DAVELOWE) files
These use a different, smaller Eagleplayer. The same three-step structure exists but: the chain of back-to-back calls starts **two
whole aligned frames later than the others** (at 9236730 in engine clock = `A + 710510`: the check call is at the normal `A`, a
41-cc call one frame later, and the init chain a frame after that); the chain is 113 + 123 + 18 + 118 + 118 + 3593 cc long:
`T = 4083` (`older_Frontier`) and 4082 (`older_Formula One GP`), constant in the two files (both run a 272-iteration state-clearing
loop). The first tick again starts exactly 70940 cc after the chain. The init contains five volume-zero writes (no reset burst), at
669 cc after the chain start. Only these two files exist; the 1-cc difference is the rounding of section 3.
Whether the port must reproduce the 710510-cc absolute shift depends on how refcmp aligns the two logs (not investigated).

### 7.2 Subsong n > 1
Selecting subsong n makes the reference run the song-init n times in a row (subsong 1, then 2, ..., n), each preceded by a
300 cc (380 cc for the 6095 class) stop call and the two 118-cc calls, and the first tick follows the last one by 70940. Each
song-init `k` costs `c_song_init` with `L`, `U` summed over table entries 1..k (the `U` counter restarts for each entry) plus **254
cyc for each entry beyond the first**. Checked on 4 files x 2 or 3 subsongs: 8 runs, exact in cycles for every `k`.
This only matters if the port is compared at subsong 2 or more; default subsong 1 is the corpus comparison.

## 8. Validation

| claim | files | result |
|---|---|---|
| `T` formula incl. rounding | 45 UNCLEART | 45 of 45 exact (0 cc) |
| refit of the coefficients of the data-dependent part on 26 files, predicting 12 held-out files (`logs/heldout_names.json`) | 12 | 0 cycles error on all 12 |
| first tick = chain end + 70940 | 47 | exact for all |
| tick spacing 70940 | 47 x 148 intervals | exact |
| call-level rounding rule | 906 calls | exact for all |
| `P` (Eagleplayer part of song-init) = 1605 + 29L + 16U (+37, -290) | 45 | exact in cycles |
| subsong chain | 4 files, 8 runs | exact in cycles |
| port start error = `21193 - T` | 5 files compared | within 2 cc on 4 files, 29 cc on Altered Beast (in-tick noise) |

The held-out run excludes the files that are the only instance of a term (midwinter, timescanner, no table, initplayer): those
four coefficients (395, 37, -290, 20) rest on one file each (Midwinter 2; Time Scanner; IK+; the four init-player modules share
the 20). `M` and `X` are measured constants per class/module, not predicted.

## 9. In-tick write order and costs (partial)

A tick is one call of the Eagleplayer's interrupt wrapper, which runs the module's play routine. The play routine handles the four voices
in order; the Eagleplayer's patches (installed in the init) intercept every register write the module makes and add their own cost. Observed in the
reference logs of all 47 files:

* **Write order per voice** (voice i, after voice i-1's volume write): idle voice: `VOL_i` only. Note stop: `DMACON off (1<<i)` then
  `VOL_i`. Note start: `[PER_i]`, `LOC_i` (high, low), `LEN_i`, `DMACON on (0x8000|1<<i)`, `VOL_i`. (`PER_i` is only written when
  the period changes.) Same order in all builds.
* **Cost of the Eagleplayer's intercept** is the same in every build (the same code): between the register writes of one event, the gap
  medians over the 45 UNCLEART files are `PER_i -> LOC_i` 184 or 185 (i=0), 200 (i=1), 215 or 216 (i=2), 219 or 220 (i=3);
  `LEN_i -> DMACON on` 82 or 83, 98, 113 or 114, 117 or 118; `LOC_i -> LEN_i` 36, 44, 51.5, 53.5 (the longword is written as two
  words; the gap is from the low word). In a path measurement of one build (Altered Beast, trace of jumps with clocks) the volume
  write goes through the intercept in 165, 189, 208 and 225 cc for voices 0..3 (call to return), 86, 95, 98, 112 of them before the
  register write: the cost grows with the voice index (the intercept identifies the channel by comparison).
* **Everything else is per build**: the gap `VOL_i -> next write` is the module code between two volume writes plus the intercept.
  Idle ticks (exactly four volume writes) have, per file, a stable shape: start offset `s0` from the tick call's entry to the first
  write, three gaps `g01 g12 g23`, a tail from the last write to the return. Examples (cc): Altered Beast-type builds 519/427..431/511..515/533/221,
  Line-of-Fire-type 560/475/494/523/292, Black Hornet-type 593/508/527/556/292, California-type 562/477/496/525/292, Beneath a Steel Sky
  551/460/479/508/286, Frontier II 718/490/508/538/285, IK+ 438/408/430/469/216. All 38 files that have idle ticks are in `logs/idle.tsv`;
  other transitions (`DOFF -> VOL`, `DMACON on -> VOL`, `LEN -> VOL`, `VOL -> PER/LOC`) in `logs/event_gaps.tsv` (median over the ticks
  of the file, 71 transition columns).
* The port's existing gap table (`DL_GAPS`, fitted on Line of Fire only) matches Line of Fire's ticks (single gaps within 36 cc over
  100 ticks, first write within 5 cc) and is off by up to 100 to 500 cc per gap on most other files (`logs/intick.py` run on the
  port/reference logs: maximum single-gap difference 36 to 512 cc over 100 ticks, depending on the file).
* Tick sizes: an idle tick is 1961 to 2601 cc (call duration), a tick with a note start about 400 cc more.
* The write **sequence** differs between port and reference in 0 to 34 of the first 100 ticks per file (a functional, not a timing,
  difference; `logs/intick.py`); not analysed here.

## 10. Limits

* `M` (per build) and `X` (four modules) are measured, not derived from the header. A module outside the corpus needs its class.
* The 380-byte scan start and the 76 / 72 / 29 / 16 cyc costs are per-item costs measured on the corpus; terms that are the only
  instance of a feature (see section 8) are unvalidated by held-out data.
* Section 9 is not a model of the in-tick costs: idle-slot gaps also vary within a file (for example `g12` of 511 vs 540 vs 545 in
  the same build, tick to tick) with the voice's state; that dependence was not worked out. The event gaps listed as invariant are
  medians; a few ticks per file deviate (see min/max in `logs/gaps_all.json`).
* Older files: two files, one flow; absolute alignment not investigated.
