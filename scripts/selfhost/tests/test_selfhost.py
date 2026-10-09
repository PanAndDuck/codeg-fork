#!/usr/bin/env python3
"""macOS integration checks; never launch codeg or use official data."""
import ctypes
import json
import os
from pathlib import Path
import plistlib
import shutil
import signal
import sqlite3
import subprocess
import tempfile
import time
import unittest
import uuid

for key in ('CODEG_DATA_DIR', 'CODEG_HOME', 'CODEG_STATIC_DIR', 'CODEG_ACP_TMP_ROOT',
            'CODEG_TOKEN', 'CODEG_PORT', 'CODEG_HOST'):
    os.environ.pop(key, None)
SCRIPTS = Path(__file__).resolve().parents[1]
LIBPROC = ctypes.CDLL('/usr/lib/libproc.dylib')


def executable(pid):
    buf = ctypes.create_string_buffer(4096)
    return buf.value.decode() if LIBPROC.proc_pidpath(pid, buf, len(buf)) else ''


def run(*args, **kwargs):
    return subprocess.run(args, check=True, stdout=subprocess.PIPE,
                          stderr=subprocess.STDOUT, text=True, **kwargs).stdout


class SelfhostTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix='codeg-selfhost-test-')).resolve()
        self.addCleanup(self.cleanup)
        self.app = self.root / 'Applications/codeg.app'
        self.stage = self.root / 'staged/codeg.app'
        self.backup = self.root / 'backups'
        self.data = self.root / 'data'
        self.home = self.root / 'home'
        self.identifier = 'app.codeg.selfhost.test.' + uuid.uuid4().hex
        self.env = {k: v for k, v in os.environ.items() if not k.startswith('CODEG')}
        self.env.update(CODEG_SWAP_APPS_DIR=str(self.app.parent),
                        CODEG_SWAP_DATA_DIR=str(self.data), CODEG_SWAP_HOME_DIR=str(self.home),
                        CODEG_SWAP_BACKUP_DIR=str(self.backup),
                        CODEG_SWAP_BUNDLE_ID=self.identifier, CODEG_SWAP_PORT='3097')
        for p in (self.data, self.home, self.app / 'Contents/MacOS'):
            p.mkdir(parents=True)
        self.db = sqlite3.connect(self.data / 'codeg.db')
        self.db.executescript("PRAGMA journal_mode=WAL; CREATE TABLE sample(value TEXT);"
                             "INSERT INTO sample VALUES ('original');"
                             "CREATE TABLE app_metadata(key TEXT, value TEXT, deleted_at TEXT);"
                             "INSERT INTO app_metadata VALUES ('web_service_auto_start','true',NULL);")
        self.db.commit()
        (self.home / 'settings').write_text('original')
        (self.data / 'other').write_text('original')
        run('clang', str(SCRIPTS / 'tests/dummy.m'), '-framework', 'Cocoa',
            '-o', str(self.app / 'Contents/MacOS/codeg'))
        self.write_plist(self.app, 'original')
        run('codesign', '--force', '--sign', '-', str(self.app))
        shutil.copytree(self.app, self.stage)
        self.write_plist(self.stage, 'upgraded')
        run('codesign', '--force', '--sign', '-', str(self.stage))

    def write_plist(self, app, commit):
        with (app / 'Contents/Info.plist').open('wb') as f:
            plistlib.dump(dict(CFBundleIdentifier=self.identifier, CFBundleName='codeg',
                              CFBundleExecutable='codeg', CFBundlePackageType='APPL',
                              CFBundleShortVersionString='1.0', CFBundleVersion='1',
                              CodegBuildCommit=commit, LSUIElement=True, TestPort='3097'), f)

    def cleanup(self):
        if hasattr(self, 'db'):
            self.db.close()
        # Only signals exact executables inside this test's unique temporary root.
        for raw in run('ps', '-axo', 'pid=').split():
            pid = int(raw)
            if executable(pid) == str(self.app / 'Contents/MacOS/codeg'):
                os.kill(pid, signal.SIGTERM)
        if self.backup.exists():
            for plist in self.backup.glob('jobs/*/job.plist'):
                with plist.open('rb') as f:
                    label = plistlib.load(f)['Label']
                assert label.startswith('app.codeg.selfhost.test.')
                subprocess.run(['launchctl', 'bootout', f'gui/{os.getuid()}/{label}'],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(.3)
        shutil.rmtree(self.root)

    def script(self, name, *args, success=True):
        result = subprocess.run([str(SCRIPTS / name), *map(str, args)], env=self.env,
                                capture_output=True, text=True, timeout=200)
        self.assertEqual(result.returncode == 0, success, result.stdout + result.stderr)
        return result.stdout + result.stderr

    def commit(self):
        with (self.app / 'Contents/Info.plist').open('rb') as f:
            return plistlib.load(f)['CodegBuildCommit']

    def transactions(self):
        return sorted(self.backup.glob('manifests/*.json'))

    def test_inline_swap_backup_and_explicit_data_rollback(self):
        # Catches missing online WAL backup, failed restart, and implicit data restore.
        run('open', '-n', '-a', str(self.app), env=self.env)
        self.script('swap.sh', '--staged', self.stage, '--inline')
        self.assertEqual(self.commit(), 'upgraded')
        manifest = json.loads(self.transactions()[0].read_text())
        saved = Path(manifest['data_snapshot']) / 'data/codeg.db'
        with sqlite3.connect(saved) as db:
            self.assertEqual(db.execute('PRAGMA integrity_check').fetchone()[0], 'ok')
            self.assertEqual(db.execute('SELECT value FROM sample').fetchone()[0], 'original')
        self.assertTrue(Path(manifest['app_backup']).is_dir())
        self.assertIn('OK', Path(manifest['log']).read_text())
        self.db.close()
        (self.home / 'settings').write_text('changed')
        self.script('rollback.sh', '--to', manifest['app_backup'], '--inline')
        self.assertEqual(self.commit(), 'original')
        self.assertEqual((self.home / 'settings').read_text(), 'changed')
        self.script('rollback.sh', '--to', manifest['app_backup'], '--restore-data',
                    manifest['id'], '--inline')
        self.assertEqual((self.home / 'settings').read_text(), 'original')

    def test_detached_job_survives_launcher_and_restores(self):
        self.script('swap.sh', '--staged', self.stage)
        manifest = json.loads(self.transactions()[0].read_text())
        status = Path(manifest['job_dir']) / 'status'
        deadline = time.monotonic() + 110
        while not status.exists() and time.monotonic() < deadline:
            time.sleep(.5)
        self.assertTrue(status.exists(), Path(manifest['log']).read_text())
        self.assertEqual(status.read_text().strip(), 'OK', Path(manifest['log']).read_text())
        self.assertEqual(self.commit(), 'upgraded')
        self.assertIn('ppid=1', Path(manifest['log']).read_text())
        self.script('rollback.sh', '--to', manifest['app_backup'], '--inline')
        self.assertEqual(self.commit(), 'original')

    def test_failed_relaunch_automatically_restores_original(self):
        resources = self.stage / 'Contents/Resources'
        resources.mkdir()
        (resources / 'fail').touch()
        run('codesign', '--force', '--sign', '-', str(self.stage))
        self.script('swap.sh', '--staged', self.stage, '--inline', success=False)
        self.assertEqual(self.commit(), 'original')
        log = Path(json.loads(self.transactions()[0].read_text())['log']).read_text()
        self.assertIn('AUTO-ROLLBACK OK', log)

    def test_dry_run_creates_nothing(self):
        before = self.app.stat()
        self.script('swap.sh', '--staged', self.stage, '--dry-run')
        self.assertFalse(self.backup.exists())
        self.assertEqual(self.app.stat().st_ino, before.st_ino)
        self.assertEqual(self.commit(), 'original')

    def test_identical_or_invalid_signature_rejected_before_backup(self):
        self.script('swap.sh', '--staged', self.app, '--inline', success=False)
        (self.stage / 'Contents/MacOS/codeg').write_bytes(b'invalid')
        self.script('swap.sh', '--staged', self.stage, '--inline', success=False)
        self.assertFalse(self.backup.exists())


if __name__ == '__main__':
    unittest.main(verbosity=2)
