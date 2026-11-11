# Face The Music: how the Eagleplayer plays the eight tracks on Paula

What this file describes was measured on runs of the Eagleplayer "Face The Music"
(Eagleplayer 2.06) in the emulated Amiga: its register writes with their times
(`DELI_WRITE_LOG`), the words Paula fetched (`DELI_DMA_LOG`) and the module's sample
bytes. Nothing here comes from the player's code. The song itself (tracks, effects,
scripts, LFOs) is NostalgicPlayer's reading of the format; only what the runs show to
differ from it is listed here. Times are in colour clocks.

## Tracks and channels

Track `t` plays on Paula channel `t / 2`: tracks 0 and 1 on channel 0 (left), 2 and 3
on channel 1 (right), 4 and 5 on channel 2 (right), 6 and 7 on channel 3 (left).

Each channel has two buffers of 200 bytes (100 words) and plays them in turn; one more
buffer of 200 bytes stays empty and is what the channels play before the first mixed
buffer. In the runs the buffers lie in one block: channel `c`'s at `200 * c` and
`800 + 200 * c`, the empty one at 1624.

## A buffer's period, volume and bytes

When a buffer of channel `c` is mixed, with tracks `a = 2c` and `b = 2c + 1`, each with
the note (index of the period table, eighths of a semitone) and the volume the song gave
it at the last tick:

- the channel's period is the period of the higher note, without detune; its volume is
  the higher of the two track volumes, times the global volume, over 64. A track that
  plays nothing (its sample ended, or it never started) counts with its note and volume
  all the same;
- the track with the lower volume is scaled: each byte times `k`, shifted right 6
  (arithmetic: towards minus infinity), with `k = 64 * v / V` rounded down, `v` and `V`
  the two track volumes before the global volume. Measured: 8, 26, 35, 46, 55 for `v` =
  7, 21, 28, 36, 43 under `V` = 50, and 7 for 7 under 64 (thresholds 10, 19, 28, 37, 46,
  55 of the sample byte for results 1 to 6);
- the track with the lower note is stepped; with the same note, the even track. Its step
  is `2 ^ (-d / 96)` in 16 bits rounded up (`d` the notes' distance in table entries),
  less the pair's detune value as it is. Measured steps: `0xF1A2`, `0xE412`, `0xD745`,
  `0xCB30`, `0xBFC9`, `0xB505`, `0xAADD`, `0xA146`, `0x9838`, `0x8FAD`, `0x879D`,
  `0x78D1`, `0x6BA3`, `0x47D7` for `d` = 8, 16, 24, 32, 40, 48, 56, 64, 72, 80, 88, 104,
  120, 176 (rounding up shows at 56, 64, 88, 120, 176), `0x10000` for 0 and so `0xFE80`
  under the detune 384;
- the other track gives a byte of its sample for each byte of the buffer;
- the two are added (and held within -128 to 127; in the runs of a module whose samples
  overflow the bytes are the same as with that).

### The stepped track

Byte `n` of a stepped track (from where it started, or from where it became the stepped
one) is byte `ceil((n + 1) * step) - 1` of its sample: it falls behind by `1 - step` a
byte, and each time that makes a whole byte the byte before is given once more. Seen on
the places of the repeated bytes over more than 1000 buffers (step `0xFE80`: three
repeats in 512 bytes: the 171st, the 342nd and the 512th).

What it has fallen behind (the fraction) is kept from buffer to buffer and when the step
changes; a track that has been at the channel's rate starts again from nothing behind.

### The end of a sample

A track's bytes left are counted at the start of a buffer, from the byte it plays next
to the end of the sample (or of the loop), and that many bytes of the buffer are taken
from it, whatever its step. Then a sample with a loop goes on at the loop's start, one
without gives zeros. A stepped track so goes to its loop, or stops, before it has reached
the end of its sample (it has only passed `step` times the count), and a loop shorter
than a buffer is counted in the buffer's bytes too.

## The interrupts

All register writes of the song come from the audio interrupt handlers; the timer
interrupt (CIA timer of the module, `(timer + 1) * 5` colour clocks) only runs the song
and writes no register.

The handler of channel `c`'s interrupt (the channel has started a buffer):

1. `INTREQ` = the channel's bit (acknowledge);
2. `AUDxPER` and `AUDxVOL` of all four channels, 0 to 3, each with the period and volume
   of the buffer mixed last for that channel (so a channel's new period and volume are
   written at the next interrupt of any channel after its buffer was mixed, which is
   while the buffer before it still plays);
3. `INTENA` = `$2000` (the timer interrupt off);
4. `AUDxLC` = the channel's other buffer;
5. that buffer is mixed (the channel's period and volume for step 2 are now this buffer's);
6. `INTENA` = `$A000`.

Times from the acknowledge: period and volume writes at 93, 208, 246, 373, 411, 550, 588,
721; `INTENA` 51, 66, 82 or 97 later (channel 0 to 3); `AUDxLC` 154 after that. The
acknowledge comes 18 after the interrupt; when a handler waited for another, 166, 151,
135 or 129 (channel 1, 2, 3, 0) after that one's last write.

Before any buffer is mixed the stored values are period 214, volume 64.

### The time of a mix

From the `AUDxLC` write to `INTENA $A000`, with "stepped track" the one of the lower note
when its step is not 1, and `p` the sample bytes it passes in the buffer (200 times the
step, rounded down):

| what | time |
| --- | --- |
| both tracks play, same note, same volume | 3347 |
| the stepped track | + 500 + 4 `p` |
| the stepped track scaled | + 6 `p` + 10 more |
| the lower-note track scaled, not stepped | + 1207 |
| the higher-note track scaled | + 1794 |
| a track that plays nothing | + 108 (even track), + 113 (odd) |
| a track whose sample ends in this buffer | + 172 (even), + 177 (odd) |
| a track that has not played yet (or has the quiet sample), not stepped | + 16 (even), + 21 (odd) in place of 108, 113 |
| both tracks at the end of the same sample, neither stepped nor scaled | - 59 |

The first six rows were fitted on 12699 mixes of three modules (8997 within 4, 12315
within 70), the last two on the mixes of silent pairs of 19 modules. Not fitted: a
track that ended exactly at a buffer's end (3343, then 3512, then 3384 a buffer where
the table gives 3696, 3509, 3509), and smaller differences (up to about 200) when both
tracks play nothing and one is stepped or scaled.

### The time of a tick

A handler whose interrupt came while a tick ran makes its acknowledge about 1100 after
the timer's interrupt (1057 to 1130), 1580 when the tick took a row, and about 335 more
for each track that row started a sample on (1917, 2266, 2648, 2901, 3534, 3554, 3913
for 1 to 7 tracks; medians of 3 to 40 ticks each). The port takes 1080, 1560 and 335 a
track.

## Start

The init routine: `INTENA $A000`, `INTREQ $0780`, `INTENA $8780`, `DMACON $000F`,
`ADKCON $00FF`, `AUDxDAT` 0 and `AUDxVOL` 0 for the four channels, then for each channel
`AUDxLC` = the empty buffer, `AUDxLEN` 100, `AUDxPER` 428, `AUDxVOL` 1. The start
routine: `AUDxVOL` 64 twice for each channel, the same four writes a channel with volume
64, the timer (started 10 before the next write), `DMACON $800F`. The first tick of the
song (row 0) is played before the start: the first buffers are mixed, at the interrupts
the start raises, before the timer's first interrupt, and they have row 0's notes.

The `AUDxDAT` writes of the init leave the four audio interrupts requested, so the four
handlers run one after the other when the start routine ends, 88 after the `DMACON`
write (first acknowledge 106 after it). Paula starts the channels at the first raster
line after the handlers' `AUDxLC` writes can count, and that depends on where the
`DMACON` write falls in the line (227 clocks):

- a line starts within the 88 clocks after the write (9 of 20 runs: the write 141 to 214
  clocks into the line): the channels start at that line, 13 to 86 after the write;
- otherwise (11 runs: the write 5 to 114 into the line) they start 315 after the write,
  and channel 0's interrupt is raised once more (its handler runs a second time about
  20000 after the first).

Which of the two a module gets depends on the time the original's init takes, to the
clock (about 23 clocks a track line and more by the samples; a fit on the modules' sizes
leaves several hundred clocks). The port always takes the first, with the write 150
clocks into the line. The two differ in how the four channels' buffers lie against each
other for the whole song, which is heard as a different sum when the same sample plays
on two channels.

A pair's mix before the timer's first interrupt takes the time of a pair without detune
(3343 where the same pair, silent, takes 4680 from the first tick on).

## The volume effects

The track volume effects 1 to 10 give 0, 7, 0, 21, 28, 36, 43, 50, 56, 64: the third is
silent and the ninth 56, where the even steps would be 14 and 57. Seen on the channel
volumes of 20 modules under the global volume 63: 6, 0, 20, 27, 35, 42, 49, 55, 63.
Volumes that scripts and slides pass through (13, 56) are played as they are.

## Open

- The count of the bytes left of a stepped track is, in the runs, 0 to 2 less than "from
  the byte played next" gives (25 ends looked at: 12 one less, 7 the same, 6 two less),
  and a track that goes from stepped to the channel's rate goes on from the byte played
  next (26 of 42 changes), the byte before (13) or the one before that (3). What decides
  is not found; the port takes the count from the byte after the one played last and goes
  on from the byte played next.
- The times above are fits. Where an interrupt and a tick, or two interrupts, fall
  within what the fit is off, the port's tick comes on the other side of a mix and a note
  starts one buffer earlier or later than in the original (a buffer is 200 periods).
- The start's place in the raster line (see "Start").
- A muted track (bit clear in the module's mute byte): none of the modules looked at has
  one. The port lets it count as a silent track with volume 0.
- A detune below zero or above the step: none seen. The port holds the step at 0.
