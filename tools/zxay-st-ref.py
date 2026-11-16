"""Build the reference of a ZXAY Sound Tracker song (ZXAYST11 or ZXAYSTRC) for the original Z80
player: the song is compiled to the Sound Tracker "compiled song" layout (STC) and put in place of
the song of an EMUL rip of the original compiled-song player, which the project's ZXAYEMUL
player (src/replay/ay.rs: its Z80 and AY) then runs as a black box.

    python3 -I tools/zxay-st-ref.py SONG RIP.emul OUT.emul [--stc OUT.stc] [--length]
                                    [--repeat-envelope]

--length sets the EMUL song length to one pass of the position list (delay x pattern length x
positions, in 1/50 s) and the fade to 0; without it the rip's own length and fade stay.
--repeat-envelope writes envelope codes that repeat the stream's last effect code (by default
they are left out like every repeated code; which one the original compiler does is unverified).

SONG: a ZXAYST11 or ZXAYSTRC file. RIP: the agent-x "nonamedzak 01" EMUL rip (Project AY;
modland "agent-x nonamedzak 01.emul", 4 901 bytes, one block of $FF43 bytes at $C000, the
player's code and data at $C000-$C43B, its song at $C43C, interrupt routine $C006). The rip is
only copied: its first $43C block bytes are kept as they are, the song after them is replaced,
the block length in the header is set to the new length. Nothing of the rip is read or decoded.

The conversion (docs/specs/ay-soundtracker/SPEC.md section 5) is data layout only. It gives
byte for byte the song data of the two rips whose ST11 sources we have (agent-x 01 = st11.AGENT1,
agent-x 02 = st11.AGENT4). The rules, in short:
- header: delay, then the offsets (from the start of the data, little-endian) of the position
  list, the ornaments and the pattern table, the text "SONG BY ST COMPILE", the data length;
- the samples the streams select, in increasing number (99 bytes each: number, 32 x (volume |
  add bits 8-11 << 4, noise byte & $DF | add sign bit 12 >> 7, add bits 0-7), rep, replen);
- the position list: number of positions - 1, then pattern number and transposition per position;
- ornament 0 and the ornaments the streams select, in increasing number (number, 32 bytes);
- the pattern table: number and three channel stream offsets of each pattern the position list
  names (in increasing order), $FF; a named pattern the file does not hold is an empty pattern;
  a stream equal to one already written is not written again, its offset points to that one;
- each channel stream: per row, for a note: [$60 + sample] [effect code] note; for a rest (byte 1
  bit 7): [$60 + sample] [effect code] $80; the sample code only when the sample differs from the
  stream's last one, the effect code only when it differs from the stream's last effect code
  (both forgotten at each stream start); an empty cell (note name 0, bit 7 clear) is an empty row
  and its sample and effect nibbles are dropped; empty rows after an event are counted in a
  duration code $A1 + n written before the event whenever n changes; a stream that starts with
  empty rows starts with $81; $FF ends the stream. Effect codes: 1 and 2 -> $82;
  3-14 -> $80 + effect, parameter byte; 15 -> $70 + (parameter & 15); 0 -> none.
STRC songs are first decoded to the same cells (the STRC block's tables and row streams).
"""
import struct
import sys

BASE = 0xC43C        # where the rip's player expects its song
CODE_OFF = 0xBE      # file offset of the rip's block ($C000)
SEMI = {1: 9, 2: 11, 3: 0, 4: 2, 5: 4, 6: 5, 7: 7}   # note name nibble -> semitone (C = 0)


def be16s(d, o):
    return struct.unpack('>h', d[o:o + 2])[0]


def container(d):
    if d[:8] not in (b'ZXAYST11', b'ZXAYSTRC'):
        raise ValueError('not a ZXAYST11 / ZXAYSTRC file')
    ss = 18 + be16s(d, 18)
    return d[4:8], ss + 2 + be16s(d, ss + 2)


def cell_st11(b1, b2, b3):
    """An ST11 cell as (kind, note, sample, effect, parameter): kind 'note', 'rest' or 'empty'."""
    if b1 & 0x80:
        return ('rest', 0, b2 >> 4, b2 & 15, b3)
    name = (b1 >> 4) & 7
    if name == 0:
        return ('empty', 0, 0, 0, 0)
    n = 12 * (b1 & 7) + SEMI[name] + ((b1 >> 3) & 1)
    if n > 0x5F:
        raise ValueError('note index %d has no STC note code' % n)
    return ('note', n, b2 >> 4, b2 & 15, b3)


def parse_st11(d, sd):
    m = d[sd + 8:]
    s = {'samples': [], 'ornaments': [], 'patterns': {}}
    for i in range(15):
        x = m[i * 130:(i + 1) * 130]
        s['samples'].append(dict(vol=list(x[0:32]), noise=list(x[32:64]),
                                 add=[x[64 + 2 * k] | x[65 + 2 * k] << 8 for k in range(32)],
                                 rep=x[128], replen=x[129]))
    length = m[2462]
    s['positions'] = [(m[1950 + 2 * i], m[1951 + 2 * i]) for i in range(length + 1)]
    s['ornaments'] = [list(m[2463 + 32 * i:2495 + 32 * i]) for i in range(17)]
    s['delay'] = m[3007]
    s['patlen'] = m[3008] or 64
    npat = (len(m) - 3009) // 576
    for p in range(npat):
        x = m[3009 + 576 * p:3009 + 576 * (p + 1)]
        s['patterns'][p + 1] = [[cell_st11(*x[r * 9 + c * 3:r * 9 + c * 3 + 3]) for c in range(3)]
                                for r in range(s['patlen'])]
    return s


def parse_strc(d, sd):
    load = struct.unpack('>H', d[sd + 4:sd + 6])[0]
    mem = bytearray(65536)
    blk = d[sd + 10:sd + 10 + 65536 - load]
    mem[load:load + len(blk)] = blk

    def w(a):
        return mem[a & 0xFFFF] | mem[(a + 1) & 0xFFFF] << 8

    D = load
    s = {'delay': mem[D], 'patlen': mem[D + 1] or 64, 'samples': [], 'ornaments': [], 'patterns': {}}
    addp = [w(D + 2 + 2 * i) for i in range(16)]
    ornp = [w(D + 34 + 2 * i) for i in range(16)]
    volp = [w(D + 66 + 2 * i) for i in range(16)]
    noip = [w(D + 98 + 2 * i) for i in range(16)]
    for k in range(1, 16):
        add = []
        for p in range(32):
            v = struct.unpack('<h', bytes([mem[(addp[k] + 2 * p) & 0xFFFF], mem[(addp[k] + 2 * p + 1) & 0xFFFF]]))[0]
            add.append((0x1000 | v) if v >= 0 else (-v & 0xFFF))
        s['samples'].append(dict(vol=[mem[(volp[k] + p) & 0xFFFF] for p in range(32)],
                                 noise=[mem[(noip[k] + p) & 0xFFFF] for p in range(32)], add=add,
                                 rep=mem[D + 130 + 2 * k], replen=(mem[D + 131 + 2 * k] - 1) & 255))
    s['ornaments'] = [[mem[(ornp[k] + p) & 0xFFFF] for p in range(32)] for k in range(16)] + [[0] * 32]
    o = D + 162
    s['positions'] = []
    while w(o) and len(s['positions']) < 256:
        streams = [w(o), w(o + 2), w(o + 4)]
        rows = []
        for q in streams:
            cells = []
            for _ in range(s['patlen']):
                x = mem[q & 0xFFFF]
                if x == 0xFE:
                    cells.append(('empty', 0, 0, 0, 0)); q += 1
                elif x == 0xFF:
                    cells.append(('rest', 0, 0, 0, 0)); q += 1
                else:
                    if x > 0x5F:
                        raise ValueError('STRC note byte %02x' % x)
                    b2, b3 = mem[(q + 1) & 0xFFFF], mem[(q + 2) & 0xFFFF]
                    cells.append(('note', x, b2 >> 4, b2 & 15, b3)); q += 3
            rows.append(cells)
        n = len(s['positions']) + 1
        s['patterns'][n] = [[rows[0][r], rows[1][r], rows[2][r]] for r in range(s['patlen'])]
        s['positions'].append((n, mem[o + 6]))
        o += 7
    return s


def effect_codes(eff, par, state):
    """The codes for an effect nibble, given the last effect codes written in this stream (None
    at its start): written only when they differ from those; returns (codes, new state)."""
    if eff == 0:
        return [], state
    if eff in (1, 2):
        codes = [0x82]
    elif eff == 15:
        codes = [0x70 + (par & 15)]
    else:
        codes = [0x80 + eff, par]
        if REPEAT_ENV:
            return codes, codes
    return ([] if codes == state else codes), codes


# --repeat-envelope: write an envelope code on every cell that has the effect, also when it
# repeats the stream's last one (unverified alternative, SPEC section 5.3).
REPEAT_ENV = False


def channel(rows, c):
    events = []
    smp_state = None
    fx_state = None
    for r, row in enumerate(rows):
        kind, n, smp, eff, par = row[c]
        if kind == 'empty':
            if r == 0:
                events.append((r, [0x81]))
            continue
        codes = []
        if smp and smp != smp_state:
            codes.append(0x60 + smp)
            smp_state = smp
        fx, fx_state = effect_codes(eff, par, fx_state)
        codes += fx
        events.append((r, codes + ([n] if kind == 'note' else [0x80])))
    out, cur = bytearray(), None
    for k, (r, codes) in enumerate(events):
        nxt = events[k + 1][0] if k + 1 < len(events) else len(rows)
        dur = nxt - r - 1
        if dur != cur:
            out.append(0xA1 + dur)
            cur = dur
        out += bytes(codes)
    out.append(0xFF)
    return out


def stream_codes(stream):
    """The sample and ornament numbers a channel stream selects."""
    smp, orn, q = set(), set(), 0
    while stream[q] != 0xFF:
        x = stream[q]
        if 0x60 <= x <= 0x6F:
            smp.add(x - 0x60)
        elif 0x70 <= x <= 0x7F:
            orn.add(x - 0x70)
        q += 2 if 0x83 <= x <= 0x8E else 1
    return smp, orn


def compile_stc(s):
    used = sorted(set(p for p, t in s['positions']))
    empty = [[('empty', 0, 0, 0, 0)] * 3] * s['patlen']
    streams = {p: [channel(s['patterns'].get(p, empty), c) for c in range(3)] for p in used}
    smp_used, orn_used = set(), {0}
    for p in used:
        for st in streams[p]:
            a, b = stream_codes(st)
            smp_used |= a
            orn_used |= b
    body = bytearray()
    for i, sm in enumerate(s['samples']):
        if i + 1 not in smp_used:
            continue
        body.append(i + 1)
        for k in range(32):
            a = sm['add'][k]
            body += bytes([(sm['vol'][k] & 15) | ((a >> 8) & 15) << 4,
                           (sm['noise'][k] & 0xDF) | (0x20 if a & 0x1000 else 0), a & 255])
        body += bytes([sm['rep'], sm['replen']])
    pos_off = 27 + len(body)
    body.append(len(s['positions']) - 1)
    for p, t in s['positions']:
        body += bytes([p, t & 255])
    orn_off = 27 + len(body)
    for i, o in enumerate(s['ornaments']):
        if i not in orn_used:
            continue
        body.append(i)
        body += bytes(v & 255 for v in o)
    pat_off = 27 + len(body)
    tab = len(body)
    body += bytes(7 * len(used) + 1)
    placed = {}   # a stream equal to one already written points to that one
    for j, p in enumerate(used):
        offs = []
        for c in range(3):
            st = bytes(streams[p][c])
            if st not in placed:
                placed[st] = 27 + len(body)
                body += st
            offs.append(placed[st])
        body[tab + 7 * j:tab + 7 * j + 7] = bytes([p]) + b''.join(struct.pack('<H', o) for o in offs)
    body[tab + 7 * len(used)] = 0xFF
    hdr = bytearray(27)
    hdr[0] = s['delay']
    hdr[1:3] = struct.pack('<H', pos_off)
    hdr[3:5] = struct.pack('<H', orn_off)
    hdr[5:7] = struct.pack('<H', pat_off)
    hdr[7:25] = b'SONG BY ST COMPILE'
    hdr[25:27] = struct.pack('<H', 27 + len(body))
    stc = bytes(hdr + body)
    if BASE + len(stc) > 0x10000:
        raise ValueError('song too long for the rip')
    return stc


def wrap(stc, rip, length=None):
    """The rip with its song replaced by `stc`; `length` (1/50 s) also sets the EMUL song length
    and a fade of 0 (None: the rip's own are kept)."""
    d = bytearray(rip)
    i = d.find(bytes([0xC0, 0x00, 0xFF, 0x43]))
    if i < 0 or len(d) < CODE_OFF + (BASE - 0xC000):
        raise ValueError('not the expected rip (block $C000, length $FF43)')
    new = bytearray(d[:CODE_OFF + (BASE - 0xC000)]) + stc
    new[i + 2:i + 4] = struct.pack('>H', BASE - 0xC000 + len(stc))
    if length is not None:
        ss = 18 + be16s(d, 18)
        sd = ss + 2 + be16s(d, ss + 2)
        new[sd + 4:sd + 8] = struct.pack('>HH', min(length, 0xFFFF), 0)
    return bytes(new)


def pass_length(s):
    """Interrupts of one pass of the position list."""
    return s['delay'] * s['patlen'] * len(s['positions'])


def song(d):
    typ, sd = container(d)
    return parse_st11(d, sd) if typ == b'ST11' else parse_strc(d, sd)


def main():
    a = sys.argv[1:]
    stc_out = None
    if '--stc' in a:
        i = a.index('--stc')
        stc_out = a[i + 1]
        del a[i:i + 2]
    global REPEAT_ENV
    one_pass = '--length' in a
    if one_pass:
        a.remove('--length')
    if '--repeat-envelope' in a:
        a.remove('--repeat-envelope')
        REPEAT_ENV = True
    if len(a) != 3:
        sys.exit(__doc__)
    s = song(open(a[0], 'rb').read())
    stc = compile_stc(s)
    open(a[2], 'wb').write(wrap(stc, open(a[1], 'rb').read(), pass_length(s) if one_pass else None))
    if stc_out:
        open(stc_out, 'wb').write(stc)
    print('%s: STC %d bytes' % (a[2], len(stc)))


if __name__ == '__main__':
    main()
