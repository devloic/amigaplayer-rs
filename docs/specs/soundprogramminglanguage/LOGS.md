# Logs and reproduction

The files in `logs/` are write logs of the reference 68k player (the Eagleplayer "Sound Programming
Language" of the UADE players directory run through the host with DELI_WRITE_LOG). Format: first line
`# module BASE SIZE` (address and size of the module file buffer in the reference's memory, not the load
address of the code hunk), then `ADDRESS VALUE COLOUR-CLOCK` per line (hex address, hex 16-bit value,
absolute colour clock from reset). Ticks are separated by gaps of more than 20 000 cc (the tools/replay-cmp.py
default); for the 17.7 Hz module the gaps inside a tick stay below that as well. The first group of every log
is the host start-up (SPEC section 7) followed by the module's own start writes.

## Files

| file | content |
|---|---|
| spl.Boulder_Dash.sub1.ref.log | family 1, 25 s, code load address 0x16188 |
| spl.Clever_and_Smart.sub1.ref.log | family 2, 25 s, load address 0x16750 (the stack overflow of SPEC 4.9 shows in these AUDxLEN values) |
| spl.Graffiti_Man.sub1..sub8.ref.log | family 2, songs 1..8, 25 s each, load address 0x15B50; songs 7 and 8 end by the stop command (last write at 3.2 s and 3.7 s) |
| spl.Hollywood_Poker.sub1.ref.log, sub2 | family 3, songs 1 and 2, 25 s, load address 0x15AD8 |
| synthetic.BoulderDash-envelope.sub1.ref.log | Boulder Dash with three instrument records patched (see below), 25 s |
| synthetic.CleverSmart-envelope.sub1.ref.log | Clever & Smart with three instrument records patched, 25 s |

`subN` is the 1-based subsong number the driver is called with (the command line option is 0-based).

Module files (from the module corpus; file name prefix `spl.`), sizes and md5 prefixes:
Boulder Dash 71 584 bytes (1dcda8e6f21b), Clever & Smart 73 060 (951de62a5cad), Graffiti Man 69 992
(afc653073290), Hollywood Poker 69 868 (cbf6108d5caf).

The load addresses are what the reference's loader chose; they are consistent with: code load address =
(file buffer base 0x45E0) + (file size) + 0x408 (+4 for two of the files); they only matter for
the values in SPEC 4.9 and for the absolute LC addresses (compare modulo BASE).

### Synthetic modules

Made by patching bytes of instrument records in a copy of the module (offsets are code hunk offsets of
the first byte after the 0xF0 command):
- Boulder Dash: records at 0x39E: flags word (+0x0E) = 0x0008, envelope reload (+0x14) = 1, step (+0x16) = 3;
  at 0x31A: flags 0x000C, reload 2, step 5; at 0x3CA: flags 0x0009, reload 1, step 2.
- Clever & Smart: records at 0x96E: flags word (+0x06) = 0x0008, envelope reload (+0x0C) = 2, step (+0x0E)
  = 4; at 0x9DA: flags 0x002F, reload 1, step 3; at 0xA22: flags 0x000C, reload 3, step 5.
The real modules never set flag bit 3 (envelope restart); these exercise it.

## Reproduce a reference log (silent, no speakers)

    SDL_AUDIO_DRIVER=dummy RUST_LOG=error DELIPLAYERS=EMPTY_DIR DELIGENIES=EMPTY_DIR DELI_NATIVE=0 \
    EAGLEPLAYERS=<UADE players dir> DELI_WRITE_LOG=ref.log \
    amigaplayer-rs --player 'Sound Programming Language' --wav /dev/null --seconds 25 --subsong K /ABSOLUTE/PATH/spl.NAME

K is 0-based. Beam reads and writes with times can be traced with DELI_TRACE=trace.txt (lines
`R 006 ... clk= line= h=` for beam reads, `W reg=value ... clk=` for writes); SPEC section 8 was derived
from such traces (90 s runs).
Port log: `EAGLEPLAYERS=EMPTY_DIR DELI_WRITE_LOG=port.log <binary> --open-replayers --wav /dev/null ...`; compare
with tools/replay-cmp.py REF.log PORT.log.

## What was checked

A value model written from SPEC.md alone was run against every log above, comparing (address, value) per
write in order, tick by tick: all ticks identical (SPEC section 10). The model also reproduces the
module's own start writes (hardware preset, ADKCON). The 90 s timing runs (8 runs) match as well.
