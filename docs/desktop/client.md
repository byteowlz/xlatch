# Optional paired desktop client

`xlatch-client` is a standalone CLI and Rust library for macOS, Linux and Windows.
It uses the signed, pinned HTTPS device protocol, not local-operator authority.
The optional native app uses the same library with `--client-config FILE`.
Neither client starts, installs or reconfigures services. The server runs elsewhere
or is explicitly started by its operator.

## Install and pair

```sh
just install-crate xlatch-client
xlatch-client pair ticket.json --name Desktop
xlatch-client status --json
xlatch-client list --json
```

Obtain the ticket through the operator's existing pairing flow. `pair -` reads
ticket JSON from stdin, never an argument containing the secret. A protected
server can return pending enrollment: use `status`, compare its exact
`pending_payload` with the trusted approver, and wait for approval. Pairing does
not approve capabilities or promote this device to an approver.

Config defaults to `$XDG_CONFIG_HOME/xlatch-client/config.toml`, falling back to
`~/.config/xlatch-client/config.toml`. It is created on first use and currently
contains `timeout_seconds = 25`. Select another server with
`--config FILE` (highest priority) or `XLATCH_CLIENT_CONFIG`. Each config has
its own adjacent `<config-stem>.device.json` credential and pairing lock file.
These are shared by CLI and GUI; integrations must not read/copy private keys.
There is no separate client daemon, key service, auto-pairing or local discovery
dependency. `config-schema --json` returns the config JSON Schema.

Credentials are private files, not hardware-backed OS keychain entries. Unix
credential reads reject symlinks and group/world permissions; generated files
are mode 0600. On Windows, use a user-private config directory with restricted
ACLs; runtime ACL and native GUI verification remain outstanding. Backups of
the device file carry invocation authority. `forget --yes` removes only that
local file, **not** the server's device record or grants. A failed/uncertain
pairing preserves its candidate key and never automatically resubmits; inspect
the server device records before forgetting it and using a fresh ticket.

## Discovery, sharing and receipts

```sh
xlatch-client --json list
xlatch-client --json send TARGET --revision SHA256 --key UUID --text 'Hello'
printf 'Hello' | xlatch-client --json send TARGET --revision SHA256 --key UUID --stdin
xlatch-client --json send TARGET --revision SHA256 --key UUID --file exported.bin --mime-type application/octet-stream
xlatch-client --json send TARGET --revision SHA256 --key UUID --input payload.json
xlatch-client --json job JOB_ID
xlatch-client --json jobs
xlatch-client --json cancel JOB_ID
```

`list` is an authenticated target list: an array of existing capability records
`{manifest,revision,status}`, where `manifest` includes `id`, `title`,
`description`, `accepts`, `input_schema`, `output_schema`, `execution`
and optional `file_input`. Only active granted revisions are visible.
Preserve the selected ID and full revision; refresh rather than substituting
a new revision. `send` repeats granted-target/MIME/large-file preflight, and
the server authorizes again at invocation and execution.

Destinations and sessions are **capabilities**, not arbitrary caller-supplied
paths or a universal session ID. A save destination's reviewed binding fixes
its directory. A session adapter must expose a granted capability bound to the
intended session (or a reviewed schema defining an authorized session selector).
This client does not discover application sessions or create destination grants.

Files up to 4 MiB are inline. Larger files use the existing 1 MiB owner-scoped
chunk protocol, within server quota and retention policy. Large inputs require
a save-file target or an action declaring `file_input: "path"` and an
artifact-aware schema. Custom schemas can impose additional restrictions.
A private temporary snapshot uses local disk approximately equal to the file
size, keeping memory bounded and network transfer independent of source edits.
A content-bound deterministic upload UUID lets the same key and bytes resume
after an uncertain chunk acknowledgment. No invocation is sent before upload
completion. Unused uploads expire under server retention; there is no client
background queue. Large result downloads are not implemented.

### Context bundle plus prompt

A finalized portable ctx export (including fdbk's finalized ctx archive) is an
ordinary file source, with the producer's actual MIME `application/zip`.
Choose/export it explicitly with ctx/fdbk first; xlatch does not capture,
refresh, unpack, inspect, hand off or infer the completeness of a Bundle.

```sh
xlatch-client --json send GRANTED_SESSION_ACTION --revision SHA256 --key UUID \
  --file finished.ctx --mime-type application/zip --prompt-file instruction.txt
```

Alternatively `--prompt 'Review this context'` supplies the instruction inline;
`--prompt-file -` reads UTF-8 stdin, without exposing instructions as process
arguments. These options require `--file`, are mutually exclusive and bound the
nonblank instruction to 64 KiB. Existing file-only arguments/envelopes are unchanged.

The native input is `{file,mime_type,text}`: `text` is the exact instruction.
The selected reviewed capability must accept the **whole** input schema and,
for large archives, artifact references. Unsupported combined schemas fail
before upload; save-only actions reject prompts rather than silently discard
them. This adds no built-in session adapter, grants or universal session selector.
A schema-compatible echo fixture proves transport, not Session delivery.

Rust callers can use `Client::prepare_file(path,mime,key)`, inspect its frozen
`input()`, call `file_with_prompt(&target,input.clone(),prompt)`, then
`prepared.upload(&client).await` and invoke that exact combined input.
Preparation makes no network writes. Keep the prepared file/payload across
uncertainty; upload resumes that snapshot even if the original path changes.
CLI callers must retain immutable exported bytes **and the exact prompt** across
process invocations. A changed prompt is a new explicit intent, never an edited
retry; an accepted key rejects a changed instruction. There is no durable client
outbox, automatic re-export, upload retry or POST replay.

`send` returns the broker Job as its receipt: `id`, `capability_id`,
`revision`, `owner`, `status`, timestamps and optional `result`/`error`.
Without `--wait`, this proves acceptance, not completed delivery.
Poll `job ID` for `succeeded|failed|cancelled`; `jobs` is a summary list.
`--wait --timeout 60` polls the same job. Timeout does not cancel it and reports
its ID in the error message. A successful CLI query can return a failed Job:
check `status`, not just exit code or envelope `ok`.

Caller-generated `--key` is required. Retain one key per logical share,
together with the selected revision, MIME, basename, immutable exported file and optional prompt.
A retry uses exactly those inputs and the same key, obtaining the same Job.
Changed target, revision, bytes or prompt with an already accepted key are rejected.
Do not generate a fresh key after a timeout. HTTPS health fallback is read-only;
each signed POST is sent once, without redirects or automatic retries.
TLS keeps certificate/key pins, validity, hostname and handshake checks.

## Machine contract and optional app integrations

`--json` emits exactly one v1 envelope on stdout, also for invalid arguments:

```json
{"schema_version":1,"ok":true,"value":{"id":"job-id","status":"queued"}}
{"schema_version":1,"ok":false,"error":{"code":"client_error","message":"..."}}
```

Failures exit nonzero. Codes are `invalid_arguments` and `client_error`;
the latter deliberately does not promise detailed transport/auth classification.
`output-schema --json` returns the envelope JSON Schema without contacting a
server. Domain values follow `docs/protocol.md` and the Rust capability types.
Help, version and shell-completion output are ordinary text, even with `--json`.

For an optional toolbar integration (for example, sharing finished feedback
bundles): detect `xlatch-client` on PATH; keep recording/export independent of
its presence. Load targets asynchronously through `--json list`. On explicit
click, send a **regular exported file**, one command/key per bundle, using an
argument vector without a shell. A bundle directory must first be explicitly
exported/archived by its owning application; xlatch never recursively captures
it. Use the exporter's actual MIME, not a guessed format. Leave pairing and
credentials in xlatch-client. An absent, unpaired, offline, pending or denied
client disables sharing with an explanation; it never blocks capture. Do not
stop/reconfigure a server or silently select a destination.

Rust consumers use `xlatch_client::Client::load(config)`, `target(...)`,
`file_input(path,mime,key).await`, and `call(&request).await`; the native
app uses exactly this transport. Caller authorization remains unchanged.

## Proof and scope

Run `just client-check` (or `just agent-check`). Synthetic localhost HTTPS tests
exercise real Store/service authorization, CLI/library parity, text/file jobs,
large-file lost-ack resumption, frozen local preparation, prompted file/schema preflight,
lost invocation acknowledgements, changed-prompt rejection, deduplication, stale revisions, ungranted actions,
job ownership, pending enrollment, revocation, TLS pin/hostname rejection,
redirect refusal and same-key certificate renewal. No real devices, credentials,
service installation or deployments are involved.

Native paired mode is compile-checked on macOS. GUI render/accessibility and
Linux/Windows runtime parity are not claimed by those fixtures. A Windows
cross-check on the macOS development host is blocked by the absent Windows SDK
(`windows.h` in the TLS C dependency), not recorded as a passing Windows build. The existing
desktop layout is preserved; a minimal redesigned client surface awaits a
separately recorded visual target.
