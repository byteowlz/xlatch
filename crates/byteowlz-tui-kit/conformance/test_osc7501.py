"""Specification-derived vectors and independent CLI/capture checks."""
import base64
import json
from pathlib import Path
import subprocess
import sys
import unittest
import osc7501

ROOT = Path(__file__).resolve().parent

class ConformanceTests(unittest.TestCase):
    def test_vectors(self):
        fixture = json.loads((ROOT / "fixtures.json").read_text())
        self.assertEqual(fixture["spec_revision"], "0.3")
        for case in fixture["cases"]:
            with self.subTest(case=case["name"]):
                body = case["body"]
                if "text" in case:
                    text = case["text"]
                    body += ":" + text["field"] + "=" + base64.b64encode((text["value"] * text["repeat"]).encode()).decode()
                end = b"\x07" if case.get("terminator") == "bel" else b"\x1b\\"
                sequence = osc7501.PREFIX + body.encode() + end
                if case["valid"]:
                    result = osc7501.parse_emission(sequence)
                    if "state" in case:
                        self.assertEqual(result["state"], case["state"])
                else:
                    with self.assertRaises(osc7501.InvalidEmission):
                        osc7501.parse_emission(sequence)

    def test_sequence_limit_and_framing(self):
        with self.assertRaises(osc7501.InvalidEmission):
            osc7501.parse_emission(osc7501.PREFIX + b"state=idle:extra=" + b"x" * 4096 + b"\x07")
        result = osc7501.validate_capture(b"normal text" + osc7501.PREFIX + b"state=idle")
        self.assertFalse(result["valid"])
        self.assertEqual(result["errors"][0]["reason"], "unterminated_frame")

    def test_capture_and_json_do_not_echo_messages(self):
        capture = b"noise" + osc7501.PREFIX + b"state=working:msg=c2VjcmV0" + b"\x1b\\" + osc7501.PREFIX + b"state=done\x07"
        run = subprocess.run([sys.executable, str(ROOT / "osc7501.py"), "--json", "--require-state", "done"], input=capture, capture_output=True, check=False)
        self.assertEqual(run.returncode, 0)
        result = json.loads(run.stdout)
        self.assertEqual(result["states"], ["working", "done"])
        self.assertNotIn(b"secret", run.stdout + run.stderr)
        self.assertNotIn(b"c2VjcmV0", run.stdout + run.stderr)

    def test_no_output_and_negative_capture(self):
        self.assertTrue(osc7501.validate_capture(b'{"valid":true}', expect_none=True)["valid"])
        self.assertFalse(osc7501.validate_capture(b"no reports")["valid"])
        self.assertFalse(osc7501.validate_capture(osc7501.PREFIX + b"state=idle\x07", expect_none=True)["valid"])
        run = subprocess.run([sys.executable, str(ROOT / "osc7501.py"), "--max-bytes", "8"], input=b"x" * 9, capture_output=True, check=False)
        self.assertEqual(run.returncode, 2)

    def test_required_states_and_duplicate_reports(self):
        sequence = osc7501.PREFIX + b"state=idle\x07"
        result = osc7501.validate_capture(sequence * 2, required_states=("done",))
        self.assertEqual(result["frames"], 2)
        self.assertFalse(result["valid"])
        self.assertEqual(result["errors"][0]["reason"], "missing_required_state")

if __name__ == "__main__":
    unittest.main()
