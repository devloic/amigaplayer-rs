# Future Composer BSI (Amiga, `BFC.*`, `BSI.*`, `FC-BSI.*`) replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Reference: the UADE player "FutureComposer BSI" (player file `FutureComposer-BSI`, "replay by Tony
Bybell, adapted for UADE by mld", April 2002), run through the host with DELI_WRITE_LOG. In the host it
registers as `FutureComposer BSI` (with a space); `--player 'FutureComposer BSI'` selects it.

This format has nothing in common with Future Composer 1.3 / 1.4 (`SMOD` / `FC14` modules, played by
`src/replay/futurecomposer.rs`) except the name. There are no frequency or volume sequences, no sound
macros, no 3-byte pattern rows and no song-speed table. A BSI module has fixed-size blocks: four
per-voice sequences of one-byte entries, 64 byte-coded patterns (notes and commands in one byte stream
per voice, each voice running its own pattern independently), and a table of plain PCM instruments.
Nothing in the FC 1.3/1.4 code can be reused beyond Paula output.

Numbers are decimal unless written 0x...; "word" = 16 bits, "long" = 32 bits, big-endian.

## 1. File format

### 1.1 Layout

All offsets are from the start of the file. The layout is fixed:

| offset | size | content |
|---|---|---|
| 0x0000 | 4 | magic `FUCO` (0x46 0x55 0x43 0x4F) |
| 0x0004 | 4 x 256 | sequences, voice 0..3 (voice `v` at 0x0004 + 256 v) |
| 0x0404 | 64 x 256 | patterns 0..63 (pattern `p` at 0x0404 + 256 p) |
| 0x4404 | 4 | unused |
| 0x4408 | 64 x 16 | instrument records 0..63 |
| 0x47FC | rest | sample data |

Instrument record `i` (at 0x4408 + 16 i), four longs:

| +0 | +4 | +8 | +12 |
|---|---|---|---|
| ignored (the player overwrites it) | length in bytes | loop start, bytes from sample start | loop length in bytes (0 = no loop) |

Sample data is packed in instrument order: instrument `i`'s sample starts at
`0x47FC + (sum of the length fields of instruments 0 .. i-1)`. The sample block starts 12 bytes before
the end of the record table, so record 63 overlaps the first 12 bytes of sample data (its length field
is the first long of sample data). Record 63 cannot be selected by the pattern commands (1.3), so this
only matters in that its fields are never used; records 0..62 are the usable instruments.

The player keeps every pointer as an address in the loaded module. Reads past the end of the file are
not checked by the reference (it reads whatever follows in memory); a port should read 0 there.

### 1.2 Detection

The first four bytes are `FUCO`. Nothing else is checked by the reference. A port should also require
the file to be at least 0x47FC bytes long (shorter files would make the reference read outside the
module).

### 1.3 Sequences (one per voice)

A voice's sequence is read with an 8-bit position (0..255, wrapping within the voice's 256 bytes; it
starts at 0). Entry byte `b`:

| b | meaning |
|---|---|
| 0x00..0x3F | play pattern `b`; the position moves past it and reading stops |
| 0x40..0x7F | repeat count: the next pattern is played `b - 0x3F` times (1..64); continue reading |
| 0x80..0xBF with `b & 0x3F` <= 0x23 | transpose := `b & 0x3F` (0..35 semitones, never negative); continue |
| 0x80..0xBF with `b & 0x3F` >= 0x24, and 0xC0..0xFB | audio filter: LED bit := bit 0 of `b` (see 3.6); continue |
| 0xFC `x` | volume := `x` (0..255), written to the voice's AUDxVOL at once (3.6); position moves past both bytes; continue |
| 0xFD `x` | jump: position := `x` (the byte after 0xFD in the same voice's sequence); repeat count := 1; continue |
| 0xFE | song end (3.7) |
| 0xFF | position := 0; repeat count := 1; continue |

"Continue" means the next entry is read at once, in the same tick. Every entry read (including each of
0xFD and 0xFF) counts towards the read limit of 3.5. The value byte `x` of 0xFC / 0xFD is not counted
separately. The position of the byte after 0xFC / 0xFD wraps within 0..255 like any position.

### 1.4 Patterns

A pattern is a byte stream; the byte read is at `0x0404 + idx`, where
`idx = (pattern_position + 256 * pattern_number)` taken as a 16-bit value and then as signed 16-bit
(pattern_number is 0..63, so in practice `idx` is positive). Byte `b`:

| b | meaning |
|---|---|
| 0x00..0x2F | note `b` (3.4); ends the voice's reading for this tick |
| 0x30..0x6C | portamento speed := `2 * (b - 0x30)` (0, 2, .., 120) |
| 0x6D | portamento mode := down (period grows) |
| 0x6E | portamento mode := off |
| 0x6F | portamento mode := up (period shrinks) |
| 0x70..0x7F | global speed := `b & 0x0F` |
| 0x80..0xBF | duration := `b & 0x3F`, with 0 meaning 64 |
| 0xC0..0xFE | instrument := `b & 0x3F` (0..62) |
| 0xFF | end of pattern (3.3) |

All values except notes and 0xFF are commands: they take effect and reading continues with the next
byte in the same tick.

Pattern position is a 16-bit counter with an odd rule (it matters only for patterns of 255+ bytes
without an end marker):
- a command increments only its low byte (0xFF wraps to 0x00, the high byte is unchanged);
- a note increments the whole 16-bit value;
- a pattern change (3.3) sets it to 0; at song start it is 0xFFFF.
- The "end of pattern reached" test (3.2) looks only at the low byte being 0xFF.

So a pattern without 0xFF ends when its position reaches 255 (the byte at offset 255 is not played),
except when offset 255 is reached by commands within the reading of one tick: then a note at offset 255
makes the position 0x0100, and reading continues in the following pattern's bytes (pattern_number + 1
at offset 0, ...), until the low byte next reaches 0xFF.

### 1.5 Period table

84 periods, indexed by `note + transpose` (at most 47 + 35 = 82):

```
1712, 1616, 1520, 1440, 1360, 1280, 1200, 1136, 1072, 1016, 960, 904
856, 808, 760, 720, 680, 640, 600, 568, 536, 508, 480, 452
428, 404, 380, 360, 340, 320, 300, 284, 268, 254, 240, 226
214, 202, 190, 180, 170, 165, 150, 142, 134, 127, 120, 113
107, 101, 95, 90, 85, 83, 75, 71, 67, 64, 60, 57
54, 51, 48, 45, 43, 41, 38, 36, 34, 32, 30, 28
27, 25, 24, 23, 21, 20, 19, 18, 17, 16, 15, 14
```

The values 165 (index 41) and 83 (index 53) are as the player has them (not 160 / 80).

### 1.6 Info

No title, author or instrument names. One song (no subsongs). The song length is not stored: a song
ends only by a 0xFE sequence entry or the read limit (3.5); 0xFF / 0xFD sequences loop forever.

## 2. State

Global: `speed` (initial 2), `counter` (16-bit, initial 0), `ended` (initial false), and `reads`
(16-bit, reset per voice, 3.5).

Per voice `v` (0..3):

| field | initial |
|---|---|
| sequence position | 0 |
| pattern number | 0 |
| pattern position (16-bit) | 0xFFFF |
| repeat count (16-bit) | 1 |
| row countdown (16-bit, signed when tested) | 0 |
| duration | 16 |
| instrument | 0 |
| transpose | 0 |
| portamento mode | off |
| portamento speed | 0 |
| period (16-bit) | 0 |
| volume | 64 |

Instrument sample starts are computed once at start (1.1).

## 3. Per tick

The player runs once per host tick (the host's 50 Hz timer, 70 940 colour clocks apart in the
reference).

### 3.1 Tick

1. If `ended`: write DMACON = 0x000F and do nothing else.
2. `counter := counter - 1` (16-bit). If the result is not negative (as signed 16-bit), stop here.
3. `counter := speed`. Process voices 3, 2, 1, 0 in that order (3.2).

So with speed `s` the voices are processed every `s + 1` ticks (speed 0: every tick), the first time
on the first tick. A speed command takes effect at the next reload.

### 3.2 One voice

1. `reads := 0`.
2. Portamento: if the mode is not off: `p := period - speed_p` for mode up, `period + speed_p` for mode
   down (`speed_p` = portamento speed), taken modulo 65536 as an unsigned value; then clamp: if
   `p <= 127` then `p := 127`; if `p >= 2048` then `p := 2048`. Write AUDxPER = `p`; `period := p`.
   This runs whether or not a note was ever played (period 0 then gives 127 or 2048 at once). A period
   below 127 set by a note jumps to 127 on the first portamento step, even for mode down.
3. If the low byte of the pattern position is 0xFF: pattern change (3.3).
4. Row countdown: `countdown := countdown - 1`. If the result is greater than 0 (signed), the voice
   is done for this tick. Otherwise read pattern bytes (1.4) from the current position:
   - each byte read counts towards the read limit (3.5) before it is looked at;
   - a command: apply, advance the position (low byte only), read the next byte;
   - 0xFF: pattern change (3.3), then repeat step 4 from the countdown decrement (the countdown is now
     0 or negative, so it stays <= 0 and reading continues in the new pattern);
   - a note: start it (3.4); the voice is done.

### 3.3 Pattern change

`pattern position := 0`; `repeat count := repeat count - 1`. If the result is not 0, the same pattern
is played again (back to the caller). Otherwise `repeat count := 1` and sequence entries are read
(1.3), each counting towards the read limit, until a pattern entry (0x00..0x3F) sets the pattern
number, or 0xFE (or the limit) ends the song.

### 3.4 Note start (note `n` on voice `v`; bit = 1 << v)

Let `I` be the voice's instrument record and `start` its sample start (1.1).

If `I.length` is 0, nothing is written: only the countdown and position step below happen (the
previous sound keeps playing, the period and volume are not changed).

Otherwise, in this order:
1. DMACON = bit (voice off).
2. A delay (section 5; about 2 330 colour clocks).
3. AUDxLC = `start` (one long write: high word and low word).
4. AUDxLEN = `I.length / 2` (low 16 bits).
5. `p := table[n + transpose] + portamento speed` (16-bit; the portamento speed is added whatever the
   mode, also when it is off). AUDxPER = `p`; `period := p`.
6. DMACON = 0x8000 | bit (voice on).
7. If `I.loop_length` is 0: AUDxLEN = 1. Otherwise: wait for 15 raster-line changes (section 5),
   then AUDxLEN = `I.loop_length / 2`, then AUDxLC = `start + I.loop_start`.
8. AUDxVOL = the voice's volume (0..255 as stored, not clamped).

Then (also for an empty instrument): `countdown := duration`; pattern position := position + 1 (whole
16-bit value).

A non-looping instrument plays once and then repeats the first word of its sample at length 1 (the
usual Paula "silence" idiom; the LC is not changed).

### 3.5 Read limit

`reads` is reset to 0 at the start of each voice's processing (3.2 step 1) and increased by one
before each sequence entry and each pattern byte is looked at. When it reaches 256 the song ends
(3.7) at that point. This catches sequences that loop without a pattern (0xFF alone, 0xFD to itself)
and patterns of 255+ commands in a row.

### 3.6 Other writes

- Volume entry 0xFC `x`: AUDxVOL = `x` immediately, and `volume := x`.
- Filter entries: one write to the CIA-A port A register (`bfe001` in the log): its bit 1 := bit 0 of
  the entry byte, the other bits as read. In the reference host the value written is `2 * (b & 1)`
  (0x0002 = LED off / filter off, 0x0000 = LED on / filter on). The power LED starts on.

### 3.7 Song end

At a 0xFE entry or the read limit: `ended := true`, the host's song-end notification is sent, and the
current voice's processing ends for this tick. The remaining voices of the same tick are processed
normally (they can signal song end again). From the next tick on, the player only writes DMACON =
0x000F each tick (3.1). The host's offline render stops shortly after the first song end.

### 3.8 Stop

When playback is stopped, the player writes DMACON = 0x000F and AUDxVOL = 0 for voices 0..3 (not in
the logs).

## 4. Write order summary

Per processed tick, for voices 3, 2, 1, 0: [PER (portamento)], then in sequence/pattern reading
order: [VOL from 0xFC entries], [CIA filter writes], and at a note with a non-empty instrument:
DMACON off, LC, LEN, PER, DMACON on, LEN (and for a loop: LC), VOL.

## 5. Timing

The reference host charges a fixed number of CPU cycles for every step of the player. Times below are
in CPU cycles (`u`, half colour clocks) from the tick's start `E`, the colour clock at which the host
enters the player. A write (or beam read) said to happen "at `u`" is logged at colour clock
`E + floor(u / 2)`. In the reference host `E = 75 559 + 70 940 k` for tick `k` (k = 0 is the first
tick) in every log.

"`+n`" means `u := u + n`; "W at u" means the write happens at the current `u` (after the preceding
`+n`).

### 5.1 Tick

- Start: `u = 200`.
- If `ended`: `+20`, DMACON 0x000F at `u`. Done.
- `+35` (counter step). If not expired: done (no writes).
- `+32`. Then for each voice 3, 2, 1, 0: `+18`, the voice (5.2), `+10`.

### 5.2 Voice

- `+95`.
- If portamento is on: `+135`, then `+4` if mode up, `+8` if `p <= 127`, `+8` if `p >= 2048` (tested
  as in 3.2, so exactly 127 or 2048 count as clamped);
  `+14`, PER at `u`; `+14`.
- `+37` (end-of-pattern test). If the low byte of the position is 0xFF: pattern change (5.3).
- Countdown: `+41`. Result 0: nothing more, read bytes. Result negative: `+11`, read bytes. Result
  greater than 0: `+11 + 16` and the voice is done.
- Each pattern byte: `+55`, then the read-limit test (song end: 5.5); `+127`; then by byte:
  - 0xFF: pattern change (5.3), then the countdown again (`+41`, ...).
  - note: `+19`, note start (5.4); the voice is done.
  - 0x30..0x6C: `+191`; 0x6D: `+141`; 0x6E: `+160`; 0x6F: `+179`.
  - 0x70..0x7F: `+131`.
  - 0x80..0xBF: `+171`, and `+8` more if the value is 0 (duration 64).
  - 0xC0..0xFE: `+146`.

  (Each command figure includes the position step.)

### 5.3 Pattern change and sequence

- `+67`. If the repeat count is still not 0: back to the countdown.
- `+26`. Then each sequence entry: `+55`, the read-limit test (song end: 5.5); `+119`; then by entry:
  - pattern 0x00..0x3F: `+62`; back to the countdown.
  - 0x40..0x7F: `+99`.
  - transpose (0x80..0xBF, low 6 bits <= 0x23): `+156`.
  - filter, 0x80..0xBF (low 6 bits >= 0x24): `+128`, `+16` CIA write at `u`, `+40`.
  - filter, 0xC0..0xFB: `+101`, `+16` CIA write at `u`, `+40`.
  - 0xFC: `+101`, `+14` VOL at `u`, `+72`.
  - 0xFD: `+152`.
  - 0xFE: `+95`, song end (5.5).
  - 0xFF: `+176`.

### 5.4 Note start

- `+131`. Empty instrument (length 0): `+86`; the voice is done.
- `+28` DMACON (off) at `u`.
- `+4658` (the delay).
- `+32` LC at `u` (both words at the same clock).
- `+42` LEN at `u`.
- `+88` PER at `u`.
- `+74` DMACON (on) at `u`.
- `+29`.
- No loop: `+18` LEN (1) at `u`; `+10`.
- Loop: `+4`; the raster wait (5.6); `+42` LEN at `u`; `+56` LC at `u`.
- `+32` VOL at `u`; `+86`; the voice is done.

### 5.5 Song end

`+192` (for 0xFE: after its `+95`; for the read limit: right after its `+55`). Then the host's
notification: `u := 2 * floor(u / 2) + 16` (the host continues from a whole colour clock). Then `+140`;
the voice is done.

### 5.6 Raster wait

The beam's line at time `u` is `floor((E + floor(u / 2)) / 227)`; only changes of it matter (the
player compares the low 8 bits of the vertical position, which change on every line, including frame
wrap). The wait repeats 15 times:

- `+16`: the line `L` is sampled at `u`.
- Poll: `+16`, the line is sampled at `u`; `+9`; if the sampled line equals `L`, poll again.
- After the poll that saw a change: `+10`.

The 15 waits together last a little over 14 raster lines (227 colour clocks each), depending on the
beam phase at the start; the exact end follows from the rule above.
