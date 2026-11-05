# NTSP-system: reference logs

The files in `logs/` are register-write logs of the 68k reference (the UADE player "NTSP-system" running
the unmodified module on the emulated machine through the host with `DELI_WRITE_LOG`), one line per
Paula register write: address, value, colour clock. A 32-bit write to a pointer register appears as two
lines (high word, low word) with the same colour clock. Audio went to a WAV with the dummy audio driver. The
host renders in 0.1 s steps and stops at the end of the step in which the player signals the song end, so a
log ends shortly after the song end. The first line of a log is `# module BASE SIZE`.

The modules in `modules/` are synthetic: no real `SPNT` module exists in the available corpus (README).
`two.X` is the module of `logs/X.log`. Each is `SPNT`, a length `N`, then pseudo-random bytes (so that
every shift value, every nibble and every 5-bit value occurs), except `sine5400`, which is a real encoding of
a two-sine signal. In the modules named `sh*`, `sw*` and `n*` the header byte of each block has its shift
field forced as stated below.

| module | N | what it exercises |
|---|---|---|
| sine5400 | 5400 | three buffers, encoded signal |
| rand1800 | 1800 | N = one buffer: the first interrupt hits the end of stream (whole-buffer clear) |
| rand3600 | 3600 | two buffers |
| rand4000 | 4000 | partial last chunk of 400 bytes (44 blocks, 4 bytes unwritten) |
| rand9000 | 9000 | five buffers, end of stream on an exact multiple |
| rand9005 | 9005 | last chunk of 5 bytes (no block decoded) |
| big40p | 72 777 | 41 buffers, last chunk 777 bytes (86 blocks, 3 bytes unwritten) |
| sh0, sh7 | 5400 | every block has shift 0, resp. 7: the s = 0 cost |
| n0, n1, n5, n9, n18, n450, n1790, n1799 | 3600 + R (R = 0, 1, 5, 9, 18, 450, 1790, 1799) | last chunk of R bytes: partial-chunk cost B as a function of R; no block has shift 0 |
| loop7p | 11 001 | song end at the sixth interrupt and, in the same render step, the restart (full chunk from the first block) |
| sw0, sw9, sw12, sw111 | 3600 | the first 0, 9, 12, 111 blocks have shift 0, the others 1 to 7: shifts the time of the DMACON write by 2 cc per block, to see how the first interrupt entry follows it |

Other runs made (logs not shipped, regenerate from the recipe): shift-value sweeps with every block at the
same shift 0 to 7 (cost equal for 1 to 7), 40-buffer random module (the interrupt spacing), and modules
with N = 0, 14, 15, 16 and a changed magic (refused / accepted as described in SPEC 1.1).

## What they confirm

- Every (register, value) pair, in the order of the log, equals what SPEC sections 4 and 5 produce, for all 22
  logs, with the reference's own interrupt entry times as input.
- The tick-relative time of every write (time minus the interrupt's first write) equals the SPEC 6 model
  within 2 cc (a jitter of 0 or 2 cc is seen between runs with the same chunk); the start-up writes follow
  the section 4 table exactly.
- The audio DMA words of all four channels equal the model's buffers, including the zero tail, the unwritten
  bytes of a tail that is not a multiple of 9 (they equal what the buffer held before) and the restart from
  the beginning after the song end (9 modules, DMA logs not shipped).
- Interrupt spacing: 289 800 cc from the third entry on; 289 705 between the first and the second.

## Typical numbers (cc)

| item | value |
|---|---|
| first LC write (no s = 0 block in the first chunk) | 51 749 |
| first LC write to DMACON write | 211 |
| DMACON write to first interrupt entry | 91, 145, 266, 294 (see SPEC 5) |
| entry to INTREQ write | 8 |
| INTREQ write to first LC write, full chunk | 51 202 (+2 per s = 0 block) |
| INTREQ write to first LC write, nothing to decode | 20 011 |
| first LC write to closing INTENA | 40 |
| gaps inside the start-up (LC pairs / LEN / PER / VOL) | 8 / 6 / 6 / 6 (an extra 2 before the third VOL, 18 from last LC to first LEN, 10 from last LEN to first PER, 8 from last PER to first VOL) |
