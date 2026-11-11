# Dave Lowe (dl) behaviour spec, round 2

> **Sanitised for publication.** This copy was reviewed before it was added to the repository. Removed or rewritten: instruction-level wording of the two volume-store window forms (kept as the 4-byte window constants) and of an address-formation difference. Numbers, formulas and behaviours are unchanged. References to `logs/` files, scripts and traces name data that is not shipped (see `docs/specs/README.md`).

Scope: UADE DaveLowe Eagleplayer + the module's embedded 68k driver, as run by the repo's 68k engine.
Units: cc = colour clocks (clock ticks, 2 CPU cycles each). "cyc" = 68000 cycles from `m68k_rc::cycles`.
Clock model: at each instruction the bus clock is epoch + floor(cumulative cyc / 2). A call with no host
calls advances floor(cyc/2); a call with host calls advances ceil(cyc/2)-1. All gaps below are cyc/2 with
that cumulative flooring (the source of the +-1 residues).
Validation data: 45 UNCLEART corpus modules (+2 older DeliTracker files). Logs (not shipped) in logs/, commands in LOGS.md.

Main finding: the gaps are NOT per-build free parameters. Every in-tick gap is a sum of cycle costs of three
layers: (A) Eagleplayer intercept costs, universal across builds; (B) a driver "core" voice path, universal
in cycles, with state-dependent variants; (C) a per-build wrapper that is single-valued per build and per
slot and takes few values per code family. A port that models A+B exactly and takes C from a family table
(or from executing the module code) transfers to unseen builds; per-build gap tables do not.

## 1. In-tick gaps (item 1)

Tick = 70940 cc apart, exactly. Inside a tick the writes per voice v (0..3) are, in order when present:
PER, LOC hi, LOC lo, LEN, DMA-on, VOL, (DMA-off at stop). The gap between consecutive writes in the
whole tick is

  gap = ( EPpost(prev) + Σ module-path cycles + EPpre(next) ) / 2     [cumulative floor]

### 1a. Eagleplayer intercept costs (universal, cyc)
| Write | pre (before the store) | post (after the store, return path) |
|---|---|---|
| PER | 24 | 129 / 160 / 191-203 / 199 for voice 0..3 (+12 in some builds) |
| LOC hi | 52 | - |
| LOC lo | - | 125 / 156 / 187 / 195 |
| LEN | 125 / 156 / 187 / 195 | 141 / 172 / 203 / 211 |
| VOL, store form 1 | 193 / 210 / 217 / 244 | 169 / 200 / 231 / 239 |
| VOL, store form 2 (see s4) | 173 / 190 / 197 / 224 | 157 / 188 / 219 / 227 |
So form 2 is 32 cyc (16 cc) cheaper per volume write than form 1; 10 corpus builds use it.
A LEN written through the intercept (note start) costs +52/+68/+83/+87 cc over the direct loop-reload LEN
write (voice 0..3). Loop reload (retrigger mode 3) writes LOC/LEN directly: LOC lo -> LEN = 10 cc,
versus 62-97 cc through the intercept. The tick wrapper in the Eagleplayer costs 256 cyc before the driver.

### 1b. Core voice path (universal in cycles)
The per-voice driver path between landmarks has identical cost in every build (cyc):
gate+fetch-state test 23 / 27 / 27 (three steps), decay-or-sustain step 36 steady (62 or 46 in other states),
12, 23, 50 (99 or 122 on slide steps), 23, 74, 16, then 41 or 76 to the volume write depending on the
volume-write form. Baseline idle core per voice = 352 cyc. 60 distinct core paths over 14452 voice
segments; the top 3 paths cover 10653. State deltas (cyc, equal to the dln port's SLOT constants x2):
 +49  vibrato/portamento delay-count active   (24.5 cc)
 +72  rate-count active                        (36 cc)
 +9   boost-flag test
 +8   envelope held-terminator vs normal       (4 cc)
 +10  retrigger-mode test variant              (5 cc)
Of 47816 voice segments 6301 are non-modal; 5820 of those carry deltas shared by >=3 builds. The 70-400 cc
jumps are exactly these deltas plus the EP post/pre costs of whichever writes happen in that tick
(a note start adds PER+LOC+LEN+DMA intercepts, ~+300..+800 cyc); the 1-40 cc residues are (i) the
floor rounding (+-1), (ii) the 12-cyc PER variant, (iii) the 9/10-cyc state tests above.

### 1c. Per-build wrapper (C)
The module's non-core cycles per voice slot (W1, W2, W3 for voices 1..3 relative to the previous voice's
volume write; W0 = everything before voice 0's core incl. the 256-cyc EP wrapper; W4 = tail to tick end).
Single-valued per build and slot. Values:
- Family A (play routine located before init): modal (W1,W2,W3) = (103,233,219); variants (99,229,215),
  (95,225,211), (103,161,161), (103,227,219), Betrayal title (99,157,157). W4 = 443 or 431.
- Family B (init before play): the three slots are equal, 199 / 203 / 265 / 261 / 289 / 83. W4 = 571/583/607;
  Time Scanner 1273.
- W0 seen: 393 397 401 469 473 489 556..622 921.
Idle vectors (s0, g01, g12, g23, tail in cc): modal A = (519,427,511,533,221) shared by 12 builds;
B examples (562,477,496,525,292), (593,508,527,556,292), Frontier (718,490,508,538,285).
Table: logs/wrapper_table.json, logs/family.json. Predicting an unseen build's slot by family mode
(leave-one-out, 40 builds, 200 slots): 98 exact, median |err| 4 cyc, p90 66 cyc, max 830 cyc.
Unseen builds therefore get approximately-right idle spacing; fully general exactness needs the build's
own executed cycle sum (README "decision").

## 2. First write offset inside the first tick (item 2)
At the first tick every voice is in the fetch state (counter 0), so the first write is the ordinary
cycle ledger along the fetch path, not a special rule:
  offset = EP tick wrapper 256 + build prologue (W0) + voice gate and fetch preamble (~100 cyc)
           + Σ cost of the commands handled before the first note (data dependent, from the sequence;
           handler cost per command index is universal: index 1 = 135 cyc, index 3 = 62, ...)
           + EP pre-write cost (24 for PER, 0 for a direct VOL form).
Measured offsets (cc from tick start to first write): After Burner PER0 654, Bangkok Knights 664,
Betrayal ingame 693 / title 734, Black Hornet 761, California Games 2 730, Darius 646, Galaxy Force 2 650,
Line of Fire 706, Power Drift 648, Turbo Out Run 648, Altered Beast VOL0 590, Games Summer Edition
VOL0 590 or PER0 686, Frontier 1002, Summer Camp 954. Full table: logs/first_tick.txt (cols: W0, cycles
after core start, EP pre, steady idle s0). The earlier -203..-507 "offsets" are the difference between this
fetch path and the steady idle path.

## 3. DMA-on word: master bit 0x0200 (item 3), 45/45 verified
The DMA-on value is a 16-bit immediate in the build's driver, one per voice. Exactly one of
(0x8000 | 1<<v) and (0x8200 | 1<<v) occurs per voice in the driver's code area (before the sample-table
start, header offset 28). The logs agree in all builds. It is per voice and per build: 25 builds have
0x0200 on voice A; 27 builds write the master bit on at least one voice; Beneath a Steel Sky (both files)
has it on voice D only (0x8208) so a single per-build flag is wrong; older DeliTracker files write the
0x8001 style. No header field predicts it. Paula needs DMAF_MASTER in the accumulated DMACON for DMA to
run, so builds writing 0x8001 on every voice rely on the master having been set earlier. Rule: read the
immediate. Evidence: logs/master_rule.txt.

## 4. Second volume-write scan form (item 4)
The Eagleplayer finds the volume store in the module by scanning windows for either of two store forms:
form 1 = the 16-bit volume store with window word 0x35400008 and form 2 = the same store taking its value from a
post-incremented word pointer, window word 0x35590008. It stops at the first window equal to either. In all 45 corpus files form 1
occurs first; form 2 occurs only later or in the 10 form-2 builds (where form 1 is absent).
Effect on `steps` (player-init cycle count, cyc): matching window costs 29 (form 1) or 58 (form 2); after a
form-1 hit the branch-check costs 16+9 (+20 if a long branch follows) and the branch rewrite is done; for
form 2 the branch rewrite is skipped. So for the same position: c_player_init(form2) = formula with bra
term 0 and +4. Synthetic tests: original 23599; form 2 at the same spot 23583 (= -20+4 -... measured -16);
form-1 hit removed so the scan moves on to a later hit 25707 (28 more windows x 76, bra = 0). If neither
form exists the scan runs past the module end (not observed). The intercept routine used afterwards is the
form-specific one of s1a.

## 5. older_* (DeliTracker player; module is data) (item 5)
Every tick each voice writes VOL then DMA-on. VOL->DON 54 cc constant; DON->END 70; tick 1769 (1682 idle).
START->VOL0 ~429/433 (Frontier) vs 406/409 (F1 GP); DON->VOL1/2/3: Frontier 345/353/356, F1 321/329/333.
Frontier-F1 difference = the data-driven fade step (~23 cc path skipped when fade level is 0x40).
LEN->VOL variants differ by +66 cc; LOCL->LEN 10; PER->LOCH 94; DON->PER 89/90; DON->LOCH 88.
The song-start fix of s8 also applies (older_Frontier lag 1 -> 0, 98.8% equal samples).

## 6. X and M class for builds outside the corpus (item 6)
M (song-init cycles) = S + 22*N, where N = the number of words in the state-clear loop (205 words) and
S depends on init setup code + reset-burst code (no header predictor; default 5664 for unseen builds).
State block: 1 flag word + 6 voice structs x 34 words (stride 0x44). Class 193: structs of 32 words
(envelope-header-1 builds: After Burner, Time Scanner, Raffles, Soldier of Light). Frontier (137) and
F1 GP (136): 4 structs of 34 words. X = module-specific table-fixup routine cost: it is code, so only
its cycle sum is a rule. Known: Beneath 3 fixup calls, plain record 103 cyc, special record 153 (3 per
file), intro vs game_complete differ by -12 (the two builds form an address in two ways costing 8 vs 12 cyc) explaining
503 vs 515; F-15 49 iterations x 60 + 44; Raffles straight part + 13 x 42. Measured X (866, 2984, 2853,
3356) = routine sum minus 32 cyc of non-module trace entries. Unseen-build default: X = 0 (no fixups) is
the only safe value; anything else needs the module code.

## 7. Subsong 2+ song-init runs (item 7), verified against real --subsong runs
`--subsong N` performs N times change_subsong(+1), each = stop call + 118 + 118 + song-init_j; first tick
70940 cc after the last. Chain: [stop][118][118][init_1..j].
 song-init_j cyc = M + 1605 + 29*ΣL + 16*ΣU + 37*ts - 290*notable + 254*(j-1)
with L and U summed over song entries 1..j (U restarts per entry, counting voices whose n >= running
max), ts = Time Scanner term. Stop-call duration is a per-class constant: 300 (classes 5664/5400/5420),
306 (5876 Betrayal ingame), 348 (6009 SF2), 380 (6095), 390 (6113 LoF), 60 (Frontier II).
Result: 61/64 runs exact; the 3 Time Scanner runs are -16 off, explained by ts = 37 on song-init 1 but 21
on inits >= 2 (one file; with it 64/64). refcmp itself never uses --subsong. logs/subsong_check.txt.

## 8. Output-frame phase (item 8)
Paula's output grid restarts at reset; song_starts() clears queued samples at the start-return clock.
Reference: S = R + T(file) (R = clock at reset). Port: S_port = origin + T - 100, i.e. 100 cc (1.35
samples) too early, giving the 1-2 frame tick-1 lag. Rule: clear at reset clock + T(file); keep first tick
and the writes in place (timer delay reduced by 100). Tested with the +100 adjustment on all 45 files:
best lag 1 or 2 -> 0 everywhere; equal-sample % jumps to 100 for Bangkok Knights, GB2 jingle, GB2 title,
Midwinter 2; others 41-92% (After Burner 85.8, LoF 51). Remaining mismatch is write spacing (s1):
Darius, SF2, Raffles. older_F1 unrelated (11%). logs/lag_adj_all.txt, lag_adj_older.txt.

## 9. Dave Lowe New (`dln`, layouts A and B): the tick as a cycle ledger

Scope: UADE's Dave Lowe New Eagleplayer (the module is data, the replayer is the player's) in the repo's
68k engine. Black box: the reference's register writes logged in half colour clocks, compared with the
port's; no player code read. Data: the 41 corpus files (24 layout A, 17 layout B) and their 87 further
subsongs.

Clock model. The first tick starts exactly 70940 cc after the song start (the return of the last start-up
call, or of a subsong change), every further tick 70940 cc later; a tick makes no host call except at the
song's end. A write therefore comes at tick start + floor(C / 2) cc, C = the 68000 cycles the tick has run.
C is a sum of the costs of the path pieces walked; the pieces and their costs (cycles, voices 0..3) are:

| piece | cost | layout B adds |
|---|---|---|
| tick entry up to voice 0 | 738 | +130 |
| voice reads its stream (before its commands) | 13 | |
| voice counts down, up to its slides | 242 | |
| last tick of a note without / with a sample (DMA-off write at the end of the piece) | 252 / 0, then 268 | |
| loop reload: to LOC 8, LOC to LEN 20, then 284 | | |
| layout B: DMACON 87E0 after every channel DMA write | 20 | |
| portamento: delay count 49; step 0; step reaching its target, down 30, up 20 | | |
| vibrato: delay count 49, rate count 72, step down 42, up 59, the step closing the cycle 75 | | |
| portamento and vibrato stepping in one tick | +113 | |
| after a slide's PER write | 262 / 293 / 324 / 332 | |
| to the VOL write: held value | 157 / 174 / 181 / 208 | |
| to the VOL write: envelope step | 139 / 156 / 163 / 190 | |
| VOL write to the next voice | 507 / 538 / 569 / - | +59 |
| command 1 (sample) 305, 3 (envelope) 168, 4 (portamento) 140, 5 (vibrato) 180, 6 108, 7 108, 12 116, 14 (jump) 128 | | command 1: -2 |
| command 2, next sequence | 228 (+54 for the first voice with the longest order list) | -2 |
| command 2 at the list's end (wrap) | 434 / 465 / 488 / 501; 412 / 439 / 462 / 485 when the voice's counter is already 0 | -2 |
| a rest carrying the list flag | 177 / 208 / 231 / 244 | |
| the wrap or flagged rest that ends the song | +104, see below | |
| note: PER write at once; with a vibrato running +16 | | |
| note: PER to LOC | 293 / 324 / 355 / 363 | |
| note without a loop pair: PER to DMA-on | 259 / 290 / 321 / 329 | |
| note: LOC to LEN 20, LEN to DMA-on | 294 / 356 / 418 / 434 | |
| note: DMA-on to VOL, held / step | 304 / 321 / 328 / 355; 286 / 303 / 310 / 337 | |
| rest: to the VOL 0 write 132, to the envelope's VOL write | 286 / 303 / 310 / 337 | |

The pieces are only determined in sums along paths (what a piece holds of the path before or after it is
a convention; the sums are measured). Solved exactly (whole cycles, no remainder) from 1,211,013 writes.
The voice dependence is the write routines' (+31, +62, +70 after a write; +17, +24, +51 before a VOL write).

Rules found on the way:
- A rest's silent envelope steps for eight ticks before it holds, in both layouts (a step is 18 cycles
  cheaper than the held value; the values written are 0 either way).
- The +54 of the next-sequence command for the first voice with the longest order list applies to layout A
  as to layout B.
- Song end: the player calls its host inside the piece, and the host call resumes at a whole colour clock, so
  an odd cycle is dropped there. After a wrap the ledger is odd behind the piece ((C - 1) | 1), after a
  flagged rest even (C & ~1) (one rest case in the corpus).
- A subsong change needs no model of its own: the song start moves to the change's return, the tick
  follows 70940 cc later.

Result: 41 files x 120 s (700,630 writes) and x 600 s (914,084), 128 runs (all subsongs) x 60 s (1,274,532
writes): every write at the reference's colour clock; the 18 refcmp rows IDENTICAL. Split-half check (20 s,
before the song-end rule): costs solved from every second file leave 0 of 116,605 writes of the other files
off, the other way round 10 of 111,320 (one file, a piece the half has only in a sum); 456 and 326 writes
contain a piece their half does not have.
Not in the corpus (costs assumed): a stopped voice, a direct sample (record type 0), commands 11 and 13, the
stop commands, a held value right after a rest's first write.

