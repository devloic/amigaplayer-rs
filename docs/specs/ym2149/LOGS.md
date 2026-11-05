# Reference logs

All logs come from the reference 68k player ("The YM2149 Player v1.52c", the UADE player `YM-2149`) run
through the host with DELI_WRITE_LOG, `--seconds 4` for the synthetic modules and 6 for the real ones.
Each line reads `ADDRESS VALUE COLOUR-CLOCK`, and the first line is `# module ADDRESS SIZE`. The first group
of writes is the emulator initialisation (SPEC 4.2). Every later group is one tick.

The emulator's own buffers sit at a host-dependent address. In these runs the silent word `S` is at the
address of the first LC write. Compare with `tools/replay-cmp.py`, which accepts a constant offset for
player-owned addresses.

## Synthetic modules (`mods/syn_*.ym`)

These are type-0 MYST files made for this spec. Their play routine stores a list of words into the emulator
block every tick and loops through the frames. The tick routine is called at `1 274 251 + 70 940 n` cc.

| log | what it exercises |
|---|---|
| `syn_tone.log` | tone only: periods 0..0xFFF over all octaves, below 10, same-octave and octave changes, volumes, mixer switching |
| `syn_noise.log` | noise only: noise periods 0..0x1F, values above 5 bits (masking), mixer changes |
| `syn_tonenoise.log` | tone + noise mixing (512-byte buffers), period 0 (falls back to noise), noise changes |
| `syn_env.log` | envelope only: all 16 shapes, periods 0, 1, below 10, above 0x11E1, negative, retrigger |
| `syn_toneenv.log` | tone + envelope mixing, all octaves and shapes, negative envelope periods, SID mode 1 with large, negative, small periods |
| `syn_sid.log` | SID modes 1, 2, 0 on all three voices with different periods and level pairs |
| `syn_unsup.log` | noise + envelope and tone + noise + envelope (unsupported modes), SID with noise |
| `syn_copy.log` | fourth-voice source 0, 1, 2, 3, negative |
| `syn_random.log` | 150 random frames over all YM fields |
| `syn_thresh.log` | the tone threshold field |
| `syn_digi.log` | module-driven voices (flag bit 7) with both register sets, on A and C, PER/VOL-only flag on such a voice |

## Real modules

All come from the MYST distribution on Aminet (`mus/play/MYST_Data_1` to `_5`), except Astaroth, which
comes from the archive. Its replay file `YM.MCoder_replay` is in `MYST_Data_2`, where it is named
`YM.Mcoder_replay`. The host's file lookup is case-sensitive, so the file needs the name in the module's
`YO` string.

| log | module | type | notes |
|---|---|---|---|
| `real_Elite.log` | Beben/YM.Elite | 0 | plain tone/noise, has `Y[` |
| `real_Cybernoid.log` | Hippel/TSD/YM.Cybernoid | 2 (YM.TsdPart2_replay) | relocation through a0/a1 |
| `real_astaroth-game.log` | astaroth-game.ymst | 2 (YM.MCoder_replay) | Hippel COSO, envelopes, 3 subsongs |
| `real_BigBubble.log` | HotLine/Eternal/YM.BigBubble | 0 | module-driven digital samples, service 11 |
| `real_Thrust_ULM.log` | Hubbard/YM.Thrust_ULM | 0 | one of the modules the private check could not reproduce (README) |
| `real_Androids_Sid.log` | Tao/YM.Androids_Sid | 0 | SID voices |
| `real_Klax_sub3.log` | Furniss/YM.Klax, subsong 3 | 0 | `` Y` `` subsong map; two subsong changes at start (end-sound writes) |
