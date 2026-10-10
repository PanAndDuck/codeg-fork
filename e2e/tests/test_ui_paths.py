"""UI evidence must use caller-owned paths without a machine-specific import."""
import contextlib
import importlib
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))


class UiPathTests(unittest.TestCase):
    def test_scenario_evidence_uses_configured_directory(self):
        scenarios = importlib.import_module('ios_scenarios')
        with tempfile.TemporaryDirectory() as tmp:
            destination = Path(tmp) / 'evidence'
            with patch.dict(os.environ, {'CODEG_E2E_UI_EVIDENCE': str(destination)}):
                with contextlib.redirect_stdout(io.StringIO()):
                    scenarios.record('sample', {'ok': True})
            self.assertEqual(json.loads((destination / 'sample-result.json').read_text()), {'ok': True})

    def test_ui_requires_explicit_paths_before_network_access(self):
        from ui_command import ui
        with patch.dict(os.environ, {}, clear=True):
            with self.assertRaisesRegex(ValueError, 'config/evidence'):
                ui({'action': 'state'})
