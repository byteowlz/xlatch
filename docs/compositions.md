# Compose actions

Transcription and sending to Pi remain independent capabilities. Register the standalone eaRS adapter by generating a manifest with your absolute executable path and explicit server endpoint:

```sh
python3 scripts/transcription-manifest.py --ears /absolute/path/to/ears --server ws://127.0.0.1:8798/ > /tmp/transcribe.json
xlatch register /tmp/transcribe.json
```

Review and approve the transcription action normally. In protected mode, install its adapter and interpreter in administrator-owned locations first; a user-writable checkout is not a trusted executable installation.

Create a separate composed target using registered action IDs:

```sh
xlatch compose audio.to-pi --steps audio.transcribe,pi.send.link-analysis --title 'Transcribe → Pi' --dry-run
```

Omit `--dry-run` to submit the proposal. It is pending until approved and granted. Existing Pi actions keep accepting arbitrary files and copying them to their configured directory; composition does not modify them.

For the Pi adapter that reports delivery as `ok`, use a JSON plan to require a successful receipt:

```json
{
  "title": "Transcribe → Pi",
  "steps": [
    {"capability_id": "audio.transcribe"},
    {"capability_id": "pi.send.link-analysis"}
  ],
  "output_schema": {
    "type": "object",
    "required": ["ok"],
    "properties": {"ok": {"const": true}}
  }
}
```

Pass this file with `xlatch compose audio.to-pi --spec plan.json`. TOML plans work too. Each step optionally specifies an exact `revision`; otherwise the current registered revision is captured. Approval requires active dependencies.

Default input is the previous result (the original input for step one). An explicit mapping uses `input: {"kind":"fields","fields":{"text":{"kind":"previous","pointer":"/text"},"file":{"kind":"original","pointer":"/file"},"mime_type":{"kind":"literal","value":"text/plain"}}}`. JSON pointers select exact values without coercion. Missing fields or schema violations fail before the next step starts.

The supplied transcription adapter accepts up to 4 MiB of audio. A chain has 2–16 leaf steps and at most one hour of total declared timeout. Job details include each step and its receipt. Failed or interrupted steps are never automatically replayed; inspect their results before starting a new invocation.
