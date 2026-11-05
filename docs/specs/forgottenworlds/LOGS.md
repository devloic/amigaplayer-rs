# Forgotten Worlds: reference write logs

Logs are DELI_WRITE_LOG output of the reference 68k player run through the host (`ADDRESS VALUE COLOUR-CLOCK`
per line, hex address/value, decimal colour clock; the first line is `# module ADDRESS SIZE`). Each run was
made with `--seconds 30`, audio to a WAV file with the dummy SDL driver, no other options (the player is chosen
by the module's `FWMP` magic). `tools/replay-cmp.py REF.log PORT.log` can compare them; note it starts a new tick
at gaps over 20000 cc, so the catch-up ticks that follow a long stall (7.2 of the spec) are merged with the
tick before them, and the long stalls themselves show as one very long tick.

The first 24 lines of every log are the song-initialisation writes (spec 7.1). The first tick's
DMACON write comes 71249 cc after the first line.

## Modules

Five real modules exist in the archive (`fw.game_end`, `fw.jingle`, `fw.level_1_and_3`, `fw.level_2_and_4`,
`fw.title`), all rips from the game.

| module | size | md5 | channel-0 note on tick 0 | reference result |
|---|---|---|---|---|
| game_end | 17842 | 1f814649dd6e69de1f3bef5846d0da7b | no (first notes on channels 1 and 3) | plays (1499 ticks in 30 s) |
| level_1_and_3 | 29024 | c707e5c2815feb877632bcabdad6e7d7 | no | plays |
| title | 22400 | 481d5559f1abd18a8ad9d219adc98a81 | yes | reference hangs in the first tick |
| jingle | 11396 | b0fec3aedb46caca12a4f8fe9f92883d | yes | reference hangs in the first tick |
| level_2_and_4 | 34634 | 6f1c48c832d648d04699beed29b4d150 | yes | reference hangs in the first tick |

"Hangs": the host reports "guest code did not return" after 30 write lines, the wait of step 5 (W1) of
the first note start on channel 0 never ends (apparently the host's Paula does not raise the audio interrupt flag because the
channel's DMA was switched off after only one word of the silence buffer had been fetched). Logs of those
runs (`*.hang.log`, 30 lines) are kept as evidence only.

## Synthetic variants (real data, one change)

To get complete logs for the three hanging tunes, a variant of each was made that starts channel 0 three ticks
late: after padding the file to even length, append two bytes `86 03` (rest of 3 ticks) followed by a byte-exact copy
of channel 0's event stream (from its first byte up to and including the first stream-level `82`
or `80` command), and set channel 0's start offset (the first 16-bit word after the header, offset 16) to the
position of the appended bytes minus 16. Appended sizes: title 9 bytes (stream of 7: `88 00 88 03 88 04 82`),
jingle 39 (stream of 37, ends with `80`), level_2_and_4 33 (stream of 31). The logs of these variants:
`title.patched.log` (module size 22409), `jingle.patched.log` (11435), `level_2_and_4.patched.log` (34667). These
variants differ from the originals only in channel 0's first three ticks (nothing is written there) and in
the position of channel 0's stream in the file.

## Logs

| log | module | lines | ticks | note starts per channel (0/1/2/3) |
|---|---|---|---|---|
| game_end.log | game_end | 9373 | 1499 | 52 / 32 / 0 / 125 |
| level_1_and_3.log | level_1_and_3 | 12919 | 1497 | 61 / 172 / 61 / 47 |
| title.patched.log | title (variant) | 8687 | 1499 | 48 / 50 / 63 / 77 |
| level_2_and_4.patched.log | level_2_and_4 (variant) | 10942 | 1499 | 150 / 32 / 150 / 28 |
| jingle.patched.log | jingle (variant) | 6638 | 1499 | 17 / 0 / 12 / 10 |
| title.hang.log, jingle.hang.log, level_2_and_4.hang.log | originals | 30 | - | - |

Tick count = DMACON 0x800F writes. In the jingle logs the song ends at tick 320 for channel 2 and tick 321
for channel 3 (the 0x80 commands); the reference behaviour after the end is garbage (README) and the
tick at 577 is where the model of the spec first diverges.

Clock facts from the logs: 50 Hz tick grid 70940 cc exactly (1377 of 1499 tick starts in game_end are on the grid; the others are
catch-up ticks after the stalls described in spec 7.2/7.3); an on-time tick starts at
`first tick + 70940*k` exactly.

## Validation

A private model was written only from the numbers in SPEC.md (event decoding, envelope, note-start
sequence, per-turn costs, tick schedule). Run against the five usable logs:

* functional: write sequence (address and value) identical for every tick of game_end (1499), level_1_and_3
  (1497), title variant (1499) and level_2_and_4 variant (1499), and for the 320 ticks of jingle variant before the
  tune ends.
* tick start clocks: all of them predicted exactly (including the catch-up ticks).
* write clocks, with the Paula-dependent waits W1, W2 and W3 taken from the log (the iteration count k that
  makes the following write land where it did; W4 predicted from the line-boundary rule except for the notes
  whose flag is missed, whose wait length is taken from the log): of 43748 audio writes 43475 (99.4%) are on
  the exact clock, 69 are one clock off, 204 are 2 to 5 clocks early or late. The 204 are the first tick of
  each tune (first tick, first note sequences) and a few ticks in which one channel calls a pattern and then
  selects an instrument (about 3 clocks).
