#!/usr/bin/env python3
"""68k-player inventory as a SQLite database, rebuilt on demand.

Which installed DeliPlayers / Eagleplayers take which sample files when run alone
(`--68k-first --info`), and what the default build does with the same files (`--info`, no
players loaded). A player whose files the default build cannot open is a gap: a format that
still needs a clean-room open replayer. Method as in docs/specs/COVERAGE-DELI.md; no player
code is read, only `$VER:` strings and black-box runs.

    tools/inventory.py build  --render BIN [--cli BIN2] [--db DB] [--jobs N] [--refresh-68k]
    tools/inventory.py import-cov-deli DIR [--db DB]     # seed the 68k cache from an old matrix
    tools/inventory.py report [--db DB] [--all]

BIN is an `amigaplayer-render` built with `--features host-68k`. With --cli (a default-build
`amigaplayer-rs`), every custom module under CUSTOM is also rendered for 20 s on the project's
own 68000 host: silent or not, and the OS calls it made that the host does not implement. The 68k results are cached by
(player sha1, sample path, size, mtime), so a rebuild only runs new players or samples; the
default-build column is re-measured on every build. Each build adds a row to `run`; tables
with a run_id keep history, the views show the latest run. Hand-kept judgements (difficulty,
priority, docs) come from tools/inventory-notes.tsv; extra per-player probe files from
tools/inventory-probes.tsv, files always sampled from tools/inventory-samples.tsv.
"""
import argparse, csv, hashlib, json, os, re, sqlite3, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCRATCH = '/media/lolo/kingston2tb/claude-scratch/deliplayerrust'
DB = os.path.join(SCRATCH, 'inventory', 'inventory.sqlite')
CONF = os.path.expanduser('~/.config/deliplayer')
UADE = os.path.expanduser('~/.local/opt/uade/share/uade/players')
EXTRA = os.path.join(SCRATCH, 'fixround', 'cov-deli', 'work', 'extra')
# one directory per custom module (the cust.* file first by name, its companions beside it)
CUSTOM = os.path.join(SCRATCH, 'fixround', 'custlvo', 'all')
NOTES = os.path.join(REPO, 'tools', 'inventory-notes.tsv')
PROBES = os.path.join(REPO, 'tools', 'inventory-probes.tsv')
SAMPLES = os.path.join(REPO, 'tools', 'inventory-samples.tsv')
OPEN_BACKENDS = ('open-replayer', 'xmp', 'protracker', 'pcm', 'sid')

SCHEMA = """
CREATE TABLE IF NOT EXISTS run(id INTEGER PRIMARY KEY, started TEXT, git_head TEXT, git_dirty INT,
  render TEXT, render_sha1 TEXT, samples INT, jobs_68k INT, secs REAL);
CREATE TABLE IF NOT EXISTS player(run_id INT, rel TEXT, kind TEXT, file TEXT, size INT, sha1 TEXT,
  ver TEXT, PRIMARY KEY(run_id, rel));
CREATE TABLE IF NOT EXISTS sample(run_id INT, path TEXT, src TEXT, key TEXT, size INT,
  PRIMARY KEY(run_id, path));
CREATE TABLE IF NOT EXISTS open_path(run_id INT, path TEXT, backend TEXT, format TEXT,
  PRIMARY KEY(run_id, path));
CREATE TABLE IF NOT EXISTS m68k_cache(player_sha1 TEXT, path TEXT, size INT, mtime INT,
  backend TEXT, format TEXT, PRIMARY KEY(player_sha1, path, size, mtime));
CREATE TABLE IF NOT EXISTS accept(run_id INT, rel TEXT, path TEXT, format TEXT,
  PRIMARY KEY(run_id, rel, path));
CREATE TABLE IF NOT EXISTS refcmp(run_id INT, rate INT, id INT, fmt TEXT, cls TEXT, backend TEXT,
  port_player TEXT, port_format TEXT, path TEXT, PRIMARY KEY(run_id, rate, id));
CREATE TABLE IF NOT EXISTS spec(run_id INT, name TEXT, has_spec INT, PRIMARY KEY(run_id, name));
CREATE TABLE IF NOT EXISTS note(run_id INT, player TEXT, formats TEXT, extensions TEXT,
  local_modules INT, refcmp_rows INT, docs TEXT, difficulty TEXT, priority INT, notes TEXT,
  PRIMARY KEY(run_id, player));
CREATE TABLE IF NOT EXISTS custom(run_id INT, id TEXT, file TEXT, rc INT, frames INT,
  nonzero_pct REAL, peak REAL, md5 TEXT, missing TEXT, fatal INT, error TEXT, PRIMARY KEY(run_id, id));
DROP VIEW IF EXISTS v_custom;
CREATE VIEW v_custom AS
  SELECT id, file, nonzero_pct, peak, missing, fatal, error,
    CASE WHEN rc != 0 OR frames IS NULL THEN 'fails' WHEN nonzero_pct < 1 THEN 'silent'
         ELSE 'plays' END AS status
  FROM custom WHERE run_id = (SELECT id FROM latest);
DROP VIEW IF EXISTS latest;
CREATE VIEW latest AS SELECT max(id) AS id FROM run WHERE secs IS NOT NULL;
DROP VIEW IF EXISTS v_accept;
CREATE VIEW v_accept AS
  SELECT a.rel, a.path, a.format AS format_68k, o.backend AS open_backend, o.format AS open_format,
    CASE WHEN o.backend IN ('open-replayer','xmp','protracker','pcm','sid') THEN 'open'
         WHEN o.backend = '68k' THEN 'custom' ELSE 'gap' END AS state
  FROM accept a JOIN open_path o ON o.run_id = a.run_id AND o.path = a.path
  WHERE a.run_id = (SELECT id FROM latest)
    -- a custom module runs on the project's own 68000 host whichever player is loaded
    AND NOT (o.backend = '68k' AND a.format = o.format);
DROP VIEW IF EXISTS v_player;
CREATE VIEW v_player AS
  SELECT p.rel, p.kind, p.ver,
    count(v.path) AS taken,
    sum(v.state = 'open') AS open, sum(v.state = 'custom') AS custom, sum(v.state = 'gap') AS gap,
    CASE WHEN p.kind = 'genie' THEN 'helper'
         WHEN count(v.path) = 0 THEN 'untested'
         WHEN sum(v.state = 'gap') = 0 THEN 'covered'
         WHEN sum(v.state = 'gap') = count(v.path) THEN 'gap' ELSE 'partial' END AS status,
    group_concat(DISTINCT CASE WHEN v.state = 'gap' THEN v.format_68k END) AS gap_formats,
    n.difficulty, n.priority, n.notes
  FROM player p LEFT JOIN v_accept v ON v.rel = p.rel
  LEFT JOIN note n ON n.run_id = p.run_id AND (n.player = p.rel OR n.player = p.file)
  WHERE p.run_id = (SELECT id FROM latest) AND p.kind != 'uade'
  GROUP BY p.rel;
DROP VIEW IF EXISTS v_gap_format;
CREATE VIEW v_gap_format AS
  SELECT format_68k, count(DISTINCT path) AS samples, group_concat(DISTINCT rel) AS players
  FROM v_accept WHERE state = 'gap' GROUP BY format_68k ORDER BY samples DESC;
"""


def sha1(path):
    h = hashlib.sha1()
    with open(path, 'rb') as f:
        for b in iter(lambda: f.read(1 << 20), b''):
            h.update(b)
    return h.hexdigest()


def ver_string(path):
    """The `$VER:` string a player file carries (a data string, not code)."""
    with open(path, 'rb') as f:
        d = f.read()
    m = re.search(rb'\$VER: ?([\x20-\x7e]{1,80})', d)
    return m.group(1).decode('ascii', 'replace').strip() if m else ''


def players():
    out = []
    for top, kind in (('DeliPlayers', 'deli'), ('Eagleplayers', 'eagle'), ('DeliGenies', 'genie')):
        for root, dirs, files in os.walk(os.path.join(CONF, top)):
            dirs.sort()
            if 'AYPlayers' in root:
                continue
            for f in sorted(files):
                p = os.path.join(root, f)
                out.append((os.path.relpath(p, CONF), kind, f, p))
    if os.path.isdir(UADE):
        for f in sorted(os.listdir(UADE)):
            p = os.path.join(UADE, f)
            if os.path.isfile(p):
                out.append(('uade/' + f, 'uade', f, p))
    return out


def baseline(path):
    rows = []
    with open(path, newline='') as f:
        for r in csv.DictReader(f, delimiter='\t'):
            m = re.search(r'backend=(\S+) player=(\S+) format=(.*?) filter', r.get('out_port') or '')
            rows.append((int(r['id']), r['fmt'], r['cls'], *(m.groups() if m else ('', '', '')), r['path']))
    return rows


def samples(base_rows):
    """Two local files per port format of the 48 kHz baseline, the extra/ files, the probes."""
    seen, out = {}, []
    for rid, fmt, cls, be, pl, pf, path in base_rows:
        k = pf or 'noport'
        if seen.get(k, 0) >= 2 or path.startswith('/media/lolo/seagate12tb') or not os.path.isfile(path):
            continue
        seen[k] = seen.get(k, 0) + 1
        out.append((path, 'base', k, None))
    if os.path.isdir(EXTRA):
        for d in sorted(os.listdir(EXTRA)):
            for f in sorted(os.listdir(os.path.join(EXTRA, d)))[:3]:
                if not (f.endswith('.nt') or f.lower().startswith('smpl.')):
                    out.append((os.path.join(EXTRA, d, f), 'extra', d, None))
    if os.path.isfile(SAMPLES):
        for line in open(SAMPLES):
            if line.strip() and not line.startswith('#'):
                key, path = line.rstrip('\n').split('\t')[:2]
                if os.path.isfile(path):
                    out.append((path, 'known', key, None))
    if os.path.isfile(PROBES):
        for line in open(PROBES):
            if line.strip() and not line.startswith('#'):
                rel, path = line.rstrip('\n').split('\t')[:2]
                if os.path.isfile(path):
                    out.append((path, 'probe', rel, rel))
    return out


def one_dir(work, rel, p):
    """The player alone in a directory (PlayAY with its AYPlayers/)."""
    d = os.path.join(work, 'one', rel.replace('/', '__'))
    os.makedirs(d, exist_ok=True)
    link = os.path.join(d, os.path.basename(p))
    if not os.path.lexists(link):
        os.symlink(p, link)
    if os.path.basename(p) == 'PlayAY' and not os.path.lexists(os.path.join(d, 'AYPlayers')):
        os.symlink(os.path.join(CONF, 'Eagleplayers', 'AYPlayers'), os.path.join(d, 'AYPlayers'))
    return d


def info(render, args, path, cwd):
    env = dict(os.environ, SDL_AUDIO_DRIVER='dummy', RUST_LOG='off')
    try:
        p = subprocess.run([render, *args, '--info', path], capture_output=True, text=True,
                           timeout=15, env=env, cwd=cwd)
        out = p.stdout.strip()
        j = json.loads(out.splitlines()[-1]) if out else {}
        return j.get('backend', 'none'), j.get('format', p.stderr.strip()[-80:])
    except subprocess.TimeoutExpired:
        return 'timeout', ''
    except Exception as e:  # noqa: BLE001 - a bad line is a result, not a crash of the survey
        return 'error', str(e)[:80]


def git(*a):
    return subprocess.run(['git', '-C', REPO, *a], capture_output=True, text=True).stdout.strip()


def build(a):
    t0 = time.time()
    os.makedirs(os.path.dirname(a.db), exist_ok=True)
    work = os.path.join(os.path.dirname(a.db), 'work')
    os.makedirs(work, exist_ok=True)
    db = sqlite3.connect(a.db)
    db.executescript(SCHEMA)
    run = db.execute('INSERT INTO run(started, git_head, git_dirty, render, render_sha1) VALUES (?,?,?,?,?)',
                     (time.strftime('%Y-%m-%d %H:%M:%S'), git('rev-parse', 'HEAD'),
                      int(bool(git('status', '--porcelain', '--untracked-files=no'))),
                      a.render, sha1(a.render))).lastrowid
    pl = players()
    for rel, kind, f, p in pl:
        db.execute('INSERT INTO player VALUES (?,?,?,?,?,?,?)',
                   (run, rel, kind, f, os.path.getsize(p), sha1(p), ver_string(p)))
    rows = {r: baseline(os.path.join(REPO, 'tools', f)) for r, f in
            ((48000, 'refcmp-baseline.tsv'), (44100, 'refcmp-baseline-44100.tsv'))}
    for rate, rr in rows.items():
        db.executemany('INSERT INTO refcmp VALUES (?,?,?,?,?,?,?,?,?)', [(run, rate, *r) for r in rr])
    sp = os.path.join(REPO, 'docs', 'specs')
    for n in sorted(os.listdir(sp)):
        if os.path.isdir(os.path.join(sp, n)):
            db.execute('INSERT INTO spec VALUES (?,?,?)', (run, n, int(os.path.isfile(os.path.join(sp, n, 'SPEC.md')))))
    if os.path.isfile(NOTES):
        with open(NOTES, newline='') as f:
            for r in csv.DictReader((l for l in f if not l.startswith('#')), delimiter='\t'):
                db.execute('INSERT OR REPLACE INTO note VALUES (?,?,?,?,?,?,?,?,?,?)',
                           (run, r['player'], r.get('formats'), r.get('extensions'),
                            int(r.get('local_modules') or 0), int(r.get('refcmp_rows') or 0), r.get('docs'),
                            r.get('difficulty'), int(r['priority']) if (r.get('priority') or '').isdigit() else None,
                            r.get('notes')))
    smp = samples(rows[48000])
    for path, src, key, _ in dict((s[0], s) for s in smp).values():
        db.execute('INSERT OR IGNORE INTO sample VALUES (?,?,?,?,?)', (run, path, src, key, os.path.getsize(path)))
    db.commit()

    # default build: no players loaded
    paths = sorted({s[0] for s in smp})
    with ThreadPoolExecutor(a.jobs) as ex:
        res = list(ex.map(lambda p: info(a.render, [], p, work), paths))
    db.executemany('INSERT INTO open_path VALUES (?,?,?,?)', [(run, p, *r) for p, r in zip(paths, res)])
    db.commit()

    # 68k players alone, cached
    runnable = [(rel, kind, p) for rel, kind, f, p in pl if kind in ('deli', 'eagle')]
    psha = {rel: s for rel, s in db.execute('SELECT rel, sha1 FROM player WHERE run_id = ?', (run,))}
    stat = {p: os.stat(p) for p in paths}
    jobs, cached = [], []
    for rel, kind, p in runnable:
        for path, src, key, only in smp:
            if only and only != rel:
                continue
            st = stat[path]
            k = (psha[rel], path, st.st_size, int(st.st_mtime))
            hit = None if a.refresh_68k else db.execute(
                'SELECT backend, format FROM m68k_cache WHERE player_sha1=? AND path=? AND size=? AND mtime=?', k).fetchone()
            if hit:
                cached.append((rel, path, *hit))
            else:
                jobs.append((rel, kind, p, path, k))
    jobs = list({(j[0], j[3]): j for j in jobs}.values())
    print(f'run {run}: {len(paths)} samples, {len(runnable)} players, {len(cached)} cached, {len(jobs)} to run', flush=True)

    dirs = {rel: one_dir(work, rel, p) for rel, kind, p in runnable}

    def go(j):
        rel, kind, p, path, k = j
        flag = '--players' if kind == 'deli' else '--eagleplayers'
        return j, info(a.render, [flag, dirs[rel], '--68k-first'], path, work)

    done = 0
    with ThreadPoolExecutor(a.jobs) as ex:
        for (rel, kind, p, path, k), (b, f) in ex.map(go, jobs):
            db.execute('INSERT OR REPLACE INTO m68k_cache VALUES (?,?,?,?,?,?)', (*k, b, f))
            cached.append((rel, path, b, f))
            done += 1
            if done % 1000 == 0:
                db.commit()
                print(f'  {done}/{len(jobs)}', flush=True)
    db.executemany('INSERT OR IGNORE INTO accept VALUES (?,?,?,?)',
                   [(run, rel, path, f) for rel, path, b, f in cached if b == '68k'])
    if a.cli:
        customs(db, run, a, work)
    db.execute('UPDATE run SET samples=?, jobs_68k=?, secs=? WHERE id=?', (len(paths), len(jobs), time.time() - t0, run))
    db.commit()
    report(argparse.Namespace(db=a.db, all=False))


def custom_one(cli, d, work):
    f = sorted(x for x in os.listdir(d) if x.lower().startswith('cust'))
    f = f[0] if f else sorted(os.listdir(d))[0]
    wav = os.path.join(work, 'cust', os.path.basename(d) + '.wav')
    env = dict(os.environ, SDL_AUDIO_DRIVER='dummy', RUST_LOG='warn')
    try:
        p = subprocess.run([cli, '--wav', wav, '--seconds', '20', os.path.join(d, f)], capture_output=True,
                           text=True, timeout=180, env=env, cwd=work)
        rc, err = p.returncode, p.stderr
    except subprocess.TimeoutExpired:
        rc, err = -1, 'timeout'
    miss = re.findall(r'^(?:warning|error): the module called (.*), which the host does not implement$', err, re.M)
    fatal = int(bool(re.search(r'^error: the module called', err, re.M)))
    m = re.search(r'^error: (?!the module called)(.*)$', err, re.M)
    frames = nz = peak = md5 = None
    if os.path.isfile(wav):
        import array, wave
        with wave.open(wav) as w:
            frames = w.getnframes()
            data = w.readframes(frames)
        x = array.array('h', data)
        nz = round(100 * sum(1 for v in x if v) / max(1, len(x)), 1)
        peak = round(max((abs(v) for v in x), default=0) / 32768, 3)
        md5 = hashlib.md5(data).hexdigest()[:12]
        os.remove(wav)
    return (os.path.basename(d), f, rc, frames, nz, peak, md5, '; '.join(miss), fatal,
            (m.group(1) if m else ('timeout' if rc == -1 else ''))[:200])


def customs(db, run, a, work):
    os.makedirs(os.path.join(work, 'cust'), exist_ok=True)
    dirs = sorted(os.path.join(CUSTOM, d) for d in os.listdir(CUSTOM)) if os.path.isdir(CUSTOM) else []
    dirs = [d for d in dirs if os.path.isdir(d) and any(os.path.isfile(os.path.join(d, x)) for x in os.listdir(d))]
    print(f'custom modules: {len(dirs)} to render', flush=True)
    with ThreadPoolExecutor(a.jobs) as ex:
        for r in ex.map(lambda d: custom_one(a.cli, d, work), dirs):
            db.execute('INSERT OR REPLACE INTO custom VALUES (?,?,?,?,?,?,?,?,?,?,?)', (run, *r))
    db.commit()


def import_cov_deli(a):
    """Seed m68k_cache from fixround/cov-deli/work/matrix-68k.jsonl and pass2.jsonl, for
    player files and samples that are unchanged since (sha1 of the current file, current stat)."""
    os.makedirs(os.path.dirname(a.db), exist_ok=True)
    db = sqlite3.connect(a.db)
    db.executescript(SCHEMA)
    psha, n, probes = {}, 0, set()
    for name in ('matrix-68k.jsonl', 'pass2.jsonl'):
        fp = os.path.join(a.dir, name)
        if not os.path.isfile(fp):
            continue
        for line in open(fp):
            r = json.loads(line)
            rel, path = r['player'], r['path']
            if rel == '(open)' or not os.path.isfile(path):
                continue
            pp = os.path.join(CONF, rel)
            if not os.path.isfile(pp):
                continue
            if rel not in psha:
                psha[rel] = sha1(pp)
            st = os.stat(path)
            db.execute('INSERT OR REPLACE INTO m68k_cache VALUES (?,?,?,?,?,?)',
                       (psha[rel], path, st.st_size, int(st.st_mtime), r['backend'], r['format']))
            n += 1
            if name == 'pass2.jsonl':
                probes.add((rel, path))
    db.commit()
    if probes and not os.path.isfile(PROBES):
        with open(PROBES, 'w') as f:
            f.write('# player (relative to ~/.config/deliplayer)\tprobe file: extra files run only on that player\n')
            for rel, path in sorted(probes):
                f.write(f'{rel}\t{path}\n')
    print(f'{n} cached results, {len(psha)} players, {len(probes)} probes')


def report(a):
    db = sqlite3.connect(a.db)
    run = db.execute('SELECT id, started, substr(git_head,1,9), samples, jobs_68k, round(secs) FROM run WHERE secs IS NOT NULL ORDER BY id DESC LIMIT 1').fetchone()
    if not run:
        sys.exit('no run yet: tools/inventory.py build --render BIN')
    print(f'run {run[0]} at {run[1]} (HEAD {run[2]}): {run[3]} samples, {run[4]} 68k runs, {run[5]} s')
    for st, n in db.execute('SELECT status, count(*) FROM v_player GROUP BY status ORDER BY 2 DESC'):
        print(f'  {st:9} {n}')
    q = "SELECT rel, status, taken, gap, gap_formats, difficulty, priority FROM v_player"
    q += '' if a.all else " WHERE status IN ('gap','partial')"
    c = db.execute("SELECT status, count(*) FROM v_custom GROUP BY status ORDER BY 2 DESC").fetchall()
    if c:
        print('custom modules (own 68000 host): ' + ', '.join(f'{n} {st}' for st, n in c))
        for r in db.execute("SELECT status, file, missing, error FROM v_custom WHERE status != 'plays' OR missing != '' ORDER BY status, file"):
            print('  ' + '\t'.join('' if x is None else str(x) for x in r))
    print('\nplayer\tstatus\ttaken\tgap\tgap formats\tdifficulty\tpriority')
    for r in db.execute(q + ' ORDER BY coalesce(priority, 999), gap DESC, rel'):
        print('\t'.join('' if x is None else str(x) for x in r))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest='cmd', required=True)
    b = sub.add_parser('build')
    b.add_argument('--render', required=True)
    b.add_argument('--cli', help='default-build amigaplayer-rs: also render the custom modules')
    b.add_argument('--jobs', type=int, default=2)
    b.add_argument('--refresh-68k', action='store_true')
    i = sub.add_parser('import-cov-deli')
    i.add_argument('dir')
    r = sub.add_parser('report')
    r.add_argument('--all', action='store_true')
    for p in (b, i, r):
        p.add_argument('--db', default=DB)
    a = ap.parse_args()
    {'build': build, 'import-cov-deli': import_cov_deli, 'report': report}[a.cmd](a)


main()
