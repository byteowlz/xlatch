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

## Build a chain while sharing on iPhone

Tap a target to send normally. Swipe left fully on a leaf action, or choose **Add step** from its menu, to make it the first step. Only server-verified compatible, granted next actions remain. Tap one to send through the chain, or swipe it to keep extending. **Undo** and **Clear** change the selection without sending anything. **Save as target** submits a reusable chain for approval after at least two steps are selected.

The phone stores the exact sequence and revisions in its outbox before upload. Retries cannot replace recipients or bypass changed permissions. Compatibility discovery requires a reachable server; a fully selected chain can still be retained by the outbox if delivery then loses connectivity. Activity shows the chain name and the server job exposes individual step receipts. Nested compositions are not offered as steps. A chain can contain at most 16 leaf actions.

## Send the same input to several targets

A fan-out group is different from a chain. Each selected target receives the original share independently, so “analyze this link” and “make slides from this link” can run together without either target consuming the other's output. A one-off multi-send requires a current direct grant for every selected leaf and creates one parent job with per-target receipts. Saving the selection creates a pending group capability; after approval and a grant, it appears as one reusable share target.

The phone's normal tap remains an immediate single-target send. Multi-select mode adds targets explicitly, then offers **Send to N** and **Save as target**. Fan-out groups and chains cannot contain other compositions. Both are limited to 16 leaves.
