#!/usr/bin/python3
"""Standalone xlatch audio -> text action using a fixed eaRS executable/server."""
import argparse
import base64
import json
from pathlib import Path
import subprocess
import sys
import tempfile


def transcribe(request, ears, server):
    attachment = request["file"]
    audio = base64.b64decode(attachment["data_base64"], validate=True)
    if not audio or len(audio) > 4 * 1024 * 1024:
        raise ValueError("Audio must contain 1 byte to 4 MiB")
    suffix = Path(attachment["name"]).suffix
    if not suffix or len(suffix) > 12 or not suffix[1:].isalnum():
        suffix = ".audio"
    with tempfile.TemporaryDirectory(prefix="xlatch-transcribe-") as directory:
        source = Path(directory) / ("input" + suffix)
        source.write_bytes(audio)
        # Verbose suppresses partial-word stdout in eaRS. Discard diagnostic output:
        # it may contain transcript fragments. Only the final text goes into the job.
        result = subprocess.run([ears, "--server", server, "--verbose", "--file", str(source)],
                                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                check=True, timeout=290)
    text = result.stdout.decode("utf-8").strip()
    if not text or len(text) > 100000:
        raise ValueError("eaRS returned no final transcript or exceeded the text limit")
    return {"text": text, "mime_type": "text/plain"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ears", required=True)
    parser.add_argument("--server", required=True)
    args = parser.parse_args()
    if not Path(args.ears).is_absolute():
        raise ValueError("eaRS executable must be absolute")
    raw = sys.stdin.buffer.read(8 * 1024 * 1024 + 1)
    if len(raw) > 8 * 1024 * 1024:
        raise ValueError("Request exceeds 8 MiB")
    json.dump(transcribe(json.loads(raw), args.ears, args.server), sys.stdout)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, OSError, subprocess.SubprocessError):
        print("Transcription failed: check audio format and the configured eaRS server; nothing was sent onward.", file=sys.stderr)
        sys.exit(1)
