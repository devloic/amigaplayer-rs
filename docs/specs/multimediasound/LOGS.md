# MultiMedia Sound reference logs

Taken with the reference 68k player ("MultiMedia Sound", UADE player file `MultiMedia_Sound`) run
through the host with DELI_WRITE_LOG, `--player 'MultiMedia Sound'`, silent (`--wav`, dummy audio
driver), `--subsong N` as listed. Lines: `ADDRESS VALUE COLOUR-CLOCK`; the `# module` line gives the
load address and size. Each log starts with the host's 8 volume writes and the filter write (SPEC
3.2), then the ticks. The host stops a few ticks after the first song end (SPEC 5.6), or at the time
limit of the run (`syn_twoempty.s1`, `syn_speed1`).

`E0` = colour clock at which the host entered the player's interrupt routine for the first tick;
ticks follow every `(tempo + 1) * 5` cc exactly (SPEC 4.1, 8.1). "writes" counts the lines after the
`# module` line.

Corpus modules (SoundFX 2.0 format files, named `SFX2.*` in the archive; Roman Werner, except
EuphonicBass by DJ Tekjam). The modules themselves are not included.

| log | module | subsong | tempo | tick spacing (cc) | ticks | E0 | writes | exercises |
|---|---|---|---|---|---|---|---|---|
| CafeDeParis | SFX2.CafeDeParis | 0 | 13277 | 66390 | 2307 | 73180 | 7841 | volume down, STP, BRK |
| ClownOMania | SFX2.(Clown-O-Mania [DFC])clown-o-mania | 0 | 12934 | 64675 | 3460 | 81340 | 14484 | steps, 2 subsongs |
| ClownOMania.s1 | same | 1 | 12934 | 64675 | 6531 | 84526 | 28001 | |
| CobraStrikesAgain | SFX2.CobraStrikesAgain | 0 | 15529 | 77650 | 9135 | 85749 | 44685 | arpeggio, filter byte 0xFF |
| ElefantPolka | SFX2.ElefantPolka | 0 | 13437 | 67190 | 6144 | 74478 | 22284 | step down, volume down |
| EuphonicBass | SFX2.Euphonic Bass | 0 | 14693 | 73470 | 9655 | 85443 | 37333 | samples 17..31, many STP, steps |
| Genesis | SFX2.Genesis | 0 | 11143 | 55720 | 3074 | 62447 | 10805 | |
| JungleFever | SFX2.JungleFever | 0 | 15743 | 78720 | 2306 | 85235 | 10465 | filter byte 0xFF |
| LeonardoSong | SFX2.(Leonardo [DFC])LeonardoSong | 0 | 13952 | 69765 | 8068 | 113388 | 39299 | arpeggio, steps, 5 subsongs |
| LeonardoSong.s1 | same | 1 | 13952 | 69765 | 772 | 116574 | 3629 | |
| LeonardoSong.s2 | same | 2 | 13952 | 69765 | 4992 | 119760 | 23427 | |
| LeonardoSong.s3 | same | 3 | 13952 | 69765 | 3457 | 122946 | 13053 | |
| LeonardoSong.s4 | same | 4 | 13952 | 69765 | 1154 | 126132 | 4174 | |
| MixedStyle | SFX2.MixedStyle | 0 | 14719 | 73600 | 9638 | 90624 | 46640 | arpeggio, command 9 (no effect) |
| PaceOfPleasure | SFX2.PaceOfPleasure | 0 | 14743 | 73720 | 5066 | 89763 | 23472 | BRK, 2 subsongs |
| PaceOfPleasure.s1 | same | 1 | 14743 | 73720 | 1924 | 92949 | 7068 | |
| PirateParty | SFX2.PirateParty | 0 | 9096 | 45485 | 3462 | 52469 | 11655 | |
| RootBeerRag | SFX2.RootBeerRag | 0 | 7820 | 39105 | 18140 | 53742 | 64364 | fast tempo, 55 positions |
| RunninUp | SFX2.RunninUp | 0 | 11647 | 58240 | 1540 | 64550 | 6166 | no commands |
| WorkingInTheMine | SFX2.WorkingInTheMine | 0 | 13791 | 68960 | 2690 | 75033 | 8098 | the 95 688-byte special case |

Synthetic modules (made for this spec, in `modules/`):

| log | module | subsong | tempo | tick spacing (cc) | ticks | E0 | writes | exercises |
|---|---|---|---|---|---|---|---|---|
| syn_effects | mms.effects | 0 | 14187 | 70940 | 464 | 77346 | 1927 | every command, PIC/STP/BRK/0xFFFB/0xFFFF words, samples 17..19, zero-length sample, speeds 3, 2, 8, 6, 5 (F25) |
| syn_edge | mms.edge | 0 | 14187 | 70940 | 499 | 77147 | 131 | steps from periods outside the table, F20 (speed 0 -> 1) |
| syn_speed1 | mms.speed1 | 0 | 14187 | 70940 | 999 | 77147 | 31 | F01: the 65 535-tick stall |
| syn_subsongs | mms.subsongs | 0 | 12000 | 60005 | 644 | 91976 | 2210 | 4 subsongs (empty and STP-only separators), BRK |
| syn_subsongs.s1 | same | 1 | 12000 | 60005 | 585 | 95162 | 1691 | |
| syn_subsongs.s2 | same | 2 | 12000 | 60005 | 384 | 98348 | 1140 | |
| syn_subsongs.s3 | same | 3 | 12000 | 60005 | 514 | 101534 | 2252 | |
| syn_fasttempo | mms.fasttempo | 0 | 6000 | 30005 | 2364 | 36341 | 9606 | same orders as mms.subsongs, tempo below the split limit: 1 subsong |
| syn_twoempty | mms.twoempty | 0 | 14000 | 70005 | 1013 | 97793 | 3230 | split aborted by two empty patterns |
| syn_twoempty.s1 | same | 1 | 14000 | 70005 | 1519 | 100979 | 6516 | the subsong without an end (runs past the order list) |
| syn_ledff | mms.ledff | 0 | 14187 | 70940 | 384 | 77156 | 1112 | filter byte 0xFF |
| syn_led55 | mms.led55 | 0 | 14187 | 70940 | 384 | 77169 | 1112 | filter byte 0x55 |

Validation: a model written from SPEC.md, starting tick `j` at `E0 + j * spacing`, reproduces every
write of every log after the 9 host writes (address, value, order and colour clock).
