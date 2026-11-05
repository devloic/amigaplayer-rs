# TFMX ST (mdst.*) - behaviour specification

*Sanitised for publication: behaviour only.*

Format name: TFMX_ST (Atari ST variant of TFMX, Chris Huelsbeck driver, here run as a YM2149 driver whose
register image is converted to Paula writes once per tick). Module prefix `mdst.`, companion sample file
`smpl.mdst.*`.

All numbers are hexadecimal when written `$xx`. "Word" = 16 bit big-endian, "long" = 32 bit big-endian.
Times are colour clocks (cc) unless stated as CPU cycles (1 cc = 2 CPU cycles on the host).

## 1. Shared with Amiga TFMX, and different

The repo's Amiga TFMX port (Pro / 1.5 / 7V) and this format share the **container and the sequencer**:

* the same `TFMX-SONG ` module header, 6x40 text, song tables (first step, last step, speed), mute words;
* the same table location rule (explicit offsets at $1D0/$1D4/$1D8, or fixed $800/$400/$600);
* the same 16-byte track steps with `EFFE` command steps, the same 4-byte pattern rows and pattern
  commands $F0-$FF, the same 4-byte macro steps and much of the same macro command numbering
  (wait, loop, continue, gosub/return, volume, glide, vibrato, envelope, riff/"arpeggio", conditional jumps).

Different:

* **Output**: macros do not drive Paula. Each of the four "fields" (voices) writes a YM2149 register image
  (three tone/noise voices plus one field controlling the noise/envelope period registers). Once per tick
  a conversion step turns that image into Paula writes (section 9). Paula DMA, sample pointers and
  lengths never come from the module, except one digital sample channel.
* **Macro set**: commands $40-$4A are ST-only (mixer, YM envelope, digital sample). The Amiga DMA/sample
  commands (e.g. $00-$03, $11, $18-$1A, $22-$2F) do nothing here and only step past.
* **Period tables**: notes played by macros use a PSG period table; portamento notes from patterns and
  riff notes use the Amiga period table (both given in section 7).
* **Detection** is disjoint from the Amiga variants: the ST variant needs a macro command byte above $3F
  (an ST command), which the Amiga Pro detection rejects, and it needs the `SONG` header, which the 1.5
  detection rejects.

## 2. Detection

All on the module file bytes:

1. bytes 0-9 are `TFMX-SONG ` (with the trailing space);
2. if the long at 464 is 0, the word at 12 must be 0;
3. choose a scan range:
   * if the long at 468 is non-zero (explicit-offset layout): from the first macro table entry
     (long at macro-table offset + 0) up to the pattern table offset;
   * else: from the long at 1536 up to the long at 2044 (first and last macro table entries of the fixed
     layout);
   each bound must be non-zero, positive and even;
4. scan every 4th byte (the command byte of each macro step) in that range: at least one must be > 63;
   the number of bytes equal to $48 (digital sample command) is the sample count.

## 3. Companion sample file

Name: `smpl.` + module file name, after stripping a leading `mdat.` (case-insensitive) from the name.
For `mdst.X` this is `smpl.mdst.X`. It is loaded only when the sample count from detection is non-zero.

4-bit conversion: if the long at offset 4 of the sample file has no bit set in $F0F0F0F0, every byte b
of the file is replaced by `T[b & 15]` with
`T = 00 93 A7 BF CF DF EF FF 0F 1F 2F 3F 4F 5F 6F 7F` (hex bytes).

## 4. Subsongs, title, length

Song number s uses first step `fsteps[s]` (word at 256+2s), last step `lsteps[s]` (word at 320+2s),
speed (word at 384+2s). Length shown = last - first + 1.

The host subsong list is built from song 0 plus songs k = 1..31 in order, with a flag f (initially 0):

* skip k if its last step is 0 and f is set;
* skip k if its first step is $1FF;
* skip k = 31 if its last step equals song 30's last step;
* if song k has the same first and last step as song k+1:
  set f (if it was clear, and song k's last step equals song k-1's last step, skip k);
  skip k if its last step equals its first step; skip k if k <= 30;
  otherwise (k = 31) take k only if its last step differs from the last step of song k+2 (reads past the
  table, i.e. the speed table's first entries);
* otherwise take k.

`--subsong N` plays the N-th entry of that list. Each subsong change first stops the sound (DMACON
$000F, then the four AUDxVOL = 0) and then runs the song initialisation.

Title: the 6x40 text at 16 (as in Amiga TFMX).

## 5. Initialisation

YM register image: reg7 = $3B, reg10 = $10, reg8 = reg9 = 0, reg5 = 4, reg4 = 0, reg11-13 = 0; other
registers 0. Digital sample rate $240, digital state 0; the three conversion channel states 0
(silent).

Global fade: volume $40, target $40, counters 0, step 0, no fade in progress. Speed 5.

Song start for song number s:

1. all off: playback flag off, mixer shadow = $FF, and for each field: macro stopped, skip flag 0,
   riff off, sample flag 0;
2. current step = first step = fsteps[s & 31]; last step = lsteps[s & 31]; speed = speeds[s & 31];
3. all 8 tracks: pattern address = an empty pattern (rows `F4000000 F0000000`), pattern byte $FF,
   transpose 0, row position 0, row wait 0;
4. unless bit 7 of s is set, read the track step (section 6.1);
5. speed counter 0; high byte of the track-loop counter = $FF; playback flag on.

Field constants (period register pair low/high, volume register, mixer masks):

| field | period regs | volume reg | tone-on AND | tone-off OR | noise-on AND | noise-off OR |
|---|---|---|---|---|---|---|
| 0 | 0/1 | 8 | $FE | $01 | $F7 | $08 |
| 1 | 2/3 | 9 | $FD | $02 | $EF | $10 |
| 2 | 4/5 | 10 | $FB | $04 | $DF | $20 |
| 3 | 11/12 | 6 | $FF | $00 | $FF | $00 |

Field 3's "period" is the YM envelope period (not used by the conversion); its "volume" is the noise
period register 6. Every write of the mixer shadow also copies it into reg7.

## 6. Per tick

1. Song-end check: find the first of the 8 tracks whose pattern byte is not $FF; if none, signal song
   end (the host stops a few ticks later).
2. If the playback flag is on: process fields 0, 1, 2, 3 (section 8), then the sequencer (6.2) unless
   bit 7 of the song number is set.
3. Always: the Paula conversion (section 9).

### 6.1 Reading a track step

Address = track table + current step * 16 (16-bit wrap of the product).

* First word `EFFE`: command = second word; a command >= 5 acts as 0.
  * 0: playback flag off, song end; stop reading.
  * 1 (loop): with loop counter L (signed 16-bit): if L = 0, set L = $FFFF and go to the next step;
    if L < 0, take count C = fourth word (signed): if C <= 0 signal song end; L = C - 1, current step =
    third word; if L > 0: L -= 1, current step = third word. Read again.
  * 2 (speed): speed = speed counter = third word; next step; read again.
  * 3 and 4 (fade): next step; start a fade with speed = low byte of the third word and target = low byte
    of the fourth word (6.4); read again.
* Otherwise for tracks 0-7, word w: pattern byte = w >> 8, transpose = w & 255; if w < $8000 the
  track starts pattern (w >> 8) from the pattern table: row 0, row wait 0, loop count 0. Words >= $8000
  only change the two bytes (so $80xx keeps playing the current pattern with a new transpose, $FFxx stops
  the track, $FExx stops it and turns its channel off, see 6.3).

### 6.2 Sequencer

Speed counter -= 1 (signed). If still >= 0, done. Else counter = speed, reload each track's mute word
(word at 448 + 2*track; non-zero = muted), and process tracks 0..7 (6.3). If a track reached a pattern
end and a new track step was read, start the pass again from track 0 (repeat until a pass ends without
a new step).

### 6.3 Track processing

* pattern byte >= $90: if it is $FE, set it to $FF and, if the track is not muted, turn off channel
  (transpose byte) - see "channel off" below. Otherwise nothing.
* row wait non-zero: decrement, done.
* otherwise read rows (row position as signed 16-bit index of 4-byte rows) until a row ends the track's
  work for this tick. Row bytes b0 b1 b2 b3:
  * b0 < $F0 (note): wait note when $7F <= b0 < $C0: row wait = b3 and b3 becomes 0. Note byte = (b0 +
    transpose) & 255, masked with $3F when b0 < $C0. Unless muted, dispatch the row as a note (8.1).
    Position + 1. A wait note ends the track; other notes continue reading.
  * b0 >= $F0, command c = b0 & 15:
    * 0 end: pattern byte = $FF; if the current step equals the last step, the current step becomes the
      first step and song end is signalled, else step + 1; read the track step; restart the pass.
    * 1 loop: if loop count is 0: count = b1, position = b2b3 word; else count - 1: if it reaches 0,
      position + 1, else position = b2b3. Continue.
    * 2 jump: pattern = b1, position = b2b3. Continue.
    * 3 wait: row wait = b1, position + 1. End.
    * 4 and 14: pattern byte = $FF. End.
    * 5: b1 += transpose, then dispatch (key off); 6 (vibrato), 7 (envelope), 12 ($FC portamento with note
      $3C) dispatch the row unchanged; muted tracks skip the dispatch. Position + 1. Continue.
    * 8 pattern call: save address and position, pattern = b1, position = b2b3. Continue.
    * 9 return: restores the saved address/position of **track 0** (whatever track runs it), then this
      track's position + 1. Continue.
    * 10 ($FA) fade: speed b1, target b3 (6.4); position + 1. Continue.
    * 11 ($FB): the track numbered (b2 & 7) starts pattern b1 (address from entry b1 & $7F) with
      transpose b3, position 0, wait 0, loop 0. Position + 1. Continue.
    * 13, 15: position + 1. Continue.

Channel off (byte x): field x & 3; if (x & 15) >= 6, the mixer shadow gets the field's tone-off and then
noise-off bits (reg7 updated after each) and the field's envelope bit is cleared. Then the field's macro
is stopped and its riff turned off.

### 6.4 Fade start

Ignored while a fade is in progress. Otherwise: in progress, target = T, both counters = speed S. If
S = 0: fade volume = T at once, step 0, not in progress. Else step = +1 if volume < T, -1 ($FF) if
volume > T, and if equal: step 0, not in progress.

## 7. Tables

PSG periods (64 entries, notes 0-63):
2703 2551 2407 2273 2145 2025 1911 1804 1703 1607 1517 1432 1351 1275 1203 1136 1072 1012 955 902 851 803
758 716 675 637 601 568 536 506 477 451 425 401 379 358 337 318 300 284 268 253 238 225 212 200 189 179
168 159 150 142 134 126 119 112 106 100 94 89 84 79 75 71

Amiga periods (64 entries): 1710 1614 1524 1438 1357 1281 1209 1141 1077 1017 960 908 856 810 764 720
680 642 606 571 539 509 480 454 428 404 381 360 340 320 303 286 270 254 240 227, then
214 202 191 180 170 160 151 143 135 127 120 113 twice, then 214 202 191 180.

Paula volume per YM volume value v (index v & 31): 0 1 2 3 4 6 8 10 13 16 20 24 30 38 48 64 for 0-15,
32 for 16-31 (envelope mode).

## 8. Field processing (per field, per tick)

Order: macro part, then modulations, then riff, then fader. (Each part continues into the next.)

Macro part: if the macro is stopped, skip to modulations. If the macro wait is non-zero, decrement it
and go to modulations. Otherwise execute steps (address = macro address + step*4, signed) until one ends
the macro part for this tick. Step bytes c b1 b2 b3, w = b2b3 word. "Next" = step + 1.

| cmd | behaviour |
|---|---|
| $04 | if b1 bit 0: when the riff trigger flag is set, stay on this step and end; else set it, next, continue. Otherwise wait = w, next, end |
| $05 | loop: count 0 -> count = b1, step = w; else count-1, reaching 0 -> next, else step = w. Continue |
| $10 | as $05 but if the key flag is 0: next, continue |
| $06 | continue at macro (b1 & $7F) step w; loop count 0, skip flag 0. Continue |
| $15 | as $06 but first saves the current macro address and step |
| $16 | back to the saved address/step, then next. Continue |
| $07 | stop the macro. End |
| $08 / $09 / $1F | note period from b1 + current note / b1 / b1 + previous note (8.2), next, end |
| $0D / $0E | if b2 = $FE: note period from b1 + current note with no extra detune. Volume = basevol*3 + b3 ($0D) or b3 ($0E). Next, continue |
| $0A | riff off, envelope 0, vibrato size 0, glide time 0, envelope bit 0. Next, continue |
| $40 | as $0A without touching the riff |
| $0B | glide: speed b1, counter 1, if no glide active the glide period = base period; glide time = w. Next, continue |
| $0C | vibrato: size b1, half-size b1>>1, rate b3, counter 1; if no glide active, vibrato offset 0. Next, continue |
| $0F | envelope: length b2, speed b1, counter b2, target b3. Next, continue |
| $14 | key-up wait: key flag 0 -> next, continue. Count 0 -> count = b3, end (stay). Else count-1: 0 -> next, continue; else end (stay) |
| $17 | base period = w (no register write). Next, continue |
| $1B | riff: macro b1, speed b2, mode b3, counter 1, riff on; run the riff part and the fader once right now (8.4, 8.5); set the trigger flag; next, continue |
| $1C | if b1 >= current note: next, else step = w. Continue |
| $1D | if b1 >= volume: next, else step = w. Continue |
| $1E | store b1 (no audible effect). Next, continue |
| $21 | dispatch the note (current note, b1, b2 OR (basevol << 4), b3) (8.1). Next, continue |
| $31 | dispatch a key-off with bytes ($F5, b1, b2, b3). Next, continue |
| $32 / $33 | the word at (this step's address + 2*b1) in the module image += w / AND= w. Next, continue |
| $30 | if the skip flag is set: step = w; else set it, next. Continue |
| $41 | b1 != 0: tone on else tone off; then b3 != 0: noise on else off (mixer shadow, reg7 after each). Next, continue |
| $42 | reg11 = b3, reg12 = b2, reg13 = b1, envelope bit = $10. Next, continue |
| $43 | YM envelope period from b1 + current note (8.2 with the same detune rule), stored as base period; written to reg11/12 only if no glide active. Next, continue |
| $44 | field 3 volume (noise period) = b1. Next, continue |
| $45 | the field's period registers = w (low, high), base period = w. Next, continue |
| $47 | mixer = (mixer AND b1) OR b3. Next, continue |
| $48 | digital sample: tone on and noise off for this field, period registers 0, base period 0, sample offset = low 20 bits of the step, sample length 0, sample flag on, digital state 1. Next, continue |
| $49 | digital rate = w. Next, continue |
| $4A | sample flag 0, digital state 0. Next, continue |
| other < $50 | next, continue |
| >= $50 | next, end |

### 8.1 Note dispatch (bytes b0 b1 b2 b3; field = b2 & 3)

* b0 = $F7: envelope speed b1, length = counter = (b2 >> 4) + 1, target b3.
* b0 = $F6: vibrato size b1 & $FE, half-size its half, rate b3, counter 1, offset 0.
* b0 = $F5: key flag 0.
* other b0 >= $BF (portamento note): glide speed b1, counter 1; if no glide active, glide period =
  base period; glide time b3; current note = b0 & $3F; base period = Amiga period of that note.
* otherwise (normal note): detune = b3 sign-extended to 16 bits; basevol = b2 >> 4; previous note =
  current note; current note = b0; new macro address = macro table entry b1; if it differs from the
  current macro address the skip flag is cleared; macro step 0, wait 0, "first tick" marker set, loop
  count 0, macro running, key flag 1.

### 8.2 Note period (n, extra)

n = n & $3F; p = PSG period; d = detune + extra (16-bit); if d != 0, p = (p * (d + 256)) >> 8 (16-bit).
Base period = p; if no glide is active, write p to the field's period register pair (low, high).

### 8.3 Modulations

On the first tick after a normal note (marker set by 8.1), clear the marker and skip directly to the
fader. Otherwise:

* Vibrato (size non-zero): offset += signed rate; p = base period; if offset != 0,
  p = (p * (offset + 2048)) >> 11; if no glide active, write p. Half-size - 1; at 0 it reloads from
  the size and the rate is negated.
* Glide (time non-zero): counter - 1; at 0: counter = speed; compare glide period G with base period B:
  G = B ends the glide; G < B: G = (G * (256 + time)) >> 8, ends when G >= B; G > B:
  G = (G * (256 - time)) >> 8, ends when G <= B. On end: time 0 and G = B. G &= $7FF, stored, written.
* Envelope (length non-zero): if counter non-zero, decrement. Else counter = length and the volume
  moves toward the target by speed using signed 8-bit compares: up until >= target; down until the
  result is negative or <= target; on reaching it, volume = target and the envelope stops.

### 8.4 Riff

If the riff is off, go to the fader. On its first run: riff address = macro table entry (riff macro &
$7F), position 0, marked started. (Mode bit 0 selects a random variant driven by the beam position: not
specified, see README.) Counter - 1:

* non-zero: if mode bit 1 ("echo") and the counter equals ((speed*3) >> 3) & 255, volume =
  (volume*5) >> 3. Go to the fader.
* zero: counter = speed; read the byte at the riff position; 0 at position 0 leaves everything
  unchanged; 0 elsewhere wraps to position 0. Note = (byte + current note) & $3F (a result of 0 also
  takes the random variant); period = Amiga period, scaled by (detune + 256) >> 8 when detune != 0; base
  period = it. If a glide is active, stop here (position unchanged). Else write it; if byte bit 7 is
  set, clear the trigger flag; position + 1.

### 8.5 Fader (per field, so 4 times per tick)

If the fade step is non-zero: counter 1 - 1; at 0: counter 1 = counter 2, fade volume += step (8-bit);
equal to the target ends the fade (step 0, not in progress).

d = field volume. If fade volume bit 6 is clear: d = ((d * 4 * fadevol) & $FFFF) >> 8 and the field's
envelope bit is cleared. If the sample flag is set, no write. Else d >>= 2; the field's volume register
= ($0F if d bit 4 is set, else d) OR envelope bit.

## 9. Paula conversion (every tick)

Let M = (NOT reg7) & $3F. YM voices map to Paula: A (period regs 0/1, volume reg 8) -> channel 0,
B (2/3, 9) -> channel 3, C (4/5, 10) -> channel 2, processed in that order; digital -> channel 1, last.

Per YM voice v (0, 1, 2):

* tone (M bit v): if its state is not "tone", state = tone and restart the channel with the pulse buffer
  (one-shot = loop = pulse, lengths 2/2). Volume = table[reg & 31]; period = ((period word & $FFF)*7)+1.
* else noise (M bit v+3): if state is not "noise", state = noise and restart with the noise buffer
  (lengths $200/$200). Volume as above; period = $280 - $10*(reg6 & 31).
* else: restart with the silent sample (lengths 1/1) **every tick**, state = silent, volume 0, period
  $100.
* then write AUDxVOL, then AUDxPER.

Digital channel: state 0: restart with silent 1/1, then AUDxPER = rate, AUDxVOL = $40. State 1: state = 2,
restart with one-shot = sample data + offset, length 0, loop = silent, length 1, then PER = rate, VOL =
$40. State 2: no writes.

Channel restart (mask m; "running" = the channel was restarted before; it stays running forever):

1. wait for the beam (9.2); AUDxPER = 1;
2. INTREQ = (m if running else 0) << 7; DMACON = (m if running else 0);
3. if running, wait until INTREQR shows that channel's audio bit (9.2);
4. AUDxLC = one-shot, AUDxLEN = one-shot length;
5. wait for the beam; INTREQ = m << 7; DMACON = $8000 | m; wait until INTREQR shows the bit;
6. wait for the beam; AUDxLC = loop, AUDxLEN = loop length.

Buffers: pulse = 4 bytes `B2 B2 4D 4D`; silent = 4 zero bytes after it; noise = 1024 bytes generated
from seed s = $48495050 by repeating: a = s; b = 3677*s (32-bit); a = s + $E90 with its low word
replaced by (low word << 4) & $FFFF; s = ((b + a) & $7FFFFFFF) - 1 (32-bit); output byte (s >> 8) & 255.
(The reference keeps noise immediately before the pulse, but only relative layout matters.)

### 9.1 Song end

Signalled by the all-tracks-stopped check, pattern end on the last step, track command 0, and a
non-positive loop count (6.1). The host stops a few ticks after the signal; writes continue until then.

### 9.2 Timing

Clock: host cc = floor(cumulative CPU cycles / 2), each instruction's cost counted before its accesses.
Ticks: the player's interrupt entry is exactly 70940 cc apart.

Beam: VHPOS = line << 8 | (cc mod 227). The beam wait reads the line, polls until it changes, then
polls until the horizontal position is >= $16.

Measured cadences (CPU cycles unless cc): line poll every 21 cycles (the first loop read 12 cycles after
the first read); horizontal poll every 25 cycles; INTREQR poll every 29 cycles.

Measured gaps (cc):

* restart entry -> first line read 21 (10.5); last line read -> first horizontal read 12/13;
  last horizontal read -> AUDxPER=1 12.5 (25 cycles); PER -> INTREQ 26; INTREQ -> DMACON 6;
* not running: DMACON -> LC 20/21; running: LC on the 29-cycle INTREQR grid after DMACON;
* LC -> LEN 6; one-shot LEN -> next first line read: channel 0: 64/65, 1: 80, 2: 95/96, 3: 99/100;
* last horizontal read -> INTREQ (start) 36/37; DMACON start -> next first line read 180 (165/166 when
  the audio bit is already up); last horizontal read -> loop LC 10/11; loop LC -> LEN 6;
* conversion entry -> channel 0 VOL 303 when it needs no restart; entry -> restart entry 166/180;
  VOL -> PER 12;
* channel 0 PER -> channel 3 VOL 404 (tone held) / 439-440; channel 3 PER -> channel 2 VOL
  421/422/457; loop LEN -> following VOL: ch0 162/181, ch3 222/241/249, ch2 205/224/232;
* digital: loop LEN -> PER 20; PER -> VOL 239; ch2 PER -> digital restart entry 144/169/170;
  ch3 PER -> ch2 restart entry 241/242/255/264; ch0 PER -> ch3 restart entry 206/207/220/229.

The INTREQR exits depend on the Paula channel's period counter: an implementation must poll its own
Paula model. Observed: with DMA off the bit appears at the next state-3 handler; when DMA starts from
state 0 it appears at the next line boundary.

### 9.3 Per-tick CPU cost before the conversion step

CPU cycles from interrupt entry to conversion entry = sum of these event costs (cc = floor(sum/2)).
Fitted on 28,693 ticks of 200 synthetic modules; with these rounded values 98.1% of ticks are exact and
99.1% within 1 cc (max 5 cc).

* Per tick: base 1972. First active track index k (song-end check): k = 1: 33, 2: 66, 5: 165, 6: 198
  (about 33 per track), all stopped: 131; each song-end signal +164.
* Per field: macro waiting 47; macro executing 178 + per step below; first tick after a note -62.
  Vibrato 111 (+100 when the offset is non-zero, +87 when written, +56 at a direction flip).
  Glide active 25, plus on a glide step: equal 206, up 317, up end 335, down 307, down end 335 (or 326
  when it lands exactly). Envelope: counting 47, up 123, up end 169, down 142, down end 178, negative 157.
  Riff: counting 61 (echo mode: 168, echo applied 373); step 395 (+96 on wrap, +86 with detune, -138
  when a glide holds it, +16 when the trigger flag is cleared). Fader: fading 25, fade step 81, fade
  end 40, volume scaling 121, sample flag set -71, volume bit 4 clamp 8.
* Macro steps: $04 79, $05 set 226 / jump 233 / exit 229, $06 323, $07 24, $08 195, $09 197, $1F 205,
  note-period extras: detune 86, register write 87; $0A 271, $0B 288 (from base) / 268, $0C 300 / 284
  (glide active), $0D 258 (479 with $FE), $0E 226 (447 with $FE), $0F 261, $10 set 249 / jump 256 /
  exit 252 / no key 204, $14 set 72 / hold 79 / exit 252 / no key 204, $15 371, $16 239, $17 211,
  $1B 670 + one riff and fader pass, $1C 216 / jump 220, $1D 216 / jump 220, $1E 211, $20 245,
  $21 661 + dispatch, $30 set 228 / jump 206, $31 615 + dispatch, $32 237, $33 237, $40 255,
  $41 329 (+10 tone on), $42 267, $43 329 (+86 detune, +46 write), $44 219, $45 283, $47 251, $48 473,
  $49 215, $4A 227, other < $50 181, >= $50 0 beyond the executing cost.
* Note dispatch: normal note 206 (+16 new macro), portamento note 116 (+20 when no glide was active),
  vibrato 27, envelope 0, key off -44.
* Sequencer: row tick 307 (includes track 0); each of tracks 1-7 visited 113; row wait 36.
  Rows: note 675, portamento note 633, wait note 731 (each + dispatch; muted -392 instead).
  Commands: $F1 set 206 / exit 209 / jump 213, $F3 191, $F4 171, $F5 628, $F6 600, $F7 600, $FC 600
  (each + dispatch; muted -392), $FA fade: busy 192, immediate 339, equal 368, up 337, down 363,
  $FB 367, $FD 225, $FF 151; channel off 166.
* Track step read: each of tracks 1-7 with a new pattern 225, kept 121; speed command 206; loop new 241,
  loop again 230; fade command: busy 194, equal 371, up 340, down 366.

Not covered by the fit (absent from the fitted set): rows $F2, $F8, $F9, $FE; track command 0 and the
loop-end case; channel off with a channel byte >= 6. Ticks after the song has run past its last step
(reading unrelated data as track steps) were excluded.
