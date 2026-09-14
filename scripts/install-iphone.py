#!/usr/bin/env python3
"""Build and install xlatch on an available iPhone using Xcode signing."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile


def run(*args):
    subprocess.run(args, check=True)


def select_device():
    with tempfile.TemporaryDirectory(prefix="xlatch-devices-") as directory:
        report = Path(directory) / "devices.json"
        run("xcrun", "devicectl", "list", "devices", "--json-output", str(report))
        devices = json.loads(report.read_text())["result"]["devices"]
    available = [d for d in devices
                 if d.get("hardwareProperties", {}).get("deviceType") == "iPhone"
                 and d.get("connectionProperties", {}).get("tunnelState") == "connected"]
    if len(available) != 1:
        raise RuntimeError("Connect and unlock your iPhone, trust this Mac, and enable Developer Mode. "
                           "If automatic discovery is unavailable or multiple phones are connected, "
                           "run just install-iphone 'DEVICE NAME OR ID' using the list above.")
    return available[0]["identifier"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("device", nargs="?", default="", help="Device name, identifier or UDID; otherwise discover one connected iPhone")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    device = args.device or select_device()
    derived = root / "target" / "ios-device"
    run("xcodebuild", "-project", str(root / "ios/XLatch.xcodeproj"),
        "-scheme", "XLatch", "-configuration", "Debug", "-destination", "generic/platform=iOS",
        "-derivedDataPath", str(derived), "-allowProvisioningUpdates", "build")
    app = derived / "Build/Products/Debug-iphoneos/XLatch.app"
    run("xcrun", "devicectl", "device", "install", "app", "--device", device, str(app))
    print("xlatch installed. Open it on your iPhone.")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, subprocess.CalledProcessError) as error:
        print(f"Install failed: {error}", file=sys.stderr)
        sys.exit(1)
