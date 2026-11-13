"""C64 PSID: the `sid`-feature port against the original Amiga player, PlaySID (EP206's Eagleplayer
"PlaySid", V2.1-3.0 Eagleplayer V1.01, 16 Jan 1995), run on the engine. Not part of refcmp's gate:
the gate's binary is built without `sid` (GPL-2.0-only, off by default) and the corpus list has no
PSID file. Classes are refcmp's (tools/refcmp.py: compare, classify).

    cargo build --release -j 4 --features "host-68k sid"     # (private CARGO_TARGET_DIR)
    python3 -I tools/psid-playsid.py --bin BIN --out DIR FILE.sid ...

Both sides render 20 s at 48 kHz, the file's start song, with --filter none: the port's C64 output
(reSIDfp's resampler) never passes the Amiga filter (same samples with and without the flag), and
PlaySID's Paula output is compared before the A500's filter, which is no part of a C64's sound.
PlaySID needs no library, config or companion file. It is another emulation of the SID (its own
waveform synthesis on Paula), not the hardware: the class says how far the port is from it.
"""
import argparse, os, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import refcmp as R  # noqa: E402

PLAYSID = os.path.join(R.EP206, 'PlaySid')


def render(binary, out, mod, original, secs, work):
    empty = os.path.join(work, 'empty')
    alone = os.path.join(work, 'playsid')
    os.makedirs(empty, exist_ok=True)
    os.makedirs(alone, exist_ok=True)
    link = os.path.join(alone, 'PlaySid')
    if not os.path.lexists(link):
        os.symlink(PLAYSID, link)
    env = dict(os.environ, SDL_AUDIO_DRIVER='dummy', RUST_LOG='error', DELIPLAYERS=empty, DELIGENIES=empty,
               EAGLEPLAYERS=alone if original else empty)
    side = ['--68k-players', '--player', 'PlaySid'] if original else ['--open-replayers']
    r = subprocess.run([binary, *side, '--filter', 'none', '--wav', out, '--seconds', str(secs), mod],
                       env=env, capture_output=True, text=True, timeout=secs * 4 + 120)
    lines = r.stderr.splitlines()
    play = next((l for l in lines if l.startswith('playing')), '')
    outl = next((l for l in lines if l.startswith('output:')), '')
    return (out if r.returncode == 0 and os.path.exists(out) else None), play, outl, (lines[-1] if lines else '')


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--bin', required=True, help='amigaplayer-rs built with --features "host-68k sid"')
    ap.add_argument('--out', required=True, help='work directory (renders, psid.tsv)')
    ap.add_argument('--seconds', type=int, default=20)
    ap.add_argument('files', nargs='+')
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    rows = []
    for i, f in enumerate(a.files):
        stem = f'{i:03d}'
        pw, pplay, pout, plast = render(a.bin, os.path.join(a.out, stem + '.port.wav'), f, False, a.seconds, a.out)
        rw, rplay, rout, rlast = render(a.bin, os.path.join(a.out, stem + '.playsid.wav'), f, True, a.seconds, a.out)
        if not pw or 'backend=sid-port' not in pout:
            cls, r = 'FAIL', {}
            note = 'port: ' + (pout or plast)[:80] + (' (a build without `sid`?)' if pw else '')
        elif not rw or 'backend=68k' not in rout:
            cls, r, note = 'NOREF', {}, 'PlaySID: ' + rlast[:80]
        else:
            ra, rb = R.read_wav(rw), R.read_wav(pw)
            r = R.compare(ra[1], rb[1], ra[0])
            cls = R.classify(r)
            mm = R.stage_mismatch(R.parse_output(rout), R.parse_output(pout))
            if mm and cls not in ('IDENTICAL', 'LAG-ONLY', 'NEAR'):
                cls = 'STAGE-MISMATCH'
            note = ' '.join(mm)
        rows.append((cls, r, note, f))
        print(f'{cls:14s} level {r.get("level_db")} dB bands {r.get("bands_db")} {note} {os.path.basename(f)}')
    with open(os.path.join(a.out, 'psid.tsv'), 'w') as fh:
        fh.write('cls\tlevel_db\tbands_db\tpitch_cents\tlag\tnote\tpath\n')
        for cls, r, note, f in rows:
            fh.write(f'{cls}\t{r.get("level_db", "")}\t{r.get("bands_db", "")}\t{r.get("pitch_cents", "")}\t{r.get("lag", "")}\t{note}\t{f}\n')


if __name__ == '__main__':
    main()
