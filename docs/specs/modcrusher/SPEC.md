# modCRUSHER V2.0 packed module format ("Okm0" / "OkmC")

Status: lossless stage and all four lossy sample modes established and verified
byte-exactly against the tool's own unpack output, including the point where the
root frequency of the adaptive Huffman model reaches 0x8000 after 32451 coded
symbols: from there on the model is frozen (section 7).

## 1. Method (clean room)

Everything here was derived **black-box** from the tool's input/output behaviour.
The tool is modCRUSHER V2.0 by Olly Koenders (1996), CLI version `modCRUSHER_CLI_Vers`,
usage `MODCRUSHER mode in out`. It was run only inside the `vamos` emulator
(amitools), never natively. No 68k code of the tool was disassembled, decompiled or
read. The only static inspection was `strings` (usage text, magic words) and the
distributed `modCRUSHER.doc`.

Procedure:

1. Pack crafted inputs and corpus modules in every mode (0-4), then unpack each packed
   file with the tool. Inputs used: all-zero runs of many lengths, constant runs for
   every byte value, periodic patterns, controlled "k distinct literals + one copy of
   length L" files, ascending, random and permutation data, 31-instrument M.K. modules
   from the project corpus, and edited copies of those modules (synthetic sample data,
   trailing bytes, truncation, out-of-song order entries, wrong sample lengths). There
   were also 1.9 MB probe files built to cross the 32451-symbol point under control.
2. Analyse the bitstreams with our own Python scripts. Hypotheses about the coding
   were tested by decoding and comparing against the tool's unpack output.
3. Finally, write a reference decoder from this description
   (`fixround/crunch-spec/mcdecode.py`, outside the repo) and check it byte for byte
   against the tool's unpack output on every triple (section 9).

The lossless stage turned out to be an LZ77 + adaptive-Huffman scheme of the
well-known LZHUF family (public algorithm, 1988). That family was used as a
*hypothesis*, and every detail below was confirmed or corrected against the tool's
output. Where this tool differs from the textbook LZHUF parameters, the tool's
observed behaviour is what is specified. Section 7 records the LHA-style variants
that were tested as hypotheses and rejected.

## 2. File layout

All multi-byte fields are big-endian.

| off | size | field |
|----:|-----:|-------|
| 0   | 4    | magic: `"Okm0"` (mode 0, lossless) or `"OkmC"` (modes 1-4, lossy samples) |
| 4   | 4    | `total`: length of the unpacked file |
| 8   | 4    | `smp_off`: start of sample data in the unpacked file |
| 12  | 4    | `smp_len`: `total - smp_off` (signed; negative for non-module input) |
| 16  | ...  | one bitstream (section 3), to end of file |

The packer computes `smp_off = 1084 + 1024 * (1 + max(order[0..127]))`. It takes the
highest pattern number over **all 128** order-table bytes at offset 952, not just the
song-length entries; this was verified with an edited module. This is informative
only: a decoder takes the stored value. The packer writes the fields for any input,
even a non-module, in which case `smp_len` can be negative.

The packed file has no checksum. The decoder reads the **mode** from the stream
itself (section 5), as the documentation says ("the mode of pack will be gleaned from
the packed file").

## 3. Bitstream

Bits are read MSB first from consecutive bytes. The stream is a sequence of tokens,
each one an adaptive-Huffman-coded **symbol** (alphabet of 317):

* symbol `0..255`: literal byte;
* symbol `256..316`: match of length `symbol - 255` (1..61; the packer was seen to use
  3..60), followed by a distance code.

Distance code: a prefix code selects a class `c` (0..63), then 6 raw bits `r` follow.
The distance is `d = c*64 + r`, and the match copies `length` bytes from `d` bytes back
in the output, one byte at a time (so `d < length` repeats, e.g. `d = 1` repeats the
previous byte). The `d` stored is the actual distance (1-based); `d = 0` was never seen.
The largest distance seen is 4036 (window 4096 minus 60).

Class prefix codes (canonical, assigned in increasing class order):

| length | classes | codes |
|-------:|---------|-------|
| 3 | 0       | `000` |
| 4 | 1-3     | `0010` .. `0100` |
| 5 | 4-11    | `01010` .. `10001` |
| 6 | 12-23   | `100100` .. `101111` |
| 7 | 24-47   | `1100000` .. `1110111` |
| 8 | 48-63   | `11110000` .. `11111111` |

Decoding stops once the required output length is reached. The trailing bits are
padding or junk: the packer writes one extra token after the end. Required output
length: mode 0 needs `total` bytes; `OkmC` needs `smp_off + 2 + smp_len/2` bytes
(section 5).

There is no preset dictionary. The first token is always a literal, and a match never
reaches before the start of the output.

## 4. Adaptive Huffman model

A binary tree over the 317 symbols is kept in an array of `2*317-1 = 633` nodes,
indexed 0..632, in order of non-decreasing weight. The root is node 632. Each node
holds a weight and either a symbol (leaf) or the index `k` of its first child; its two
children are then nodes `k` (bit 0) and `k+1` (bit 1). Each node also knows its parent.

**Initial state.** Leaves 0..316 hold symbols 0..316 with weight 1. Then internal nodes
are created in index order 317, 318, ..., 632. Node `317+i` gets children `2i` and
`2i+1`, and its weight is the sum of their weights. The resulting initial code is a
truncated-binary code: symbols 122..316 get the 8-bit codes 0..194, and symbols
0..121 get the 9-bit codes 390..511.

**Decoding a symbol.** Start at the root. Repeatedly read a bit and move to child
`k + bit` until a leaf is reached.

**Update after every symbol** (encoder and decoder alike). Start at the symbol's leaf
node `n` and repeat until the root has been processed:

1. Increase `weight[n]` by 1; call the new value `w`.
2. If `w > weight[n+1]`, find the largest index `m > n` with `weight[m] < w`. That node
   has the old weight of `n` and is the highest-indexed node of the same weight.
   Exchange the contents of `n` and `m`: the weights, and which subtree hangs there
   (re-point the children's parent links, or the symbol-to-leaf link). Continue with
   `n := m`.
3. Move to the parent of `n`.

A guard weight larger than any real weight sits just above the root, so the search in
step 2 always stops.

Pseudocode:

```
decode_symbol():
    k = first_child[ROOT]
    while k is not a leaf marker: k = first_child[k + read_bit()]
    s = symbol_of(k); update(s); return s

update(s):
    n = leaf_node_of(s)
    loop:
        weight[n] += 1; w = weight[n]
        if w > weight[n+1]:
            m = n + 1
            while w > weight[m+1]: m += 1
            swap_node_contents(n, m)      # weights and attached subtree/leaf, fix parent links
            n = m
        n = parent[n]
        if n is "above root": break       # the root itself was incremented last
```

The tool does this rescale-free for the first **32451** symbols, until the root weight
reaches 0x8000 = 317 + 32451. From then on it skips the update entirely (section 7).

## 5. Sample section of `OkmC` files (modes 1-4)

The LZ output of an `OkmC` file is the original file's bytes `0 .. smp_off-1` (module
header and patterns, lossless), followed by a transformed sample section:

| byte | meaning |
|------|---------|
| `smp_off + 0` | `mode - 1` (0..3 for modes 1..4) |
| `smp_off + 1` | `0x00` in every file seen (purpose unknown) |
| `smp_off + 2 ...` | `smp_len/2` bytes, two 4-bit codes per byte, **high nibble first**, one code per output sample |

Each 4-bit code `n` is a Fibonacci delta, scaled by `unit = 2^(mode-1)` (1, 2, 4 or 8):

```
F = [0, 1, 2, 3, 5, 8, 13, 21, 34]
delta(n) = +F[n-8] * unit   if n >= 8
           -F[8-n] * unit   if n <  8
```

That is, n = 0..15 maps to -34, -21, -13, -8, -5, -3, -2, -1, 0, 1, 2, 3, 5, 8, 13, 21,
times `unit`.

Reconstruction runs over the whole sample area as **one continuous stream**. It does
not reset at instrument boundaries, starts at 0, and folds when the value runs off the
end of the signed 8-bit range:

```
acc = 0; flip = false
for i in 0 .. smp_len-1:
    n = (i even) ? packed[i/2] >> 4 : packed[i/2] & 15
    acc = acc + delta(n)
    if acc > 127 or acc < -128:
        acc = ((acc + 128) mod 256) - 128     # wrap into -128..127
        flip = not flip
    out[smp_off + i] = (flip ? -acc : acc) & 0xFF
```

The accumulator keeps the wrapped value. Only the output is negated while `flip` is
set. This is the same as folding (reflecting) an unbounded running sum at the ±128
range edges. It is what the documentation calls the integrated "error correction"
that removes clicks. Mode 1 uses the same rule (unit 1).

The unpacked file is `lz_output[0 : smp_off] + reconstructed samples` (length `total`).
The lossy modes are lossy only in the sample area; header and patterns are always
exact.

Packer-side observations (informative): modes 1-4 never produced an output file when
`total - smp_off` was odd (the packer hangs), and no output was produced for very
small inputs (8 bytes or fewer).

## 6. Mode 0 (`Okm0`)

The whole file is the LZ stream of section 3/4; the output is the first `total` decoded
bytes. Mode 0 is lossless for any input. The packer accepts any file, but the tool's
own *unpack* of non-module files (where `smp_off > total`) produced corrupted output
or crashed in vamos. All verification therefore uses module-shaped inputs.

## 7. Model behaviour at root weight 0x8000: the model freezes

**Rule.** Before each update, if the root weight is already 0x8000 or more, the update
is skipped: no weight changes, no exchanges, no rescaling. The root reaches 0x8000 with
the update of symbol 32451, so symbol 32452 and every later symbol is coded and decoded
on the tree as it stood then, to the end of the stream. No later rebuild or reset was
seen in streams of up to 119,351 symbols.

**How it was established** (behaviour observation only; no program code was read or
copied). The tool was run in vamos with the emulated RAM saved just before exit. The
tree arrays sit in that dump as 633 rising 16-bit weights, a 0xFFFF guard word and the
parent/child tables.

* A probe of 1 literal and 60 × 32455 zero bytes ends with root weight exactly 0x8000
  and the copy symbol's leaf at 32451: nothing grew and nothing was halved.
* Decoding with the frozen model, plus the one junk token the packer writes past the
  end, gives the same 633 weights and the same child links as the dump: checked on that
  probe, on a 96 KB random file (98,894 symbols) and on a corpus module (83,972 symbols).
* It also explains the earlier observations: a model that keeps updating agrees with a
  frozen one until the first update that would exchange nodes (10-35 symbols after the
  trigger, or the 69th literal in the literal probe), and every rescale variant tried
  changes the tree at the trigger, so each one failed.

Notes on the analysis: `fixround/crunch-rebuild/REBUILD.md` (scratch).

## 8. Decoder summary (pseudocode)

```
decrunch(file):
    magic, total, smp_off, smp_len = file[0:4], be32(4), be32(8), be32(12)
    bits = file[16:]
    if magic == "Okm0": return lz_decode(bits, total)
    if magic != "OkmC": error
    raw  = lz_decode(bits, smp_off + 2 + smp_len/2)
    unit = 1 << raw[smp_off]                      # raw[smp_off] = mode-1
    return raw[0:smp_off] + fib_unpack(raw[smp_off+2 :], smp_len, unit)

lz_decode(bits, n):
    tree = initial_tree(); out = []               # tree.update is a no-op once root >= 0x8000
    while len(out) < n:
        s = tree.decode_symbol()                  # section 4 (update included)
        if s < 256: out.append(s)
        else:
            length = s - 255
            c = read_class_code(); d = c*64 + read_bits(6)
            repeat length times: out.append(out[len(out)-d])
    return out[0:n]
```

## 9. Verification and test vectors

Reference decoder: `/media/lolo/kingston2tb/claude-scratch/deliplayerrust/fixround/crunch-spec/mcdecode.py`.
Checker: `.../fixround/crunch-spec/verify.py pk un [pk un ...]`.

Triples (input → tool pack → tool unpack); the decoder output was compared with the
tool's unpack output:

| set | triples | identical | mismatched |
|-----|--------:|----------:|-----------:|
| 19 corpus M.K. modules × modes 0-4 | 95 | 72 (the other 23 stopped at 32451 symbols before the freeze rule and were not rerun as a set; `corpus_c11_m0` among them is in the vectors) | 0 |
| synthetic samples (square, noise, sine, zeros) on a corpus header × modes 0-4 | 20 | 20 | 0 |
| edited modules (odd tail, truncated, order entry past last pattern, sample length +2) | 6 | 6 | 0 |
| large files past 32451 symbols (5 corpus mode 0, 3 corpus lossy, 6 synthetic) | 14 | 14 | 0 |

With the freeze rule all 43 vectors below and the 14 large ones decode identically.

The tool's mode-0 unpack matched the original input for all 19 corpus modules
(lossless).

Vectors: `/media/lolo/kingston2tb/claude-scratch/deliplayerrust/fixround/crunch-spec/vectors/modcrusher/`
(43 vectors with `MANIFEST.tsv`: packed file `.okm`, the tool's unpack output
`.expected`, the original `.input`, mode, sizes, hashes, origin). It covers three small
corpus modules in all modes, the synthetic-sample set in all modes, the edited
modules, and `corpus_c11_m0/m1`. `corpus_c11_m0` crosses the 32451-symbol point and
is a regression vector for section 7. The 14 large vectors are in
`.../fixround/crunch-rebuild/vectors/`. The repo test fixture `synth_freeze_m0.okm`
(fully synthetic, 37,072 symbols) checks the freeze in the Rust decoder.
