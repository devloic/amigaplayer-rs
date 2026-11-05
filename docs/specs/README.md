# Behaviour specs

These are the behaviour specifications written by a spec team that examined the original Amiga players (their write
logs, DMA logs, execution traces and black-box runs). Separate implementers then wrote the Rust ports from these texts and
from the logs, without seeing the originals. Each spec states formats, formulas, per-tick behaviour, register-write
order and timing measured in colour clocks and 68000 cycles, plus the unknowns that remain. Each `SPEC.md` was reviewed
before publication so that it describes behaviour only; what was removed or reworded is listed in
`SANITISE-REPORT.md`. The spec team's private working notes are not part of the repository.

| Directory | Subject | Port it belongs to |
|---|---|---|
| `dl/` | Dave Lowe, original driver: when the reference starts and how its ticks are spaced | `src/replay/davelowe.rs` (`DL_INIT_CLOCKS`, `DL_GAPS`) |
| `dl2/` | Dave Lowe, round 2: in-tick gaps, first-write offset, DMA-on word, subsongs, output-frame phase | `src/replay/davelowe.rs`, `src/replay/davelowe_gaps.rs`, `src/replay/davelowe_tables.rs` |
| `dl3/` | Dave Lowe, Eagleplayer-side patching and cycle costs (`DELI_DL_CPU=1`) | `src/replay/davelowe_cpu.rs` |
| `jesperolsen/` | Jesper Olsen replayer, write timing (engines B, C-T, C-G) | `src/replay/jesperolsen.rs` |
| `musicline/` | MusicLine Editor replayer, timing of 4-voice modules | `src/replay/musicline.rs`, `src/replay/musicline_tables.rs` |
| `davidhanney/` | David Hanney replayer (`dh.*`): format, replay, Paula writes, timing | no `src/replay` file at this commit |
| `markcooksey/` | Mark Cooksey / Richard Frankish replayer (`mc.*`, `mcr.*`/`mcs.*`) | no `src/replay` file at this commit |
| `robhubbard/` | Rob Hubbard, Amiga `rh.*` (not the ST variant `rho.*`, which is `src/replay/robhubbardst.rs`) | no `src/replay` file at this commit |

The Syntracker spec is not finished and is not included.

Each directory holds `SPEC.md` (the spec), `LOGS.md` (how the evidence was produced) and `README.md` (what is solved and what
stays unknown). Files and scripts they call `logs/...`, `scripts/...` or "traces" are not shipped: the reference logs are
large, and the instruction-level traces contain code addresses of the originals.
## Regenerating logs

The reference is the original Eagleplayer (or DeliTracker player) run on this repository's 68k engine, with the module corpus and
the UADE players directory (`EAGLEPLAYERS`). Write the Paula/CIA register writes of a silent render with `DELI_WRITE_LOG`
(`DELI_DMA_LOG` adds the audio DMA word fetches, `DELI_TRACE` an instruction-level call trace):

```sh
SDL_AUDIO_DRIVER=dummy RUST_LOG=error DELIPLAYERS=EMPTY_DIR DELIGENIES=EMPTY_DIR DELI_NATIVE=0 \
EAGLEPLAYERS=<UADE players dir> DELI_WRITE_LOG=ref.log \
amigaplayer-rs --wav /dev/null --seconds 20 [--subsong N] MODULE

EAGLEPLAYERS=EMPTY_DIR DELI_WRITE_LOG=port.log \
amigaplayer-rs --open-replayers --wav /dev/null --seconds 20 [--subsong N] MODULE

python3 -I tools/replay-cmp.py ref.log port.log        # tick by tick; first difference
```

`EMPTY_DIR` is an empty directory, so that no player other than the one under test is loaded. Never send the audio to
the speakers when testing: `--wav /dev/null` with the dummy SDL driver is enough. A log line is `ADDRESS VALUE CLOCK`;
`tools/replay-cmp.py` compares tick-relative times, so the start phase of the host does not matter for most specs (the
two Dave Lowe start-up specs are about that phase). See the header of `tools/replay-cmp.py` for options (`--ticks`,
`--tick-gap`).
