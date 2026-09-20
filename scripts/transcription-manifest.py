#!/usr/bin/env python3
"""Generate a standalone, reviewable eaRS transcription capability. Does not register or approve."""
import argparse
import hashlib
import json
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--ears", type=Path, required=True)
parser.add_argument("--server", required=True)
parser.add_argument("--id", default="audio.transcribe")
args = parser.parse_args()
adapter = Path(__file__).resolve().parents[1] / "examples/adapters/transcribe_ears.py"
file_schema = {"type":"object", "required":["name","mime_type","data_base64"],
               "properties":{"name":{"type":"string"},"mime_type":{"type":"string","pattern":"^audio/"},"data_base64":{"type":"string","minLength":1}}, "additionalProperties":False}
manifest = {
 "id":args.id, "title":"Transcribe audio", "description":"Transcribe shared audio with the configured eaRS server and return plain text. Does not send to an agent session.",
 "accepts":["audio/*"],
 "input_schema":{"type":"object","required":["file","mime_type"],"properties":{"file":file_schema,"mime_type":{"type":"string","pattern":"^audio/"},"text":{"type":"string"}},"additionalProperties":False},
 "output_schema":{"type":"object","required":["text","mime_type"],"properties":{"text":{"type":"string","minLength":1,"maxLength":100000},"mime_type":{"const":"text/plain"}},"additionalProperties":False},
 "execution":{"kind":"command","program":str(adapter),"args":["--ears",str(args.ears.resolve(strict=True)),"--server",args.server],"sha256":hashlib.sha256(adapter.read_bytes()).hexdigest()},
 "timeout_seconds":300
}
print(json.dumps(manifest,indent=2))
