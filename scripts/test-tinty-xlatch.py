"""Run with python3 scripts/test-tinty-xlatch.py; never touches user config."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("tinty-xlatch.py")
spec = importlib.util.spec_from_file_location("tinty_xlatch", SCRIPT)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class TintyExportTests(unittest.TestCase):
    def environment(self, system):
        result = {"TINTY_SCHEME_SYSTEM": system, "TINTY_SCHEME_VARIANT": "dark", "TINTY_SCHEME_ID": "test"}
        for index in range(24 if system == "base24" else 16):
            for channel, value in zip("RGB", ("12", "34", "56")):
                result[f"TINTY_SCHEME_PALETTE_BASE{index:02X}_HEX_{channel}"] = value
        return result

    def test_both_systems_export_all_slots(self):
        for system, count in (("base16", 16), ("base24", 24)):
            result = module.palette(self.environment(system))
            self.assertEqual(result, {"name": "test", "system": system, "mode": "dark", "slots": {f"base{i:02X}": "#123456" for i in range(count)}})

    def test_incomplete_palette_is_rejected(self):
        environment = self.environment("base24")
        del environment["TINTY_SCHEME_PALETTE_BASE17_HEX_B"]
        with self.assertRaises(ValueError):
            module.palette(environment)

    def test_atomic_export_and_invalid_update_preserves_previous(self):
        with tempfile.TemporaryDirectory() as config:
            environment = dict(os.environ, **self.environment("base16"), XDG_CONFIG_HOME=config)
            subprocess.run(["python3", str(SCRIPT)], env=environment, check=True)
            path = Path(config) / "xlatch/theme.json"
            previous = path.read_text()
            self.assertEqual(json.loads(previous), module.palette(environment))
            environment["TINTY_SCHEME_PALETTE_BASE00_HEX_R"] = "bad"
            result = subprocess.run(["python3", str(SCRIPT)], env=environment, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(path.read_text(), previous)
            self.assertEqual([item.name for item in path.parent.iterdir()], ["theme.json"])


if __name__ == "__main__":
    unittest.main()
