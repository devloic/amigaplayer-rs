# ADPCM2 / ADPCM3 — spec folder

Behaviour spec of the Eagleplayer `ADPCM` (ADPCM2/ADPCM3 files) and of the standalone PlayADPCM:
SPEC.md (format, decoder, what the player needs from the host, register writes and timing), LOGS.md
(reference runs, measurements, validation), `logs/` (write and DMA logs, the synthetic modules and their
generator), `modules/` (links to the three real files, for local testing only), `data/` (the six step
multipliers). A private folder with the reading notes and validation scripts exists for the lead only.

## What the format is

A 10-byte header (`ADPCM2` or `ADPCM3`, then the sample rate) and one mono stream of 2-bit or 3-bit ZyXEL
voice-modem ADPCM. Not IMA ADPCM (the IMA reference code offered for comparison was not needed: there is no
IMA step table or index). Not the headerless ADPCM_mono format of the existing port either (that player
rejects `ADPC` files). Only three real files are known: Dancing Moon (ADPCM3, 16 726 Hz) and the two halves
of Para Mi Amiga (ADPCM2, 8 526 Hz). Both come with PlayADPCM; the Eagleplayer is a later wrapper around
the same decoding routines.

## Why the host is silent, and feasibility

The Eagleplayer works through the amplifier interface in direct mode: it pokes Paula channels 0 and 1 and
relies on the host to call its AudioStruct-0 interrupt at the start of every 1024-sample block (SPEC 3).
Eagleplayer 2.06 itself only runs such players with an amplifier engine loaded (it has no plain Paula path
for them); the engines call that interrupt when the voice wraps into its loop. This host takes the direct
path but never calls the interrupt, so the zeroed first buffer loops: silence.

Fully feasible, two ways (SPEC 6): (1) a small host change, calling the AudioStruct interrupt of direct-mode
amplifier players on channel 0's audio interrupt (with it, the unchanged 68k player plays all three files
limited to AudioStruct 0 / channel 0 as here or generalised per voice); (2) an open replayer from SPEC,
bit-exact on the sample stream and register-exact on the writes (validated, LOGS.md). No player-side data
given as values in SPEC 2).

## What a faithful log is here

On a real Amiga the Eagleplayer never wrote Paula: an amplifier engine mixed its voices (resampled to the
engine's mix rate). PlayADPCM's Paula writes are audio.device's. So no original "Paula write log" exists.
The logs here are the original 68k player's writes on this host once the host relays the interrupt: they
define one natural Paula rendering (period `floor(3 546 895 / rate)`, both channels, volume 64, 1024-sample
double buffering, one silent buffer at the start). What is fully original and validated is the decoded
sample stream (bit-exact against the 68k decoder's DMA output over the complete files).

## Unknowns and caveats

1. Amplifier flag bit 7 (flags 0x83): meaning unknown; no part of the engines read tests it.
2. The refill timing (117 cc after the request; refill 0 at the host's first service) is this host's; under
   Eagleplayer the timing is the amplifier engine's.
3. Overload: the 68k decode costs 145 000–177 000 cc per block on the host's 68000; files above about
   20–25 kHz underrun and repeat buffers in the reference (SPEC 4.4). Not seen with real files; a port need
   not reproduce it.
4. The ADPCM2 duration in the module info is wrong in the original (16-bit shift, SPEC 1); a port should
   show the correct duration.
5. Tail loss: the last 1–2 bytes of a stream that are not a whole 3-byte unit are never played by the
   Eagleplayer (also for ADPCM2). A partial ADPCM3 block of only 1–2 bytes still decodes one 3-byte group,
   reading past the end of the module, and those 8 samples are played: their value depends on the memory
   after the module (zeros in the host; the model assumes zeros).
6. Song end loops: the player restarts the stream itself; the host stops shortly after the report.
7. PlayADPCM differences (SPEC 5): no leading silence, no loop, 16 384-sample writes, state reset per
   file, optional filter switch. Para Mi Amiga's two parts are played one after the other by its script;
   the join is audible in the original too (the uploader notes it).
8. Output stage: the host's PC-format list matches `adpcm` and turns the Amiga filters off for this player;
   these files are Amiga productions played by Paula, so the A500 filter model would be the natural choice.
   Lead decision.
9. Rates of 0 or below 55 Hz break the period division (SPEC 4.1); not handled by the original.

## Errata found by the implementation

- Detection requires the 6-byte magic plus at least 10 header bytes; shorter files are not recognised.
- A rate of 0 (low 16 bits) is refused; an empty code stream plays silence and announces song end;
  the bytes the last ADPCM3 group reads past the end are zeros (confirmed against the logs).
- The port names the formats `ADPCM2` and `ADPCM3`, so the host's output stage treats them as PC formats
  (no Amiga filters); renaming without "adpcm" would select the A500 filter.
- The 68k decode cost is not modelled, so the original's overload repeat on high-rate files is not reproduced.
- The first refill's latency is a constant 2238 cc (the reference varies 2232..2243 with the raster phase).
