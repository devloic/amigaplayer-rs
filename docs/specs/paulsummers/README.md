# Paul Summers: unknowns and caveats

See SPEC.md for the format and replay description, LOGS.md for the reference logs. A private folder with the
working notes and scripts exists beside these files and is not meant for the implementer.

## No data/

The format needs no bytes from the player: the period table, song table, instrument tables and streams are all
inside the module. The `data/` folder is empty; there is nothing for the lead to decide on embedding. The
synthetic modules in `mods/` are edited game data (the lead decides on publishing them).

## Not determined / not specifiable

- A positive speed value: the reference runs one tick, then the host fails.
- Stream commands with low nibble 10 to 13, and a zero divisor in the pitch effect: the reference hangs (the run
  never ends). Do not emulate; any behaviour is acceptable.
- Stacks deeper than 128 bytes overwrite neighbouring data in the reference; unspecified.
- Host volume and balance gains were ignored; only register writes were compared.
- The first tick call time t0 is host-defined (LOGS.md lists the observed values); the replay is only exact
  relative to it.
- The one data fix for Fighting_Soccer (SPEC 1.2) was never seen to matter.
- A call that runs longer than the tick period was never observed (longest 23 025 cc against 47 610);
  behaviour in that case is untested.
- What the host does after song end beyond the three extra ticks.
- The silent-block address written to AUDxLC after each note start is a host constant (not in the module).

## Paths not exercised by the corpus (specified from synthetic modules only)

Loop start/end commands, duration-high, the pitch effect (and its carry word), rests with zero duration, the
voice-3 one-shot rule when voice 0's stack is 8 to 23 records deep, and song end with voices finishing in
particular orders. The synthetic runs match the model exactly, but real music may use them differently.

## Detection

The detection rule was checked on the four corpus files (accepted) and 23 files of other formats (rejected);
a small sample.
