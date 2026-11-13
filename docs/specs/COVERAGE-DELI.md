# Coverage of the installed DeliPlayers, Eagleplayers and DeliGenies by the default build

Inventory of 2026-11-13. `COVERAGE.md` reconciles UADE's 178 player files; this file does the same for
the player files installed under `~/.config/deliplayer/`:

- `DeliPlayers/`: 28 DeliPlayers, plus 57 in `DeliPlayers/PlayerStore/` (85 files);
- `Eagleplayers/`: 114 players plus the 4 `AYPlayers/` sub-players of PlayAY. This directory is the same
  set as EP206 (`claude-scratch/deliplayerrust/eplicense/x/Eagleplayer_2.06/Eagleplayers`, where
  `tools/refcmp.py` finds it: `diff` of the two listings is empty);
- `DeliGenies/`: 9 genies, plus 19 in `DeliGenies/GenieStore/` (28 files).

The default build (`cargo build --release`: `ui`, `archives`, `custom-modules`) loads none of these.
For each player the questions are: which format it plays, what plays that format in the default build,
and whether the test corpus has a file of the format with a refcmp class.

## Method

No player code was read. Sources: file names, the name, version and `$VER:` strings the players
carry, the engine's own reports, the project's sources and docs, and black-box runs.

1. Names: a `--features host-68k` build of `amigaplayer-render` (private target directory, deleted
   afterwards) loaded each directory with `RUST_LOG=info` on a random file. The name and version in
   the tables are what the engine logged (`player: NAME vX.Y (file)`).
2. Which files each player takes: every player file alone in a directory, `--68k-first --info` on
   448 samples (2 local files for each of the 171 port formats in `tools/refcmp-baseline.tsv`, plus
   about 70 files of formats the corpus lacks, copied from the archive by the header index
   `headers/headers.sqlite` and from `dl-modules/`, `ident/tme/`). That is 89,152 runs. A second pass
   gave each player that took nothing every corpus file of its likely format (about 900 more runs).
   `backend: 68k` in the JSON means the player took the file.
3. What the default path does with the same files: `--info` with no players loaded. It names the open
   replayer, libxmp loader, ProWizard + built-in ProTracker path, or built-in decoder that takes the file.
4. Corpus: `tools/refcmp-baseline.tsv` (1735 rows). The listen list
   (`listen/files.json`) has the same 1674 paths. The refcmp class of a format is the tally of its rows
   by the port's `format=`. The `ports/*.txt` lists were used where the baseline has no row.
5. Smoke test: 6 s renders (`--wav`-equivalent `--out`, `SDL_AUDIO_DRIVER=dummy`) of one extra file
   for each format the corpus lacks. All 30 have audio (peaks 2316 to 32768). This is not a comparison
   against a reference.

Scripts and raw results: `claude-scratch/deliplayerrust/fixround/cov-deli/work/` (`matrix.py`, `pass2.py`,
`matrix-68k.jsonl`, `matrix-open.jsonl`, `pass2.jsonl`, `accept.txt`, `names.tsv`).

Status column:

- **V**: covered, and verified: the corpus has a file of the format that the default path plays, with a
  refcmp class other than NOREF.
- **U**: covered but unverified. The default path plays the format (an extra test file, or by the
  project's docs), but the corpus has no file of it, or only NOREF rows.
- **X**: not covered.

Evidence column (how the player was tied to the format):

- **A**: the player took corpus or sample files of that format in the black-box run.
- **E**: it took only extra test files; the corpus has none.
- **R**: its check passed, but the engine then reported "could not load additional files" (companion
  naming) or the player crashed. The format is identified; that the player plays it is not shown.
- **P**: the file never reaches a 68k player. The engine decodes 8SVX, AIFF, RIFF WAVE and ZX AY files,
  and ProWizard converts packed modules, before any player is asked, even with `--68k-first`. The
  format then comes from the name, and the default path was checked on test files.
- **N**: name and strings only; the player refused every file tried.
- **ref n**: refcmp's reference for n baseline rows was this very file (EP206 or PlayerStore source).

Classes: I = IDENTICAL, S = SIMILAR, N = NOREF, T = TEMPO, D = DIFF, F = FILTER, X = STAGE-MISMATCH,
L = LENGTH, Q = SILENT, NE = NEAR. "open `Name`" is the replayer of that name in `src/replay`.

## DeliPlayers (`DeliPlayers/`, 28)

| File | Name, version | Format | Default-build path | refcmp classes | Evidence | Status |
|---|---|---|---|---|---|---|
| Delta1.0 | DeltaMusic_1.0 1.30 | Delta Music 1.0 | open `Delta Music 1.0` | 13 S | A | V |
| Delta2.0_note | DeltaMusic_2.0 1.40 | Delta Music 2.0 | open `Delta Music 2.0` | 9 S, 1 I | A | V |
| FastTracker2 | FastTracker_2.0 1.73 | FastTracker 2 XM | libxmp port, Fast Tracker II loader | 5 I (against libxmp 4.7.4 as oracle, not a 68k player) | A | V |
| FC1.3_note | FutureComposer_1.3 2.60 | Future Composer 1.3 | open `FutureComposer1.3` | 7 S | A | V |
| FC1.4_note | FutureComposer_1.4 2.50 | Future Composer 1.4 | open `FutureComposer1.4` | 4 S | A | V |
| FredMonitor | FredMonitor 1.70 | Fred Editor (it took the corpus's Fred Editor files) | open `Fred Editor` | 9 S, 1 N | A | V |
| FTM | FaceTheMusic 2.70 | Face The Music | open `Face The Music` | 20 S | A | V |
| Hippel | Hippel 1.50 | Jochen Hippel (TFMX-type) | open `Hippel TFMX` | 3 S | A | V |
| Hippel-COSO_note | Hippel-COSO 2.10 | Hippel COSO (also took a 7V file) | open `Hippel COSO`, `Hippel 7V` | COSO 6 S, 2 N; 7V 16 S, 7 N | A | V |
| IFF-8SVX | IFF-8SVX 2.90 | IFF 8SVX sample | built-in decoder `src/iff8svx.rs` (PCM, Fibonacci) | none | P | U |
| IFF-SMUS | IFF-SMUS 1.10 | IFF SMUS + instruments | open `IFF-SMUS` | 17 S | R | V |
| JamCracker_note | JamCracker 2.40 | JamCracker | open `JamCracker` | 9 S | A | V |
| MarkII_note | Mark_II 1.70 | Mark II | open `Mark II` | 8 I | A | V |
| MED_note | MED 3.40 | MED / OctaMED MMD0-MMD2 | open `MED`, `OctaMED` | MED 6 S, 1 D, 3 F; OctaMED 7 S | A | V |
| M.O.N | M.O.N. 1.40 | Maniacs of Noise | open `Maniacs of Noise` | 10 S | A | V |
| Oktalyzer | Oktalyzer 1.90 | Oktalyzer | open `Oktalyzer` | 50 S | A | V |
| SAplayer_note | SonicArranger 3.10 | Sonic Arranger | open `Sonic Arranger` | 9 S | A | V |
| ScreamTracker3 | ScreamTracker_3.0 1.62 | Scream Tracker 3 (S3M) | libxmp port, S3M loader | none | E | U |
| Sidmon1.0_note | Sidmon_1.0 1.60 | SidMon 1.0 | open `SidMon 1.0` | 9 I | A | V |
| Sidmon2.0_note | Sidmon_2.0 2.20 | SidMon 2.0 | open `SidMon 2.0` | 10 S | A | V |
| SoundFX13_note | SoundFX_1.3 1.30 | SoundFX 1.3 | open `SoundFX1.3` | 10 I | A | V |
| SoundFX20_note | SoundFX_2.0 1.0 | SoundFX 2.0 | open `SoundFX2.0`; files named `mms.`/`sfx20.` go to open `MultiMedia Sound` | MultiMedia Sound 9 S, 1 I; `SoundFX2.0` itself none | A | V |
| SoundMon20_note | SoundMon_2.0 1.91 | SoundMon 2.0 (also takes 1.1 files) | open `SoundMon 2.0` | 6 S | A | V |
| SoundMon22_note | SoundMon_2.2 1.20 | SoundMon 2.2 | open `SoundMon 2.2` | 4 S | A | V |
| TakeTracker | TakeTracker 1.70 | 1-32 channel ProTracker-type MOD (xCHN, xxCH, Octalyser tags) | open `Multichannel MOD` | 18 S, 1 F | A | V |
| TFMX | TFMX 2.29 | TFMX 1.5, Pro, 7V, ST (mdat + smpl) | open `TFMX 1.5`, `TFMX Pro`, `TFMX 7V`, `TFMX ST` | 1.5: 5 S, 3 N; Pro: 18 S, 1 I, 1 N; 7V: 2 S; ST: 7 I, 1 S | A | V |
| VectorDean_note | VectorDean 1.40 | Richard Joseph Player (`rjp.` + `smp.`) | open `Richard Joseph Player` | 81 S, 1 I, 1 N, 2 D | R | V |
| Whittaker | Whittaker 4.23 | David Whittaker | open `David Whittaker` | 10 S | A | V |

## PlayerStore (`DeliPlayers/PlayerStore/`, 57)

| File | Name, version | Format | Default-build path | refcmp classes | Evidence | Status |
|---|---|---|---|---|---|---|
| 669 | Composer669 0.80 | Composer 669 | libxmp port, 669 loader | none | E | U |
| Actionamics | Actionamics 1.0 | Actionamics | open `Actionamics` | 3 S | A | V |
| AHX-DeliPlayerII020 | AHX v2 2.33 | AHX / THX 1.x and 2.x | open `AHX` | 11 S | A | V |
| AIFF | IFF-AIFF 1.30 | AIFF / AIFF-C sample | built-in decoder `src/aiff.rs` | none | P | U |
| AMComposer | A.M.Composer 1.2 0.2 (an Eagleplayer) | A.M.Composer (`amc.`) | open `A.M.Composer` | none | E | U |
| Amos | AmosMusicBank 0.80 | AMOS Music Bank | open `AMOS Music Bank` | 10 I (ref 10) | A | V |
| ArtOfNoise | ArtOfNoise 1.20 | Art of Noise 4V / 8V | open `Art of Noise 4V`, `8V` | 4V 17 S; 8V 7 S, 1 N | A | V |
| DataType | DataType 1.20 | any sound the system datatypes decode | built-in 8SVX, AIFF and RIFF WAVE decoders only | none | N (its check crashes in the engine, which disables it) | U (partial) |
| DDMF | XTracker 0.80 | X-Tracker (DMF) | `src/dmf` (behind the libxmp loader list) | 4 X (ref 4) | A | V |
| DeliStone_Note | StoneTracker 1.60 | StoneTracker | open `StoneTracker` | 1 F, 1 D | A | V |
| Delta | DeltaMusic 0.90 ("DeltaMusic x.x, modified by Tronic") | Tronic's Delta Music variant (it took the Tronic files) | open `Tronic` | 7 I, 1 S | A | V |
| DIGIbooster | DigiBooster 1.60 | DigiBooster 1.x | open `Digibooster` | 26 S | A | V |
| DigiMugi | Digital_Mugician 1.30 | Digital Mugician 1 / 2 | open `Digital Mugician 1`, `2` | 1: 8 S, 1 I; 2: 1 NE | A | V |
| DSS | DSS 1.40 | Digital Sound Studio | open `Digital Sound Studio` | 30 I | A | V |
| EarAche | EarAche 1.10 | EarAche (it took 3 of the 8 corpus files) | open `EarAche` | 8 I | A | V |
| EMS | EMS 1.20 | EMS before v6 (`ems.`); it refuses the EMS v6 files | open `EMS` | none (EMS v6 has 10 I, another format) | E | U |
| FutureComposer | FutureComposer 0.80 ("FutureComposer V1.02") | Future Composer BSI (it took those files) | open `Future Composer BSI` | 8 I | A | V |
| GlueMon | GlueMon 1.30 | GlueMon | open `GlueMon` | 5 I | A | V |
| GMOD | GMOD 0.80 (Bryan Ford's MultiPlayer code) | GMOD (no file found) | none | none | N | X |
| Hippel-ST_note | Hippel-ST 1.10 | Jochen Hippel ST | open `Jochen Hippel ST` | 8 I | A | V |
| HolyNoise | TheHolyNoise 0.50 ("The Holy Noise V2.01") | Major Tom's format (it took the MajorTom files) | open `MajorTom` | 8 I | A | V |
| Impulse | ImpulseTracker 0.98 | Impulse Tracker (IT), by name; it refused the 4 IT files tried | libxmp port, IT loader | none | N | U |
| JasonPage_note | JasonPage 1.20 | Jason Page (`jpn.` + samples) | open `Jason Page` | 10 S | R | V |
| JasonPageOld | Jason Page Old 0.3 (an Eagleplayer) | Jason Page old (`jpo.`) | open `Steve Turner` | 4 S | A | V |
| JesperOlsen_note | JesperOlsen 1.10 | Jesper Olsen (took 1 of 10 files) | open `Jesper Olsen` | 18 I | A | V |
| LME | LeglessMusicEditor 1.0 | Leggless Music Editor | open `Leggless Music Editor` | 8 I | A | V |
| MaxTrax | MaxTrax 1.0 | MaxTrax (`mxtx.`) | open `MaxTrax` | 5 I, 3 N | R (crashes after its check) | V |
| Medley | Medley 0.95 | Medley | open `Medley` | 8 I | A | V |
| Mline | MusicLineEditor 1.3 | Musicline Editor | open `Musicline Editor` | 28 I | A | V |
| Mon_old | Maniacs of Noise 1.3 | old Maniacs of Noise (it took the Jeroen Tel files) | open `Jeroen Tel` | 5 S, 1 D, 1 Q | A | V |
| MultiTracker | MultiTracker 1.0 | MultiTracker (MTM) | libxmp port, MTM loader | none | E | U |
| Music-Ass_note | Music-Assembler 1.30 | Music Assembler | open `Music Assembler` | 16 I | A | V |
| MusicMaker_note | MusicMaker 1.30 | MusicMaker 4V/8V, by name; it refused the corpus's single-file `MM8.` modules | open `MusicMaker 4V`, `8V` | 4V 1 S; 8V 9 N | N | V |
| NESA_note | NESA 1.23 ("NESA - NES Audio", Morgan Johansson) | NES music (no file found) | none | none | N | X |
| NSF | NSF 1.0 (Eagleplayer; "based on CoolNESs") | NES Sound Format (`NESM`; none in the header index) | none | none | N | X |
| OctaMED | OctaMED 1.20 | OctaMED SoundStudio (MMD3) | open `OctaMED SoundStudio` | 4 I | A | V |
| Paul Shields | Paul Shields 0.2 (an Eagleplayer) | Paul Shields | open `Paul Shields` | 8 S | A | V |
| PierreAdane | Pierre Adane Packer 0.1 (an Eagleplayer) | Pierre Adane Packer (`pap.`) | open `Pierre Adane Packer` | 10 S | R | V |
| Poly | PolyTracker 0.90 | PolyTracker | open `PolyTracker` | 20 S | A | V |
| PowerTracker | PowerTracker 1.20 | PowerTracker | open `PowerTracker` | 2 I (ref 2) | A | V |
| ProRunner21 | ProRunner_2.1 0.90 | ProRunner 2 | ProWizard `prorunner.rs` + built-in ProTracker | none (`ports/prowizard.txt` lists 32 `pru2.` files, no refcmp row) | P | U |
| PSA | PSA 1.0 | Professional Sound Artists | open `Professional Sound Artists` | 8 I | A | V |
| PumaTracker | PumaTracker 1.10 | PumaTracker | open `PumaTracker` | none | E | U |
| QuadraComposer | QuadraComposer 1.40 | Quadra Composer (EMOD) | open `Quadra Composer` | 18 T, 2 S | A | V |
| Quartet | Quartet 0.90 | Quartet (`qpa.` + samples) | open `Quartet` | 13 I | R | V |
| RIFF-WAV | RIFF-WAV 2.30 | RIFF WAVE | built-in decoder `src/riffwave.rs` (PCM, IEEE float, Microsoft ADPCM; not A-law, µ-law, IMA ADPCM, MPEG) | none | P | U |
| RobHubbard_2 | RobHubbard_2 1.31 | Rob Hubbard 2 and Infogrames | open `Rob Hubbard 2`, `Infogrames` | RH2 9 S (ref 9); Infogrames 8 I | A | V |
| SCUMM | SCUMM_Player 1.0 | SCUMM | open `SCUMM` | 6 I, 4 S | A | V |
| SoundControl | Soundcontrol 3.0-5.0 0.1 (an Eagleplayer) | SoundControl 3.x / 5.0 | open `SoundControl` | 3.x 1 S; 5.0 6 S | A | V |
| Soundfactory | Soundfactory 0.50 | Sound Factory | open `Sound Factory` | 8 I, 1 D, 1 F | A | V |
| SoundImages | SoundImages 0.90 | Dave Lowe's later format (it took the Dave Lowe New files) | open `Dave Lowe New` | 18 I | A | V |
| StarTrekker4AM | StarTrekker4AM 0.1 | StarTrekker AM, 4 voices (`.mod` + `.nt`) | open `AudioSculpture` (it claims a StarTrekker module with an `.AS`/`.NT` companion) | 8 S | A (1 of 2 extra files) | V |
| Synthesis | Synthesis 0.80 | Synthesis | open `Synthesis` | 19 I | A | V |
| SynTracker | SynTracker 0.50 | SynTracker | open `SynTracker` | 8 S | A | V |
| TimFollin_note | FollinPlayer_II 1.20 | Tim Follin II | open `Tim Follin II` | 10 I | A | V |
| TME | TME 1.40 | The Musical Enlightenment | open `The Musical Enlightenment` | none (one file in `ident/tme/`) | E | U |
| VSSDeliPlayer | VSS 1.50 | Voodoo Supreme Synthesizer | open `Voodoo Supreme Synthesizer` | 8 I | A | V |

## Eagleplayers (`Eagleplayers/`, 114, the EP206 set)

| File | Name, version | Format | Default-build path | refcmp classes | Evidence | Status |
|---|---|---|---|---|---|---|
| 669 | Composer669 0.80 (a DeliPlayer) | Composer 669 | libxmp port, 669 loader | none | E | U |
| Action Amics | Action Amics 0.2 | Actionamics | open `Actionamics` | 3 S (ref 3) | A | V |
| Activision Pro | Activision Pro 1.8 | Activision Pro / Martin Walker | open `ActivisionPro` | 19 S (ref 19) | A | V |
| ADPCM | ADPCM 0.0 | ADPCM2 / ADPCM3 | open `ADPCM2 / ADPCM3` | 2 N, 1 N (UADE's headerless ADPCM mono, 8 I, is another format that this player refused) | A | U |
| AIFF | AIFF 0.1 | AIFF sample | built-in decoder `src/aiff.rs` | none | P | U |
| A.M.Composer 1.2 | A.M.Composer 1.2 0.1 | A.M.Composer | open `A.M.Composer` | none | E | U |
| AProSys | AProSys 0.1 | AProSys | open `AProSys` | 8 I (ref 8) | A | V |
| Art&Magic | Art&Magic 0.51 (a DeliPlayer) | Art And Magic | open `Art And Magic` | 8 I (ref 8) | A | V |
| Art of Noise | Art of Noise 0.0 | Art of Noise 4V / 8V | open `Art of Noise 4V`, `8V` | 17 S; 7 S, 1 N (ref 23) | A | V |
| AYPlayers/ZXAYAMAD, ZXAYEMUL, ZXAYST11, ZXAYSTRC | not DeliTracker players (sub-players PlayAY loads) | see PlayAY | | | | |
| Benn Daglish | Benn Daglish 0.1 | Ben Daglish | open `Ben Daglish` | 8 S (ref 8) | A | V |
| BFB Master | (loaded as a genie by the engine; "BFBPlayMaster.Library 1.0") | BFB music disks played through `bfbplaymaster.library` and its `bfbplay*.library` replayers | none: the six `modules/N` files of the BFB disk in `dl-modules/BFB/` are unknown to the default path | none | N | X |
| Chiptracker | Chiptracker 0.3 | Chiptracker (KRIS) | ProWizard `chiptracker.rs` + built-in ProTracker (libxmp `chip` loader too) | 10 S | P | V |
| Datatype | (loaded as a genie by the engine; "Datatype Eagleplayer V0.85") | any sound the system datatypes decode | built-in 8SVX, AIFF, RIFF WAVE decoders only | none | N | U (partial) |
| David Hanney | David Hanney 1.1 | David Hanney | open `David Hanney` | 8 S (ref 8) | A | V |
| DavidWhittaker | DavidWhittaker 0.10 | David Whittaker | open `David Whittaker` | 10 S (ref 10) | A | V |
| DeltaMusic 1.0 | DeltaMusic 1.0 2.1 | Delta Music 1.0 | open `Delta Music 1.0` | 13 S (ref 13) | A | V |
| DeltaMusic 2.0 | Deltamusic 2.0 2.2 | Delta Music 2.0 | open `Delta Music 2.0` | 9 S, 1 I (ref 10) | A | V |
| Digibooster | Digibooster 1.1 | DigiBooster 1.x | open `Digibooster` | 26 S (ref 26) | A | V |
| DigiBoosterPRO | DigiBoosterPRO 1.14 | DigiBooster Pro (DBM0) | libxmp port, DigiBooster Pro loader | none (the one "DigiBooster Pro XM" row, 1 I, is an XM file) | E | U |
| Digital Mugician | Digital Mugician 2.7 | Digital Mugician 1 / 2 | open `Digital Mugician 1`, `2` | 8 S, 1 I; 1 NE (ref 10) | A | V |
| DSS | DSS 0.8 | Digital Sound Studio | open `Digital Sound Studio` | 30 I (ref 23) | A | V |
| Dynamic Synthesizer | Dynamic Synthesizer 0.1 | Dynamic Synthesizer | open `Dynamic Synthesizer` | 2 I | A | V |
| EMS | EMS 2.1 | EMS before v6 (`ems.`); it refuses the EMS v6 files | open `EMS` | none | E | U |
| Face The Music | Face The Music 0.7 | Face The Music | open `Face The Music` | 20 S (ref 20) | A | V |
| Fasttracker II | Fasttracker II 1.3 | FastTracker 2 XM | libxmp port, Fast Tracker II loader | 5 I (against libxmp as oracle) | A | V |
| Fred | Fred 2.3 | Fred Editor | open `Fred Editor` | 9 S, 1 N (ref 9) | A | V |
| FutureComposer1.3 | FutureComposer1.3 0.4 | Future Composer 1.3 | open `FutureComposer1.3` | 7 S (ref 7) | A | V |
| FutureComposer1.4 | FutureComposer1.4 0.6 | Future Composer 1.4 | open `FutureComposer1.4` | 4 S (ref 4) | A | V |
| GlueMaster | GlueMaster 2.1 ("ripped from His Master's Noise Reset") | unidentified: it refused the GlueMon files and two His Master's Noise modules | none found | none | N | X |
| GlueMon | GlueMon 1.11 | GlueMon | open `GlueMon` | 5 I | A | V |
| GMOD | GMOD 0.1 ("GMOD Custom player module", Bryan Ford's MultiPlayer) | GMOD (no file found) | none | none | N | X |
| Hippel | Hippel 2.1 | Jochen Hippel (TFMX-type) | open `Hippel TFMX` | 3 S (ref 3) | A | V |
| Hippel_7V | Hippel_7V 1.0 (a DeliPlayer) | Hippel 7 voices | open `Hippel 7V` | 16 S, 7 N | A | V |
| Hippel-COSO | Hippel-COSO 2.12 | Hippel COSO (and a 7V file) | open `Hippel COSO` | 6 S, 2 N (ref 6) | A | V |
| Hippel-ST_note | Hippel-ST 1.10 (a DeliPlayer) | Jochen Hippel ST | open `Jochen Hippel ST` | 8 I | A | V |
| IFF 8SVX | IFF 8SVX 0.2 | IFF 8SVX sample | built-in decoder `src/iff8svx.rs` | none | P | U |
| IFF EMOD | IFF EMOD 0.1 | Quadra Composer (EMOD) | open `Quadra Composer` | 18 T, 2 S | A | V |
| IFF-SMUS | IFF-SMUS 2.2 | IFF SMUS + instruments | open `IFF-SMUS` | 17 S (ref 17) | A | V |
| In Stereo 1.0 | In Stereo 1.0 5.0 | InStereo! 1.0 | open `InStereo! 1.0` | 4 I, 1 S (ref 5) | A | V |
| In Stereo 2.0 | In Stereo 2.0 2.1 | InStereo! 2.0 | open `InStereo! 2.0` | none | E | U |
| JamCracker | JamCracker 1.33 | JamCracker | open `JamCracker` | 9 S (ref 9) | A | V |
| Jason Page New | Jason Page New 0.3 | Jason Page (`jpn.`) | open `Jason Page` | 10 S (ref 10) | A | V |
| Jason Page Old | Jason Page Old 0.2 | Jason Page old (`jpo.`) | open `Steve Turner` | 4 S | A | V |
| J.C.Brooke | J.C.Brooke 3.2 | Jason C. Brooke | open `Jason Brooke` | 11 S | A | V |
| Jesper Olsen | Jesper Olsen 1.0 | Jesper Olsen | open `Jesper Olsen` | 18 I | A | V |
| Laxity-Player | Laxity-Player 2.1 | Laxity (it also took PowerTracker and Digital Sonix & Chrome files) | open `Laxity` | 8 I | A | V |
| LME | LME 2.0 | Leggless Music Editor | open `Leggless Music Editor` | 8 I | A | V |
| Major Tom 2.01 | Major Tom 2.01 2.1 | Major Tom | open `MajorTom` | 8 I | A | V |
| Mark Cooksey | Mark Cooksey 1.0 | Mark Cooksey | open `Mark Cooksey` | 8 S | A | V |
| Mark II | Mark II 2.1 | Mark II | open `Mark II` | 8 I | A | V |
| Mark II Old | Mark II Old 2.0 | old Mark II (Darius Zendeh) | open `Darius Zendeh` | 3 S | A | V |
| MaxTrax | MaxTrax 1.2 | MaxTrax | open `MaxTrax` | 5 I, 3 N (ref 5) | A | V |
| MCMD | MCMD 0.1 | MCMD | open `MCMD` | 7 S, 1 I | A | V |
| MED | MED 0.4 | MED MMD0-MMD2 | open `MED` | 6 S, 1 D, 3 F (ref 10) | A | V |
| M. Grouleff | M. Grouleff 2.0 | unidentified Morten Grouleff format: it refused all 8 EarAche corpus files | none found | none | N | X |
| M.O.N. | M.O.N. 2.0 | Maniacs of Noise | open `Maniacs of Noise` | 10 S (ref 10) | A | V |
| M.O.N. Old | Maniacs of Noise 1.1 (a DeliPlayer) | old Maniacs of Noise (it took the Jeroen Tel and Art And Magic files) | open `Jeroen Tel` | 5 S, 1 D, 1 Q | A | V |
| MPEGAudio | MPEGAudio 0.79 (needs mpega.library) | MPEG audio layers 1-3 (its check took two `.mp2` files) | none: `src/riffwave.rs` leaves MPEG to "other players" | none | R | X |
| Music Assembler | Music Assembler 2.1 | Music Assembler | open `Music Assembler` | 16 I (ref 16) | A | V |
| MusiclineEditor | MusiclineEditor 1.2 | Musicline Editor | open `Musicline Editor` | 28 I (ref 28) | A | V |
| MusicMaker4 | MusicMaker4 0.4 | MusicMaker 4V, by name (refused the corpus files) | open `MusicMaker 4V` | 1 S | N | V |
| MusicMaker8 | MusicMaker8 0.4 | MusicMaker 8V, by name (refused the corpus files) | open `MusicMaker 8V` | 9 N | N | U |
| OctaMED | OctaMED 0.1 | OctaMED | open `OctaMED` | 7 S (ref 7) | A | V |
| Oktalyzer | Oktalyzer 1.45 | Oktalyzer | open `Oktalyzer` | 50 S (ref 50) | A | V |
| PierreAdane | Pierre Adane Packer 0.1 | Pierre Adane Packer | open `Pierre Adane Packer` | 10 S | R | V |
| PlayAY | PlayAY 2.1 (+ AYPlayers) | ZX Spectrum / CPC AY: ZXAYEMUL, ZXAYAMAD, ST11, STRC | built-in Z80 + AY (`src/replay/ay.rs`): EMUL and AMAD; ST11 and STRC not played | none (13 test files in `dl-modules/PlayAY/`, used by unit tests only) | P | U (ST11/STRC: X) |
| PlaySid | PlaySID 0.1 | C64 PSID | `src/sid` (C64 + SID emulation) with `--features sid` (used by the AzuraCast renderer build); left out of the default build only for its licence (GPL-2.0-only) | none | E | U |
| Polytracker | PolyTracker 0.90 (a DeliPlayer) | PolyTracker | open `PolyTracker` | 20 S (ref 20) | A | V |
| Profiteam-Soundfactory | Profiteam-Soundfactory 0.1 | Sound Factory | open `Sound Factory` | 8 I, 1 D, 1 F (ref 8) | A | V |
| Promizer 0.1 | Promizer 0.1a 1.1 | Promizer 0.1 | ProWizard `promizer.rs` + built-in ProTracker | none | P | U |
| Promizer 1.x | Promizer 1.x 0.2 | Promizer 1.x | ProWizard `promizer.rs` + built-in ProTracker | none | P | U |
| Promizer 4.0 | Promizer 4.0 0.1 | Promizer 4.0 | ProWizard `promizer.rs` + built-in ProTracker | none | P | U |
| Protracker4 | Protracker4 1.0 | ProTracker 4 (IFF MODL) | open `Protracker4` | 10 S (ref 10) | A | V |
| PS3M | PS3M 2.4 | S3M, MTM and multichannel MOD | libxmp S3M/MTM loaders, open `Multichannel MOD` | Multichannel MOD 18 S, 1 F (ref 19); S3M and MTM none | A | V |
| PSA | PSA 1.1 | Professional Sound Artists | open `Professional Sound Artists` | 8 I | A | V |
| Pumatracker 1.0 | Pumatracker 1.0 1.1 | PumaTracker | open `PumaTracker` | none | E | U |
| Rob Hubbard | Rob Hubbard 0.3 | Rob Hubbard | open `Rob Hubbard` | 7 S, 1 I (ref 4) | A | V |
| Rob Hubbard 2 | Rob Hubbard 2 2.0 | Infogrames (taken), Rob Hubbard 2 (check passes, banks not loaded) | open `Infogrames`, `Rob Hubbard 2` | 8 I; 9 S | A | V |
| RonKlaren | RonKlaren 2.1 | Ron Klaren | open `Ron Klaren` | 13 I (ref 13) | A | V |
| Scott Johnston | Scott Johnston 1.0 | Scott Johnston | open `Scott Johnston` | 1 I, 4 S (ref 5) | A | V |
| SCUMM | SCUMM 1.2 | SCUMM | open `SCUMM` | 6 I, 4 S | A | V |
| Sean Connolly | Sean Connolly 0.1 | Sean Connolly (it also took Quartet PSG files) | open `Sean Connolly` | 5 S (ref 5) | A | V |
| Sean Conran | Sean Conran 0.1 | Sean Conran (4 of 10 files) | open `Sean Conran` | 10 S | A | V |
| SIDMon 1.0 | SIDMon 1.0 0.4 | SidMon 1.0 (and a UFO file) | open `SidMon 1.0` | 9 I (ref 9) | A | V |
| SIDMon 2.0 | SIDMon 2.0 2.3 | SidMon 2.0 | open `SidMon 2.0` | 10 S (ref 10) | A | V |
| SonicArranger | SonicArranger 4.1 | Sonic Arranger | open `Sonic Arranger` | 9 S (ref 6) | A | V |
| SoundControl | SoundControl 2.1 | SoundControl 3.x / 5.0 | open `SoundControl` | 1 S; 6 S (ref 7) | A | V |
| SoundFX 1.3 | SoundFX 1.3 2.1 | SoundFX 1.3 | open `SoundFX1.3` | 10 I (ref 10) | A | V |
| SoundFX 2.0 | SoundFX 2.0 2.1 | SoundFX 2.0 | open `SoundFX2.0` / `MultiMedia Sound` | 9 S, 1 I | A | V |
| Sound Images | Sound Images 1.0 | Dave Lowe's later format (it took the Dave Lowe New files, not the `tw.` Sound Images files) | open `Dave Lowe New` | 18 I | A | V |
| SoundMaster | SoundMaster 0.1 | Sound Master | open `Sound Master` | 15 S | A | V |
| SoundMon | SoundMon 5.2 | SoundMon 2.0 (and 1.1) | open `SoundMon 2.0` | 6 S (ref 6) | A | V |
| SoundMon 2.2 | SoundMon 2.2 2.1 | SoundMon 2.2 (and 2.0) | open `SoundMon 2.2` | 4 S (ref 4) | A | V |
| Soundtracker 2.6 | Soundtracker 2.6 0.2 | SoundTracker 2.6 / IceTracker | open `SoundTracker 2.6`, `IceTracker` | 8 S; 2 S (ref 10) | A | V |
| SoundTracker Pro II | Soundtracker Pro II 0.1 | Soundtracker Pro II (STP3) | open `Soundtracker Pro II` | 1 I (ref 1) | A | V |
| Stonetracker | StoneTracker 1.60 (a DeliPlayer) | StoneTracker | open `StoneTracker` | 1 F, 1 D (ref 2) | A | V |
| SUNtronic | SUNtronic 0.1 | SUN-Tronic | open `SUN-Tronic` (+ `SUN-Tronic driver`) | 8 S | A | V |
| SUNtronic Custom | SUNTronic Custom 1.2 ("Suntronic CustomPlayer amplifier module") | unidentified: it refused the 8 corpus and 7 `dl-modules/SUNtronic` `.sun` files | none found | none | N | X |
| Symphonie Pro | Symphonie 3.1 0.4 | Symphonie / Symphonie Pro (`.symmod`) | `src/symmod` (behind the libxmp loader list) | none (4 files in `dl-modules/Symphonie/`) | R (crashes after its check) | U |
| Synth 4.0 | Synth 4.0 0.11 | Synthesis | open `Synthesis` | 19 I (ref 19) | A | V |
| SynTracker | Syntracker 0.2 | SynTracker | open `SynTracker` | 8 S (ref 8) | A | V |
| TFMX_1.5 | TFMX_1.5 0.6 | TFMX 1.5 | open `TFMX 1.5` | 5 S, 3 N (ref 5) | A | V |
| TFMX_7V | TFMX_7V 0.16 | TFMX 7V | open `TFMX 7V` | 2 S (ref 2) | A | V |
| TFMX_Pro | TFMX_Pro 0.5 | TFMX Pro | open `TFMX Pro` | 18 S, 1 I, 1 N (ref 16) | A | V |
| THX | THX 2.1 | AHX / THX (it refuses HivelyTracker) | open `AHX` | 11 S | A | V |
| Tim Follin II | Tim Follin II 0.0 | Tim Follin II (and a CustomMade file) | open `Tim Follin II` | 10 I (ref 10) | A | V |
| Tiny Williams | Tiny Williams 0.1 (a DeliPlayer) | Tony Williams' format (`tw.`) | open `Sound Images` | 31 S, 2 I | A | V |
| TME | TME 1.1 | The Musical Enlightenment | open `The Musical Enlightenment` | none | E | U |
| Tronic | Tronic 2.2 | Tronic | open `Tronic` | 7 I, 1 S | A | V |
| Vectordean | Vectordean 4.2 | Richard Joseph Player | open `Richard Joseph Player` | 81 S, 1 I, 1 N, 2 D | A | V |
| VSS | VSS 0.1 | Voodoo Supreme Synthesizer | open `Voodoo Supreme Synthesizer` | 8 I | A | V |
| WAVE | WAVE 0.1 | RIFF WAVE | built-in decoder `src/riffwave.rs` | none | P | U |
| Wizard of Sound | Wizard of Sound 1.3 (needs `EP-WOS.cfg` and instruments) | Wizard of Sound (no file found) | none | none | N | X |
| XMOD | XMOD 0.1 (Bryan Ford's MultiPlayer) | XMOD (no file found) | none | none | N | X |

## Counts

| | DeliPlayers | PlayerStore | Eagleplayers | All 199 |
|---|---|---|---|---|
| V: covered, a corpus file with a refcmp class | 26 | 43 | 87 | **156** |
| U: covered, unverified | 2 | 11 | 18 | **31** |
| X: not covered | 0 | 3 | 9 | **12** |

PlayAY is counted under U (EMUL and AMAD play). The two Datatype players are counted under U as
partial (only 8SVX, AIFF and WAV).

PlaySid is counted under X in the table above but is covered by builds with `--features sid`
(C64 PSID, GPL-2.0-only, so not in the default build): 11 player files are really not covered.

Formats that are not covered (11 player files):

- NES: NESA (`NESA_note`) and NSF (`NSF`);
- Bryan Ford's MultiPlayer formats GMOD (`PlayerStore/GMOD`, `Eagleplayers/GMOD`) and XMOD;
- BFB (`BFB Master`, bfbplaymaster.library music disks);
- MPEG audio (`MPEGAudio`);
- Wizard of Sound;
- formats not identified from any file: `GlueMaster`, `M. Grouleff` (not the EarAche modules),
  `SUNtronic Custom` (not the `.sun` modules);
- also ZX AY types ST11 and STRC (PlayAY), which `src/replay/ay.rs` does not play;
- also the formats the Datatype players get from system datatypes beyond 8SVX, AIFF and WAV.

Covered but unverified (no corpus file, or NOREF rows only):

- PC trackers on the libxmp port: S3M, IT, MTM, Composer 669, DigiBooster Pro DBM0. Corpus S3M/MTM
  files exist only as PS3M's multichannel MODs;
- samples: IFF 8SVX, AIFF, RIFF WAVE;
- AY (ZXAYEMUL/AMAD), unit-tested on 13 files outside the corpus;
- Symphonie / Symphonie Pro;
- ProWizard paths: ProRunner 2, Promizer 0.1, 1.x, 4.0;
- Amiga formats: A.M.Composer, EMS before v6, PumaTracker, The Musical Enlightenment (TME),
  InStereo! 2.0;
- NOREF only: MusicMaker 8V, ADPCM2/3.

## Name collisions found by the runs

Some Eagleplayer and DeliPlayer names stand for another format than the open replayer of the same or a
similar name. Watch for these when choosing a reference:

- Eagleplayer "Sound Images" and PlayerStore "SoundImages" play the modules the open path calls
  `Dave Lowe New`. Open `Sound Images` (`tw.`, Tony Williams) is what Eagleplayer "Tiny Williams" plays.
- "Jason Page Old" plays the `jpo.` files: open `Steve Turner`.
- "M.O.N. Old" and PlayerStore "Mon_old" (both "Maniacs of Noise") play the open `Jeroen Tel` files.
  "M.O.N. Old" also takes Art And Magic files.
- PlayerStore "Delta" (Tronic's DeltaMusic) plays open `Tronic`. PlayerStore "FutureComposer" (V1.02)
  plays open `Future Composer BSI`. PlayerStore "HolyNoise" plays open `MajorTom`.
- "IFF EMOD" is Quadra Composer, "Synth 4.0" is Synthesis, "Vectordean" (and `VectorDean_note`) is
  Richard Joseph Player, "Mark II Old" is Darius Zendeh, "FredMonitor" is Fred Editor.
- Overlapping checks (one file taken by two formats' players): Whittaker players take a Rob Hubbard
  file; "SIDMon 1.0" takes a UFO file; "Sean Connolly" takes Quartet PSG files; "Tim Follin II" takes a
  CustomMade file; "Laxity-Player" takes PowerTracker and Digital Sonix & Chrome files; "SoundMon 2.2"
  takes SoundMon 2.0 files.

## DeliGenies (28)

The engine runs only the NotePlayer genies (`NOTE_GENIES`, for `_note` players) and tells the others
about each module. The default build runs no 68k player, so no genie is needed to play anything. The
table says what each genie does (from its own description strings) and what the default build has
in its place.

| File | Name, version | What it does | Default build |
|---|---|---|---|
| 4Voc-NotePlayer | 4Voices-NotePlayer 1.90 | NotePlayer: renders a `_note` player's NoteStruct on 4 voices | not needed: no `_note` player runs. The open replayers write Paula (or the host amplifier `src/amplifier.rs` for up to 32 voices) |
| 8Voc-NotePlayer | 8Voices-NotePlayer 2.10 | NotePlayer, 8 voices | not needed (as above) |
| GenieStore/32Voc-NotePlayer | 32Voices-NotePlayer 1.90 | NotePlayer, up to 32 voices | not needed (as above) |
| GenieStore/32Voc-NotePlayerPlus | 32Voices-NotePlayer+ 2.10 | NotePlayer, 32 voices, modules in FAST memory | not needed (as above) |
| GenieStore/14Bit-NotePlayer | 14Bit-NotePlayer 2.30 | NotePlayer, 32 voices, 14-bit output (Christian Buchner's routine) | not needed: the host mixes at full resolution |
| Headphone-NotePlayer | Headphone-NotePlayer 1.90 | NotePlayer, 32 voices, for headphones | not needed. Stereo separation is `--separation` / `--mono`; no headphone filter |
| GenieStore/ModSave-NotePlayer | ModSave-NotePlayer 2.10 | NotePlayer that saves the module as an IFF-8SVX mono/stereo sample (up to 16 channels) | `--wav` / `amigaplayer-render --out` (WAV or raw, not 8SVX) |
| Converter | Converter 1.20 | converts "exotic" module formats into common ones (its strings name none) | not needed: the open path plays formats directly; which formats it converted is unknown |
| Decruncher | Decruncher 1.31 | decrunches packed files; its strings name Imploder (`IMP!`), LhA, GZip, Zip, LZX | `src/decrunch` (PP20, XPK methods, Pack-Ice, S404, MMCMP, CrunchMania, PowerPlayer) and the `archives` feature (LhA, LZX, zip, gzip, xz, tar). No Imploder (`IMP!`) depacker found in `src/decrunch` |
| Deli-Wizard | Deli-Wizard 2.20 (Nicolas Franck, "based on Pro-Wizard convert-routines") | converts packed modules to Noise/ProTracker | `src/prowizard` (47 converter files) + the built-in ProTracker |
| GenieStore/ST-NT-PT_Conv | ST-NT-PT-Song_Converter 1.0 | turns Sound/Noise/ProTracker song files into modules | none found. Song files without samples are not played |
| GenieStore/ST-Cruncher | ST-Cruncher 0.40 | decrunches modules packed with SoundTracker-Cruncher (OTT Design) | none found |
| GenieStore/modCRUSHER | modCRUSHER 0.60 | decrunches modules packed with modCRUSHER (Olly Koenders) | none found |
| GenieStore/UnMMCMP-Genie | UnpackMMCMP 1.0 | unpacks MMCMP | `src/decrunch/mmcmp.rs` |
| GenieStore/ImpulseDecruncher | ImpulseDecruncher 1.0 | no description string; its strings name only `IMPM`, the Impulse Tracker magic (probably IT sample unpacking for the Impulse DeliPlayer; not verified) | the libxmp IT loader unpacks compressed IT samples itself (`src/xmp/loaders/itsex.rs`) |
| ModuleInfo | ModuleInfo 1.80 | shows information about the active module | the window's module info; `amigaplayer-render --info` |
| MonoScope | MonoScope 1.90 | oscilloscope | the window's oscilloscopes (`src/ui.rs`) |
| QuadraScope | QuadraScope 1.80 | 4-channel oscilloscope | same |
| GenieStore/QuadraScopePlus | QuadraScope+ 1.13 | 4-channel oscilloscope | same |
| GenieStore/StereoScope | StereoScope 1.90 | stereo oscilloscope | same |
| GenieStore/FFTScope | FFTScope 1.0 | FFT display | none (display only, not needed) |
| GenieStore/FFTAnalyzer | FFTAnalyzer 1.60 | FFT spectrum analyzer | none (display only) |
| GenieStore/Analyzer | Analyzer 1.40 | simple spectrum analyzer | none (display only) |
| GenieStore/Levelmeter | Levelmeter 1.30 | level meter | none (display only) |
| GenieStore/LightOrgan_Color | LightOrgan_Color 1.80 | colour light organ (OS 3.0, a free pen) | none (display only) |
| GenieStore/DrWhoGenie | DrWhoGenie 1.20 | shows a rotating tunnel | none (effect) |
| GenieStore/NewYearGenie | NewYearGenie 1.30 | season's greetings | none (effect) |
| GenieStore/Xmas-Genie | Xmas-Genie 1.1 | season's greetings | none (effect) |

Genie functions the default build lacks and that affect which files play: the Imploder, SoundTracker-Cruncher
and modCRUSHER depackers, and ST/NT/PT song-file conversion. None was checked on a file: the corpus
has no such packed file.
