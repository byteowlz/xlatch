---
name: xlatch
description: Build, register, grant, and invoke xlatch actions for phone share sheets, local agents, or server integrations. Use when integrating a tool with xlatch or configuring server-side save destinations.
---

# Build against xlatch

xlatch is a capability registry and durable job service. The iOS client discovers actions dynamically; a new action normally needs a server manifest and execution binding, not an app release. Read the installed command's `--help` before relying on newer features.

## User-mode operator workflow

Use `xlatch service run` for foreground execution and `xlatch service start|stop|restart|enable|disable|status` for an installed user service. `--port` changes the port. `--listen` selects the interface; the default accepts network clients and advertises discovered LAN/mesh addresses. `--public-url` supplies an explicit HTTPS origin. Use the same global `--data-dir` for server and operator commands.

- `xlatch list` returns registered manifests and revisions, including pending ones.
- `xlatch register action.json` submits a typed manifest in pending state.
- `xlatch approve ACTION --revision REVISION` activates exactly the reviewed digest. Command execution additionally requires `--allow-host-execution`.
- `xlatch devices` lists paired devices.
- `xlatch grant DEVICE ACTION --revision REVISION` grants an approved revision without re-pairing. Updating a manifest requires approval and a fresh grant for that revision.
- `xlatch pair` guides explicit grants and renders a terminal QR. Use `--json` with `--capability ID` in scripts. Tickets expire after five minutes and are single-use.
- `xlatch invoke ACTION --revision REVISION --input input.json --key STABLE_KEY --wait` submits a durable job. Use `jobs`, `job ID`, `events --after CURSOR`, and `cancel ID` for asynchronous work.

Use a stable idempotency key when retrying the same logical invocation. A timeout is not proof that the job failed. Retrieve its status before starting another operation with a new key.

## Protected mode

Check the service mode before choosing an integration workflow. User mode gives same-UID local agents operator authority. Protected mode runs the broker under a dedicated service account and executes host actions through a separate, unprivileged executor account. Broker state, trusted keys, binaries and service configuration must remain inaccessible for modification by agents; biometric approval alone does not protect a user-owned daemon.

Use the global `--control-dir PATH` to address an installed protected service. Its local transport accepts only the configured executor OS identity. It allows identity/discovery, pending registration, pairing requests, device listing and executor operations. Local approval, grant changes, revocation and invocation/job shortcuts are denied. Use the existing signed device API for authorized invocations and result retrieval; do not invent agent credential support.

When phone enrollment protection is enabled, QR pairing creates a pending device with no grants. An existing approver reviews and signs the exact enrollment with its biometric-protected key. Enrollment approval does not approve capability revisions or promote the new device to an approver. For a new or revised integration, register the manifest pending, then direct the approver to Server → Action approvals in the iOS app. They review the full manifest and exact revision, select devices to grant, and sign approval with Face ID or Touch ID. Grants are additive; selecting no devices activates without adding access. Existing active revisions can receive additional grants through the same flow. Rejection consumes the review without deleting the registration. Grant revocation and per-job phone approval are not implemented. Do not switch modes or edit broker state to bypass approval.

Protected installation is available for macOS/Linux through `xlatch service enable --protected --executor-user NAME --dry-run`. Actual migration requires administrator access and the installation fingerprint shown by the trusted phone. Treat installation and trust migration as a separate operator task, not a prerequisite an integration agent performs automatically. Check the installed command's help for its current requirements.

## Save destinations (user mode)

```sh
xlatch destination add incoming
xlatch destination add documents ~/Documents/Shared --device DEVICE_ID
```

The first command defaults to `~/xlatch/incoming`. Each destination becomes `save.NAME` and is explicitly registered and approved by this operator command. `--device` also grants it to an already paired phone. Without that option, grant it separately or select it during pairing.

Text/links become timestamped `.txt` files. Shared files retain a safe basename and extension. Name collisions get numeric suffixes; existing files are never overwritten. The destination is a canonical server-side path fixed in the manifest, never a path supplied by the phone.

## Custom manifests

Example for a trusted executable that reads one JSON document on stdin and emits one JSON result on stdout:

```json
{
  "id": "example.action",
  "title": "Example action",
  "description": "Describe the concrete result for the person sharing content.",
  "accepts": ["text/plain", "text/uri-list"],
  "input_schema": {
    "type": "object",
    "required": ["text", "mime_type"],
    "properties": {"text": {"type": "string"}, "mime_type": {"type": "string"}},
    "additionalProperties": false
  },
  "output_schema": {"type": "object"},
  "execution": {"kind": "command", "program": "/absolute/path/to/adapter", "args": [], "sha256": "REPLACE_WITH_EXECUTABLE_SHA256"},
  "timeout_seconds": 60
}
```

Inspect the tool before binding it. Use an absolute executable, fixed arguments and its real SHA-256; shared input belongs on stdin, never interpolated into a shell command. Commands run as the daemon's OS user in user mode and as the separate executor user in protected mode, with a cleared environment and fixed PATH. They are trusted host execution, not a sandbox. Resolve executable and destination access for that execution identity; registration validates the manifest declaratively and does not prove host access. Do not assume inherited API keys or a credential resolver.

Schemas must be inline/self-contained: remote references are rejected. Native share input is `{text,mime_type}` or `{file:{name,mime_type,data_base64},mime_type}`. Files up to 4 MiB remain inline. Larger files use resumable uploads and `{file:{artifact_id,name,mime_type,size},mime_type}`. Native save destinations accept these automatically. Command manifests opt in with `"file_input":"path"` and a schema accepting the reference (and optionally legacy inline data). The executor injects a private `file.path` after validation; consume or copy it before returning, and retain the inline fallback for older clients. Never accept a caller-supplied host path. Requests stay limited to 8 MiB and results to 6 MiB. `xlatch uploads` shows storage policy; `xlatch uploads --policy policy.json` (or TOML) configures `max_file_bytes` (null/unset means no per-file cap), `quota_bytes` (default 64 GiB reserved across uploads), and `retention_hours` (default 168). Protected installations require the database-owning service identity for policy changes; the executor socket cannot change policy. See `docs/adr/0007-owned-chunked-file-inputs.md`. A result containing `text` renders conveniently in the app; a `file` object can be retrieved/shared. Advertise only MIME types the adapter really handles.

The phone shows only active, granted revisions compatible with shared content. App toggles hide individual actions on that device; they do not revoke permissions or affect other devices. Registration and approval are distinct; activate or grant only within the user's authorized scope.

## Integration boundaries

JSON is the protocol and CLI interchange format; application configuration uses JSON or TOML. MCP is an adapter (`xlatch-mcp`) over the same local service, not the core protocol. In user mode, the private Unix socket gives same-UID agents local-operator authority; do not expose it to an untrusted app or tenant. Protected mode applies the narrower transport permissions described above.

Keep mobile credentials and request signatures in the existing native client implementation. Address fallback must retain the QR certificate pin and hostname validation; do not bypass TLS or replay an uncertain side-effecting POST across candidates.

External HTTP actions and a central server-side credential store are agreed design directions, not implemented interfaces yet. Do not invent credential commands, HTTP executor variants, or client-side secret distribution. Future Oqto integration must map its account/workspace permissions explicitly; a paired device is not an Oqto account.

## Compositions

Use `xlatch compose NEW_ID --steps FIRST,SECOND --dry-run` to inspect a proposal joining registered capabilities. Omit `--dry-run` to register it pending. Each composition has its own exact revision, approval and grants; a parent grant delegates only the pinned steps, not direct child access. Protected mode uses the normal phone approval flow.

Transcription should remain an independent action, then compose it with the existing Pi action. Do not rewrite Pi's arbitrary-file input or copied-file-path behavior. Direct joins require compatible output/input schemas; use `--spec plan.json` or TOML for explicit mappings and a stricter final receipt schema. See `docs/compositions.md` in the xlatch repository. A leaf reporting business failure in an otherwise successful JSON response must expose that in its schema or the composition's final receipt constraint.

Inspect step receipts after failure; never automatically replay an interrupted side effect. Changing a dependency requires reviewing a new composition revision. Phone one-off chains use signed `chain_candidates` and `invoke_chain` RPCs with exact ID/revision references. They require direct grants for every leaf and never activate registry entries. `save_chain` submits a fresh pending target for the normal approval flow. Nested compositions and fan-out remain unsupported.

## Action icons

Use `xlatch register action.json --icon icon.svg` (or PNG) to embed an offline-capable target icon. The CLI rasterizes supported SVG shapes to bounded PNG bytes; native clients never load remote image URLs. Raw manifest form is `"icon": {"png_base64": "..."}` (PNG ≤128 KiB, ≤256 pixels per axis). Omit the field for the default glyph. Existing iconless revisions remain unchanged; adding or replacing an icon requires normal revision approval/grants. Keep SVG source with the adapter. See `docs/action-icons.md` for import limits and supported SVG features.
