# Darius Zendeh sound system (old Mark II) -- behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the Eagleplayer "Darius Zendeh" (UADE player name DariusZendeh, prefixes `dz`, `mkiio`). The
newer Mark II driver (prefixes `mk2`, `mkii`) is a different driver and is NOT covered; see README.

A module of this family is a plain 68k program image (no hunk header) that carries its own tiny driver
together with its data. The driver is NOT to be executed by the port: the port reads the data tables
of the module and does what the driver does (this spec). Everything below was checked bit-exactly
against the reference 68k player on three modules (one of each variant, section 9).

## 1. Identification

Prefix `dz` or `mkiio` (case-insensitive), then a content check on the first bytes. All values big endian.

Variant A (old, "katakis"): first long `48E778F0`, word at offset 4 `41FA`, long at offset 8
`4CD80600`, long at offset 12 `0C0000FF`.

Variants B and C: first long `48E700F0`, long at offset 8 `4CD80600`, and the 12-byte string
`70 00 33 FC 00 0F 00 DF F0 96 41 FA` occurs at some even offset not above 0x240 (the scan is done
from offset 0 in word steps and gives up after 289 positions). In addition the word at offset 12 must
be `4A44`, `4A00` or `0C00`; `4A44` is the player's "module with sub-songs" kind, the other two are
single-song. (All three real B/C modules seen have `4A00`.)

B versus C is not signalled by the header: C is the variant whose row routine contains the pitch-slide
feature, recognisable by the 32-bit value `08830007` appearing within 0x60 bytes after the record
size marker (section 2). B and A do not have it.

The module has no title, author or comment text. The title shown is the file name. Exactly one
sub-song (number 0) exists for variants A, B and C as seen. (The "4A44" kind advertises sub-songs; no
module of that kind was available, so its sub-song handling is unspecified.)

The module file is loaded as is, at the load address the host chooses ("module base"); the
reference runs used 0x0045E0. All sample pointers written to Paula are absolute: module base + offset.

## 2. Locating the tables

Offsets below are offsets inside the module file; "address constant" means a 16-bit word `41FA`
(or `43FA` / `45FA` where said) followed by a signed 16-bit displacement; its target is
(offset of the displacement word) + displacement.

1. Record-size marker: the first even offset >= 0x10 holding the word `C0FC`. The word after it is the
   record size RS (0x62 for A and B, 0x82 for C).
2. Immediately after the marker, the first word equal to `E742` (B, C) or `E542` (A) selects the sample
   table entry size: 8 bytes (B, C) or 4 bytes (A).
3. From just after the marker scan forward for the first three address constants `41FA` in order:
   (a) the sample table, (b) the sample data base, (c) the period table.
4. The record table: scan backwards from the marker for the nearest word `45FA` and take its target.
5. The song (row) table: variant A: scan backwards from the marker for the nearest word `43FA`;
   variants B and C: find the first even offset >= 0x10 where `0C00 00FF` occurs (the module's entry
   routine tests its command for the value FF there); the first `41FA` address constant after it is the song table.

For the three modules: A katakis: RS 0x62, sample table 0x1188, data base 0x1200, period table
0x11AC, record table 0x732, song table 0x170. B crashburn: RS 0x62, 0x323A, 0x3326, 0x32D2, 0xE3E,
0x6AC. C threatduel: RS 0x82, 0x1654, 0x16E2, 0x168C, 0x40C, 0x34A.

## 3. Data structures

Song table: rows of 8 bytes; the row is four (record number, note) byte pairs, channel 0 first
(channel c = Paula channel c = register block 0xDFF0A0 + 0x10*c). The table ends at the first row whose
first byte is 0xFF.

Record table: record r starts at record-table + r*RS. A record is a list of 4-byte entries, entry k at
+4k: byte 0 sample number, byte 1 base period index, byte 2 volume, byte 3 flags. The list of record 0
is terminated by an entry whose first byte is 0xFF; its number of entries is the number of steps per
song row (24 for the A and B modules, 32 for C; all records are supposed to have at least as many
entries and only record 0 is tested for the end).

Sample table: entry s. Variant A (4 bytes): unsigned word offset, word length. Variants B and C (8
bytes): long offset at +0, word length at +6 (+4 unused). The offset is relative to the data base
(2.(c)); the length is in words and goes to Paula unchanged. Sample 0 is a tiny silent sample (length 1).

Period table: 16-bit words, indexed by (base index + note). 42 entries, from 1440 downwards (B and C
and A all use the same series: 1440, 1356, 1280, 1208, 1140, 1076, 1016, 960, 906, 856, 808, 762,
720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240,
226, 214, 202, 190, 180, 170, 160, 151, 143, 135 in the B module; read it from the module).

## 4. Replay model

State: song row index, step k, a phase flag, a per-channel "previous period" (C only), a fade amount
(word, 0 at start, only changed by external commands, section 7).

Time: the driver is called by a 50 Hz timer (70 940 colour clocks at PAL; no tempo or speed commands
exist). Calls alternate between two kinds:

- Gate tick: write DMACON with the mask built by the previous row tick (bit c set for each channel
  that was marked "retrigger"), no other write. The value is written even when zero. A write with
  bit 15 clear stops those channels' DMA.
- Row tick: advance to the next step and write all four channels, then DMACON.

The first tick after start is a gate tick (it uses the mask from the start-up row, section 5), then
row, gate, row, ...

Advance: k = k+1. If the entry k of record 0 has first byte 0xFF: k = 0 and the song row advances by
one; if the new row's first byte is 0xFF the song restarts at row 0 (endless loop, no song-end event).

Per channel c = 0..3 in order, with (rec, note) from the song row, entry e = record rec, step k, fields
(sample s, base, vol, flag):

1. Sample (offset, length) from the sample table entry s. pointer = module base + data base + offset.
2. Period p:
   - A and B: p = period table[base + note].
   - C: if base has bit 7 clear: p = period table[base + note]. If bit 7 is set, the table is not used:
     amount = base & 0x7F, p = previous period of this channel + amount if flag bit 7 is set, else
     previous period - amount (16-bit wrap). In C "previous period" is the period last written to this
     channel (any row, slide or not), 0 at start and kept across song loops.
3. Volume v = vol - fade if vol > fade, else 1 (so volume 0 plays at volume 1).
4. Retrigger: A and B: if flag == 0; C: if flag bit 0 is 0 -> add bit (1 << c) to the DMACON mask.
   (Flag non-zero means "tie": the channel is not stopped, the new pointer/length become effective when
   its current loop ends. Flag bit 7 in C is only the slide direction.)
5. Write to Paula in this order: AUDxLEN = length; AUDxLCH = high 16 bits of pointer; AUDxLCL = low 16
   bits (the two address halves are written by one long write, so they appear with the same colour
   clock); AUDxPER = p; AUDxVOL = v.

After channel 3: write DMACON = 0x820F (set, channels 0-3 on). Then the row tick ends; the mask collected in
step 4 is written on the next (gate) tick.

Sample playback after DMA start is by Paula; loops use the latched pointer/length, so a sample with
length 1 is the "silence".

## 5. Start-up

Variants B and C, in order: DMACON = 0x000F; the row at step 0 of song row 0 is written as in a row tick (nothing
is advanced); DMACON = 0x820F; AUDxVOL = 0 for channels 0, 1, 2, 3 in that order; DMACON = 0x000F.
The mask from the row at step 0 is kept for the first gate tick. (Hence the start row is silent: DMA is
off and volume is 0; the first audible row is step 1.)

Variant A: the row at step 0 is written, then DMACON = 0x820F; no volume zeroing and no stop. Row 0
sounds until the first gate tick (20 ms later) stops its marked channels.

Timer: the first driver call after start-up occurs one timer period (70 940 colour clocks) after the
end of the start-up call; later calls follow every 70 940.

## 6. Timing (colour clocks)

All times from the start of the timer call ("entry"), measured in the reference.

- Gate tick: the DMACON write is at +253 (B, C) or +238 (A).
- Row tick, first write (AUD0LEN): B, C +366, A +341; add +22 (B, C) or +19 (A) when this tick moves to
  a new song row (the step counter wrapped); add a further +61 (C, seen once) when the song restarts.
- Gaps after each write in the row tick (cck), variant B (A in brackets, C in braces):
  LEN -> LCH/LCL 23 (21) {23}; LCH and LCL are simultaneous; LCL -> PER 21 (21) {60.5; 98 for a
  downward slide, 100 for an upward slide}; PER -> VOL 36 (37) {40.5} plus 7 when vol is 0;
  VOL -> next channel LEN 118 (116) {118.5} plus 6 when the channel was marked retrigger;
  VOL of channel 3 -> DMACON 58 (53) {58.5} plus 6 when channel 3 was marked retrigger.
- Jitter of +-1 colour clock (A, B) or about +-2 (C: the two values of each gap come in about equal
  proportion; the cause, DMA-slot alignment of the 68k bus accesses, is not modelled) is seen around
  these values.
- Start-up call (B, C), counted from the start of the call: first DMACON write at +167 (B) / +177 (C);
  then the row writes with the same gaps as above but the first LEN at +212 after that write;
  DMACON 0x820F follows the last VOL by the usual last gap (+58); then +101 to the first volume
  clear, +6, +6, +6 for the next three, +8 to the final DMACON; the call ends 18 later. A: first LEN
  at +342 from the call start, the row as above, DMACON 0x820F; the call ends 164 later.
- The call start itself depends on the host's detection scan of B and C modules: it begins about
  203 + 23.5 * (offset of the 12-byte string of section 1 / 2) colour clocks after detection starts
  (log clock 100); the call starts 71 later (A: 58 after a detection of 110). This is host overhead:
  call starts were 6954 (B, string at 0x230), 4016 (C, string at 0x136), 268 (A).

## 7. Commands not used by the player

The driver also reacts to command values in its entry register: FF = start, 1 and 2 = fade (set the
fade amount to 0x3F or step it up by 2 per call until 0x3E, then stop DMA), 0 with a non-zero
sub-song selector and sound-effect triggers for channels 1 and 2 (B). The Eagleplayer uses
the fade for its "end" handling only; no module ends by itself. They were not exercised; do not
implement them for the first port.

## 8. Song length

Length of one pass: rows * steps * 2 / 50 seconds (A katakis 184 rows x 24 steps = 176.64 s;
B crashburn 242 x 24 = 232.32 s; C threatduel 24 x 32 = 30.72 s). The song loops forever; the player
reports no end.

## 9. Validation

A private model built from sections 1-6 reproduced the reference write logs (values and order of
every custom-chip write): crashburn all 132 006 writes of the first 240 s (whole pass), katakis 110 000
writes of 200 s, threatduel the first 2 900 ticks including one song restart; 0 mismatches. Timing
model (section 6) matches the write clocks within 1 cc on 15 379 writes for A and B, within 2.5 cc for C.
