#!/usr/bin/env python3
"""Generate a reviewable manifest for a fixed trusted-host JSON executable."""
import argparse
import hashlib
import json
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('program', type=Path)
parser.add_argument('--id', required=True)
parser.add_argument('--title', required=True)
parser.add_argument('--description', required=True)
parser.add_argument('--accept', action='append', required=True)
parser.add_argument('--arg', action='append', default=[])
args = parser.parse_args()
program = args.program.resolve(strict=True)
manifest = {
    'id': args.id, 'title': args.title, 'description': args.description,
    'accepts': args.accept,
    'input_schema': {'type': 'object'}, 'output_schema': {'type': 'object'},
    'execution': {'kind': 'command', 'program': str(program), 'args': args.arg,
                  'sha256': hashlib.sha256(program.read_bytes()).hexdigest()},
    'timeout_seconds': 60,
}
print(json.dumps(manifest, indent=2))
