"""Pure configuration contract tests; they do not import LinuxCNC bindings."""

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from config import BridgeConfig


class BridgeConfigTests(unittest.TestCase):
    """Verify path identity, listener safety, and shared-token gate behavior."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.ini = Path(self.temp.name) / "mill.ini"
        self.ini.write_text("[TRAJ]\nCOORDINATES=XYZ\n", encoding="utf-8")

    def tearDown(self):
        self.temp.cleanup()

    def test_loopback_defaults_and_stable_machine_id(self):
        config = BridgeConfig.from_ini(self.ini, "127.0.0.1", 8766, None)
        self.assertTrue(config.token_valid(None))
        self.assertTrue(config.machine_id.startswith("linuxcnc:mill:"))

    def test_remote_bind_requires_token(self):
        with self.assertRaises(ValueError):
            BridgeConfig.from_ini(self.ini, "0.0.0.0", 8766, None)
        protected = BridgeConfig.from_ini(self.ini, "0.0.0.0", 8766, "secret")
        self.assertTrue(protected.token_valid("secret"))
        self.assertFalse(protected.token_valid("wrong"))


if __name__ == "__main__":
    unittest.main()
