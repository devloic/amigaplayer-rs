# Face The Music: how the Eagleplayer plays the eight tracks on Paula

What this file describes was measured on runs of the Eagleplayer "Face The Music"
(Eagleplayer 2.06) in the emulated Amiga: its register writes with their times
(`DELI_WRITE_LOG`), the words Paula fetched (`DELI_DMA_LOG`) and the module's sample
bytes. Nothing here comes from the player's code. The song itself (tracks, effects,
scripts, LFOs) is NostalgicPlayer's reading of the format; only what the runs show to
differ from it is listed here.

## Tracks and channels

Track `t` plays on Paula channel `t / 2`: tracks 0 and 1 on channel 0 (left), 2 and 3
on channel 1 (right), 4 and 5 on channel 2 (right), 6 and 7 on channel 3 (left).

Each channel has two buffers of 200 bytes (100 words) and plays them in turn; one more
buffer of 200 bytes stays empty and is what the channels play before the first mixed
buffer. In the runs the buffers lie in one block: channel `c`'s at `200 * c` and
`800 + 200 * c`, the empty one at 1624.

## A buffer's period, volume and bytes

When a buffer of channel `c` is mixed, with tracks `a = 2c` and `b = 2c + 1`, each with
the period and volume the song gave it at the last tick (volume after the global volume,
0 to 64):

- the channel's period is the shorter of the two periods, its volume the higher of the
  two volumes. A track that plays nothing (its sample ended, or it never started) counts
  with its period and volume all the same;
- a track whose volume is the channel's is taken as it is; the other is scaled: each byte
  times `k`, shifted right 6 (arithmetic: towards minus infinity), with
  `k = (64 * v + V / 2) / V` (`v` the track's volume, `V` the channel's). Measured `k`
  under `V` = 49: 8, 26, 35, 46, 55 for `v` = 6, 20, 27, 35, 42;
- a track whose period is the channel's gives a byte for each byte of the buffer; the
  other steps on by `P / p` bytes a byte (`P` the channel's period, `p` the track's), the
  byte under the position is taken (no interpolation);
- the two are added.

A sample without loop that reaches its end gives zeros from there; one with a loop goes
on at the loop's start.

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

Times from the acknowledge, in colour clocks: period and volume writes at 93, 208, 246,
373, 411, 550, 588, 721; `INTENA` 51, 66, 82 or 97 later (channel 0 to 3); `AUDxLC` 154
after that. The mixing takes 3347 when both tracks play at the channel's period and
volume, and more otherwise (see "Open").

Before any buffer is mixed the stored values are period 214, volume 64.

## Start

The init routine: `INTENA $A000`, `INTREQ $0780`, `INTENA $8780`, `DMACON $000F`,
`ADKCON $00FF`, `AUDxDAT` 0 and `AUDxVOL` 0 for the four channels, then for each channel
`AUDxLC` = the empty buffer, `AUDxLEN` 100, `AUDxPER` 428, `AUDxVOL` 1. The start
routine: `AUDxVOL` 64 twice for each channel, the same four writes a channel with volume
64, the timer, `DMACON $800F`. The first tick of the song (row 0) is played before the
start: the first buffers are mixed, at the interrupts `DMACON` raises, before the timer's
first interrupt, and they have row 0's notes.

## The volume effects

The track volume effects 1 to 10 give 0, 7, 0, 21, 28, 36, 43, 50, 56, 64: the third is
silent and the ninth 56, where the even steps would be 14 and 57. Seen on the channel
volumes of 20 modules under the global volume 63: 6, 0, 20, 27, 35, 42, 49, 55, 63.
Volumes that scripts and slides pass through (13, 56) are played as they are.

## Open

- The time a mix takes depends on what the two tracks do (stepped, scaled, ended, loop);
  the port has one time for all, so its interrupts drift against the original's by some
  hundred colour clocks and a tick can fall on the other side of a mix: a note then
  starts one buffer earlier or later than in the original.
- The time a tick takes is not known (it writes nothing).
- A muted track (bit clear in the module's mute byte): none of the modules looked at has
  one. The port lets it count as a silent track with volume 0.
