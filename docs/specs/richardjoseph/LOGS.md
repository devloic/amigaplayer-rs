# Richard Joseph: reference logs

All logs are register-write logs (lines `ADDRESS VALUE COLOUR-CLOCK`, first line `# module ...`)
of the reference 68k player run through the host with DELI_WRITE_LOG: the Eagleplayer
"Richard Joseph" (UADE players directory) selected with `--player 'Richard Joseph'`, empty
DeliTracker player directories, `--wav` output with `SDL_AUDIO_DRIVER=dummy`, `--seconds N`,
`--subsong k` where given. Execution traces (DELI_TRACE) of the same runs were used for the cost
tables; they are not shipped.

## Modules (all synthetic, in `modules/`)

No real `RJ.*` file exists in the archive (the `rj`-prefixed files there belong to the Richard
Joseph Player, `rjp.*`/`smp.*`, a different format). The containers were built by wrapping the
music driver and data of two DeliTracker custom rips (Cadaver, Speedball 2) into the RJ layout of
SPEC 1.1: the chosen hunks of the rip concatenated into one chip hunk behind the 68-byte header,
the rip's relocations kept and the six header pointers added to them.

| file | variant | source hunks | header (image offsets): Play, Audio, InitPlayer, InitSong, SampleInfo, EndSampleInfo; subsongs |
|---|---|---|---|
| RJ.Cadaver | C | rip hunk 1 at image 0x48 | 0x40F8C, 0x40F18, 0x410FC, 0x008B6 (the game's title-tune routine), 0x40D62, 0x40EDC; 1 |
| RJ.Cadaver16 | C | same | same, InitSong 0x412B4 (the driver's song-table start routine); 9 |
| RJ.CadSynA | C | RJ.Cadaver with new music data | as RJ.Cadaver |
| RJ.Speedball2 | S | rip hunks 0, 1, 2 at image 0x48, 0x12D4, 0x17A8 | 0x005BC, 0x0055C, 0x00732, 0x00128 (a request of song n with priority 15), 0x2EF92, 0x2F1C4; 3 |
| RJ.Speedball2m | S | RJ.Speedball2 with song entry 1 = entry 0 (the main tune as subsong 0) | as RJ.Speedball2 |
| RJ.SB2SynA | S | RJ.Speedball2 with new music data | as RJ.Speedball2 |

Load address (`base`) in all reference runs: 0x46A30 for the C files, 0x33DE0 for the S files.

Synthetic data (no code changed): RJ.CadSynA replaces modulation sets 6..8, envelopes 12..15 and
patterns 1, 2, 4, 5, 6, 7 (new pattern bytes at image 0x9E8 onward, pattern-table entries
pointing at them). RJ.SB2SynA puts the same modulation sets and patterns in the S tables
(envelopes 0 and 1, as the S envelope table has room for two; envelope numbers in the patterns
remapped), and song entry 1 = priority 0, flags 0, patterns 1, 2, 4, 7. The patterns use every
command of SPEC 6.4 (including 0x84/0xA0/0xA4, 0x98, call/return, nested loop with counts 0 and
2, jump, transpose, stop and restart), all envelope phases and all modulation stages with and
without loop flags.

## Logs

| log | module, subsong, seconds | writes | notes |
|---|---|---|---|
| cadaver_title_s0 | RJ.Cadaver 0, 30 | 13 507 | real title music, loops |
| cadaver16_s0, s4, s8 | RJ.Cadaver16 0, 4, 8, 20 | 8 538, 8 340, 8 604 | jingles from the song table; s4, s8 include the subsong stepping (InitSound 1..k+1) |
| cadsynA_s0 | RJ.CadSynA 0, 40 | 2 453 | song end in the tick at 16 804 257 (notification at 16 807 359, a restart command); the host stops the render shortly after |
| speedball2_s0 | RJ.Speedball2 0, 30 | 12 307 | subsong 1 of the file is a short effect: song end (stop commands) at clock 327 264; the host skips to the next subsong (see below) |
| speedball2_s1, s2 | RJ.Speedball2 1, 2, 30 | 12 163, 12 214 | subsong stepping |
| speedball2m_s0 | RJ.Speedball2m 0, 60 | 48 722 | the main in-game tune |
| sb2synA_s0 | RJ.SB2SynA 0, 40 | 2 576 | S-variant commands, envelopes, modulation; song end, render stops |

Host timeline in these runs (SPEC 8.2): InitSound(1) starts at 1 391 144 (RJ.Cadaver, scan
132 393 words), 1 458 (RJ.Cadaver16), 11 885 (RJ.Speedball2); the first tick 70 940 after the
end of the last start-up InitSound.

Song-end restart in speedball2_s0: the song end is signalled in the tick at 314 811; the host
calls EndSound at 385 684 (70 873 cc after that tick, i.e. 67 cc before the next grid tick; the
host's choice, an audio-buffer boundary), then two volume settings (118 cc each), InitSound(2)
at 385 968 which ends at 395 667; the overdue tick runs at 395 667, the following one at 456 691
(on the original grid).

## What they confirm

- A private model written from SPEC.md reproduces every log above exactly: address, value, order
  and absolute colour clock of every write (about 129 000 writes), with the host timeline
  computed from SPEC 8.2 (only the EndSound clock of the speedball2_s0 restart is taken from the
  log).
- Raster waits: every Audio emulation call contains the nine-line wait of SPEC 8.6; it is visible
  in the logs as roughly 2 000 cc between a DMACON set and the following location/length writes
  of the emulated interrupt.
- The 1-cycle re-basing at the song-end notification (SPEC 8.1) is needed for cadsynA_s0 (one
  write would be 1 cc late without it).

## Typical gaps (cc), cadaver_title_s0

Voice start, channel 0: DMACON clear -> VOL 14, VOL -> LC 12, LC -> LEN 8, LEN -> PER 18,
PER -> DMACON set 8, DMACON set -> the emulated interrupt's LC 2 069 (nine raster lines plus costs).
Writer pass: PER -> VOL 98/99, 107, 110/111, 124 for channels 0..3; VOL -> next channel's PER
144, 159/160, 166 (channels 0->1, 1->2, 2->3).
