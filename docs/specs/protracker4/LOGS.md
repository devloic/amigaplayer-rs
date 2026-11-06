# Logs and reproduction

*Sanitised for publication: behaviour only.*

Write-log format (DELI_WRITE_LOG): `ADDRESS VALUE COLOUR-CLOCK` per line, absolute colour clock;
first line `# module BASE SIZE` (load address and size of the whole `mod3.` file). Sample addresses
in the logs are absolute. Subtract BASE plus the offset of the PTDT data (PTDT id offset + 8) to
get offsets into the ProTracker module. 8-voice runs log the amplifier's virtual voices at 0xD00000
| voice << 4 | register (0/2 LOC, 4 LEN, 6 PER, 8 VOL, 0xA DMA with 0x8000 on, 0 off).

All logs are of the reference 68k player run through the host with DELI_WRITE_LOG (UADE players
directory, `--player Protracker4`), `--wav /dev/null`, `SDL_AUDIO_DRIVER=dummy`. Nothing was
played through speakers.

## Modules

Real: `MOD3.heaVen` (DJ Pie, 1998), md5 prefix 7aafa3c0cb13, the only `mod3.` ProTracker 4 file in
the module corpus. The other corpus names that match `mod3` are an S3M and plain `.mod` files.
No `ptm.` file in the corpus is a FORM/MODL file.

Synthetic: ordinary `M.K.` modules from the corpus, wrapped into a ProTracker 4 container by
`logs/wrap_pt4.py`. That script writes VERS with size 0x16, INFO and CMNT with sizes that count
their headers, and PTDT. INFO tempo and flags come from its arguments, default 125 / 0x81. The
modules were picked to cover all effects:

| name | md5 prefix of the wrapped module | effects it covers |
|---|---|---|
| m01 | def80867527b (Inflatable Dartboard) | 0-D, E0, E1, E2, E6, E9, EB, F, F >= 0x20 |
| m02 | 62ecd83ffe4b (in media res) | EA, ED, EE, F00 |
| m03 | d411bed5f8ea (alwayz happy!) | E4, E7 |
| m04 | 637623d1b203 (Ghosts N Goblins jingle) | E8, F00, B to itself on the last position |
| m05 | 8212b03070a0 (crystal drum hammer) | EC |
| m06 | 832c9bb04fdd (warez on) | EF, song end |
| m07 | a75344049f6a (smash the feeling) | E5 |
| s1vbl | m01 with INFO flags 0x80 (bit 0 clear) | Fxx >= 0x20 as speed |
| s2t0 | m07 with INFO tempo 0 | default tempo 125 |
| s3vers | m07 with a VERS chunk of size 10 | chunk walk fails: silence |
| s4t90 | m07 with INFO tempo 90 | tick period 98 525 cc |
| s5ch8 | m01 turned into `8CHN` (each row's 4 cells copied to voices 4-7) | 8 voices through the amplifier |

E3x (glissando) and arpeggio reads past the period table occur in none of them.

## Files

| file | content |
|---|---|
| heaVen.ref.log | MOD3.heaVen, 60 s |
| MOD3.m0N.ref.log, MOD3.s*.ref.log | 120 s runs (60 s for s1-s5), or until the host stops at song end (m06) |
| MOD3.*.40s.ref.log | 40 s runs made with DELI_TRACE, for the timing table |
| MOD3.*.irq.txt | entry and exit clock of every interrupt call of those 40 s runs (from DELI_TRACE `C`/`E` lines) |
| gap-table.tsv | measured write spacing in CPU cycles (SPEC 5.2), from the 40 s runs of heaVen, m01-m07 |
| wrap_pt4.py | the container builder |

Columns of gap-table.tsv: previous write's role, this write's role (`fx` effect tick, `row` row tick,
`ENTRY` interrupt entry, `+W` after the host's DMA wait, `EXIT` end of the call), register, branch tags
(`w<wave><p|n>` vibrato/tremolo waveform and sign, `u`/`d` volume slide up/down, `t0`/`t1` tone
portamento direction, `o1`/`o0` sample offset inside/outside the sample, `l`/`z` LEN written as is / as 0),
command, instrument in the cell, non-zero repeat start, funk active. Then the cycles, the cycles per
period-lookup index, the cycles per tone-portamento lookup index, the number of samples and the number
of samples off by more than 1 cc.

## Reproduce (silent)

    SDL_AUDIO_DRIVER=dummy RUST_LOG=error DELIPLAYERS=EMPTY_DIR DELIGENIES=EMPTY_DIR \
    EAGLEPLAYERS=<UADE players dir> DELI_WRITE_LOG=/path/ref.log \
    amigaplayer-rs --wav /dev/null --seconds 60 --player Protracker4 /ABSOLUTE/PATH/MOD3.heaVen

Add DELI_TRACE=/path/trace.txt to get interrupt entry clocks (lines `C <addr> depth=1 clk=`). The
most frequent call address is the interrupt. The built-in engine for comparison is the same
binary without `--player`, on the PTDT data saved as a plain module. Compare with
tools/replay-cmp.py, keeping in mind that sample addresses differ by the PTDT data offset.
