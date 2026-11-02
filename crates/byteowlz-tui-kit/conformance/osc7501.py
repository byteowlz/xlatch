#!/usr/bin/env python3
"""Language-neutral OSC 7501 emitter validation, spec 0.3; Python stdlib only.

This is deliberately stricter than a terminal consumer: invalid emitted optional
fields/pairs fail instead of being ignored. It does not certify record lifetime,
application semantics, notification policy, or terminal/mux/SSH propagation.
"""
import argparse
import base64
import binascii
import json
import re
import sys
from pathlib import Path

PREFIX = b"\x1b]7501;"
STATES = {"idle", "working", "done", "blocked", "error", "clear"}
KINDS = {"permission", "question", "auth"}
SAFE = re.compile(r"[A-Za-z0-9_.,+/=\-]*\Z")
SEGMENT = re.compile(r"[A-Za-z0-9_.+\-]{1,32}\Z")

class InvalidEmission(ValueError):
    """Safe fixed-code diagnostic; never includes captured content."""


def parse_emission(sequence):
    """Validate one complete frame; return fields or None for a feature query."""
    if len(sequence) > 4096:
        raise InvalidEmission("sequence_limit")
    end = 2 if sequence.endswith(b"\x1b\\") else 1 if sequence.endswith(b"\x07") else 0
    if not sequence.startswith(PREFIX) or not end:
        raise InvalidEmission("framing")
    try:
        body = sequence[len(PREFIX):-end].decode("ascii")
    except UnicodeDecodeError:
        raise InvalidEmission("non_ascii_body") from None
    if body == "?":
        return None
    fields = {}
    for pair in body.split(":"):
        if "=" not in pair:
            raise InvalidEmission("malformed_pair")
        key, value = (part.strip() for part in pair.split("=", 1))
        if len(key) > 16:
            raise InvalidEmission("key_limit")
        if not re.fullmatch(r"[a-z]+", key) or not SAFE.fullmatch(value):
            raise InvalidEmission("pair_grammar")
        # Check every occurrence BEFORE last-value-wins replacement.
        if key in ("msg", "title"):
            encoded_cap, decoded_cap = (2732, 2048) if key == "msg" else (256, 192)
            if len(value) > encoded_cap:
                raise InvalidEmission("text_encoded_limit")
            try:
                decoded = base64.b64decode(value + "=" * (-len(value) % 4), validate=True)
                text = decoded.decode("utf-8")
            except (ValueError, binascii.Error, UnicodeDecodeError):
                raise InvalidEmission("text_encoding") from None
            if len(decoded) > decoded_cap:
                raise InvalidEmission("text_decoded_limit")
            if any(ord(char) < 32 or 127 <= ord(char) <= 159 for char in text):
                raise InvalidEmission("text_control")
        if key == "app" and not SEGMENT.fullmatch(value):
            raise InvalidEmission("app_grammar")
        if key == "id":
            parts = value.split("/")
            if len(value) > 128 or len(parts) > 8 or not all(SEGMENT.fullmatch(part) for part in parts):
                raise InvalidEmission("id_grammar")
        fields[key] = value
    if fields.get("state") not in STATES:
        raise InvalidEmission("state")
    if "kind" in fields and (fields["state"] != "blocked" or fields["kind"] not in KINDS):
        raise InvalidEmission("kind")
    if "progress" in fields:
        value = fields["progress"]
        if fields["state"] not in ("working", "blocked") or not re.fullmatch(r"[0-9]+", value) or not 0 <= int(value) <= 100:
            raise InvalidEmission("progress")
    return fields


def validate_capture(data, expect_none=False, required_states=()):
    """Scan a bounded capture containing ordinary terminal output and frames."""
    errors, states, queries = [], [], 0
    position, count = 0, 0
    while True:
        start = data.find(PREFIX, position)
        if start < 0:
            break
        count += 1
        if count > 65536:
            errors.append({"index": count, "reason": "capture_frame_limit"})
            break
        ends = [(index, width) for token, width in ((b"\x1b\\", 2), (b"\x07", 1))
                if (index := data.find(token, start + len(PREFIX))) >= 0]
        if not ends:
            errors.append({"index": count, "reason": "unterminated_frame"})
            break
        finish, width = min(ends)
        position = finish + width
        try:
            fields = parse_emission(data[start:position])
            if fields is None:
                queries += 1
            else:
                states.append(fields["state"])
        except InvalidEmission as error:
            errors.append({"index": count, "reason": str(error)})
    if expect_none and count:
        errors.append({"index": 0, "reason": "unexpected_osc"})
    if not expect_none and not states:
        errors.append({"index": 0, "reason": "no_status_reports"})
    for state in required_states:
        if state not in states:
            errors.append({"index": 0, "reason": "missing_required_state", "state": state})
    return {"schema_version": 1, "spec_revision": "0.3", "scope": "emitter",
            "frames": count, "queries": queries, "states": states,
            "valid": not errors, "errors": errors}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("capture", nargs="?", default="-", help="raw capture file, or stdin (-)")
    parser.add_argument("--json", action="store_true", help="stable summary; never includes captured messages")
    parser.add_argument("--expect-none", action="store_true", help="require no OSC 7501 frames (redirected/headless proof)")
    parser.add_argument("--require-state", action="append", choices=sorted(STATES), default=[])
    parser.add_argument("--max-bytes", type=int, default=4 * 1024 * 1024)
    args = parser.parse_args()
    if not 1 <= args.max_bytes <= 64 * 1024 * 1024:
        parser.error("--max-bytes must be between 1 and 67108864")
    try:
        if args.capture == "-":
            data = sys.stdin.buffer.read(args.max_bytes + 1)
        else:
            with Path(args.capture).open("rb") as source:
                data = source.read(args.max_bytes + 1)
    except OSError:
        print("Cannot read capture; check the path and permissions", file=sys.stderr)
        return 2
    if len(data) > args.max_bytes:
        print("Capture exceeds --max-bytes; increase the bounded limit explicitly", file=sys.stderr)
        return 2
    result = validate_capture(data, args.expect_none, args.require_state)
    if args.json:
        print(json.dumps(result, sort_keys=True))
    elif result["valid"]:
        print(f"PASS: {result['frames']} OSC 7501 frames (emitter validation only)")
    else:
        print("FAIL: " + ", ".join(error["reason"] for error in result["errors"]), file=sys.stderr)
    return 0 if result["valid"] else 1

if __name__ == "__main__":
    sys.exit(main())
