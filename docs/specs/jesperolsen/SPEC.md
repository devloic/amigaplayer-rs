# Jesper Olsen replayer: write-timing behaviour spec

> **Sanitised for publication.** This copy was reviewed before it was added to the repository. Removed or rewritten: scratch paths and references to private material only; the spec text was already behaviour-only. Numbers, formulas and behaviours are unchanged. References to `logs/` files, scripts and traces name data that is not shipped (see `docs/specs/README.md`).

Scope: the cycle cost of every code path of the reference player ("JesperOlsen_EP", UADE
players dir), as a rule over player state, so that the port reproduces each register write's
colour-clock time inside a tick. This is a behaviour description derived from execution traces
and write logs. It contains no instruction sequences, labels or code addresses. The numbers are
measured costs (68000 cycles, 2 cycles = 1 colour clock "cc") of straight-line sections, named by
what the section does.

Status of the model described here (validated by a private model that follows this spec):

| engine | modules | result |
|---|---|---|
| B (data modules, the EP plays them itself) | 25 files, about 1.3 M writes, 90 s each | 0 writes off (tick-relative) |
| C-T (module with its own code, "title" family) | Guldkorn_Expressen jo.title | exact, apart from a constant start offset (section 7) |
| C-G (module with its own code, "ingame" family) | Guldkorn_Expressen jo.ingame, subsongs 0-13 | about 66 000 writes compared, 0 off |
| Not validated | Kalas_Puffs_Expressen jo.title (C-T should apply), Guldkorn jo.ingame subsong 14, G-path costs never exercised by the logs (see README) | |

"Validated" means the tick-relative time of every write (time minus time of the first write of
the tick) equals the reference. Tick-to-tick spacing and the start phase are the framework's
business and are not part of this spec (section 7).

## 1. Clock model (applies to all engines)

1. The reference emulator charges a fixed number of 68000 cycles per executed instruction
   (conditional branches 9 or 11 by taken/not-taken, DBcc 10, MULU 62, ...). Data-dependent
   cycles (multiply operand, shift count) are not charged. The section costs below are sums of
   these static costs, so they are constants, not functions of data values.
2. The player reaches the hardware through host calls ("stubs" that trap into the Eagleplayer
   host: DMA control, set location, set length, set period, set volume). Each host call
   re-bases the machine clock: after the trap the clock is whatever the host sets, not the sum of
   cycles. Consequently
   `write_time = clock_at_previous_rebase + floor(cycles_since_previous_rebase / 2)`
   where cycles_since_previous_rebase counts everything executed since the previous rebase,
   including the cost of the trapping instruction and the stub's set-up instructions before it.
   The floor is taken per segment between two rebases, NOT on a cumulative cycle count. This is
   the cause of the 1-2 clock errors of the old cumulative model.
3. After a rebase the code that follows the trap (the "post" cost of that call) counts toward the
   next segment.
4. Direct writes (the player storing to a custom register itself, without a host call) do not
   re-base; the segment simply continues. Which engine uses direct writes where is listed per
   engine.
5. Two 16-bit halves of a sample location written by one host call (high word then low word)
   are stamped at the same cc (gap 0 in the log). The length write is a separate call.
6. The clock jump of the host call itself (the stub's own cost, 2045-2270 cc for the closing
   DMA-on write) is not part of the per-tick write spacing: the closing DMA-on write is the last
   write of its tick and is the only call with that jump; the next tick starts after it.

Host call costs (cycles). "pre" is the cost from the previous rebase point (end of the previous
call's post cost) up to and including the trap instruction, only for the fixed stub part;
variable code before it is separate. "post" is what runs after the trap until return.

| host call | pre (stub part) | post |
|---|---|---|
| DMA control write (engine B) | 94 | 68 |
| location write | 84 | 60 |
| length write | 116 | 68 |
| volume write | 100 | 60 |
| period write | 92 | 60 |

Engine C uses the same stub costs but the stubs are called from patched module code with extra
call/save/restore overhead; those totals are given in the engine C sections.

## 2. Tick structure

A tick is one call of the player's play routine, driven by the host timer. Tick-relative time 0
is the first write of the tick. Within a tick the order of writes is:

1. Engine B: DMA-off write (value = the set of voices that retriggered or were silenced in the
   previous tick; may be 0, always written), then the four voice slots in order, then the
   closing DMA-on write (value = voices that were started in this tick).
2. Engine C: optional DMA-off write (only on the tick on which the tempo counter reads 1),
   the four voice slots in order, then the closing DMA-on write (always written, value may be
   0 plus the enable bit).

Between ticks the framework applies the real spacing (around 70 000-71 300 cc per tick,
jittering with a three-tick pattern); not part of this spec.

## 3. Engine B (data modules)

### 3.1 Tick skeleton

Costs in cycles, in time order inside a tick:

- tick entry up to the DMA-off write: 251, then the DMA-off host call (pre included in the 251;
  post 68);
- after the DMA-off call: a 72-cycle section;
- four voice slots (3.2);
- closing section 54, then the DMA-on call (pre 94, post 68, tick ends).

The pending DMA-off set is emptied by the first write and refilled during the tick; the pending
DMA-on set is written at the end and emptied.

### 3.2 Voice slot

State per voice: "active" (has a current track pointer), an 8-bit rate accumulator with step
size, a step counter (7 bits), a sound-state byte 'first' (set when a note was started and not
yet initialised), an effect/portamento set, three script slots (position, counter, reload).

- Slot head: 51. A voice without a current track pointer: head 51 + 10 = 61 and nothing else.
  (Same 61 for a slot whose track ended.)
- Rate accumulator: add the voice's rate byte to the accumulator byte; section 39. If the add
  carries: section 47 and decrement the step counter (low 7 bits); if the counter reaches 0 the
  slot runs "fetch" (3.3) instead of "effects" (3.4). If no carry, or counter not zero, run
  effects.

### 3.3 Fetch (next event of the voice)

All sections in cycles, in time order. Conditions are on stream data.

- entry 33.
- Track stream word equals the stop marker: 50, volume-0 write (pre 100, post 60), then 38, voice
  becomes inactive, then slot end 10. Fetch ends.
- Else 21 (compare). If the word is the jump marker: 40, track pointer follows the jump.
- Then 8 and 17 (loop set-up/test), then the track parameter words: each word consumed costs 53
  (36 body + 17 test). A parameter word is a 2-byte entry whose first byte has bit 7 set (it stores its second
  byte into the voice variable named by the low 7 bits); words are consumed until the first
  byte has bit 7 clear. Typical: 0-3 words.
- 88: phrase lookup (phrase pointer + saved position).
- Phrase item parameter words: 17 initial test, then 53 per word, same rule as above.
- 21: item type test. Then one of:
  - special item (first byte 0x7E or 0x7F): 61, then if 0x7F (voice stop): 58, pending DMA-off bit
    set for this voice; else (0x7E, wait/rest): 30.
  - note item: 21, then (if the note's second byte has bit 7 clear) the trigger sequence (3.3.1),
    then 60 (store note and instrument offset).
- 47: advance the saved position. If the next item is the end-of-phrase (word zero): 43; then
  decrement the repeat counter and if it underflows, 32 more (advance track pointer).
- 10 then slot end 10. (total tail 20)

#### 3.3.1 Trigger (new note)

- 216 (copy the instrument's three script descriptors into the voice, reset).
- 9. Then the instrument flag "no retrigger when playing" (flag bit 2): if set: 21; if the voice
  was already flagged: 10 and the trigger ends here (no DMA-off); else 26 and the flag is set.
  If flag clear: 16 and the flag is cleared.
- Pending DMA-off bit for this voice is set. 44, 9.
- If the instrument flag bit 0 is clear (immediate DMA stop): 42, then a DMA-control call with
  value = this voice's bit (pre 94, post 68), then 12. (This is a direct, non-pending write in
  the middle of the tick.)

### 3.4 Effects (voice already running)

- 27. If the voice's current-note byte marks note-off (value 0x7F): 10 and the slot ends (no
  writes, no pending DMA-on).
- Else the voice's DMA-on bit becomes pending; 127.
- Sample start (only if 'first' is set; this is the tick after the note was triggered):
  - 80, 'first' cleared; 9; if instrument flag bit 1: 16; 29.
  - IFF-style sample (bit 31 of the sample start): 62, location call (pre 84, post 60), 62,
    length call (pre 116, post 68), 22.
  - Raw sample: 53, if instrument flag bit 7 (wave-table selection): 74 more; then 34, location
    call, 30, length call, 22.
- Otherwise (not 'first', loop update every tick): 29, then
  - IFF: 90, length call, 50, location call, 22 (length first, then location, with the loop
    start offset).
  - Raw: 43, if wave-table flag: 94 more; 34, location call, 30, length call, 12.
  Note the order: loop update writes LEN before LOC for IFF, LOC before LEN for raw.
- Scripts: three script slots, identical cost per slot (3.4.1), run in order 1, 2, 3.
- 109 (slide accumulation and the volume product).
- If the voice's effect flag bit 0 is set: 4 more.
- 238 then the volume call (pre 100, post 60). The 238 includes the four division steps that
  derive the volume; it is a constant.
- 41. If the note byte has bit 7 clear (relative note): 12. Then 72 (period lookup, vibrato and
  slide arithmetic). Period call (pre 92, post 60). Then 12 and slot end 10.

Notes of index 96 and higher (up to 127) give period 64 * (index - 95) (the port currently
panics there).

#### 3.4.1 Script slot cost (each of 3 slots)

- 21 (slot head). If the slot's position pointer is zero: done.
- 33 (counter decrement and test). If the counter is still >= 0: done.
- 57 (counter expired). Look at the command word:
  - high byte 0x7E ("rate/stop" command): 21. If the low byte < 0x80: 20 (sets the reload
    rate). Else 21, and if the voice's stop flag (bit 7 of its sound-state byte) is set: 26,
    counter forced to 0, slot done. Otherwise (both non-stopping cases) 26 (pointer advance),
    then 20 (counter reloaded).
  - otherwise: 21; if the word is the jump marker: 50 (pointer follows the jump, value re-read)
    else 21, and if the word is the stop marker: 26, slot done. Then 54 (store the value into
    the voice variable and advance), then 20 (counter reloaded).

(Slots 2 and 3 have exactly the same costs as slot 1.)

### 3.5 Verified facts

- The three script slots, fetch, trigger, both loop-update branches and the stop path were all
  exercised in the 25 files; 0 writes differ.
- The old cumulative-floor rounding of the repo's model is wrong; per-segment floor is right.

## 4. Engine C common facts

Engine C = modules that contain their own player code; the EP patches the module with calls
into its stubs. Two families: T (title) and G (ingame). Both share the tick structure of 4.1;
costs are in sections 5 (T) and 6 (G). Write time convention in C: a write is stamped after
all the cycles listed before it in the segment since the last rebase (stub costs included for
host calls; section costs only for direct writes). Direct writes (marked DIRECT) do not
re-base; host-call writes (marked CALL) do.

### 4.1 Tick skeleton and slots

Per tick, in time order (cycles):
1. Entry: T 102 + 161; G 102 + 183.
2. Tempo counter (global, 8 bit). If it reads 1: 24, then a DIRECT DMA-off write whose value is
   the set of voices flagged for cutting during the previous tick, then 25. Otherwise just 25
   (T and G: the 24 is paid only when the counter reads 1; the 25 always).
3. The counter is decremented. If it was 0 (underflow) it is reloaded: T: 21, then if the phase
   flag is 0: 46, high nibble of the tempo byte, phase becomes 1; else: 36, low nibble, phase
   becomes 0. G: 33 + 46, low nibble only, no phase. A counter reading 0 after the decrement
   makes this a "step tick" (voices count down their durations, new events can start); on other
   ticks voices only process (modulate) their current note.
4. Four voice slots in order. Slot head: T voice 0: 74, voices 1-3: 42 each; G voice 0: 92,
   voices 1-3: 56 each.
5. After the slots: 20. Then the closing stub: 12 + 18 + 94 up to the DMA-on write (CALL; value
   0x8000 | bits of voices started this tick; written every tick).

Idle behaviour: when all four voices are inactive the reference keeps ticking (after "song
end"): each tick has only the closing DMA-on write (value 0x8000) and, on counter == 1 ticks,
a DMA-off write of value 0 first.

### 4.2 Voice routine (per slot)

- Inactive voice: T: 31 then 16. G: 18 + 11, then 16. Nothing is written.
- Active voice, common test: T: 31 + 31 (head test + active test). G: 18 + 11 + 31.
- Not a step tick: T 25, G 25; if the voice's length word high byte < 0x40 (the note is in
  gated mode): T 21 / G 25; and if its counter is at the cut point (T: 0, G: 1): T 16 / G 12 +
  9 + 16, and the voice is flagged for DMA-off at the next counter == 1 tick. Then 23 (rest
  test). A rest voice ends: 16. Otherwise the "process" path.
- Step tick: 27 (counter decrement). If the counter underflows (T: counter bit 15 set after
  the decrement; G: counter reached 0) the voice fetches its next event (5.2 / 6.2). Otherwise
  23 (rest test) and either end 16 (rest) or the process path (5.3 / 6.3).

## 5. Engine C family T (title)

### 5.1 Output stub (end of every note processing)

- 88 of code, then 4 + 18 + 92 up to the PERIOD write (CALL). Period value = table period for
  the voice's note + slide, minus vibrato.
- From that rebase: 60 + 4 + 12 + 62 + 14 + 62 + 14 + 18 + 100 up to the VOLUME write (CALL):
  346 cycles = 173 cc after the period write. Volume = ((instrument volume * voice volume)
  >> 4) * master >> 4, low byte.
- 60 + 16, then 40, then 16: voice routine done.

### 5.2 Fetch (next event) in T

- 47 entry. Track word = stop marker: 32, voice becomes inactive, no write; fetch ends.
- 21; if the word is the jump marker: 44, the track pointer follows the jump.
- Track option bytes, each "probe" always paid, "take" additionally when the option is present:
  repeat count (probe 25, take 28), transpose (21, 40), master volume (21, 28), tempo (21, 70),
  instrument map word (21, 24).
- 119: pattern lookup and first word fetch.
- Rest item (first word >= 0x7001): 36, DIRECT location write (silent sample), 16, DIRECT
  length write (value 1), 34. The voice becomes a rest voice. LOC->LEN gap is 8 cc (observed).
- Note item:
  - probe 21; instrument word present (>= 0x6000): 72.
  - 49. If the voice's length-word high byte < 0x40 (retrigger allowed): 28, then
    12 + 14 + 4 + 18 + 84 up to the LOCATION write (CALL), 60 + 12 + 18 + 116 up to the
    LENGTH write (CALL), then 68 + 12 + 16 and 64. LOC->LEN is 206 cycles = 103 cc.
  - Optional words, each probe 21 plus take: sample modifier (take 28), volume modifier (28),
    note modifier (28), length word >= 0x4001 (24), length word >= 0x3001 (24).
  - Flag clears: 16, 9, then 16 more if the voice's portamento-flag byte has value-2 bit clear;
    16, 9, then 16 more if its value-4 bit is clear.
  - Portamento word (>= 0x2000): probe 21, take 44. Flags word (>= 0x1000): probe 21, take 24.
  - Master option (byte 2): probe 21, take 28. Instrument volume option (byte 1): probe 21,
    take 28.
  - Note byte: 21; if note < 0x80 (relative): 12. Then 60.
- 107: common tail (counter masked, DMA-on bit pending).
- If the next pattern byte is the pattern end marker: 43; the repeat counter decrements and on
  underflow 42 more (the track advances).
- Then the output stub 5.1, except for rest items (no stub).

### 5.3 Process (voice playing between events)

- 47 entry.
- First processing after a note (age 0): 134 (list pointers loaded).
- Else: 21; if portamento is active (target byte bit 7 clear): 93; direction test 23 (target
  above) or 21 (below); if the target is reached: 86, the note becomes the target, portamento
  clears. If not reached or none: 56 + 9 (slide step), then:
  - flags-word value-2 bit clear: 16 + 9; and then if the voice's portamento-flag value-1 bit
    is clear: 16 + 9; then if release flag (flags word value-1) bit clear: 41; if the release
    condition holds (flag set, or age > release time): 45; release list: end probe 21, jump 44;
    value 49; list counter 42 when it wraps, else 20.
- Volume list: 25; counter fires: 91; end probe 21, jump 44; value 40 + 9; if the entry's hold
  bit is set: 37; if not holding: 16 (advance).
- Note list: 25; fires: 97; probe 21, jump 44; value 40.
- Sample list: 25; fires: 83; probe 21, jump 44; value: 34, DIRECT location write, 20, DIRECT
  length write, 16. LOC->LEN is 10 cc.
- 16, age + 1, then the output stub 5.1.

## 6. Engine C family G (ingame)

G differs from T in: the list processing only updates pending location/length, which the output
stub writes; and the output stub writes four registers.

### 6.1 Output stub (end of every note)

- 12 + 9 + 100 of code, then 4 + 18 + 92 up to the PERIOD write (CALL).
- 60 + 4 + 12 + 62 + 16, then 36, then 14 + 12 + 62 + 14 + 18 + 100 up to the VOLUME write
  (CALL): 410 cycles = 205 cc after the period write.
- 60 + 16, then 20 + 20, then 12 + 16 + 18 + 84 up to the LOCATION write (CALL): 246 cycles =
  123 cc after the volume write. Then 60 + 12 + 18 + 116 up to the LENGTH write (CALL): 103 cc
  after the location write. Then 68 + 12 + 16, then 8, then 16 (voice done).
- Observed in the reference: VOL after PER 205 cc, LOC after VOL 123 cc, LEN after LOC 103 cc
  (high and low halves of LOC at the same cc).

### 6.2 Fetch in G

- 47 entry. Track stop marker: 20, then 16 + 12 + 4 + 18 + 100 up to a VOLUME write of 0
  (CALL), then 60 + 12 + 8 + 16; the voice becomes inactive; fetch ends.
- 21; jump marker: 46.
- Track option bytes (probe, take): transpose-2 (25, 16), transpose-1 (21, 16), master (21, 16),
  tempo (21, 16).
- 111: pattern lookup.
- Rest item (word >= 0x7001): 82; the voice becomes a rest voice.
- Note item: 23; instrument word present (>= 0x6000): 16; 49. If the voice's length-word high
  byte < 0x40: 130 (instrument record loaded; location/length pending, no write). Optional
  words, probe 21 + take 16 each: length word >= 0x4001, >= 0x3001. Then 22; 9; if the
  portamento-flag value-1 bit is clear: 12; 10; 9; if the value-2 bit is clear: 24; portamento
  word probe 21, take 32; flags word probe 21, take 16; 9; instrument-volume option probe 21,
  take 16; note byte probe 21, relative transpose 24 if note < 0x80; 72.
- 32, 9, 16 (tail, DMA-on bit pending), 55; if the pattern end marker follows: 28; 23.
  - Rest voice: 20, then a LOC/LEN stub: 12 + 16 + 18 + 84 up to the LOCATION write (CALL;
    silent sample), 60 + 12 + 18 + 116 up to the LENGTH write (CALL, value 1), 68 + 12 + 16;
    then 8, 16. No period or volume write.
  - Note voice: 10, then the output stub 6.1.

### 6.3 Process in G

- 47 entry. Age 0: 126. Else 25; if the portamento target byte is non-zero: 11, direction test
  59 (same cost both ways); target reached: 74. Not reached or none: 56 + 9, then
  flags-word value-2 bit clear: 16 + 9; portamento-flag value-1 bit clear: 16 + 9;
  release-flag bit clear: 41; release condition: 45; end probe 21, jump 44; value 49; list
  counter 38 when it wraps, else 20.
- Volume list: 25; fires: 91; probe 21, jump 44; value 37; if the entry's bit 7 is set: 43 and,
  unless the voice's length word >= 0x4000: 21; advance: 16.
- Note list: 25; fires: 97; probe 21, jump 44; value 40.
- Sample list: 25; fires: 83; probe 21, jump 44; value 74 (location/length pending).
- 16, age + 1, then the output stub 6.1.

## 7. Open points (summary; see README)

- C-T start: the title first write comes 8408 cc after reset in the reference; the model's
  tick-relative times are exact, the absolute start phase differs by 18 cc in the model.
- Idle ticks after song end exist in the reference (4.1); the port currently stops ticking.
- Step-tick/counter-reload structure of T: the alternating high/low tempo nibble is read from
  the module; G uses the low nibble only.
