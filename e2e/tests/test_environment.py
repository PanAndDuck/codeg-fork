import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from environment import atomic_write, clean_env, free_ports, stop
from harness import local_url, state_path


class EnvironmentTests(unittest.TestCase):
    def test_inherited_codeg_and_tmp_are_removed(self):
        with patch.dict(os.environ, {"CODEG_DATA_DIR": "/protected", "CODEG_EXTRA": "secret", "TMPDIR": "/protected"}):
            env = clean_env()
        self.assertFalse(any(k.startswith("CODEG") for k in env))
        self.assertNotIn("TMPDIR", env)
        self.assertEqual(env["CARGO_BUILD_JOBS"], "4")

    def test_refuses_pid_reuse_before_signal(self):
        with patch("environment.identity", return_value="another command"), patch("environment.os.kill") as kill:
            with self.assertRaises(RuntimeError):
                stop({"pid": 42, "identity": "old command", "expected": "old"})
            kill.assert_not_called()

    def test_refuses_official_and_external_urls(self):
        for url in ("http://127.0.0.1:3080", "https://203.0.113.10:18443", "https://example.com:18443"):
            with self.assertRaises(ValueError):
                local_url(url)
        for name in ("../escape", "", "a/b"):
            with self.assertRaises(ValueError):
                state_path(name)
        with self.assertRaises(ValueError):
            free_ports({"server_port": 3080, "relay_port": 18443, "proxy_port": 18444, "control_port": 18445})

    def test_atomic_state_is_private_and_replaceable(self):
        with tempfile.TemporaryDirectory(prefix="codeg-e2e-test-", dir="/tmp") as tmp:
            path = Path(tmp) / "state"
            atomic_write(path, "first")
            atomic_write(path, "second")
            self.assertEqual(path.read_text(), "second")
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            self.assertEqual(list(Path(tmp).iterdir()), [path])
