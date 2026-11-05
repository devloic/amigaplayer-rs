# onEscapee (Akos Divianszky, 1997) - behaviour specification

*Sanitised for publication: behaviour only.*

Format: a raw 8-bit mono sample stream, obscured by an exclusive-or with a fixed 32-bit key, played on
all four Paula channels at one fixed period from two small alternating buffers. There is no song, no
pattern data, no tempo: the whole module is one sample. The Eagleplayer is called `onEscapee`, file name
prefix `one.` (UADE eagleplayer.conf: `prefixes=one`). Creator string of the player: "(c) 1997 by Akos
Divianszky, adapted by Wanted Team".

All times below are Amiga colour clocks (cc, 3.546895 MHz PAL, 227.5 cc per raster line). Register
addresses are offsets from the custom chip base (e.g. 0xA0 is AUD0LCH).

## 1. Module file and detection

The module is the whole file. There is no header. Let `L` be the file size with its two low bits cleared
(a size that is not a multiple of 4 loses its last 1 to 3 bytes; they are never played).

### 1.1 Decoding

The file is read as big-endian 32-bit words. Every word is exclusive-or'ed with the key 0xAA55FF00, i.e.
byte 0 of each group of four is XOR 0xAA, byte 1 XOR 0x55, byte 2 XOR 0xFF, byte 3 XOR 0x00. The result
is signed 8-bit PCM, played in file order. (A file whose decoded bytes are all 0 is therefore a file of
repeated 0xAA55FF00; a decoded 0xFF fill, which is -1 per sample, is a file of 0x55AA00FF.)

### 1.2 Detection rule (the player's own, content based)

Only the first 100 bytes (25 big-endian words, W0..W24) are examined; the file name is not used (the
prefix is only a UADE-side hint; a file named `xyz.dat` with the right content is recognised).

* Walk W0, W1, ... while the word equals 0xAA55FF00.
* If W0..W23 all equal 0xAA55FF00, the module is accepted (W24 is not examined).
* Otherwise let `j` (0..23) be the index of the first word that is not 0xAA55FF00. That word `j` is
  skipped without any test (any value is accepted there), and each of W(j+1) .. W24 must equal 0x55AA00FF.
  If they all do, the module is accepted, otherwise it is rejected.

So accepted heads are: 24 words 0xAA55FF00; or `j` words 0xAA55FF00, one arbitrary word, then
(24 - j) words 0x55AA00FF (for j = 0: any word followed by 24 x 0x55AA00FF). The tests run in the
reference (see LOGS) confirmed: 24 x AA55FF00 accepted; 5 x AA55FF00 + arbitrary + 19 x 55AA00FF
accepted; arbitrary + 24 x 55AA00FF accepted; 25 x 55AA00FF accepted (the first word counts as the
skipped one); 23 x AA55FF00 + arbitrary + 0 rejected; 23 x AA55FF00 + arbitrary + 55AA00FF accepted;
24 x 55AA00FF + 0 rejected (only 23 of the 24 following words match); 5 x AA55FF00 + arbitrary +
18 x 55AA00FF + 0 rejected. A file shorter than 100 bytes reads past its end in the original; not
tested.

The comment in the original says the 55AA00FF alternative exists for a song called "mzeperx".

### 1.3 Song, subsong, title, length

* One song, one subsong (subsongs 0..0). No title text, no author; the player's info page shows the
  file size (load size and samples size are both the file size), the duration and the prefix.
* Duration in seconds, shown by the player: floor( floor(222 * L / 64) / 55420 ) + 1 (55420 is
  (3546895 - 15) / 64). This is the time to play L bytes at period 222 with Paula's real clock, plus one
  second. The host's own duration hook returns (L >> 10) * 45466, in units of 1/709379 s (about 64.1 ms
  per 1024-byte buffer); nothing in the replay depends on either figure.
* Real song length: L bytes at period 222 = L * 222 cc, plus the zero-padded tail described in 3.

## 2. Initial state (the init sequence)

1. Chip RAM holds two buffers of 1024 bytes each, B0 and B1 = B0 + 0x400, contiguous (the original keeps
   them in its own chip section; their absolute address is the host's choice, any word-aligned chip
   address works, the reference logs show B0 = 0x646D58).
2. The first 1024 bytes of the module are decoded into B0. The decoder keeps a read position and a
   remaining-length counter (initial remaining = L); the counter is decreased by 1024 here and the read
   position advances by 1024.
3. The "buffer to decode into" and "buffer to play next" are then swapped (see 3), and all four channels
   get their location registers set to B0 (decoded buffer).
4. Then, for all four channels: length = 0x200 words (1024 bytes), period = 0xDE (222), volume 64 on
   channels 0 and 1, volume 1 on channels 2 and 3. (So the sound is mono-ish: channel 0 (left) and
   channel 1 (right) at full volume carry the stereo signal; channels 2 and 3 are at 1/64.)
5. Start: the audio interrupt-0 level is enabled (INTENA set 0x8080: master + audio 0), then DMA is
   enabled for all four audio channels (DMACON set 0x800F). No other register is touched; no filter
   (LED) writes.

The init order of register writes with timing relative to the DMA enable write `E` (cc, taken from the
reference logs; in the reference E = 7626 from the host's start phase):

| write | time relative to E |
|---|---|
| LC (hi, lo, same cc) ch0 = B0 | -211 |
| LC ch1 | -203 |
| LC ch2 | -195 |
| LC ch3 | -187 |
| LEN 0x0200: ch0, ch1, ch2, ch3 | -169, -163, -157, -151 |
| PER 0x00DE: ch0, ch1, ch2, ch3 | -141, -135, -129, -123 |
| VOL 0x0040 ch0, 0x0040 ch1, 0x0001 ch2, 0x0001 ch3 | -115, -109, -101, -95 |
| INTENA 0x8080 | -10 |
| DMACON 0x800F | 0 |

Each channel's LC is one long write: the high word at AUDxLCH and the low word at AUDxLCL, both logged
with the same colour clock. The register order inside a channel in the logs is LCH then LCL
(addresses 0xA0, 0xA2; 0xB0, 0xB2; 0xC0, 0xC2; 0xD0, 0xD2). LEN/PER/VOL are at +4, +6, +8 from the
channel base (0xA4, 0xA6, 0xA8; channel stride 0x10).

## 3. Replay: one interrupt per buffer

The whole replay is driven by the audio interrupt of channel 0 (level-4 audio 0, INTENA bit 7). There
is no timer and no vertical-blank work.

### 3.1 The sample stream seen by Paula

The four channels play the same two buffers at period 222: one byte per 222 cc, one 16-bit DMA word per
444 cc on average (the reference's Paula fetches a word every 454 cc or 227 cc, averaging 444), a buffer
of 1024 bytes lasts 1024 * 222 = 227 328 cc (about 64.1 ms). All four channels always receive the same
location pointers, so they play the same bytes; at the reference's output the combined signal is
channel 0 + channel 1 at full volume plus channels 2 and 3 at volume 1.

### 3.2 Interrupt timing (as the reference Paula produces it)

After the DMA enable write at `E` the audio-0 interrupt is requested at

* interrupt 0: E + 160 (this is the usual "DMA just started, length counter loaded" request);
* interrupt n >= 1: E + 227 271 + (n - 1) * 227 328.

(The first buffer fetch happens at E + 92, the restart of the channel at the end of the first buffer
about 50 cc after interrupt 1; this is Paula's behaviour, a Paula model with period 222 and length 512
words reproduces it.) The times given are those of the handler's first logged write, the INTENA clear
of 3.3.

### 3.3 What an interrupt does

In order:

1. Write INTENA = 0x0080 (disable audio 0 interrupt) at the interrupt time T0, then INTREQ = 0x0080
   (acknowledge) at T0 + 8.
2. Fill the buffer that is NOT currently being played (the one the previous interrupt did not point the
   channels at - see the swap below) with the next 1024 bytes of decoded data, as described in 3.4.
3. Swap the two buffers' roles, and write the new "next to play" buffer address into the location
   registers of all four channels, in channel order 0, 1, 2, 3 (each channel: LCH then LCL at the same
   cc), 8 cc apart:
   `t, t+8, t+16, t+24` where `t` is the cc of the first pointer write (see 3.5).
4. Write INTENA = 0x8080 (re-enable) at t + 40.

The buffer pointers written alternate between B1 and B0: init wrote B0, interrupt 0 writes B1,
interrupt 1 writes B0, and so on. The location registers are latched by Paula at the next buffer
restart, so a pointer written during interrupt n takes effect for the buffer that follows the one
playing during n (the decode finishes ~6600 cc after the interrupt, far before the restart at
227 328 cc).

Data order played: chunk 0 (init), chunk 1 (interrupt 0), chunk 2 (interrupt 1) ... where chunk k is
bytes k*1024 .. k*1024+1023 of the decoded module.

### 3.4 Filling a buffer, end of data, loop

State: `rem` = number of bytes still to be taken from the module (initially L - 1024 after init).
At each interrupt, the buffer to be filled holds `want` = 1024 bytes:

1. If `rem` is 0: the whole buffer is filled with zero bytes (silence), `rem` is reset to L and the
   read position to the start of the module, and the player reports the song end to the host (once).
2. Else if `rem` >= 1024: decode 1024 bytes, `rem` -= 1024.
3. Else (0 < `rem` < 1024; `rem` is a multiple of 4): decode the remaining `rem` bytes into the start
   of the buffer, fill the rest of the buffer (1024 - `rem` bytes) with zero bytes, reset `rem` to L and
   the read position to the start, and report the song end.

The song end is reported during the handler, before the new pointers are written. After it, the
next interrupt starts at the beginning of the module again: the song loops by itself, with the zero
padding of the last buffer (in 3.1) as the only gap (up to 1024 bytes of silence when L mod 1024 is
small, a whole buffer when L is an exact multiple of 1024 - in that case the reported song end happens
one interrupt after the last data buffer was set up). The reference host stops (or goes to the next
module) once it sees the song end; logs of runs that were allowed to continue show the loop (LOGS,
`one.r3072`, `one.r4096`).

Side effect to know: in the partial case (3) the original decodes a full 1024 bytes (reading up to
1020 bytes past the end of the module, which only costs time) and overwrites the tail with zeros
afterwards; the time cost is part of 3.5. Files shorter than 1024 bytes are not supported by the
original (the remaining-length counter underflows and the player decodes memory past the module
forever); synthetic modules should be at least 2048 bytes long.

### 3.5 Time cost of the handler (first pointer write after T0)

Let `d` = (cc of the first pointer write) - T0, the interrupt's first logged action. Measured, with the
reference's CPU timing (68000 at 7.09 MHz, chip RAM, audio DMA contention):

| case | d (cc) |
|---|---|
| init (before the DMA starts; no handler) | n/a |
| full 1024-byte decode, `rem` > 1024 before it | 6641 |
| full decode, `rem` == 1024 exactly before it (the last full buffer) | 6639 |
| partial (3): 0 < `rem` < 1024; Lc = (1024 - `rem`) / 4 longwords zero-filled | 6641 + 120 + 15 * Lc |
| rem == 0 (case 1): 256 longwords cleared | 224 + 15 * 256 = 4064 |

Every measured handler (11 reference runs, see LOGS) fits these numbers exactly. The decode cost per
longword is about 25 cc (1024 bytes = 256 longwords = about 6400 cc plus about 240 cc of call overhead);
the zero fill costs 15 cc per longword.

### 3.6 Pointer write pattern

After the first pointer write at `t`: pointer writes for channel k (k = 0..3) at `t + 8k`, each as two
writes (LCH, LCL) with the same colour clock; then INTENA 0x8080 at `t + 40`. The next event is the next
interrupt.

Sequence of register writes of one handler, e.g. with T0 = 234 897 and a full buffer:

    0x9A 0x0080 @234897    0x9C 0x0080 @234905
    0xA0, 0xA2 @241538     0xB0, 0xB2 @241546   0xC0, 0xC2 @241554   0xD0, 0xD2 @241562
    0x9A 0x8080 @241578

## 4. Summary of everything the player writes

Per module play: init writes (2), then per interrupt: INTENA/INTREQ (2) + 8 pointer words + INTENA (1).
There are no writes to AUDxLEN, PER or VOL after init, no DMACON writes after the enable, no filter
writes. The only other thing is the song end report to the host.

## 5. Not part of the format

When the host stops the player (end of play), the player disables DMA for the four audio channels
(DMACON 0x000F), sets the four volumes to 0 and masks/clears the audio-0 interrupt. This is a stop action
(known from the player's source; the reference run does not reach it), not part of the replay.
