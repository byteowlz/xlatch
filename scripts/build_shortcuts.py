#!/usr/bin/env python3
"""Package reviewed JSON workflows using Apple's native shortcut signer (macOS)."""
import json
from pathlib import Path
import plistlib
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    destination = ROOT / "ios/App/Shortcuts"
    destination.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory() as temporary:
        for source in sorted((ROOT / "ios/Shortcuts").glob("*.json")):
            workflow = json.loads(source.read_text())
            unsigned = Path(temporary) / (source.stem + ".shortcut")
            unsigned.write_bytes(plistlib.dumps(workflow, fmt=plistlib.FMT_BINARY))
            subprocess.run(["shortcuts", "sign", "--mode", "anyone", "--input", str(unsigned),
                            "--output", str(destination / unsigned.name)], check=True)


if __name__ == "__main__":
    main()
