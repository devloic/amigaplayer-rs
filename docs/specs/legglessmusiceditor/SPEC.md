# Leggless Music Editor: behaviour spec

Scope: the Amiga music format of Steve "Leggless" Hasler (1990, used in games such as Punisher and
Ninja Spirit; file name prefix `lme.`) as played by the reference (the Eagleplayer "Leggless Music
Editor", whose replay routine is a four-voice Paula driver that runs once per 50 Hz tick). This is a
description of observed behaviour (write logs, module data) in my own words. It contains no
instructions or addresses of the original. "cc" is an Amiga colour clock (1 cc = 2 CPU clock cycles);
"cycle" is a CPU clock cycle of the host's static 68000 cost model (section 9).

A value model written from this description (with the timing tables of section 9) reproduces the
reference's complete audio-register write stream (register, value, order and colour-clock time) for 18
(module, subsong) runs of two real modules (about 25 200 ticks) and 16 runs of modified modules (about
9 600 ticks), with no differing write; see LOGS.md.

## 1. Overview

- Four voices, one per Paula channel (channel 0..3, registers at the usual 16-byte steps from AUD0LC).
- Each voice follows its own track list: a list of pattern numbers (each optionally with a
  transposition), each pattern a list of events. An event is a note (with a duration in ticks), a rest,
  or a note modified by flags: select instrument, start a pitch-table (arpeggio), slide (portamento),
  or "tie" (change pitch without restarting the sample).
- Instruments are either 8-bit samples (looped) or "synth" instruments: a small generated waveform of
  two levels whose switch point moves (pulse-width modulation), generated into a per-voice buffer.
- Per-instrument effects: attack/hold/release volume envelope, vibrato (a triangle LFO with delay),
  sample retrigger every N ticks.
- There is no tempo control: every tick is 50 Hz, and every event length is counted in ticks.

## 2. File format

All numbers are big-endian. The file is one block; every pointer in it is an offset.

### 2.1 Header

| offset | size | content |
|---|---|---|
| 0 | 4 | the bytes `L` `M` `E` and a zero byte |
| 4 | 32 | free text (copyright line, space padded; e.g. `(c)1990 Leggless/Silents UK`), not interpreted |
| 36 | 4 | zero (checked) |
| 40 | 4 | INS: offset of the instrument table, counted from file offset 40 |
| 44 | 4 | PATP: offset of the pattern pointer table, counted from 40 |
| 48 | 4 | PATD: offset of the pattern data, counted from 40 |
| 52 | 4 | SMP: offset of the sample area, counted from 40 |
| 56 | 16 per subsong | subsong table: four longs per subsong, the first word of each channel's track list, counted from 40 |

All offsets named "counted from 40" are added to 40 to give the file offset. The subsong table starts
at file offset 56 and ends where the instrument table begins (file offset 40 + INS), so

- number of subsongs = (INS - 16) / 16;
- number of instruments = (T - INS) / 58 (integer division), where T is the first long of the subsong
  table (the start of the first track list is also the end of the instrument table);
- number of patterns = (PATD - PATP) / 4.

Typical layout (game_over, 1 subsong, 6 instruments): header and subsong table (file offset 0..71),
instrument table (72..419), track lists, pattern pointer table, pattern data, then the sample area.

### 2.2 Instrument table

58 bytes per instrument, at file offset 40 + INS. Fields (word = 2 bytes, offsets in bytes from the
start of the entry):

| offset | size | meaning |
|---|---|---|
| 0 | long | sample start, counted from the sample area (file offset 40 + SMP). Not used for synth instruments |
| 4 | word | sample length in words |
| 6 | word | initial volume (the value the envelope starts from) |
| 8 | long | loop start, counted from the sample area |
| 12 | word | loop length in words |
| 14 | word | LFO value (vibrato offset; starts at 0 in practice) |
| 16 | word | LFO speed (step per tick); 0 = no vibrato |
| 18 | word | LFO direction (0 = rising, non-zero = falling) |
| 20 | word | LFO upper limit |
| 22 | word | LFO lower limit |
| 24 | word | LFO delay in ticks (negative = no delay) |
| 26 | word | envelope phase at note start (0 = none, 1 = attack) |
| 28 | word | attack step |
| 30 | word | attack target level (peak) |
| 32 | word | hold time in ticks |
| 34 | word | release step |
| 36 | word | release target level (end level) |
| 38 | word | synth flag: 0 = sample instrument, 1 = synth instrument |
| 40 | word | synth: current width (starts at 0 in practice) |
| 42 | word | synth: width growth step |
| 44 | word | synth: width maximum |
| 46 | word | synth: width shrink step |
| 48 | word | synth: width minimum |
| 50 | word | synth: level A (low byte used, written as signed 8-bit) |
| 52 | word | synth: level B (low byte used) |
| 54 | word | retrigger counter (starts the countdown) |
| 56 | word | retrigger period in ticks; 0 = no retrigger |

Real modules use the sample start 0 with a length of 2 words for "silent" loops: the first four bytes
of the sample area are silence, and instruments that must not make noise after their one-shot sample
loop there (loop start 0, loop length 2).

### 2.3 Subsong table and track lists

Each subsong has four longs (channels 0..3), each the offset (from 40) of that channel's track list.
A track list is a list of words, ended by 0xFFFF:

- a word without bit 15 set is a pattern number;
- a word with bit 15 set: the pattern number is the word with bit 15 cleared, and the next word is a
  signed transposition in semitone steps, added to every note of the pattern;
- 0xFFFF ends the list; the voice then restarts at the first entry (so a voice loops forever).

### 2.4 Patterns

The pattern pointer table has one long per pattern: the offset of the pattern's data counted from
file offset 40 + PATD. A pattern is a sequence of events. Each event starts with a flags word; a flags
word with bit 15 set ends the pattern (the next track-list entry is then fetched). The words that
follow depend on the flags, and are read in this fixed order:

1. If bit 0 is set: **rest**. One word follows (duration in ticks) and the event is over; all other
   flag bits are ignored.
2. If bit 1 is set: one word, an instrument number (select that instrument).
3. If bit 4 is set: **pitch table**: a word n (the table has n entries, n at least 2), a word with the
   speed (ticks per step), then n-1 note words (each a note number, transposition added later).
4. Bit 3 (tie, a flag only, no data).
5. If bit 2 is set: **slide**: three words: the destination note, the number of steps, the start delay
   in ticks.
6. A word: the duration of the note in ticks, then a word: the note number (index into the period
   table, section 2.5, plus the voice's transposition).

Bits 5..14 are unused.

### 2.5 Period table

37 words, indexed by note number plus transposition (valid indices 0..36):

0x358 0x328 0x2FA 0x2D0 0x2A6 0x280 0x25C 0x23A 0x21A 0x1FC 0x1E0 0x1C5 0x1AC 0x194 0x17D 0x168
0x153 0x140 0x12E 0x11D 0x10D 0x0FE 0x0F0 0x0E2 0x0D6 0x0CA 0x0BE 0x0B4 0x0AA 0x0A0 0x097 0x08F
0x087 0x07F 0x078 0x071 0x06B

Indices outside 0..36 read other data and are not specified (never seen in the modules).

### 2.6 Sample area and acceptance

The sample area starts at file offset 40 + SMP. Samples are signed 8-bit. The information the
Eagleplayer shows (and the check that rejects short files) is computed as follows:

- song size = 44 + SMP (the sample area plus its four silent bytes counts as the start of the samples);
- walk the instruments in table order; keep a running maximum, starting at 3; an instrument whose
  start (first long) is 0 is a synth instrument; otherwise, if its start is above the running maximum,
  it is a new sample: the running maximum becomes its start, its length (word at 4, times 2) is added
  to the total sample size; otherwise it shares an earlier sample and adds nothing;
- the file is rejected ("module too short") if song size + total sample size exceeds the file size.

### 2.7 Detection, subsongs, title, length

- Detection: the first four bytes are `L`,`M`,`E`,0 and the long at offset 36 is zero. (File name prefix
  `lme.`.) Then the size rule of 2.6 must hold.
- Subsong count: section 2.1; subsongs are numbered from 0.
- Title/author: none. The 32-byte text at offset 4 is shown as "special info" only.
- Length shown by the reference: the number of words between the first long and the second long of
  the subsong's four track pointers (that is, channel 1's track list including its end marker and
  transposition words) minus 1. Display only.
- Number of steps shown: number of patterns. Samples and synth samples: counts from 2.6.

## 3. Host interface

### 3.1 Tick

The host calls the play routine from a timer interrupt every 70 940 cc (50 Hz CIA timer). The start
phase of the first tick is the host's (it differs between runs; the reference's first tick entry was
at cc 72 862 for the first subsong of a run, and later for later subsongs because the host does
other work first). The time of tick k is T0 + 70 940 k. Everything in the tick is timed from the
interrupt entry (section 9).

### 3.2 Voice levels and masks

The volume value written to a Paula channel is passed through the host's level scaling:

    written = ((value AND 0x7F) * level) shifted right by 6

where `level` is (balance of that side * master volume) shifted right by 6: the left side serves
channels 0 and 3, the right side channels 1 and 2. In the reference run both levels are 64, so the
value written is simply value AND 0x7F. A per-voice mask (all on in the reference run) additionally
ANDs the value. Hence volumes above 127 wrap. (Voices that the user switches off write 0 directly
and are not modelled.)

### 3.3 Start of a subsong

The host starts subsong s by the player's init routine (section 4.2). For a subsong other than the
first one played in the run, the reference log first contains three groups of host writes before the
first tick: DMACON = 0x000F then AUD0VOL, AUD1VOL, AUD2VOL, AUD3VOL = 0 (gaps 8, 6, 6, 6 cc inside a
group), the groups about 1 270 cc apart (the first at cc 2 790). They are the host's audio-stop
sequence, not the player's. The first tick follows much later. The first subsong of a run has no such
groups.

### 3.4 Song end

The player reports the end of the song to the host through a callback (section 8). The reference
host stops three ticks after the tick in which the report was made (that tick and three more ticks
are in the log). The callback is a host call; its cost and clock effect are in section 9.5.

## 4. Replay state

### 4.1 Per voice (v = 0..3)

- pattern position: where the next event of the current pattern is read (initially "at an end marker",
  so the first tick fetches the first track-list entry);
- track position: the next track-list word; and the first track-list word's position (loop point);
- event countdown (word): ticks until the next event; initially 1;
- transposition (signed word, set by the track-list entry; 0 when it has none);
- instrument offset (58 times the number of the last selected instrument, initially instrument 0);
- the voice's copy of an instrument (45 words = 90 bytes copied from the instrument table at an
  instrument select; the fields used are those of section 2.2, at the same offsets);
- base period (word): the period of the current note, changed by slides;
- slide record: step, remaining count, destination period, delay (all words; step 0 = no slide
  active; step is cleared at init, the other three start at 0);
- pitch-table record: position (byte offset), length in bytes (2n), countdown, speed, and the current
  base period plus up to 9 further period entries; length 0 = no pitch table;
- loop-set flag with loop address and loop length (words) waiting to be written at the next tick;
- the tie flag (one flag, shared by the voices, set by an event with bit 3 and cleared when used).

All of this state is zero at the start of the run, except as stated. State of a previous subsong is not
cleared by starting another one (the reference process starts fresh for each subsong).

### 4.2 Init for subsong s

- The four pattern positions are set to "at an end marker".
- Track position and first-track position of voice v = the first word of voice v's track list in
  subsong s's table entry.
- The four event countdowns are set to 1.
- The step word of each slide record is set to 0.
- The song-end bookkeeping (section 8) is set to "no voice has wrapped yet".
- Nothing is written to the hardware.

## 5. Tick

Order of work in one tick (all writes in the order listed):

1. **Deferred loop addresses.** For each voice v in order 0..3 whose loop-set flag is set: clear the
   flag, write AUDvLC = loop address (a long, two word writes at the same cc), write AUDvLEN = loop
   length.
2. Set the DMA-enable mask to 0x8000.
3. **Slide stage** (5.1), voices 0..3.
4. **Vibrato stage** (5.2), voices 0..3.
5. **Pitch-table stage** (5.3), voices 0..3.
6. **Volume stage** (5.4), voices 0..3.
7. **Retrigger stage** (5.5), voices 0..3.
8. **Event stage** (6): for voice v = 0..3: decrement the event countdown (16 bit, wraps); if it is now
   zero, decode the voice's next event.
9. **Synth stage** (5.6), voices 0..3 (changes buffer contents only; no register writes).
10. **Raster wait** (9.4), then write DMACON = the mask (0x8000 plus the bit of every voice that was
    started or retriggered in this tick). This write is made every tick, also with nothing to enable.

### 5.1 Slide stage

For a voice with slide step 0: nothing. Otherwise:

- Delay: if the delay word is negative, the slide runs now. Otherwise the delay is decremented; if
  the new value is still non-negative the voice waits this tick; if it became negative (that is, it
  was zero) the slide runs now. Counting the tick after the event's tick as tick 1, a delay of n makes
  the first slide step happen in tick n+1.
- Run: new base period = base period + step (16-bit); write AUDvPER = new base period; remaining
  count -= 1; when the count reaches 0: write AUDvPER = the destination period, set the base period to
  the destination, and set step to 0 (the slide ends).
- When the event sets up a slide: step := (destination period - period of the note) divided by the
  count, signed, truncated toward zero; the note's period is written at the note start and the slide
  then moves it towards the destination (the destination is the note given in the slide data, plus
  transposition). A count of 0 is not valid (division by zero).

### 5.2 Vibrato stage

Voice with LFO speed 0: nothing. Otherwise the same delay rule as 5.1 applies to the LFO delay word
(negative: active; else decrement, still non-negative: wait; else active). When active:

- Direction 0 (rising): LFO += speed; if the new LFO equals the upper limit exactly, the direction
  becomes "falling".
- Direction non-zero (falling): LFO -= speed; if it equals the lower limit exactly, the direction
  becomes "rising".
  (Both comparisons are for equality of the 16-bit values. A speed that never hits the limit exactly
  runs through the 16-bit wrap.)
- If the voice has no pitch table (length 0): write AUDvPER = LFO + base period (16-bit).

Detail of the direction word: it is tested as a whole word (zero = rising); setting "falling" sets
bit 8, clearing it clears bit 8 only. A module that stores 1 in the direction word as the initial
value therefore never returns to "rising".

The LFO value itself is the instrument's word 14 copy and is also added to the pitch-table outputs.

### 5.3 Pitch-table stage

Every tick, for every voice, the pitch-table record's "current base period" is set to the voice's base
period (also when the voice has no table). For a voice with a table (length not 0):

- countdown -= 1; if it is not zero, nothing more this tick;
- else the countdown is reloaded with the speed, the entry at the current position is taken (position 0
  is the base period; positions 2, 4, ... are the listed notes' periods in order; the periods were
  computed at the event as period-table[note + transposition]); output period = entry + the voice's
  LFO value; write AUDvPER = output; position += 2; if the position equals the length it returns to 0.

The countdown starts at 1, so the first output is made in the tick after the event, with the base
period. The table keeps running until another event starts (every event start clears the length to 0;
only an event with bit 4 sets it again), so a table lasts exactly as long as the event that set it.
A table with more than 9 listed notes (n > 10) overruns its record and is not specified.

### 5.4 Volume stage

For each voice, in order, with the voice's current volume value V (word, initially from the
instrument):

- phase 0: unchanged.
- phase 1 (attack): V += attack step; if V (signed 16-bit) minus the peak is negative, nothing else;
  else V := peak and phase := 2.
- phase 2 (hold): hold counter -= 1; if it is still non-negative, nothing else; else phase := 3 (V
  unchanged this tick).
- phase 3 (release): V -= release step; if V minus the end level is greater than 0 nothing else; else
  V := end level and phase := 4.
- phase 4 and higher: unchanged.

Then the voice's volume register is written (AUDvVOL), through the level scaling of 3.2, with the
(unmasked) V, and V is stored. This is done for all four voices, every tick, in the order 0,1,2,3,
whether or not the volume changed.

### 5.5 Retrigger stage

For a voice with retrigger period 0: nothing. Otherwise the retrigger counter is decremented; when it
becomes 0: it is reloaded with the period; the voice's bit is added to the DMA-enable mask; DMACON is
written with the voice's bit alone (DMA off); AUDvLC = the voice's sample start address (the voice's
current address) and AUDvLEN = its sample length are written; the loop-set flag is set with the loop
address and loop length of the voice's instrument copy (written at the next tick, section 5 step 1).
The retriggered sample therefore restarts at the end of this tick when DMACON is written.

### 5.6 Synth stage

For a voice whose synth flag is 0: nothing. For a synth voice:

- Width W = word 40 of the voice's copy. If the synth flag is 1 (growing): W += growth step; if W
  (signed) minus the maximum is not negative: W := maximum, flag := 2. If the flag is 2 (shrinking):
  W -= shrink step; if W minus the minimum is not greater than 0: W := minimum and flag := 1. (A flag
  of any other value is treated as "growing"; it never occurs.) Store W.
- Rewrite the voice's buffer, which holds 2 x (sample length in words) bytes: the first W-1 bytes get
  level A (low byte of word 50), the remaining ones level B (low byte of word 52). For W below 2 the
  first group is empty and B fills 2 x length - W + 1 bytes (W = 0 writes one byte more than the
  length; W - 1 above the buffer length writes A beyond the buffer, and nothing of B).

The buffer is a per-voice area of 128 bytes in chip memory (voice v: base + 128 v); the host allocates
it. A synth instrument's sample address and loop address are this buffer; the loop length equals the
sample length.

## 6. Event decode (voice v)

Done when the voice's countdown becomes 0.

1. Clear the voice's pitch-table length (the previous event's pitch table ends).
2. Take the next flags word of the current pattern. If bit 15 of it is set (or the pattern position is
   "at an end marker"), fetch the next pattern:
   - read the next track-list word; if it is 0xFFFF the voice has reached the end of its track list:
     do the song-end bookkeeping (section 8), then take the first track-list word instead (restart);
   - transposition := 0; if the word has bit 15 set, clear that bit, and transposition := the next
     word (signed);
   - the track position is moved past the words consumed; the pattern position is the start of the
     pattern whose number the word gives; the flags word is the pattern's first word (this word is not
     checked for bit 15: a pattern must have at least one event).
3. If bit 0 of the flags is set: **rest**. The countdown := the next word; write DMACON = the voice's
   bit (DMA off for that voice); the event ends. The voice's volume register is not changed (the
   envelope keeps running, but the channel is silent).
4. If bit 1: **instrument select**. Number n := next word. The instrument offset := 58 n. The voice's
   copy := 45 words from the instrument table at that offset (note that this includes the 16 words
   after the 29-word entry, which belong to the next instrument; they are not used). Then for a
   sample instrument (synth flag 0): sample address := file position of the sample area + the
   instrument's start; loop address := file position of the sample area + the instrument's loop
   start. For a synth instrument: sample address and loop address := the voice's buffer; loop length :=
   sample length.
5. If bit 4: **pitch table**. n := next word, speed := next word; the record: position 0, length 2n,
   countdown 1, speed; then n-1 note words are read, each stored as period-table[note +
   transposition].
6. If bit 3 (tie): set the tie flag. Otherwise: **re-initialise the envelope and effects** by copying
   from the voice's current instrument (the instrument whose number was last selected on this voice, 0
   if none): the volume word (offset 6) and all words from offset 14 to 57 (the LFO, envelope, synth
   and retrigger fields). The sample and loop addresses and lengths (offsets 0..5, 8..13) are kept.
   (This is how the volume restarts at its initial value and the envelope phase returns to its start
   value at every normal note; with a tie they are not reset.)
7. If bit 2: **slide**. Destination period := period-table[next word + transposition]; count := next
   word; delay := next word. The slide record is set up (step to be computed at step 9).
8. Countdown := next word (duration). Note := next word; the note's period := period-table[note +
   transposition]; base period := that period.
9. If a slide was set up: step := (destination - period) divided by count (signed 16-bit result,
   truncated toward zero; period zero-extended); the remaining count and delay stay as read; the record
   keeps the destination period.
10. If the tie flag is set: clear it, write AUDvPER = the note's period; the event ends (nothing else
    is written; the sample keeps running).
11. Otherwise **note start**:
    - write DMACON = the voice's bit (DMA off);
    - write AUDvLC = the voice's sample address (a long);
    - write AUDvLEN = the voice's sample length;
    - write AUDvPER = the note's period;
    - write AUDvVOL = the voice's volume word (offset 6 of its copy), unscaled;
    - write AUDvVOL = the level-scaled value of 8 v (that is 0, 8, 16 or 24 for voices 0..3) (a quirk
      of the original: it passes the wrong value to the scaling; the following volume stage corrects
      the volume at the start of the next tick);
    - set the voice's loop-set flag with the loop address and loop length of the copy (written at the
      next tick, section 5 step 1);
    - add the voice's bit to the DMA-enable mask.
    The voice's pattern position now points after the event.

For events with bit 0, the tie, and all others the pattern position is saved after reading the data.

## 7. Register writes per tick: order and values (summary)

Order within one tick: (1) deferred loop LC/LEN for flagged voices; (2) slide-stage and vibrato-stage
and pitch-table AUDvPER writes, in the stage order of section 5 (voice order inside each stage);
(3) four AUDvVOL writes (volume stage); (4) retrigger writes; (5) per-voice event writes (note start
5-6 writes, rest 1 write, tie 1 write), voices in order; (6) DMACON. Writes made by an event of a
later voice come after those of earlier voices; the volume stage of the tick precedes all events of
the tick.

Typical per-note write block (voice v, times in cc after the DMA-off write): DMACON off (0),
AUDvLC high and low (+13, both at the same cc), AUDvLEN (+24), AUDvPER (+30), AUDvVOL raw (+41), then
the scaled AUDvVOL write 92.5, 101, 104.5 or 118 cc after the raw one for voices 0..3 (SVw in
section 9.3). Exact timing is in section 9.

## 8. Song end

Four flags are kept, one per voice, all set at init (and after each report). When a voice reaches the
end of its track list (the 0xFFFF word, section 6 step 2) it clears its own flag. When this leaves no
flag set (all four voices have wrapped at least once since the last report), the flags are all set
again and the end of the song is reported to the host; the voice then continues with the first track
entry (the song loops). A subsong in which one voice never reaches its track end never reports.

## 9. Timing

### 9.1 Time base

All timing in this section is in cycles from the interrupt entry of the tick. A write's time in cc is
the tick's entry time (cc) plus the floor of (cycles accumulated up to the end of the instruction that
makes the write, divided by 2). Costs are the host's static per-instruction figures; branches cost the
same whether taken or not (9 or 11 depending on the form), so every block below has one cost.

Beam: the raster line is the floor of cc divided by 227, modulo 313 (a line has 227 cc; a frame 313
lines).

### 9.2 Tick skeleton (cycles, in order)

1. Interrupt entry to the start of the play routine: 320.
2. Deferred loop-address stage: head 40; then for each voice 0..3 either 47 (nothing to write) or 97
   (flagged: the AUDvLC long write ends at +49 of the voice's 97, the AUDvLEN write at +65); then 20.
3. Slide stage: call 18, setup 32, the four voices (9.3), return 16.
4. Vibrato stage: call 18, setup 40, the four voices, return 16.
5. Pitch-table stage: call 18, setup 40, the four voices, return 16.
6. Volume stage: call 18, setup 24, the four voices, return 16.
7. Retrigger stage: call 18, setup 48, the four voices, return 16.
8. Event stage: head 16; then for each voice 0..3 either 41 (not due) or 27 + the event work (9.3) + 14.
9. Synth stage: call 18, setup 12, the four voices, return 16.
10. Raster wait: call 18, then 9.4.
11. The final DMACON write: 24 (the write is at the end of these 24).

### 9.3 Per-stage voice costs (cycles)

Slide stage (per voice):
- no slide: 55;
- waiting: 101;
- running (delay already negative): 133; the AUDvPER write ends at +62;
- running in the tick where the delay just expired: 158; write at +87;
- when the count reaches 0 in that run: +44 and a second AUDvPER write (the destination) 49 after the
  first.

Vibrato stage (per voice):
- LFO speed 0: 75;
- waiting for the delay: 121;
- active: E + 29 + M + H + 21 + Wr + 54 where E = 42 (delay already negative) or 67 (delay just
  expired); M = 41 (rising or falling); H = 30 if the rising branch hit the limit, 20 if the falling
  branch hit its limit, else 0; Wr = 16 when the period is written (no pitch table), else 0. The write
  ends at E + 29 + M + H + 21 + Wr.

Pitch-table stage (per voice):
- no table: 83;
- table, countdown not zero: 108;
- table, output this tick: 203 (215 when the position wrapped to 0); the write ends at +124.

Volume stage (per voice): the cost is P + 8 + SV(v) + 136 + 46, where P depends on the phase:

| phase and outcome | P |
|---|---|
| 0 | 33 |
| 1, level not reached | 117 |
| 1, peak reached | 155 |
| 2, hold continues | 75 |
| 2, hold over | 101 |
| 3, release continues | 100 |
| 3, end level reached | 128 |
| 4 and above | 84 |

SV(v) is the cost of the level scaling and write: voice 0: 229, voice 1: 246, voice 2: 253, voice 3:
280; the AUDvVOL write ends at P + 8 + SVw(v), with SVw = 185, 202, 209, 236 for v = 0..3.

Retrigger stage (per voice): period 0: 71; counting: 96; firing: 244 with the DMACON write ending at
+102, the AUDvLC long at +122 and AUDvLEN at +142.

Synth stage (per voice): sample instrument: 43. Synth instrument: 46 + (growing: 57, plus 38 if the
maximum was reached; shrinking: 57, plus 28 if the minimum was reached) + 73 + [if W >= 2: 18 (W - 1)
+ 13] + [if D >= 0: 18 (D + 1)] + 22, where W is the width after the update and D = 2 x length - W.

Event work (cycles; a voice's event uses the items that apply, in this order):

| item | cycles |
|---|---|
| event entry | 131 |
| pattern exhausted (next track word fetched) | 51 |
| track end (wrap), by voice 0,1,2,3 | 43, 64, 81, 88 |
| after the wrap flag update | 21 |
| report of the song end: 110 up to the host call, then (host call, 9.5) 16, then 44 | |
| first track word re-read after the wrap | 34 |
| track-list entry, no transposition word / with | 123 / 155 |
| flags test | 21 |
| rest: up to and including the DMA-off write 52, then | 54 |
| instrument-select test | 21 |
| instrument select: 162 + 22 x 45, plus 91 (sample) or 119 (synth) | 1243 / 1271 |
| pitch-table test | 21 |
| pitch table: 156 + 58 (n - 1) + 10 | |
| tie test | 19 |
| tie: 34; or normal note: re-initialisation copy 444 | |
| slide test | 25 |
| slide set-up | 158 |
| duration and note decode (period lookup) | 163 |
| slide step division (only with a slide) | 238 |
| tie check | 25 |
| tie: write AUDvPER at +24, finishing at +52 | |
| note start: to the DMA-off write 120; AUDvLC long +26; AUDvLEN +22; AUDvPER +12; raw AUDvVOL +22; then the scaling call, whose write ends at SVw(v) and whose total is SV(v) (as above); then 314 | |

The cost of the whole event is the sum of the items that apply. The pattern-exhausted items apply when
step 2 of section 6 fetches a track word.

### 9.4 Raster wait

Before the final DMACON write the play routine waits for nine raster-line changes:

- 28 cycles setup;
- 9 times: read the line (16 cycles; the line is sampled at the end of this read); then repeatedly
  read the line again (16 cycles, sampled at its end) and, while it equals the first sample, spend 9
  more cycles and read again; when the line differs, spend 9 and then 10 cycles and start the next
  round;
- 44 cycles exit.

Then the 24-cycle final write (DMACON). Each round ends within one poll (25 cycles = 12.5 cc) after
the line change it waits for, so the write time follows the 227 cc line grid, quantised to the poll
period.

### 9.5 Song-end report (host callback)

The report is made by a call into the host. For timing: the 110 cycles up to the call are
counted; the cycle count at that instant is rounded down to an even number (the host re-bases its
clock to a whole cc, losing half a cc if the count was odd); then the host call costs 16 cycles and
44 cycles of return follow (section 9.3). The call's own instant in the reference run was cc
22 784 412 for an event that began at cc 22 784 212 (200.5 cc later, rounded down).

### 9.6 Example (first subsong of game_over, first tick, entry at cc 72 862)

| write | cc |
|---|---|
| AUD0VOL 0 | 73 813 |
| AUD1VOL 0 | 74 047 |
| AUD2VOL 0 | 74 285 |
| AUD3VOL 0 | 74 537 |
| DMACON 0x0001 | 76 221 |
| AUD0LC 0x00776130 (two word writes) | 76 234 |
| AUD0LEN 0x0010 | 76 245 |
| AUD0PER 0x01AC | 76 251 |
| AUD0VOL 0x0000 | 76 262 and 76 354 |
| DMACON 0x0002 | 77 771 |
| ... | |
| DMACON 0x8007 | 83 811 |

The sample address 0x00776130 here is the host's synth buffer of voice 0 (this instrument is a synth
instrument); other addresses are the module base plus the offsets of section 2.
