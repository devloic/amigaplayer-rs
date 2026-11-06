# CustomMade: reference logs

*Sanitised for publication: behaviour only.*

All logs are register-write logs of the reference 68k player (the Eagleplayer "CustomMade" V1.1,
UADE players directory) run through the host with `DELI_WRITE_LOG`. There is one line per Paula
register write: `ADDRESS VALUE COLOUR-CLOCK`. The first line, `# module 0045e0 SIZE`, gives the
load address of the image (0x45E0 in every run). The release binary ran with
`--player CustomMade --wav /dev/null --seconds 20 --subsong N`, `SDL_AUDIO_DRIVER=dummy`, empty
DeliPlayer and genie directories. Note: the host's `--subsong` is **0-based** (`--subsong 0` =
subsong 1). The file name's `sN` is the 1-based subsong.

`logs/` holds the 20-second logs (37 files). Validation used 180-second runs of the same 37
cases (regenerate with `--seconds 180`).

## Modules (all real corpus files; no synthetic modules)

| log name | file | subsongs | timer | Origin | first host write | first init write | first tick |
|---|---|---|---|---|---|---|---|
| BattleSquadron.cm.intro_and_title | Battle_Squadron/cm.intro_and_title | 2 | 0x2581 | 0x3D800 | 131162 | 137995 | 187497 |
| BattleSquadron.cm.ingame | Battle_Squadron/cm.ingame | 10 | 0x3181 | 0x246F0 | 211121 | 219294 | 284148 |
| BattleSquadron.cm.game_end | Battle_Squadron/cm.game_end | 1 | 0x2681 | 0x3D800 | 126868 | 132272 | 183046 |
| BattleSquadron.cm.hiscore | Battle_Squadron/cm.hiscore | 2 | 0x3081 | 0x246F0 | 199235 | 201724 | 265298 |
| BattleSquadronDemo.cm.ingame | Battle_Squadron/Demo_Version/cm.ingame | 5 | 0x3181 | 0x23000 | 205384 | 213557 | 278411 |
| BattleSquadron.rk-raw.intro_and_title | Battle_Squadron/Other_RK_Versions/rk.intro_and_title | 2 | 0x2581 | 0x20 | 132692 | 139525 | 189027 |
| BattleSquadron.rk-raw.ingame | Battle_Squadron/Other_RK_Versions/rk.ingame | 10 | 0x3181 | 0x20 | 212661 | 220834 | 285688 |
| BattleSquadron.rk-raw.game_end | Battle_Squadron/Other_RK_Versions/rk.game_end | 1 | 0x2681 | 0x3D800 | 126868 | 132272 | 183046 |
| Plague.cm.ingame | Plague/cm.ingame | 1 | 0x3181 | 0x13A74 | 188643 | 194415 | 259269 |
| Plague.cm.game_end | Plague/cm.game_end | 1 | 0x3581 | 0x485C4 | 188807 | 191925 | 261899 |
| Plague.rk-raw.ingame | Plague/Other_RK_Versions/rk.ingame | 1 | 0x3181 | 0x13A74 | 188643 | 194415 | 259269 |
| Plague.rk-raw.game_end | Plague/Other_RK_Versions/rk.game_end | 1 | 0x3581 | 0x20 | 199759 | 202877 | 272851 |

Times are colour clocks of subsong 1 (absolute, from machine start). For subsong N > 1 the host
runs N inits first (SPEC 3.1). Each init adds about 3 900 to 4 200 cc, depending on the track
list sizes (SPEC 8.4).

The corpus also holds the hunk-wrapped versions of these tunes (`Eagleplayer_Versions/rk.*`, with
`RON_KLAREN_SOUNDMODULE!`). CustomMade refuses them and `ronklaren.rs` plays them. The music data
are the same, but the outputs differ in details. Example: Battle Squadron ingame subsong 1, voice 1, gets
period 120 from CustomMade's extended table. The open Ron Klaren replayer, which reads on past the
game's own 70-entry table, gives 0 for the same note.

## What they confirm (180-second runs, 37 cases)

- Address, value and order of every register write: 1 726 834 writes, all equal to SPEC sections
  3 to 7.
- Tick-relative time of every write equals the SPEC 8.2/8.3 clock model: 127 178 ticks, 0
  different.
- Tick spacing `5 * (timer + 1)` cc on every tick.
- Executed in the corpus: commands 0x80, 0x81 (593 rows), 0x82, 0x83, 0x84, 0x85 (a stop ends 24 of the 37
  runs; 0x85 in Battle Squadron intro subsong 2, both copies), zero-duration notes (49), track repeats, track list
  wraps with song-end reports, phase modulation in both directions with direction changes,
  portamento up and down with clamping, vibrato with delay and wrap, all envelope cases, one-shot
  instruments, the 0x270F patch (Battle Squadron intro subsong 1, both copies).
- Never executed: zero-duration notes with a scratch high byte other than 0x80/0, period lookups
  outside the 84-entry table, durations >= 64, the 0x80-after-0x82 ordering case, an empty track
  list, a file without the `ptoff` marker.
- Host stop on song end: after a 0x83/0x85 stop the log ends at that tick. After a wrap song end
  (Plague game_end, Battle Squadron hiscore subsong 1) the host stops 3 to 5 ticks later.

## Not matched

Nothing in the logs above. Absolute times before the first tick are given only as measured
values and fits (SPEC 8.4): the time to the first host write is fitted within 11 cc. The
init-to-tick relations are exact within 1 cc.
