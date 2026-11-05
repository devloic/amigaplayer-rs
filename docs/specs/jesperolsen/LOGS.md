# Logs and reproduction

The files named below are not shipped; regenerate them as described. Format of write logs: `ADDRESS VALUE COLOUR-CLOCK` per line (DELI_WRITE_LOG),
absolute colour clock from reset. Ticks are separated by > 30000 cc gaps.

| file | content |
|---|---|
| guldkorn_title.ref.log | reference writes, Guldkorn_Expressen/jo.title (engine C-T) |
| guldkorn_ingame_sub8.ref.log, guldkorn_ingame_sub9.ref.log | reference, Guldkorn_Expressen/jo.ingame subsongs 8 and 9 (engine C-G) |
| lollypop_ingame_01.ref.log, georg_glaxo_ingame.ref.log | reference, engine B data modules |
| *.gaps.tsv | per-write gap tables (tick, index, addr, value, cc from tick start, cc from previous write) made by gaps_tsv.py |
| gaps_tsv.py | makes the gap table from a write log |
| cmpticks_public.py | compares two write logs tick by tick with tick-relative timestamps (`tools/replay-cmp.py` in this repository does the same) |

Module files: the Jesper Olsen module corpus (directories Guldkorn_Expressen, Kalas_Puffs_Expressen, Georg_Glaxo, Lollypop). The list of
the 25 files used for engine B is the Georg_Glaxo and Lollypop modules of that corpus.

## Reproduce a reference log (68k player, silent)

    SDL_AUDIO_DRIVER=dummy RUST_LOG=error DELIPLAYERS=EMPTY_DIR DELIGENIES=EMPTY_DIR DELI_NATIVE=0 \
    EAGLEPLAYERS=<UADE players dir> DELI_WRITE_LOG=/path/ref.log \
    <amigaplayer-rs binary> --wav /dev/null --seconds 12 --subsong 8 MODS/Guldkorn_Expressen/jo.ingame

(Reference = the UADE JesperOlsen_EP; subsong numbering as the --subsong option; the reference
runs the 68k player, not the port. The binary used for the logs here is a build of the repo with
an extra instruction-trace option; the plain repo binary gives the same write logs.)

## Port log

    EAGLEPLAYERS=EMPTY_DIR DELI_WRITE_LOG=/path/port.log <binary> --open-replayers --wav /dev/null \
        --seconds 12 --subsong 8 MODS/Guldkorn_Expressen/jo.ingame

## Compare

    python3 -I tools/replay-cmp.py ref.log port.log

Prints write count, number of writes whose tick-relative time differs, histogram of differences.
Results of the model described in SPEC.md (a build of the port with the exact costs):
engine B 0 of about 1.3 M writes off; Guldkorn jo.ingame subsongs 0-13 0 of about 66 000 off
(subsongs 0-8: the port stops at "song end" after 3-4 s; the reference ticks on, only the
common prefix is compared); Guldkorn jo.title exact.

## Observed write gaps (cc) to check an implementation against

Guldkorn jo.ingame (G), most common gap from the previous write in the same tick:
VOL after PER 205; LOC after VOL 123 (second half of LOC 0); LEN after LOC 103.
Guldkorn jo.title (T): VOL after PER 173; LEN after LOC 103 (retrigger) or 8 (rest item);
the DMA-on write after the last voice: see gaps.tsv.
