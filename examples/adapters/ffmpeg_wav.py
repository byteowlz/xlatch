#!/usr/bin/env python3
"""A trusted-host capability: shared media to mono 16 kHz WAV, as JSON in/out."""
import base64
import json
import pathlib
import subprocess
import sys
import tempfile

request = json.load(sys.stdin)
source = base64.b64decode(request['file']['data_base64'], validate=True)
if len(source) > 4 * 1024 * 1024:
    raise ValueError('Input exceeds 4 MB')
with tempfile.TemporaryDirectory(prefix='xlatch-ffmpeg-') as directory:
    input_path = pathlib.Path(directory) / 'input'
    output_path = pathlib.Path(directory) / 'output.wav'
    input_path.write_bytes(source)
    subprocess.run(['ffmpeg', '-nostdin', '-v', 'error', '-y', '-protocol_whitelist', 'file,pipe', '-format_whitelist', 'wav,mp3,mov,ogg,flac,matroska,aac', '-i', str(input_path), '-t', '120', '-vn', '-ar', '16000', '-ac', '1', str(output_path)], check=True, timeout=55)
    result = output_path.read_bytes()
    if len(result) > 4 * 1024 * 1024:
        raise ValueError('Output exceeds 4 MB')
    json.dump({'file': {'name': 'audio.wav', 'mime_type': 'audio/wav', 'data_base64': base64.b64encode(result).decode()}}, sys.stdout)
