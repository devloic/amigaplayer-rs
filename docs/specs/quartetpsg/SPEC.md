# Quartet PSG: behaviour specification

*Sanitised for publication: behaviour only.*

Format: Atari ST "Quartet PSG" music (Rob Povey and Steve Wetherill, 1990), the YM2149 (PSG)
member of the Quartet family, played on the Amiga by the Eagleplayer "Quartet PSG" ("Quartet PSG
player module V1.2 (30 Sep 2013)", adapted by Wanted Team), internal name `Quartet_PSG`, file name
prefix `sqt.` (`SQT.`), slug `quartetpsg`. The Amiga "Quartet" (`qpa.`) and "Quartet ST" (`qts.`)
formats are different and not covered.

**Feasibility.** The format is playable without any Atari hardware emulation. A module is the
original Atari 68000 sound driver with its music data. The Eagleplayer:

1. patches a few code words of that driver, so that it no longer touches the Atari's status
   register or hardware, and so that it hands its YM register bytes and its song-end events to
   the player instead;
2. runs the patched driver on the 68000 (start entry once, tick entry every tick);
3. converts the 11 captured YM register bytes to Paula writes itself, every tick, with fixed
   2-byte pulse waveforms and generated noise buffers.

The player offers two conversions: mode 0 (labelled "Mad Max", the default) and mode 1
("meynaf", chosen with a configuration file, section 3.2). This spec is written for a port that
**runs the driver contained in each module on the repo's 68000 core**, in the manner of the Riff
Raff, Benn Daglish SID and Mark II ports, and does natively what the Eagleplayer adds: the
patches, the two sites, the conversion, the timing.

No real module was found (see README). Everything here was measured with the reference 68k player
run through the host with DELI_WRITE_LOG, on synthetic modules whose driver is a small 68000
program built to the shape the player's patcher expects (section 2.4). A private model written
from this text reproduces every register write (address, value, order, colour clock) of all 32
reference logs (both modes, three subsong starts, 635 407 writes). It also reproduces the start-up
time (section 3.1).

All values are big-endian. "Module offset" counts from the first byte of the file. Cycles are
68000 cycles of the repo's cycle table (`m68k_rc::cycles`). "cc" means colour clocks; one colour
clock is two cycles. D0..D7 and A0..A7 are named only where the host, the driver and the player
exchange values.

## 1. File

### 1.1 Detection (the player's check)

All of the following must hold:

1. The four words at offsets 0, 4, 8 and 12 are 0x6000, and each word after them (offsets 2, 6,
   10, 14) is a positive even displacement (signed 16-bit, > 0, bit 0 clear). These are four
   branch entries: the target of the entry at offset `o` is `o + 2 + displacement`.
2. The word at offset 16 is 0x49FA.
3. At the target `T` of the entry at offset 8 (T = 10 + the word at 10): the long at T is
   0x48E7FFFE, the word at T + 4 is 0x4DFA, the word at T + 8 is 0x51EE, and the word at T + 12
   is 0x6100.

The host's prefix `SQT.` is only a name hint; the player checks content only.

### 1.2 Entries

- Offset 0: **start entry**. Called once per song start with D0 = subsong number (0-based).
- Offset 4: **tick entry**. Called once per tick.
- Offsets 8 and 12: not called by the player. The song-end patch (2.3) overwrites offset 8..13
  (and, for each further song-end site, the next 8 bytes on: 16..21, ...).

### 1.3 Module end, size and subsongs

These come from the patch walk of section 2 (walk positions are word steps from module offset 0):

- **End `E`**: at the first 0x41FA word after the first 0x40C2 word, take the 16-bit signed
  displacement 6 bytes after it; the base is the address of that displacement word plus the
  displacement. Then find the next 0x43E9 word (searching from 8 bytes after the 0x41FA word);
  E = base + 2 x (the signed word after that 0x43E9). If E - module start > file size, the
  player refuses the module ("module too short").
- **Size reported**: E - module start ("calculated size"); the load size is the file size.
- **Subsongs**: from the last patched position of 2.2 step 5 (the 0x4CDF word), search forward
  in word steps for the first position whose long equals 0x00000056. From there (including that
  position) up to E (excluding E), count the words equal to 0x0046. The subsong count is that
  number divided by 4 (integer). Subsongs are 0..count-1. With a count of 0 the reported range
  is empty (the host's behaviour is not specified here).
- The player reports 3 voices, prefix `SQT.`, no title, no author and no song length.

A module without the 0x00000056 long after the 0x4CDF word makes the player scan forever, and
one whose long lies at or past E makes the count never end (both hang the reference). A port
should reject both.

## 2. Loading: the player's patches

Nothing is copied or relocated: the driver runs where the host loaded the file (in the
reference at **0x0045E0**, the `# module` line of every log). Its code is position independent.
The player changes the loaded image as follows, in this order. All positions are word aligned
and step by 2 bytes.

### 2.1 Status register writes

From offset 0, find the first word 0x40C2. Replace it, and every word after it, with 0x4E71 up
to (not including) the next 0x41FA word. (The first replaced word is the 0x40C2 itself.) Then
compute E (1.3).

### 2.2 Sites and hardware access

1. Walk from 4 bytes after the 0x43E9 word of 1.3 (just past its displacement). At each position:
   - if the long there is **0x206C0032**, this is a **song-end site**: the k-th site found
     (k = 1, 2, ...) writes a 6-byte absolute jump to the player's song-end hook (section 4.2)
     at module offset 8·k, and replaces the site's 4 bytes with 0x6100 followed by the 16-bit
     displacement from (site + 2) to module offset 8·k (a relative call). The walk then goes on
     4 bytes later, *without* testing that next position for a site;
   - if the long there is 0x08380007 or 0x08390007, stop: this is the **hardware test**;
   - otherwise step 2 bytes.
2. Write the byte 0x60 at (hardware test position - 8). In the drivers this turns a conditional
   branch into an unconditional one around the Atari hardware access.
3. From the hardware test position, find the next word 0x40C1: this is the **register site**.
   Its word and the 4 bytes after it become a 6-byte absolute call to the player's
   register-copy hook (section 4.1).
4. Replace the word right after those 6 bytes with 0x4E71 unconditionally, then keep replacing
   words with 0x4E71 until the next word is 0x4CDF (the 0x4CDF word is kept).
5. The subsong search of 1.3 starts at that 0x4CDF word.

A port can apply the same byte changes and treat the call and jump targets as host traps (the
call, jump and relative-call instructions then cost their table cycles on the core, as in the
reference), or treat the sites as traps directly with the same total costs.

### 2.3 What the patches mean for the driver

- The driver keeps its 11 YM register bytes in a buffer and, at the register site, A5 points to
  it. The byte order there is: R1, R0, R3, R2, R5, R4, R6, R7, R8, R9, R10 (Rn = YM register n:
  R0/R1 tone A fine/coarse, R2/R3 tone B, R4/R5 tone C, R6 noise period, R7 mixer, R8..R10
  amplitudes A..C). Envelope registers (R11..R13) are never captured.
- At a song-end site A4 points to a voice record of the driver: the long at A4 + 0x32 is what
  the replaced instruction loaded into A0, and the byte at A4 + 0x3E is a command code.

### 2.4 Synthetic driver used for the logs

The logs' modules (`logs/modules/`) contain a 68000 driver of my own with exactly the patched
shape: four entries, a start entry matching 1.1 rule 3, a tick entry that saves all registers,
has the status-register words, the PC-relative pair and the 0x43E9 word, walks three voice
records through a song-end site, has the 8-byte-before hardware test, and copies 11 bytes per
frame (from a frame table, every `speed` ticks) to the register buffer before the register site.
The start entry picks the first frame from the subsong number. At the frame table's wrap it puts
8, 10 and 9 into the three voice records' command bytes. Variants: one site (`one`), two
adjacent sites (`two`), no site (`none`), and the long form of the hardware test 0x08390007
(`l`). Real drivers will differ in everything but this shape.

## 3. Host calls and the time model

### 3.1 Time model and start-up

As in the repo's other hosted-driver ports: each host call starts a cycle count at 0 at the
clock `T` where it begins. Driver instructions add their table cycles *before* they execute.
Each player step below adds its listed cycles, and an access listed after a cost happens at
`T + floor(count / 2)`. Reads of the beam position, DMACONR and INTREQR see that same clock (and
the host Paula brought up to it). A call ends at `T + floor(count / 2)` with its final count.

Start-up of the reference (one module, first load of the player):

- Detection starts at clock 100 (the engine's start phase). Then come the check, the
  configuration read, the player's initialisation, two volume calls (DTP_Volume, DTP_Balance)
  and the start (InitSound). They run back to back, and their cycles add up as one count:
  **InitSound starts at I = 100 + floor((C_pre + C_init) / 2)**, with C_pre = **1345** cycles in
  mode 1 and **1269** in mode 0 (measured; this covers the check, the configuration read and the
  two volume calls).
- `C_init`, the player's initialisation, is 3 678 493 cycles, plus the patch walk:

| part | cycles |
|---|---|
| fixed part (noise generation included) | 3 678 493 |
| each word examined from offset 0 up to and including the first 0x40C2 | 21 |
| then | 8 |
| each word replaced by 0x4E71 in 2.1 | 29 |
| then | 24 |
| each word examined from 8 bytes after the 0x41FA word up to and including the 0x43E9 word | 21 |
| then | 69 |
| each walk position of 2.2 step 1: site test | 29 |
| ... a song-end site found (then 4 bytes on, no site test at the next position) | + 72 |
| ... first hardware test (0x0838...) | + 29 (stops here if found) |
| ... second hardware test (0x0839...) | + 29 (stops here if found) |
| ... step to the next position | + 17 |
| then | 16 |
| each word examined from the hardware test position up to and including 0x40C1 | 21 |
| then | 36 |
| each word replaced by 0x4E71 in 2.2 step 4 | 29 |
| then | 4 |
| each position examined by the 0x00000056 search | 23, + 18 if it is not the marker |
| then | 8 |
| each word counted from the marker up to E | 32, + 8 if it is 0x0046 |
| end | 161, + 44 in mode 1 |

  The fixed part contains the noise generation (section 8), which the player does only when its
  1 KB noise buffer still starts with a zero long, i.e. on its first initialisation. Its share
  is 3 678 340 cycles (204 848 for the first buffer, 3 473 436 for the second, 56 for the calls
  and the marker store); without it the fixed part is 153 cycles.
- Reference values of I for every log are in LOGS.md (for example 2 030 679 for an 18 476-byte
  module in mode 1).

### 3.2 Mode selection

The player reads a 4-byte configuration file, first `Env:EaglePlayer/EP-Quartet_PSG.cfg`, then
`/Configs/EP-Quartet_PSG.cfg`. If it starts with the ASCII bytes `WT` and the following word is 0
or 1, that word is the mode; otherwise the mode is 0. The UADE setup has no such file, so mode 0
is the reference's default. A port should default to mode 0 and may offer mode 1 as an option.

### 3.3 Start (InitSound)

1. The player clears its 80-byte voice-info block, then initialises the selected conversion
   (5.1 or 6.1), re-arms the song-end flags (4.2), and jumps to the start entry with D0 = the
   subsong number and A0 = the module address. The call ends when the driver returns.
2. Costs before the driver's first instruction: mode 0 **1583** cycles (no writes). Mode 1:
   **1379** cycles, then DMACON = 0x000F, then the voice writes of 6.1, then 264 cycles to the
   driver.

Subsong k > 0 (the host's sequence): the host first starts song 0 at I. Then for j = 1..k:
EndSound (3.4), a pause of 236 cc (the two volume calls, 118 cc each), and InitSound with
subsong j. Each call starts where the previous one ended.

### 3.4 EndSound

From its start: 28 cycles, DMACON = 0x000F; 16, AUD0VOL = 0; 12, AUD1VOL = 0; 12, AUD2VOL = 0;
12, AUD3VOL = 0; 16 to the end (96 cycles in all).

### 3.5 Ticks

- The first tick is due 70 940 cc after the end of the last InitSound call, and then every 70 940
  cc (the default 50 Hz timer; the player sets no tempo).
- A tick starts at its due time, or, if the previous tick has not ended by then, at that tick's
  end (a mode-0 tick can last longer than 70 940 cc, see 5.4). Due times stay on their grid.
- One tick, from its start: **266** cycles; the tick entry runs (registers: D0..D7 = 0, A0 =
  module address); after it returns, **39** cycles; the conversion of the mode (5 or 6); then
  **182** cycles to the end in mode 0 and **172** in mode 1.

## 4. Sites

### 4.1 Register site

The 6-byte call costs 20 cycles; the hook costs 200 cycles. Behaviour: the 11 bytes at A5 are
copied into the player's YM shadow in the order of 2.3, and A5 advances by 11. No other register
changes. Execution continues after the 6 bytes (into the replaced 0x4E71 words, 4 cycles each).

### 4.2 Song-end site

The relative call costs 18 cycles and the jump at offset 8·k costs 12 cycles. The hook then:

- loads A0 from A4 + 0x32 (what the replaced instruction did);
- reads the command byte `c` at A4 + 0x3E: c = 8 clears flag 1, c = 10 clears flag 2, c = 9
  clears flag 3;
- if all three flags are now clear, it reports the song end to the host and sets all three again.

The flags are set at every start (3.3), so the song ends once the driver has shown all three
codes since the start or since the previous report, in any order and over any number of ticks.
Hook costs: 48 + 25 cycles, then c = 8: + 22; otherwise + 25, then c = 10: + 26; otherwise + 25,
then c = 9: + 16. Then + 21, + 84 if the song end is reported, + 44. Total without a report:
160 (c = 8), 189 (c = 10), 204 (c = 9), 188 (other values).

## 5. Mode 0 ("Mad Max", default)

### 5.1 State and start

Four converter voices: A, B, C (tone channels) and N (noise). Each has a volume, a period and a
status: 0 silent, 1 pulse, 2 noise. There is also a noise accumulator (a count and a volume).
At the start: all volumes 0, all periods 0x100, all statuses 0, count 0, accumulator volume 0.
The YM shadow is cleared, except R7 = 0x3F. No Paula writes.

Paula voices: **A → voice 0, B → voice 3, C → voice 2, N → voice 1.**

Waveforms: pulse = 2 bytes 0xB2, 0x4D (length 1 word); noise = the 1 KB buffer (length 0x200
words); empty = 2 zero bytes (length 1 word). Addresses: section 8.

### 5.2 Conversion of one tick (after the tick entry)

Mixer: a channel's tone is on when its R7 bit (0, 1, 2 for A, B, C) is clear. Its noise is on
when its R7 bit 3, 4 or 5 is clear.

For A, B, C in this order (channel i, tone period P = (R(2i+1) x 256 + R(2i)) & 0xFFF, level
L = R(8+i) & 15):

1. The voice's volume becomes VT0[L], with VT0 = 0, 1, 2, 3, 4, 5, 7, 9, 12, 16, 21, 27, 34, 42,
   52, 64.
2. Tone on: the period becomes ((P x 14540) >> 10) + 1 (16-bit). If its noise is on as well,
   the accumulator count increases by 1 and the accumulator volume increases by this volume.
   If the status is not 1, it becomes 1 and the voice is **restarted** (5.3) with the pulse
   waveform.
3. Tone off and noise on: the period becomes (R6 & 31) x 16 + 142. If the status is not 2, it
   becomes 2 and the voice is restarted with the noise waveform.
4. Both off: volume 0, period 0x100, status 0, and the voice is restarted with the empty
   waveform **every tick** (no status test).
5. Then AUDxVOL = the scaled volume (section 7; with the host's defaults the volume itself),
   then AUDxPER = the period.

Then voice N:

- Count 0: if N's status is not 0, N gets the "both off" handling of step 4 (restart with the
  empty waveform, then volume 0, period 0x100). Otherwise nothing at all is written.
- Count 1, 2 or 3: N's volume becomes the accumulator volume, halved for count 2, divided by 3
  (integer) for count 3. N's period and restart follow step 3 (period from R6, restart with the
  noise waveform if the status is not 2), then step 5 on voice 1. The count and the accumulator
  volume return to 0.

So a tick writes, per voice in the order 0, 3, 2, 1: the restart sequence if any, then volume,
then period; voice 1 writes nothing in a tick with count 0 and N silent.

### 5.3 Restart sequence (voice v, Paula bit b = 1 << v)

1. Line sync (5.5), then AUDxPER = 1.
2. Read DMACON; `a` = its bit b. INTREQ = a << 7, then DMACON = a (bit 15 clear: stop the voice
   if it was on; both writes are 0 if it was off). Wait until INTREQR has all bits of a << 7 set
   (no wait when a = 0).
3. AUDxLC = start of the waveform (one long store: two log lines with the same clock), AUDxLEN =
   its length, AUDxPER = the voice's period.
4. Line sync. Read DMACON; `o` = bit b if the voice's DMA is off, else 0. INTREQ = o << 7, then
   DMACON = 0x8000 | o. Wait until INTREQR has all bits of o << 7 set.
5. Line sync. AUDxLC = the loop start (the same waveform), AUDxLEN = the loop length (the same
   length).

The waits on INTREQR depend on the host Paula: after the stop the flag rises only when the
voice's current sample period runs out, with the old period (up to 65 535 cc). After the start it
rises when the first word has been fetched (about 400 cc). A port must poll the same Paula flag at
the clocks given in 5.4.

### 5.4 Mode 0 costs (cycles)

Per-voice bookkeeping costs (Paula voice 0, 1, 2, 3):

| name | voice 0 | voice 1 | voice 2 | voice 3 |
|---|---|---|---|---|
| CV (volume scaling) | 163 | 180 | 187 | 214 |
| SV (volume or period bookkeeping) | 79 | 110 | 141 | 149 |
| SL (location bookkeeping) | 95 | 126 | 157 | 165 |

Conversion, in order:

- Start: 110. Each tone channel: 37 on entry; between channels A→B and B→C: 82.
- Tone on: 102 (period), 15, + 40 if its noise is on as well, 25; if restarted: 62, the restart
  sequence, 10. Then the write step.
- Tone off: 15. Noise on: 50, 25; if restarted: 62, restart, 10; write step. Both off: 82,
  restart, write step.
- **Write step** (voice v): 56 + CV(v) + SV(v), AUDxVOL; 24, AUDxPER; 10 + SV(v).
- After C: 37. Count 0: 21; then N's both-off handling (82, restart, write step on voice 1) if
  N's status is not 0, else 16. Count ≥ 1: 13; count ≥ 2: + 13; count 2: + 12; count 3: + 158.
  Then 18, N's noise handling (50, 25; if restarted: 62, restart, 10; write step on voice 1),
  then 42.

Restart sequence (voice v):

| step | cycles, then the access |
|---|---|
| entry | 30, then line sync |
| PER = 1 | 16 |
| read DMACON | 12 |
| INTREQ write | 28 + 12 |
| DMACON write | 12 |
| each poll of INTREQR (until done) | 12, read, then 17 |
| LC write (both words) | 12 |
| LEN write | 12 |
| PER write | 18 + SL(v) + 4 + 20 |
| line sync | |
| read DMACON | 12 |
| INTREQ write | 40 + 12 |
| DMACON write | 12 |
| each poll of INTREQR | 12, read, then 17 |
| line sync | |
| LC write (loop) | 12 |
| LEN write (loop) | 12 |
| exit | 16 |

### 5.5 Line sync

The beam: line = floor(clock / 227) mod 313, horizontal position h = clock mod 227. A line sync
costs 12 cycles and samples the low 8 bits of the line. Then it repeats 12 cycles, sample, 9
cycles until the sample differs. Then it repeats 16 cycles, sample of h, 9 cycles until h ≥ 22.
The access after it follows with its own cost. So, for example, AUDxPER = 1 lands early in the line after the one in progress (the logs
show h = 34..47).

## 6. Mode 1 ("meynaf")

### 6.1 Start

At InitSound (3.3), from the DMACON = 0x000F write: 24 cycles, then for voices 0, 1, 2 and 3:
AUDxLC (one long store), 8, AUDxLEN, 8, AUDxPER = 1, 12, AUDxVOL = 0 and AUDxDAT = 0 (one long
store: two log lines with the same clock). 20 cycles separate one voice's VOL/DAT from the next
voice's LC. Voices 0..2 get the pulse-2 waveform (2 bytes 0x70, 0x90), length 1. Voice 3 gets the
16 KB noise buffer, length 0x2000. DMA stays off. The YM shadow is cleared, except R7 = 0xFF.
After the last VOL/DAT: 200 cycles, then 64 to the driver.

### 6.2 Conversion of one tick

Tone channels 0, 1, 2 go to Paula voices **0, 1, 2**, noise to voice **3**. For channel i, with
P = (R(2i+1) x 256 + R(2i)) & 0xFFF, L = R(8+i) & 15, and noise on when R7 bit (3+i) is clear
(**the tone bits R7 0..2 are ignored**):

- period p = floor(P x 1419 / 100) (0 if P = 0); if p < 123, p = 0;
- levels: tone level t = L and noise level n = 0. If its noise is on: n = L, and if L ≠ 0 and
  P ≠ 0 then t = L - 2 for L ≥ 2 (so 0 for L = 2), t = 1 for L = 1, and n = L - 1 for L ≥ 2,
  n = 1 for L = 1;
- tone volume = VT1[t], noise contribution = VT1[n], with VT1 = 0, 1, 1, 1, 2, 2, 3, 4, 6, 8,
  11, 16, 23, 32, 45, 64;
- writes: if p ≠ 0, AUDiPER = p; then AUDiVOL = the scaled tone volume (section 7).

Then AUD3PER = 0x80 + 0x20 x (R6 & 31), AUD3VOL = scaled min(64, sum of the three noise
contributions). Then DMACON = 0x8000 | bits, where bits has bit i for each channel whose period
was written, and bit 3 if any of R7 bits 3..5 is clear. Then DMACON = the other bits of 0..3,
clear (0x000F with those bits removed, bit 15 clear). The voices loop their 1-word or 8192-word
buffers forever; nothing is ever restarted.

### 6.3 Mode 1 costs (cycles)

- Start: 56.
- Each channel i (Paula voice i): 133; + 190 if P ≠ 0; 19; if noise on: 13, and if L ≠ 0: 13,
  and if P ≠ 0: 13 (+ 4 if L < 2) then 13 (+ 4 if L = 1). Then 17 (+ 4 if p < 123 or P = 0), 52,
  17. If p ≠ 0: 12, AUDiPER, then 18 + SV(i) + 12. Then 4 + 18 + CV(i) + 18 + SV(i) + 12,
  AUDiVOL, then 26.
- Noise: 46 + 12, AUD3PER; 18 + 149 + 17 (+ 4 if the sum ≥ 64); 4 + 18 + 214 + 18 + 149 + 12,
  AUD3VOL; 49 + 16, DMACON set; 8 + 16, DMACON clear; 16.

(CV and SV as in 5.4.)

## 7. Volume scaling and voice switches

Every volume written in both modes is v' = ((v x F) >> 6) & M. F is the player's left factor for
Paula voices 0 and 3 and its right factor for voices 1 and 2: left = (left balance x master
volume) >> 6, right = (right balance x master volume) >> 6, from the host's DTP_Volume and
DTP_Balance calls. M is 0xFFFF, or 0 for a voice switched off with EP_Voices (which also writes
volume 0 at once). The reference host leaves volume and balance at 64, so F = 64 and v' = v
everywhere in the logs.

## 8. Waveform buffers

One chip-memory block of the player holds, at these offsets:

| offset | size | content |
|---|---|---|
| 0x0000 | 1024 | noise buffer 1 (mode 0) |
| 0x0400 | 16384 | noise buffer 2 (mode 1) |
| 0x4400 | 2 | pulse-2: 0x70, 0x90 (mode 1) |
| 0x4402 | 2 | pulse: 0xB2, 0x4D (mode 0) |
| 0x4404 | 2 | empty: 0, 0 (mode 0) |

In the reference (with only this Eagleplayer in the player directory) the block is at
**0xADAC60**, so the LC values in the logs are 0x00ADAC60 (noise 1), 0x00ADB060 (noise 2),
0x00ADF060 (pulse-2), 0x00ADF062 (pulse) and 0x00ADF064 (empty). A port uses its own addresses;
`tools/replay-cmp.py` accepts a constant difference.

The noise buffers come from one generator with a 32-bit state `x`, one byte per step. With
`s = x + 0xE90` (mod 2^32), let `s'` be s with its **low 16 bits** shifted left by 4 (the upper
16 bits unchanged, the bits shifted out of the low half lost). The new state is
`((s' + 3677 x x) mod 2^32, with bit 31 cleared) - 1` (mod 2^32). The output byte is bits 8..15
of the new state. Buffer 1 starts from x = 0x48495050 (1024 bytes). Buffer 2 starts from
x = 0 (16384 bytes). Both are generated once, at the first initialisation (3.1). The exact bytes
are also in `data/` (README).

## 9. Song end

Reported only through the song-end sites (4.2). A module without such a site never ends. The
reference host stops at the end of the 0.1 s render chunk in which the report came, so its logs
run up to four ticks past the report. The player does not stop or restart anything itself. The
driver goes on as it would on the Atari (the synthetic one wraps to its first frame).

## 10. Notes on real drivers (unverified)

Real modules contain the original Atari driver; its music logic (sequencing, envelopes,
arpeggios) is the driver's own code and runs unchanged on the core. What a real driver may do
that the synthetic one does not:

- read or write other Atari hardware addresses outside the patched hardware test (in the
  reference these land in unmapped memory: reads give 0, writes are lost);
- use the YM envelope registers R11..R13, which the conversion ignores;
- rely on registers at entry other than D0 (start) and A0 (both entries). In the reference the
  data registers are 0 at the tick entry, A5 holds the host's player-data pointer and A6 the
  host's library base.
