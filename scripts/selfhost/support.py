#!/usr/bin/env python3
"""Structured macOS/SQLite operations for the Bash selfhost transaction."""
import ctypes
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shlex
import shutil
import signal
import sqlite3
import subprocess
import sys
import time
import uuid


def command(*args):
    return subprocess.check_output(args, stderr=subprocess.STDOUT, text=True).strip()


def identity(app):
    with (Path(app) / 'Contents/Info.plist').open('rb') as f:
        info = plistlib.load(f)
    signature = command('codesign', '-d', '--verbose=4', str(app))
    fields = dict(line.split('=', 1) for line in signature.splitlines() if '=' in line)
    return dict(bundle_id=info['CFBundleIdentifier'], executable=info['CFBundleExecutable'],
                version=info.get('CFBundleShortVersionString', 'unknown'),
                commit=info.get('CodegBuildCommit', ''), cdhash=fields.get('CDHash', ''),
                authority=next((line[10:] for line in signature.splitlines() if line.startswith('Authority=')), fields.get('Signature', 'unknown')))


def canonical(path):
    return str(Path(path).expanduser().resolve())


def overlaps(a, b):
    return a == b or a.startswith(b + '/') or b.startswith(a + '/')


def size(path):
    return sum(p.stat().st_size for p in Path(path).rglob('*') if p.is_file() and not p.is_symlink())


def plan(action, source, restore, no_relaunch, inline):
    home = str(Path.home())
    defaults = dict(apps_dir='/Applications', data_dir=home + '/Library/Application Support/app.codeg',
                    home_dir=home + '/.codeg', backup_dir=home + '/codeg-backups')
    cfg = {k: canonical(os.environ.get('CODEG_SWAP_' + k.upper(), v)) for k, v in defaults.items()}
    cfg['bundle_id'] = os.environ.get('CODEG_SWAP_BUNDLE_ID', 'app.codeg')
    cfg['port'] = int(os.environ.get('CODEG_SWAP_PORT', '3080'))
    assert re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9.-]+', cfg['bundle_id']), 'Invalid bundle id'
    assert 0 < cfg['port'] < 65536, 'Invalid port'
    cfg['current'] = cfg['apps_dir'] + '/codeg.app'
    # Reject symlink redirects and overlapping backup/data/app trees before any writes.
    assert canonical(cfg['current']) == cfg['current'], 'Current app must not be a symlink'
    roots = [cfg[k] for k in ('current', 'data_dir', 'home_dir', 'backup_dir')]
    assert all(p != '/' and p != home for p in roots), 'Unsafe root'
    assert all(not overlaps(a, b) for i, a in enumerate(roots) for b in roots[i + 1:]), 'Overlapping roots'
    fake = cfg['bundle_id'] != 'app.codeg'
    if fake or inline == 'true':
        assert fake and cfg['port'] != 3080, 'Inline/custom identity requires isolated test settings'
        for key in ('apps_dir', 'data_dir', 'home_dir', 'backup_dir'):
            assert not overlaps(cfg[key], canonical(defaults[key])), 'All test paths must be isolated'
        assert cfg['bundle_id'] != 'app.codeg.fork' and cfg['port'] != 3090, 'Codeg-Fork is protected'
    if not source:
        originals = []
        for path in Path(cfg['backup_dir']).glob('manifests/*.json'):
            m = json.loads(path.read_text())
            if m.get('backup_complete') and m.get('original', {}).get('commit', '') == '':
                originals.append((m['id'], m['app_backup']))
        assert originals, 'No upstream/original backup found; choose --to explicitly'
        source = sorted(originals)[-1][1]
    cfg['source'] = canonical(source)
    assert Path(cfg['source']).is_dir(), 'Missing staged/backup app'
    assert not overlaps(cfg['source'], cfg['current']), 'Source overlaps installed app'
    assert not any(overlaps(cfg['source'], cfg[k]) for k in ('data_dir', 'home_dir')), 'Source overlaps data'
    if action == 'swap':
        assert not overlaps(cfg['source'], cfg['backup_dir']), 'Stage outside backup directory'
    for app in (cfg['source'], cfg['current']):
        command('codesign', '--verify', '--deep', '--strict', app)
    cfg['original'], cfg['target'] = identity(cfg['current']), identity(cfg['source'])
    for info in (cfg['original'], cfg['target']):
        assert info['bundle_id'] == cfg['bundle_id'], 'Bundle identifier mismatch'
        assert info['executable'] == 'codeg', 'Expected main executable codeg'
    if action == 'swap':
        assert cfg['target']['commit'], 'Staged app must contain CodegBuildCommit'
        assert cfg['target']['cdhash'] != cfg['original']['cdhash'], 'Identical signed build'
        assert (cfg['target']['commit'], cfg['target']['version']) != (cfg['original']['commit'], cfg['original']['version']), 'Identical commit/version'
    for key in ('apps_dir', 'backup_dir'):
        existing = Path(cfg[key])
        while not existing.exists():
            existing = existing.parent
        assert shutil.disk_usage(existing).free >= 5 * 1024**3, 'At least 5 GiB free required: ' + str(existing)
    assert os.access(cfg['apps_dir'], os.W_OK), 'Applications directory not writable (run as its owner)'
    cfg['restore_snapshot'] = ''
    if restore:
        assert action == 'rollback' and re.fullmatch(r'[A-Za-z0-9-]+', restore), 'Invalid snapshot ID'
        cfg['restore_snapshot'] = cfg['backup_dir'] + '/data/' + restore
        assert canonical(cfg['restore_snapshot']) == cfg['restore_snapshot'], 'Snapshot symlink refused'
        assert (Path(cfg['restore_snapshot']) / 'data/codeg.db').is_file(), 'Missing data snapshot'
        integrity(Path(cfg['restore_snapshot']) / 'data/codeg.db')
    stamp = time.strftime('%Y%m%dT%H%M%S') + '-' + uuid.uuid4().hex[:10]
    safe = lambda value: re.sub(r'[^A-Za-z0-9.-]', '_', value)[:60]
    cfg.update(action=action, id=stamp, no_relaunch=no_relaunch == 'true',
               app_backup=cfg['backup_dir'] + '/apps/codeg-' + safe(cfg['original']['version']) + '-' + safe(cfg['original']['authority']) + '-' + stamp + '.app',
               data_snapshot=cfg['backup_dir'] + '/data/' + stamp,
               job_dir=cfg['backup_dir'] + '/jobs/' + stamp,
               log=cfg['backup_dir'] + '/logs/' + action + '-' + stamp + '.log',
               label=('app.codeg.selfhost.test.' if fake else 'app.codeg.selfhost.') + action + '.' + stamp)
    cfg['manifest'] = cfg['backup_dir'] + '/manifests/' + stamp + '.json'
    cfg['old_app'] = cfg['current'] + '.replaced-' + stamp
    cfg['new_app'] = cfg['current'] + '.new'
    assert not any(os.path.lexists(cfg[k]) for k in ('new_app', 'old_app')), 'Stale replacement path exists; inspect before retrying'
    return cfg


def connect_readonly(path):
    return sqlite3.connect(Path(path).as_uri() + '?mode=ro', uri=True, timeout=30)


def integrity(path):
    with connect_readonly(path) as db:
        assert db.execute('PRAGMA integrity_check').fetchall() == [('ok',)], 'SQLite integrity check failed'


def backup_db(source, target):
    with connect_readonly(source) as live, sqlite3.connect(target) as saved:
        live.backup(saved, pages=1024, sleep=.1)
    integrity(target)


def processes():
    lib = ctypes.CDLL('/usr/lib/libproc.dylib')
    for raw in command('ps', '-axo', 'pid=').split():
        pid = int(raw)
        buf = ctypes.create_string_buffer(4096)
        if lib.proc_pidpath(pid, buf, len(buf)):
            yield pid, os.fsdecode(buf.value)


def pids(app):
    expected = str(Path(app) / 'Contents/MacOS/codeg')
    return (pid for pid, path in processes() if path == expected)


def single_target(cfg):
    for pid, path in processes():
        if path.endswith('/Contents/MacOS/codeg'):
            app = path.removesuffix('/Contents/MacOS/codeg')
            if app != cfg['current']:
                try:
                    with (Path(app) / 'Contents/Info.plist').open('rb') as f:
                        identifier = plistlib.load(f).get('CFBundleIdentifier')
                except FileNotFoundError:
                    continue  # Process exited or its app moved during enumeration.
                assert identifier != cfg['bundle_id'], f'Ambiguous bundle id: another instance pid={pid} path={path}'


def requires_port(db):
    with connect_readonly(db) as conn:
        if conn.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_metadata'").fetchone():
            row = conn.execute("SELECT value FROM app_metadata WHERE key='web_service_auto_start' AND deleted_at IS NULL").fetchone()
            return bool(row and row[0].strip() in ('true', '1', 'yes'))
    return False


def complete_manifest(path):
    cfg = json.loads(Path(path).read_text())
    db = Path(cfg['data_snapshot']) / 'data/codeg.db'
    integrity(db)
    cfg['db_sha256'] = hashlib.sha256(db.read_bytes()).hexdigest()
    cfg['sizes'] = {k: size(cfg[k]) for k in ('app_backup', 'data_snapshot')}
    cfg['require_port'] = requires_port(db)
    cfg['restore_require_port'] = requires_port(Path(cfg['restore_snapshot']) / 'data/codeg.db') if cfg['restore_snapshot'] else False
    cfg['backup_complete'] = True
    Path(path).write_text(json.dumps(cfg, indent=2) + '\n')


def exports(cfg):
    for key, value in cfg.items():
        if isinstance(value, (str, int, bool)):
            if isinstance(value, bool):
                value = str(value).lower()
            print(key.upper() + '=' + shlex.quote(str(value)))


def launch_plist(cfg):
    # Explicit env: never inherit the host application's runtime overrides or secrets.
    env = {k: os.environ[k] for k in ('HOME', 'USER', 'LOGNAME', 'TMPDIR') if k in os.environ}
    env['PATH'] = '/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/usr/local/bin'
    data = dict(Label=cfg['label'], RunAtLoad=True, KeepAlive=False,
                AbandonProcessGroup=True, ProcessType='Interactive',
                ProgramArguments=['/bin/bash', cfg['job_dir'] + '/swap-worker.sh', cfg['manifest']],
                EnvironmentVariables=env, StandardOutPath=cfg['log'], StandardErrorPath=cfg['log'])
    with (Path(cfg['job_dir']) / 'job.plist').open('wb') as f:
        plistlib.dump(data, f)


def main():
    op, *args = sys.argv[1:]
    if op == 'plan':
        print(json.dumps(plan(*args)))
    elif op == 'exports':
        exports(json.load(sys.stdin) if not args else json.loads(Path(args[0]).read_text()))
    elif op == 'backup-db':
        backup_db(*args)
    elif op == 'complete':
        complete_manifest(*args)
    elif op == 'pids':
        print('\n'.join(map(str, pids(args[0]))))
    elif op == 'single-target':
        single_target(json.loads(Path(args[0]).read_text()))
    elif op == 'signal':
        for pid in pids(args[0]):
            if pid in set(pids(args[0])):
                print('signal ' + args[1] + ' exact-path pid=' + str(pid), flush=True)
                try:
                    os.kill(pid, getattr(signal, 'SIG' + args[1]))
                except ProcessLookupError:
                    pass
    elif op == 'verify':
        cfg = json.loads(Path(args[0]).read_text())
        expected = cfg['original'] if args[1] == 'original' else cfg['target']
        app = args[2] if len(args) > 2 else cfg['current']
        assert identity(app) == expected, 'Installed identity differs from manifest'
        command('codesign', '--verify', '--deep', '--strict', app)
        print('Verified CodegBuildCommit=' + (expected['commit'] or '(upstream: absent)') + ' cdhash=' + expected['cdhash'])
    elif op == 'plist':
        launch_plist(json.loads(Path(args[0]).read_text()))
    else:
        raise ValueError('Unknown operation ' + op)


if __name__ == '__main__':
    try:
        main()
    except (AssertionError, OSError, ValueError, sqlite3.Error, subprocess.CalledProcessError) as exc:
        print('selfhost: ' + str(exc), file=sys.stderr)
        sys.exit(1)
