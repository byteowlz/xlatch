#!/usr/bin/python3
"""Transcribe an xlatch file reference or legacy inline audio with trnscrbr on macOS."""
import argparse
import base64
import json
from pathlib import Path
import subprocess
import sys
import tempfile


def transcribe(request, executable, model):
    attachment = request["file"]
    with tempfile.TemporaryDirectory(prefix="xlatch-audio-") as directory:
        root = Path(directory)
        if "artifact_id" in attachment:
            source = Path(attachment["path"])
            if not source.is_absolute() or not source.is_file():
                raise ValueError("Missing executor file")
        else:
            source = root / "input"
            source.write_bytes(base64.b64decode(attachment["data_base64"], validate=True))
        wav = root / "audio.wav"
        subprocess.run(["/usr/bin/afconvert", "-f", "WAVE", "-d", "LEI16@16000", "-c", "1", str(source), str(wav)],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=120)
        transcript = root / "transcript.txt"
        with transcript.open("wb") as output:
            subprocess.run([executable, "-m", model, str(wav)], check=True,
                           stdout=output, stderr=subprocess.DEVNULL, timeout=450)
        with transcript.open("rb") as output:
            raw = output.read(100001)
        if not raw.strip() or len(raw) > 100000:
            raise ValueError("Empty or oversized transcript")
        return {"text": raw.decode("utf-8").strip(), "mime_type": "text/plain"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--trnscrbr", required=True)
    parser.add_argument("--model", required=True)
    args = parser.parse_args()
    if not all(Path(value).is_absolute() for value in [args.trnscrbr, args.model]):
        raise ValueError("Executable and model paths must be absolute")
    raw = sys.stdin.buffer.read(8 * 1024 * 1024 + 1)
    if len(raw) > 8 * 1024 * 1024:
        raise ValueError("Input envelope exceeds 8 MiB")
    json.dump(transcribe(json.loads(raw), args.trnscrbr, args.model), sys.stdout)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, OSError, subprocess.SubprocessError):
        print("Transcription failed; check the audio format, executable and model. Nothing was sent onward.", file=sys.stderr)
        sys.exit(1)
