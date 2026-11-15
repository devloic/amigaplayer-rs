# SoundTracker Cruncher V1.0 (OTT-Design, 1990): packed format

Status: **solved for every test file.** A clean-room decoder
(`stc-spec2/stcdec.py`, see below) rebuilds all 23 test modules (the first 20
pairs plus 3 new probes) byte for byte. One defect of the tool itself remains:
**the file that Save writes is 2 to 4 bytes short of the real bitstream** (see
"End of the stream"). For 13 of the 23 files those missing bytes hold data that is
not zero. These 13 decode exactly only with the bytes that follow the Save buffer
in emulated memory, recaptured for this work. Without those bytes the decoder fills
the end with zeros. It then gets 10 of 23 files exactly right, and the other 13 differ
only in their last 3 to 4248 bytes.

## Method

Clean-room, black-box only, as in `docs/specs/modcrusher/SPEC.md`. No 68k code of the
program was read, disassembled or copied. The package holds only the crunched `STC`
executable (no docs, no depacker, no replay). No other cruncher's source was read.
Inputs are synthetic M.K. modules made for this work, with no third-party data. The
format below comes only from comparing inputs with packed output: the known output
was lined up against the bitstream from the point where the old partial decoder
stopped. Each unknown token was then fixed by searching for the next bit and output
position where the known tokens fit again. Probes were made to test each rule.

Working files:

* first round: `/media/lolo/kingston2tb/claude-scratch/deliplayerrust/fixround/stc-spec/`
  (20 pairs in `vectors/`, emulator scripts, old partial `stcdec.py`). Not modified.
* this round: `/media/lolo/kingston2tb/claude-scratch/deliplayerrust/fixround/stc-spec2/`
  * `stcdec.py`: the full decoder.
  * `verify.py` / `verify.out`: the verification table.
  * `mkprobes2.py`: the new probes c1-c3.
  * `vectors/{input,packed,full}/`: new pairs, plus `*_re` recaptures of old ones.
  * `rig/`: a copy of the emulator rig, on Xvfb `:93`. Its capture also saves
    the Write buffer + 64 bytes.
  * analysis scripts: `catalog.py`, `ext1.py`, `resync.py`, `codes.py`, `shortfall.py`.

## How the tool was run

* `vamos` (amitools) fails: the unpacking stub calls `exec FreeMem` on a block vamos
  does not know and vamos aborts (PC $162E / $2372), whatever `-m`, `-C`, `-H` is used.
* FS-UAE 3 (`/usr/bin/fs-uae`) with these settings:
  * `kickstart_file = internal` (AROS ROM, SVN 50730), A1200 (A500 behaves the same),
    8 MB fast memory, audio off, under its own Xvfb display.
  * the host dir is mounted as `Work:`, `s/startup-sequence` is empty, and `STC` is
    typed into the AROS shell.
  * xdotool mouse clicks do not reach the emulated mouse. F1/F2 are mapped to the
    left/right button, and the pointer is moved with relative xdotool motion, found
    again by its red sprite.
* The FS-UAE console debugger (F10, driven through a FIFO) works around three AROS
  incompatibilities at OS-call level only (breakpoints on ROM entry points; the
  program's instructions were never looked at):
  1. the stub's two `FreeMem` calls on part-blocks halt the task under AROS; they are
     skipped (memory leaked). The unpacked program then runs at absolute `$10000`.
  2. Crunch calls intuition `DisplayAlert` ("START CRUNCHING / CRUNCH ... CANCEL");
     AROS returns FALSE at once, so the call is skipped and TRUE returned.
  3. after a crunch, the AROS memory list is damaged and the next DOS `Write` ends in
     `ColdReboot`. Save's `Write` calls are therefore caught, their buffer saved from
     emulated memory with the debugger `S` command, `D0 = length` returned, and the
     `Close` skipped. One fresh boot per module (about 2.5 min).
* GUI: menus Project (Load, Save, Delete, About, Quit) and Crunch (Crunch, DeCrunch,
  Edit Colors). The gadgets are:
  * Crunching power: High (it did not cycle)
  * DeCrunching style: None/Color/LED/Pointer
  * Crunching mode: Info/Fast
  * Type of file: Data/Execute

  All vectors use High / None / Info / Data. Save makes three Writes: `STC\0`
  (4 bytes), the original length (4 bytes), then the bitstream.
* The tool's output is deterministic: crunching 12 old modules again gave the same
  saved bytes (`saved==old` for p01-p06, q0, t1, t2, t4, t6, t7).

## File layout

| offset | size | content |
|---|---|---|
| 0 | 4 | `STC\0` |
| 4 | 4 | original (unpacked) length, big-endian |
| 8 | rest | bitstream, read MSB first, no end marker; decoding stops when the original length is reached; the last byte is padded with 0 bits |

## Bitstream

The first 32 bits are the first 4 output bytes, raw. Then tokens follow until the
output reaches the original length. A token is picked by a 2- or 4-bit prefix. The
prefixes form a complete prefix code:

| prefix | token | output |
|---|---|---|
| `00` | literal | `count` bytes, 8 bits each |
| `01` | repeat | the previous output byte, `count` times |
| `1011` | zero run | `count` zero bytes |
| `1100` | byte delta run, w = 3 | see below |
| `1101` | byte delta run, w = 4 | |
| `1110` | byte delta run, w = 5 | |
| `1111` | byte delta run, w = 6 | |
| `1000` | byte delta run, w = 7 | |
| `1001` | longword delta run, starting width 8 | see below |
| `1010` | longword delta run, starting width 16 | |

There are no back-references (no copy tokens). The cruncher has no special coding for
the order list, `M.K.` or patterns either: what looked like that is the longword
delta runs, which suit 4-byte pattern notes.

### `count` (literal, repeat, zero run)

A 6-bit code `n`, then maybe more bits:

| n | extra bits | count |
|---|---|---|
| 0-47 | none | n + 1 (1..48) |
| 48-53 | 7 bits `v` | ((n - 48) << 7 \| v) + 1 (1..768) |
| 54 | 16 bits `v` | v + 1 (1..65536) |
| 55 | 32 bits `v` | v + 1 |
| 56-63 | | never seen |

The tool uses n = 0-47 for 1..48, n = 48-53 for 49..768, n = 54 for 769..65536 and
n = 55 above that. Probe c1 checked every length from 28 to 51 and many others up to
4000. c3 checked 69995 and 70000 (n = 55). The codes 56-63 never appeared, and the
decoder rejects them.

### Byte delta run (`1100`, `1101`, `1110`, `1111`, `1000`)

A list of w-bit two's-complement values. Each one is added (mod 256) to the previous
output byte, and the sum is output. The value `1` followed by w-1 zeros (-2^(w-1)) ends
the run and outputs nothing.

### Longword delta run (`1001`, `1010`)

This token works on 32-bit big-endian longwords. Each step outputs 4 bytes: the
longword formed by the last 4 output bytes, plus a signed delta (mod 2^32). The run
has a current width w, 8 after `1001` and 16 after `1010`. Each step:

1. peek 4 bits. `1000` = end of run (consume the 4 bits, output nothing).
2. `0111` = width switch: consume the 4 bits and set w to the other width
   (8 to 16, 16 to 8). The switch lasts for the rest of the run. A delta always
   follows a switch (no end check on it).
3. read a w-bit two's-complement delta and output the new longword.

A plain delta therefore never starts with `1000` or `0111`. For w = 8 the values
-128..-113 and 112..127 cannot be coded directly, so the run switches to 16 bits. The
tool uses this token for the order list (e.g. t5: `00 01 06 7F` then `00 01 02 03`,
delta -0x47C) and for pattern notes that differ from the note before in the low
bits (period, effect argument). No 32-bit width was seen: notes that differ more are
coded as literals.

Examples (from t4 and t7):
`1010 0000000000110110 0000000110100111 1111110100000110 1000` takes
`1000011D` to `10000153 100002FA 10000000`.
`1001 11100100 0111 0000000011000110 1000` adds -28 (w = 8), switches to 16 bits,
adds +198, then ends.

### End of the stream (tool defect)

The bitstream really ends at the end of the token that fills the original length,
padded with 0 bits. But the length that Save passes to `Write` is 2 to 4 bytes too
small, so the saved file ends inside the last token, or just before it. Measured on
15 recaptures, where the extra memory past the Write buffer was saved as well: the
saved file lacks 2, 3 or 4 bytes (see `shortfall.py`). The missing bytes are still
in emulated memory right after the buffer, and then AROS free-memory fill
(`DB AD C0 DE`) follows. No rule linking the missing amount to the bit count was
found. The "memory list damaged after a crunch" note above may be the same overrun,
but that was not proven.

Consequences:

* If those lost bits only carry zero bytes, zero-filling to the original length gives
  the exact module. A zero run is cut, or the file ends just before it.
* Otherwise, the module's last bytes are lost from the file as saved. For example, the
  end of a sample coded by a literal or delta run. With those bits lost, the bytes
  cannot be recovered from the file. The decoder keeps what the cut token still gives
  and fills the rest with zeros.
* `stcdec.decode(data, strict=True)` raises `EOFError` instead.

Whether the tool's own DeCrunch, or a replayer's depacker, reads past the saved end
was not tested.

## Decoder and verification

`stc-spec2/stcdec.py` (`python3 -I stcdec.py file.stc [out.mod | --cmp orig.mod]`).
`verify.py` decodes each pair two ways:

* "saved": the file exactly as saved.
* "+tail": the saved file plus the 4 bytes that follow the Write buffer in memory,
  recaptured with the same rig.

| pair | module | packed | saved | +tail |
|---|---|---|---|---|
| p01 | 2108 | 108 | identical | identical |
| p02 | 2108 | 482 | last 8 differ | identical |
| p03 | 6204 | 4208 | last 4 differ | identical |
| p04 | 6204 | 1886 | last 38 differ | identical |
| p05 | 2108 | 1122 | last 3 differ | identical |
| p06 | 2620 | 302 | last 5 differ | identical |
| q0 | 2108 | 118 | identical | identical |
| q1 | 2108 | 132 | identical | not recaptured |
| q2 | 2108 | 126 | identical | not recaptured |
| q3 | 2108 | 126 | identical | not recaptured |
| r1 | 2108 | 206 | identical | not recaptured |
| r2 | 11324 | 108 | identical | not recaptured |
| r3 | 2110 | 104 | identical | not recaptured |
| t1 | 2172 | 278 | last 4 differ | identical |
| t2 | 2876 | 966 | last 15 differ | identical |
| t3 | 5180 | 118 | identical | not recaptured |
| t4 | 7228 | 5440 | last 4 differ | identical |
| t5 | 17680 | 9340 | identical | not recaptured |
| t6 | 112372 | 70962 | last 4 differ | identical |
| t7 | 4156 | 1876 | last 6 differ | identical |
| c1 (new) | 35900 | 558 | last 4248 differ | identical |
| c2 (new) | 15420 | 13480 | last 720 differ | identical |
| c3 (new) | 212108 | 70150 | last 3 differ | identical |

Result: **23/23 identical** once the stream is complete (13 with the recaptured tail,
10 already from the saved file). From the saved file alone: 10/23. Every full stream
also decodes with `strict=True`, i.e. it ends exactly where the tokens end.

New probes (`mkprobes2.py`, own synthetic data):

* c1: zero runs and 0x55 runs of 28..51, 60..1000, 1500, 2000 and 4000 bytes, each
  framed by marker bytes, in 34 patterns.
* c2: random literal runs of the same lengths, framed by 8 zeros.
* c3: one empty pattern plus three 70000-byte samples (random, zero, 0x33), the last
  one ending in `01 02 03 04`.

## Not tested / open

* Count codes 56-63; a width other than 8 and 16 in longword runs.
* Crunching power other than High, Fast mode, Execute type, and the DeCrunching
  styles. Any of these may change the format, and Execute surely adds a stub.
* The rule behind the 2-4 byte shortfall of the saved length; whether the original
  DeCrunch reproduces the end of such files.
