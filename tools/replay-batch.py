"""Check an open replayer against a 68k player on many modules.

    python3 -I tools/replay-batch.py --player 'SIDMon 2.0' --format SidMon2 \
        --out DIR [--seconds 20] [--bin target/release/deliplayer] [--in-place] LIST

LIST has one module path per line (a missing file is also looked for as
PATH.xz). Modules are copied into DIR, except with --in-place, where one found
as itself is played where it is, next to the files it loads (TFMX's smpl.*, an
IFF-SMUS song's instruments). Each distinct module is played with the 68k player (--player) and
with the open replayers (--open-replayers, whose "playing" line must show
[FORMAT]), and the write logs are compared with replay-cmp.py. Prints how
many are identical, differ (with the first line of the comparison), or were
refused by either side. The logs, renders and modules stay in DIR.
"""
import argparse, hashlib, lzma, os, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
ap = argparse.ArgumentParser()
ap.add_argument('--player', required=True)
ap.add_argument('--format', required=True)
ap.add_argument('--out', required=True)
ap.add_argument('--seconds', default='20')
ap.add_argument('--bin', default=os.path.join(HERE, '..', 'target', 'release', 'deliplayer'))
ap.add_argument('--in-place', action='store_true')
ap.add_argument('list')
a = ap.parse_args()
os.makedirs(a.out, exist_ok=True)

seen, res = set(), {}
for p in (l.strip() for l in open(a.list)):
    if not p:
        continue
    data = (open(p, 'rb').read() if os.path.isfile(p)
            else lzma.open(p + '.xz').read() if os.path.isfile(p + '.xz') else None)
    if data is None:
        res.setdefault('missing', []).append(p)
        continue
    h = hashlib.md5(data).hexdigest()
    if h in seen:
        continue
    seen.add(h)
    out = os.path.join(a.out, h)
    if a.in_place and os.path.isfile(p):
        mod = p
    else:
        mod = out
        open(mod, 'wb').write(data)

    def run(tag, extra):
        env = dict(os.environ, DELI_WRITE_LOG=f'{out}.{tag}.log')
        try:
            r = subprocess.run([a.bin, *extra, '--wav', f'{out}.{tag}.wav', '--seconds', a.seconds, mod],
                               env=env, capture_output=True, text=True, timeout=300)
        except subprocess.TimeoutExpired:
            return ['timeout']
        return [l for l in r.stderr.splitlines() if l.startswith('playing')] or [r.stderr.strip()[-200:]]

    ref = run('ref', ['--player', a.player])
    if not ref[0].startswith('playing'):
        res.setdefault('reference refuses', []).append((p, ref[0]))
        continue
    port = run('port', ['--open-replayers'])
    if not port[0].startswith('playing') or f'[{a.format}]' not in port[0]:
        res.setdefault('port refuses', []).append((p, port[0]))
        continue
    c = subprocess.run([sys.executable, '-I', os.path.join(HERE, 'replay-cmp.py'), f'{out}.ref.log', f'{out}.port.log'],
                       capture_output=True, text=True)
    key = 'identical' if c.returncode == 0 else 'differs'
    res.setdefault(key, []).append((p, h, (c.stdout or c.stderr).splitlines()[0] if (c.stdout or c.stderr) else ''))

for k in ('identical', 'differs', 'port refuses', 'reference refuses', 'missing'):
    v = res.get(k, [])
    print(f'== {k}: {len(v)}')
    for x in (v[:3] if k == 'identical' else v[:40]):
        print('  ', x)
