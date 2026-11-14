#!/usr/bin/env python3
"""Exercise real share-row gestures against synthetic targets in an isolated iOS app."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import tempfile


def run(*args):
    subprocess.run(args, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--device", help="Booted simulator UUID; defaults to the first booted iPhone")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    device = args.device
    if not device:
        devices = json.loads(subprocess.check_output(["xcrun", "simctl", "list", "devices", "booted", "--json"]))
        device = next((d["udid"] for group in devices["devices"].values() for d in group if d["name"].startswith("iPhone")), None)
    if not device:
        parser.error("Boot an iPhone simulator first, or supply --device UUID.")
    artifacts = root / "target/ios-share-gestures"
    artifacts.mkdir(parents=True, exist_ok=True)
    directory = Path(tempfile.mkdtemp(prefix="run-", dir=artifacts))
    fixtures = root / "scripts/ios-share-tests"
    shutil.copyfile(fixtures / "Preview.swift", directory / "Preview.swift")
    shutil.copytree(fixtures / "Tests", directory / "Tests")
    project = {
        "name": "SelectionPreview", "options": {"deploymentTarget": {"iOS": "17.0"}},
        "targets": {
            "SelectionPreview": {
                "type": "application", "platform": "iOS",
                "sources": ["Preview.swift", str(root / "ios/Shared"), str(root / "ios/Share/ShareViewController.swift")],
                "settings": {"base": {"PRODUCT_BUNDLE_IDENTIFIER": "com.byteowlz.xlatch.selection-preview", "CODE_SIGNING_ALLOWED": "NO", "SWIFT_VERSION": "5.0", "TARGETED_DEVICE_FAMILY": "1"}},
                "info": {"path": "Info.plist", "properties": {"UILaunchScreen": {}, "CFBundleDisplayName": "Share Gesture Tests"}},
            },
            "GestureTests": {
                "type": "bundle.ui-testing", "platform": "iOS", "sources": ["Tests"],
                "dependencies": [{"target": "SelectionPreview"}],
                "settings": {"base": {"PRODUCT_BUNDLE_IDENTIFIER": "com.byteowlz.xlatch.selection-preview.tests", "CODE_SIGNING_ALLOWED": "NO", "GENERATE_INFOPLIST_FILE": "YES", "SWIFT_VERSION": "5.0"}},
            },
        },
        "schemes": {"SelectionPreview": {"build": {"targets": {"SelectionPreview": "all"}}, "test": {"targets": ["GestureTests"]}}},
    }
    spec = directory / "project.json"
    spec.write_text(json.dumps(project))
    run("xcodegen", "generate", "--spec", str(spec), "--project", str(directory))
    runner = "com.byteowlz.xlatch.selection-preview.tests.xctrunner"
    # XCTest can reuse a live runner with the previous Swift test classes loaded.
    subprocess.run(["xcrun", "simctl", "uninstall", device, runner], check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        run("xcodebuild", "-project", str(directory / "SelectionPreview.xcodeproj"), "-scheme", "SelectionPreview",
            "-destination", f"platform=iOS Simulator,id={device}", "-derivedDataPath", str(artifacts / "build"),
            "-resultBundlePath", str(directory / "Results.xcresult"), "-parallel-testing-enabled", "NO", "test")
    finally:
        print(f"Gesture test artifacts: {directory}")
        for bundle in ["com.byteowlz.xlatch.selection-preview", runner]:
            subprocess.run(["xcrun", "simctl", "uninstall", device, bundle], check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


if __name__ == "__main__":
    main()
