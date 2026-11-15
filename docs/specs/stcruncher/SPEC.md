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

The tool's own DeCrunch does no better on as-saved files (see "What the tool's own
DeCrunch does"). It reads whatever memory follows its load buffer. Under AROS it
matches `decode(saved + DB DB ...)` on the 11 files it unpacks and crashes on 11 others.
The lost tail cannot be rebuilt from the file: at least 13 bits stay free in each
of the 13 pairs.

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

What the tool's own DeCrunch does with such a file is in the next section.

### What the tool's own DeCrunch does with an as-saved file

Observed in the emulator only (OS-call breakpoints and memory dumps; no program code
read). Work dir: `/media/lolo/kingston2tb/claude-scratch/deliplayerrust/fixround/stc-tail/`,
rig copy in `rig/` (Xvfb `:117`). Each as-saved `.stc` was loaded into the tool
(Project/Load), then Crunch/DeCrunch was run ("START DECRUNCHING" alert answered TRUE
as for Crunch), then Project/Save. The Save `Write` was captured as for crunching.
One fresh boot per file.

**Load.** The tool opens the file, `Lock`s and `Examine`s it for its size, then calls
`AllocMem(file size + 1000, 0)`. The flags are 0 (MEMF_ANY), so the block is not
cleared. It reads `STC\0` to block + 1000 and the rest of the file right after it,
so the file data ends exactly at the end of the block. DeCrunch writes to another
block, and Save writes that block with the original length.

**DeCrunch reads past the end of the load block.** It does not stop at the end of
the file. It goes on reading the bits that follow in memory until its last token is
complete. Under this AROS ROM the block is followed by 32 bytes of mungwall fill
`DB`, then other blocks and free memory. Two tests show it:

* p02 (decrunched as saved): output equals `stcdec.decode(saved + DB DB ...)`, not
  the zero-fill output, not the original. Its last 8 bytes are
  `f8 f5 f2 f1 f4 f7 f5 f3`. The original is `f8 f9 fa fb fc fd fe ff` and zero fill
  gives `00 00 ...`.
* p05 with `01 23 45 67 89 AB CD EF` poked into memory right after the load block,
  before DeCrunch: output equals `decode(saved + 0123456789ABCDEF)`. The output follows
  whatever lies after the block.

**Output length.** In byte delta runs the tool stops at the original length. p02,
p04 and p06 end inside a delta run that the `DB` bits never stop, and the mungwall
after the output block was still intact at Save. The literal, repeat and zero-run
counts are not clamped. When the bits past the end give a count larger than what is
left, the tool writes past its output block. Under AROS, mungwall then raises an
alert and the machine reboots (`ColdReboot` caught by the rig).

Per as-saved file (all 23; `check.py`, `memsim.py`):

| pair | as-saved file + `DB` fill, decoded by the format | tool DeCrunch under AROS |
|---|---|---|
| p01 | zero run, count from `DB` bits = 3082270648 | crash (reboot) |
| p02 | reads 3 bytes past end, ok | output = `DB`-fill decode (last 7 differ from original) |
| p03 | reads 4 bytes, ok | = `DB`-fill decode (last 4 differ) |
| p04 | reads 30 bytes, ok | = `DB`-fill decode (last 38 differ) |
| p05 | reads 3 bytes, ok | = `DB`-fill decode (last 3 differ) |
| p06 | reads 2 bytes, ok | = `DB`-fill decode (last 4 differ) |
| q0, q1 | byte delta run reads 896 bytes, past the 32-byte wall | crash |
| q2 | zero run of 57055, 56031 too many | crash |
| q3 | zero run of 3082270648 | crash |
| r1 | delta run reads 458 bytes, past the wall | crash |
| r2 | count code 61 (56-63, never seen) | crash |
| r3 | delta run reads hundreds of bytes, past the wall | crash |
| t1 | reads 3 bytes, ok | = `DB`-fill decode (last 4 differ) |
| t2 | reads 5 bytes, ok | = `DB`-fill decode (last 8 differ) |
| t3 | delta run reads 2049 bytes, past the wall | no crash, but bytes 1084-5179 (the whole pattern and sample) are garbage. It matches the `DB`-fill decode for 63 bytes (= 32 bytes of 4-bit deltas), then follows other memory |
| t4 | reads 4 bytes, ok | = `DB`-fill decode (last 4 differ) |
| t5 | count code 61 | crash |
| t6 | reads 4 bytes, ok | = `DB`-fill decode (last 4 differ) |
| t7 | reads 3 bytes, ok | = `DB`-fill decode (last 6 differ) |
| c1 | repeat run of 7036, 2788 too many | crash |
| c2 | zero run of 63223, 62504 too many | crash. With the 4 real tail bytes `FB D6 70 D0` poked after the load block, no crash and output = original module |
| c3 | reads 3 bytes, ok | = `DB`-fill decode (last 3 differ) |

The 10 files that zero fill rebuilds exactly (p01, q0-q3, r1-r3, t3, t5) are the
ones the tool's own DeCrunch cannot unpack under AROS. It crashes on 9 of them and
writes garbage for t3. Their lost bits hold a zero run (or the start of one), and the
`DB` bits put a different token there.

Rule (AROS rig only): **the tool's DeCrunch output equals `stcdec.decode` of the
saved stream followed by `DB` bytes, as long as the decode reads at most 32 bytes past
the end and the last count does not overrun.** All files that DeCrunch put out match it
(`stc-tail/stcdec.py --past-end db`, `decode(..., past_end=b'\xdb')`). Those are 11 of
11: p02-p06, t1, t2, t4, t6, t7, c3, all byte for byte. The rule also predicts what
goes wrong with the other 12. All 5 overrunning counts and both count codes 56-63
crash. Of the 5 files that read beyond the wall, 4 crash and t3 puts out garbage that
follows the rule only for as long as the wall lasts. Beyond the wall the bytes depend
on the other blocks at DeCrunch time, so no file-only rule can give them. With the
default zero fill, `stc-tail/stcdec.py` is unchanged (same output as `stc-spec2` on
all 23 files).

**Not deterministic on a real Amiga.** `AllocMem` gets flags 0, not MEMF_CLEAR. Exec
rounds the block up to 8 bytes, so after the file there are 0-7 bytes of uncleared
slack (`(8 - (size + 1000) % 8) % 8`). After that comes whatever is next: a free-list
chunk header or someone else's block. The bytes the last token needs (2-4, more when
the lost token is a long delta run) therefore depend on earlier memory use. The tail
of the tool's own output is not defined. A large count can overwrite memory after the
output block (no mungwall to catch it on Kickstart). This was not run under a real
Kickstart.

### Other depack paths

The package as we have it holds only the crunched `STC` executable: no text docs and
no replay or depacker. The About box says "Read documents for information", but no
documents are in this copy. The GUI's "Type of file: Execute" makes a
self-extracting file. That was not tested here, and it would carry the same Save
length. Outside the package, DeliTracker's GenieStore has `ST-Cruncher 0.40`, which
"decrunches modules packed with SoundTracker-Cruncher (OTT Design)" (see
`docs/specs/COVERAGE-DELI.md`). It was not run here.

### Can the lost tail be rebuilt from the file?

No. `stc-tail/recover.py` counts every completion of the cut stream that fits all
known constraints:
* the token grammar, with count codes 56-63 excluded;
* the last token ends exactly at the original length;
* 0-bit padding to the byte;
* the real end lies 0-4 bytes past the saved end (2-4 gives the same counts).

Literal and delta values do not affect validity and are counted by multiplicity. The
module structure adds nothing. The sample length fields only fix the total length,
which the header already gives. The lost bytes are sample PCM in 9 pairs and pattern
data in 4 (p02, p05, c1, c2, whose patterns are synthetic bytes):

| pair | lost stream bits | consistent completions | free bits (log2) | tokens hit by the cut |
|---|---|---|---|---|
| c1 | 30 | 14545 | 13.8 | rep 3996, zero 252 |
| c2 | 21 | 8928 | 13.1 | end of lit 2000, zero 719 |
| c3 | 23 | 2^23 | 23.0 | lit 4 |
| p02 | 25 | 14049022 | 23.7 | delta w3 run |
| p03 | 28 | 2^28 | 28.0 | lit 4096 |
| p04 | 24 | 2179072 | 21.1 | lit 3, zero 36 |
| p05 | 22 | 2^22 | 22.0 | lit 1028 |
| p06 | 16 | 1961548 | 20.9 | delta w3 run |
| t1 | 27 | 2470888 | 21.2 | delta w6 run |
| t2 | 27 | 1180067 | 20.2 | rep 7, lit 1, rep 7 |
| t4 | 28 | 2^28 | 28.0 | lit 4096 |
| t6 | 27 | 2^27 | 27.0 | lit 3000 |
| t7 | 20 | 5119752 | 22.3 | delta w3 run |

At least 13 bits stay free in every pair. Where the stream bits are all literal
data, every missing bit is free. Where the cut falls in a delta run, there can be
more completions than 2^(lost bits), because the run may end later within the 4-byte
window. So the as-saved file does not hold the lost tail. Zero fill (what `stcdec`
does) is a choice, as is any other fill. The tool's own DeCrunch output is not a
better reference, since it depends on memory.

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
* The rule behind the 2-4 byte shortfall of the saved length.
* The tool's DeCrunch on a real Kickstart (only AROS was run). The Execute
  (self-extracting) output and DeliTracker's ST-Cruncher genie were not tried on
  as-saved files.
* What exactly the tool does with count codes 56-63 (r2, t5: it crashes under AROS).
