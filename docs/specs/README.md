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
| `anders0land/` | Anders 0land replayer: behaviour spec | `src/replay/anders0land.rs` |
| `artandmagic/` | Art & Magic (`aam.*`) replayer: behaviour specification | `src/replay/artandmagic.rs` |
| `bladepacker/` | Blade Packer replayer: behaviour spec | `src/replay/bladepacker.rs` |
| `dariuszendeh/` | Darius Zendeh sound system (old Mark II) -- behaviour spec | `src/replay/dariuszendeh.rs` |
| `dynamicsynthesizer/` | Dynamic Synthesizer (Chris Huelsbeck, 1987-88) - behaviour specification | `src/replay/dynamicsynthesizer.rs` |
| `forgottenworlds/` | Forgotten Worlds game music ("FWMP" modules): behaviour specification | `src/replay/forgottenworlds.rs` |
| `fredgray/` | Fred Gray ("gray.*") - behaviour specification | `src/replay/fredgray.rs` |
| `futureplayer/` | Future Player (Amiga, `fp.*`) replayer: behaviour spec | `src/replay/futureplayer.rs` |
| `howiedavies/` | Howie Davies ("hd.*") - behaviour specification | `src/replay/howiedavies.rs` |
| `jochenhippelst/` | Jochen Hippel ST (hst.*, sdc.*) - behaviour specification | `src/replay/jochenhippelst.rs` |
| `laxity/` | Laxity (`powt.*`, `pt.*`) replayer: behaviour spec | `src/replay/laxity.rs` |
| `legglessmusiceditor/` | Leggless Music Editor: behaviour spec | `src/replay/legglessmusiceditor.rs` |
| `maximumeffect/` | Maximum Effect (Jonathan Scarcliffe and Alastair Dukes, 1993) replayer: behaviour spec | `src/replay/maximumeffect.rs` |
| `medley/` | Medley (Paul van der Valk, `mso.*`) replayer: behaviour spec | `src/replay/medley.rs` |
| `ntspsystem/` | NTSP-system (`two.*`, magic `SPNT`) replayer: behaviour spec | `src/replay/ntspsystem.rs` |
| `onescapee/` | onEscapee (Akos Divianszky, 1997) - behaviour specification | `src/replay/onescapee.rs` |
| `paulsummers/` | Paul Summers (Amiga, `snk.*`) replayer: behaviour spec | `src/replay/paulsummers.rs` |
| `professionalsoundartists/` | Professional Sound Artists (PSA) replayer: behaviour spec | `src/replay/professionalsoundartists.rs` |
| `quartet/` | Quartet (Amiga) replayer: behaviour spec | `src/replay/quartet.rs` |
| `richardjoseph/` | Richard Joseph (`RJ.*`, Eagleplayer "Richard Joseph") replayer: behaviour spec | `src/replay/richardjoseph.rs` |
| `seanconnolly/` | Sean Connolly (Sean "Odie" Connolly, 1992-95): behaviour specification | `src/replay/seanconnolly.rs` |
| `soundprogramminglanguage/` | Sound Programming Language (Holger Gehrmann): behaviour spec | `src/replay/soundprogramminglanguage.rs` |
| `speedya1system/` | Speedy A1 System (Michael Winterberg, 1988-89) - behaviour specification | `src/replay/speedya1system.rs` |
| `speedysystem/` | Speedy System (Michael Winterberg, 1988) - behaviour specification | `src/replay/speedysystem.rs` |
| `suntronic/` | SUN-Tronic ("SUNtronic") replayer: behaviour spec | `src/replay/suntronic.rs` |
| `syntracker/` | SynTracker (Eagleplayer "SynTracker", UADE player name SynTracker) - behaviour spec | `src/replay/syntracker.rs` |
| `tfmxst/` | TFMX ST (mdst.*) - behaviour specification | `src/replay/tfmxst.rs` |
| `thomashermann/` | Thomas Hermann (1989 game driver) - behaviour specification | `src/replay/thomashermann.rs` |
| `tomytracker/` | Tomy Tracker (`sg.*`) replayer: behaviour spec | `src/replay/tomytracker.rs` |
| `tronic/` | Tronic ("TronicTracker(?)") replayer: behaviour spec | `src/replay/tronic.rs` |
| `ufo/` | UFO replayer (MicroProse, 1994): behaviour spec | `src/replay/ufo.rs` |
| `voodoosupremesynthesizer/` | Voodoo Supreme Synthesizer (`vss.*`, `*.vss`) replayer: behaviour spec | `src/replay/voodoosupremesynthesizer.rs` |
| `ym2149/` | YM-2149 / MYST (`ym.*`, `ymst.*`) player: behaviour spec | `src/replay/ym2149.rs` |
| `aprosys/` | AProSys: behaviour specification | `src/replay/aprosys.rs` |
| `audiosculpture/` | AudioSculpture (`adsc.*`, `mod_adsc4.*`) replayer: behaviour spec | `src/replay/audiosculpture.rs` |
| `earache/` | EarAche (Amiga, `EA.*`) replayer: behaviour spec | `src/replay/earache.rs` |
| `futurecomposerbsi/` | Future Composer BSI (Amiga, `BFC.*`, `BSI.*`, `FC-BSI.*`) replayer: behaviour spec | `src/replay/futurecomposerbsi.rs` |
| `gluemon/` | GlueMon (Amiga, `glue.*` / `gm.*`) replayer: behaviour spec | `src/replay/gluemon.rs` |
| `infogrames/` | Infogrames (Amiga, `*.dum` + `*.ins`) replayer: behaviour spec | `src/replay/infogrames.rs` |
| `jannesalmijarvi/` | Janne Salmijarvi Optimizer (`JS.*`) replayer: behaviour spec | `src/replay/jannesalmijarvi.rs` |
| `kimchristensen/` | Kim Christensen (Amiga, `KIM.*`) replayer: behaviour spec | `src/replay/kimchristensen.rs` |
| `majortom/` | Major Tom (ARP.) - behaviour specification | `src/replay/majortom.rs` |
| `markcookseyold/` | Mark Cooksey (old player, "MarkCookseyOld", prefix `mco`): behaviour spec | `src/replay/markcookseyold.rs` |
| `mcmd/` | MCMD replayer: behaviour spec | `src/replay/mcmd.rs` |
| `midiloriciel/` | MIDI - Loriciel: behaviour spec | `src/replay/midiloriciel.rs` |
| `nickpelling/` | Nick Pelling Packer: behaviour spec | `src/replay/nickpelling.rs` |
| `paultonge/` | Paul Tonge replayer: behaviour spec | `src/replay/paultonge.rs` |
| `quartetst/` | Quartet ST replayer: behaviour spec | `src/replay/quartetst.rs` |
| `riffraff/` | Riff Raff: behaviour specification | `src/replay/riffraff.rs` |
| `scottjohnston/` | Scott Johnston ("SoundPlayer", `SJ.*`) replayer: behaviour spec | `src/replay/scottjohnston.rs` |
| `specialfxst/` | Special FX ST (`doda.*`) player: behaviour spec | `src/replay/specialfxst.rs` |
| `tfhd/` | TFMX single-file container (TFHD): behaviour spec | `src/replay/tfhd.rs` |
| `adpcmmono/` | ADPCM mono (Eagleplayer "ADPCM_mono", player name "Adaptative Delta Pulse Code Modulation") — behaviour spec | `src/replay/adpcmmono.rs` |
| `zoundmonitor/` | ZoundMonitor (Amiga, `sng.*`) replayer: behaviour spec | `src/replay/zoundmonitor.rs` |
| `bendaglishsid/` | Benn Daglish SID (`bds.*`, tag `DAGLISH!`): hosted-driver behaviour spec | `src/replay/bendaglishsid.rs` |
| `markii/` | Mark II Sound-System (`mk2.*`): hosted-driver behaviour spec | `src/replay/markii.rs` |
| `pokeynoise/` | Pokeynoise (Atari SAP types B/M/C, `pn.*`): 6502 + POKEY-to-Paula behaviour spec | `src/replay/pokeynoise.rs` |
| `quartetpsg/` | Quartet PSG (Atari YM driver images, `sqt.*`): hosted-driver behaviour spec | `src/replay/quartetpsg.rs` |
| `multimediasound/` | MultiMedia Sound (`mms.*`, `sfx20.*`): SoundFX-layout replayer with its own engine | `src/replay/multimediasound.rs` |
| `timetracker/` | TimeTracker (`tmk.*`, with `smp.*`): ProTracker-like replayer with its own quirks | `src/replay/timetracker.rs` |

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
