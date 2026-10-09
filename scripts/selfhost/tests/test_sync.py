#!/usr/bin/env python3
"""Real temporary Git graph, with expensive build commands replaced at the boundary."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'sync-upstream.sh'


class SyncTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='codeg-sync-test-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / 'repo'
        self.repo.mkdir()
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        self.calls = self.root / 'calls'
        self.env = {k: v for k, v in os.environ.items() if not k.startswith('CODEG')}
        self.env.update(PATH=str(self.bin) + ':' + self.env['PATH'],
                        CODEG_SWAP_BACKUP_DIR=str(self.root / 'backups'),
                        R2_TEST_CALLS=str(self.calls), GIT_CONFIG_GLOBAL='/dev/null',
                        GIT_CONFIG_NOSYSTEM='1')
        # A PATH cargo wrapper isn't usable because sync deliberately prefers rustup.
        # Put wrappers in its temporary copy only; the real matrix text remains intact.
        script = SCRIPT.read_text().replace('export PATH="/opt/homebrew/opt/rustup/bin:$PATH"', ':')
        (self.repo / 'scripts/selfhost').mkdir(parents=True)
        target = self.repo / 'scripts/selfhost/sync-upstream.sh'
        target.write_text(script)
        target.chmod(0o755)
        self.target = target
        for name in ('pnpm', 'cargo'):
            path = self.bin / name
            path.write_text('#!/bin/bash\nset -eu\n'
                            f'echo "{name} $*" >> "$R2_TEST_CALLS"\n'
                            '[[ "${CODEG_DATA_DIR:-}" == /Applications* ]] && exit 90\n'
                            'exit "${R2_TEST_BUILD_EXIT:-0}"\n')
            path.chmod(0o755)
        (self.repo / 'src-tauri').mkdir()
        build = self.repo / 'scripts/build-fork.sh'
        build.write_text('#!/bin/bash\necho build-fork >> "$R2_TEST_CALLS"\n')
        build.chmod(0o755)
        self.git('init', '-b', 'fork/main')
        self.git('config', 'user.name', 'Selfhost Test')
        self.git('config', 'user.email', 'test@example.invalid')
        (self.repo / 'sample').write_text('base\n')
        self.git('add', '.')
        self.git('commit', '-m', 'base')
        self.git('switch', '-c', 'upstream-tip')
        (self.repo / 'sample').write_text('upstream\n')
        self.git('commit', '-am', 'upstream')
        self.git('tag', 'v-test')
        self.git('switch', 'fork/main')
        self.git('clone', '--bare', str(self.repo), str(self.root / 'remote.git'))
        self.git('remote', 'add', 'upstream', str(self.root / 'remote.git'))

    def git(self, *args):
        return subprocess.check_output(['git', *args], cwd=self.repo, env=self.env,
                                       stderr=subprocess.STDOUT, text=True).strip()

    def execute(self):
        return subprocess.run([str(self.target), 'v-test'], cwd=self.repo, env=self.env,
                              capture_output=True, text=True, timeout=30)

    def test_matrix_and_build_on_sync_branch_with_backup_override(self):
        result = self.execute()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.git('branch', '--show-current'), 'sync/v-test')
        calls = self.calls.read_text()
        for fragment in ('pnpm lint .', 'pnpm test', 'pnpm build',
                         'cargo test --features test-utils',
                         'cargo clippy --no-default-features --features server-bin',
                         'cargo test --no-default-features --features mcp-bin',
                         'crates/codeg-relay/Cargo.toml', 'build-fork'):
            self.assertIn(fragment, calls)
        self.assertTrue(list((self.root / 'backups/logs').glob('sync-*.log')))
        self.assertIn('SKIP E2E', result.stdout)

    def test_conflicts_stop_before_checks(self):
        (self.repo / 'sample').write_text('fork change\n')
        self.git('commit', '-am', 'fork divergence')
        result = self.execute()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.git('diff', '--name-only', '--diff-filter=U'), 'sample')
        self.assertFalse(self.calls.exists())

    def test_check_failure_stops_before_bundle(self):
        self.env['R2_TEST_BUILD_EXIT'] = '9'
        result = self.execute()
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('build-fork', self.calls.read_text())

    def test_available_e2e_runs_smoke_probe_and_cleanup(self):
        directory = self.repo / 'e2e'
        directory.mkdir()
        for name in ('env-up.sh', 'env-down.sh'):
            path = directory / name
            path.write_text(f'#!/bin/bash\necho "{name} $*" >> "$R2_TEST_CALLS"\n')
            path.chmod(0o755)
        for name in ('smoke.py', 'probe_completion_order.py'):
            (directory / name).write_text(
                'import os\n'
                "assert not any(k.startswith('CODEG') for k in os.environ)\n"
                f"with open(os.environ['R2_TEST_CALLS'], 'a') as f: f.write('{name}\\n')\n")
        self.git('add', 'e2e')
        self.git('commit', '-m', 'test harness')
        result = self.execute()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        calls = self.calls.read_text()
        self.assertLess(calls.index('env-up.sh'), calls.index('smoke.py'))
        self.assertLess(calls.index('smoke.py'), calls.index('probe_completion_order.py'))
        self.assertLess(calls.index('probe_completion_order.py'), calls.index('env-down.sh'))
        self.assertLess(calls.index('env-down.sh'), calls.index('build-fork'))
        self.assertIn('Co-Authored-By: Codex CLI <noreply@openai.com>', self.git('log', '-1', '--format=%B'))


if __name__ == '__main__':
    unittest.main(verbosity=2)
