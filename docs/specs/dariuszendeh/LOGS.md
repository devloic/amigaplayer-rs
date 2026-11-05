# Logs and reproduction

Write-log format: `ADDRESS VALUE COLOUR-CLOCK` per line (DELI_WRITE_LOG), absolute colour clock from reset;
first line `# module BASE SIZE` (load address and size of the module in the reference's memory; BASE is
0x0045E0 in all three). Sample pointer values are absolute (module base + offsets).
All logs are of the reference 68k player (the UADE Eagleplayer "Darius Zendeh", selected with
`--player 'Darius Zendeh Player'`) run through the host with DELI_WRITE_LOG, 60 s each (about 33 000
writes), `--wav /dev/null`, `SDL_AUDIO_DRIVER=dummy`; nothing was played through speakers.

| file | module | variant |
|---|---|---|
| crashburn.dz.log | Crash and Burn (file dz.Crash_and_Burn, UnExoticA Game/Zendeh_Darius), md5 21005070e5d2 | B |
| katakis-game-end.dz.log | demozoo full2 `katakis-game end.dz`, sha1 dir 483f475f4ac8 | A |
| threat-duel-cracktro.dz.log | demozoo full2 `threat duel cracktro.dz`, sha1 dir 5274150d2ed2 | C |

Module files are not copied (DB `public.module_index`, `file_path ~* '(^|/)(dz|mkiio)\.'` gives only the Crash and
Burn file; the two others are demozoo `.dz` files found through the headers index by their first long
`48E778F0` / `48E700F0`). No `mkiio` file exists in the archive.

## Reproduce

    E=<empty directory>
    SDL_AUDIO_DRIVER=dummy RUST_LOG=error DELIPLAYERS=$E DELIGENIES=$E \
    EAGLEPLAYERS=$HOME/.local/opt/uade/share/uade/players DELI_WRITE_LOG=ref.log \
    amigaplayer-rs --player 'Darius Zendeh Player' --wav /dev/null --seconds 60 /ABSOLUTE/PATH/module.dz

Compare a port with tools/replay-cmp.py REF.log PORT.log. DELI_TRACE additionally shows each guest call
with its entry and return clock (the entry clock of the first timer call is the anchor of section 6).

## Features of the logs

- The timer calls are 70 940 cc apart. Within a log: odd-numbered calls (counted from the first timer
  call) contain one DMACON write, even-numbered ones 21 writes.
- B/C logs begin with DMACON 0x000F at 7121 (B) / 4193 (C); A begins with AUD0LEN at 610.
- Per-gap statistics (cc, value counts over 1 400 ticks): see SPEC section 6; the counts show the +-1
  jitter, e.g. B: PER->VOL 36 x5111, 37 x222; VOL->next LEN (not retrigger) 118 x2797, 117 x166;
  C: LCL->PER (no slide) 60 x1893, 61 x1748, (down slide) 98 x903, (up slide) 100 x1052.
