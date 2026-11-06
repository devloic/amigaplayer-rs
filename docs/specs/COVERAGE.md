# Coverage of the Eagleplayer set by open replayers

Reconciled 2026-11-05 against the 178 player files in the UADE player directory
(`~/.local/opt/uade/share/uade/players`). The original "63 players without an open replayer" list (from the format comparison report,
`fixround/formatdiff/REPORT.md` section 2) was recovered and is checked below. Every player is now either covered by a clean-room open replayer
(spec in this folder or a clean upstream source) or is a variant name of a covered one.

Player files whose name does not appear verbatim in `src/` (16), and what plays them:

| Player file | Covered by |
|---|---|
| ChipTracker | ProWizard converter `src/prowizard/chiptracker.rs` + libxmp `chip` loader |
| ImagesMusicSystem | ProWizard `ims.rs` + libxmp `ims` loader |
| MagneticFieldsPacker | ProWizard `mfp.rs` + libxmp `mfp` loader |
| Mosh_Packer | ProWizard `mosh.rs` |
| NovoTradePacker | ProWizard `novotrade.rs` |
| Titanics_Packer | ProWizard `titanics.rs` |
| Octa-MED-Ss | OctaMED / OctaMED SoundStudio replayers |
| DeltaMusic1.3 | Delta Music replayers |
| EMS-6 | EMS v6 replayer |
| MugicianII | Mugician replayers |
| DaveLowe_Deli | Dave Lowe replayers (specs `dl`, `dl2`, `dl3`) |
| JesperOlsen_EP | `src/replay/jesperolsen.rs` |
| JochenHippelCOSO, JochenHippel-CoSo_UADE, JochenHippel_UADE | Hippel COSO / 7V / ST replayers |
| TFMX-TFHD | `src/replay/tfhd.rs` (spec `tfhd`) |

Formats whose port is not yet register-exact against the 68k reference (all have specs and tests):
TFMX ST (start-up delay of the reference not predicted), AProSys (g11-g14 timing), Paul Tonge (TUNE2,
TUNE5, title start phase), Nick Pelling Packer (first-write origin), MaxTrax (tick length and first
tick; patch not merged), Soundtracker 2.6 (Bach_Bell, Italo_2: host RAM past the module), ZoundMonitor
(first-tick clock), and the ports checked only on synthetic modules (see each spec README).

## The original 63 (recovered)

ADPCM_mono, Anders_0land, AProSys, ArtAndMagic, AudioSculpture, BenDaglish-SID, BladePacker, DariusZendeh,
DavidHanney, DynamicSynthesizer, EarAche, ForgottenWorlds_Game, FredGray, FutureComposer-BSI, FuturePlayer,
GlueMon, HowieDavies, Infogrames, Janne_Salmijarvi_Optimizer, Jochen_Hippel_ST, Kim_Christensen, Laxity,
LegglessMusicEditor, MajorTom, Mark_Cooksey, Mark_Cooksey_Old, MarkII, Maximum_Effect, MCMD, Medley,
MIDI-Loriciel, Nick_Pelling_Packer, NTSP-system, onEscapee, PaulSummers, Paul_Tonge, Pokeynoise,
ProfessionalSoundArtists, Quartet, Quartet_PSG, Quartet_ST, RichardJoseph, RiffRaff, RobHubbard, ScottJohnston,
SeanConnolly, SoundProgrammingLanguage, Special-FX_ST, SpeedyA1System, SpeedySystem, SUN-Tronic, SynTracker,
TFMX-7V-TFHD, TFMX-Pro-TFHD, TFMX_ST, TFMX-TFHD, ThomasHermann, TomyTracker, Tronic, UFO,
VoodooSupremeSynthesizer, YM-2149, ZoundMonitor.

Checked 2026-11-05: every one of the 63 has a spec folder in `docs/specs/` and an open replayer
`src/replay/<slug>.rs` with tests (TFMX-TFHD, 7V and Pro share `tfhd`; MCMD, MIDI-Loriciel and the rest have
their own). CustomMade and Protracker4 were added beyond the list. Accuracy against the 68k reference is the
open part, listed above.
