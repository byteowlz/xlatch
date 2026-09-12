# CrossLatch v0 protocol

The reusable core is a capability registry, authorization rules, and durable jobs. Rust CLI, HTTPS, iOS, and MCP adapt the same operations. Oqto is a future consumer; it is not a dependency.

## Trust and transport

The daemon creates an HTTPS identity in its private data directory. A locally generated QR contains `{version:1,url,pin,token,expires_at}`. `pin` is lowercase SHA-256 of the DER leaf certificate; `token` is a random, single-use enrollment secret valid for five minutes. Only grants selected by the local operator are attached to a ticket. The iOS client pins the certificate, verifies TLS trust against that anchor, and rejects redirects. Changing the server certificate requires re-pairing; the initial certificate contains the advertised hostname/IP, so changing it requires a new appropriate certificate too.

`POST /v1/pair` accepts `{token,name,public_key,signature}`. Keys are raw Ed25519 (32 bytes), signatures are 64 bytes, both standard Base64. Sign exact UTF-8 bytes:

```
xlatch.pair.v1\n{token}\n{public_key}\n{name}
```

Consumption, device creation, and revision-scoped grants commit together. Public keys are unique; clients keep private keys in a shared app/extension Keychain access group, marked ThisDeviceOnly. Pairing tickets are hashed in SQLite and never logged by the server.

`POST /v1/rpc` accepts `{device_id,timestamp,nonce,payload,signature}`. `payload` is an exact JSON string, not a reconstructed object. Sign:

```
xlatch.rpc.v1\n{device_id}\n{timestamp}\n{nonce}\n{payload}
```

Timestamps are Unix seconds with 60 seconds of clock skew. Nonces are 16–120 ASCII alphanumeric/hyphen/underscore characters and are consumed before dispatch, retained beyond the timestamp validity window. Retries use a new nonce and the same invocation idempotency key. Unknown/revoked keys, forged signatures, expiry, and replay fail closed. `/health` is the only public status endpoint. There is no configuration-disclosure endpoint or permissive CORS layer.

## Core requests

| `op` | Additional fields | Result |
|---|---|---|
| `discover` | none | Active capabilities visible to the device, including schemas and revision |
| `invoke` | `capability_id`, `revision`, `input`, `idempotency_key` | Persisted job |
| `jobs` | none | 100 most recent owned job summaries |
| `job` | `id` | Owned job and result |
| `cancel` | `id` | Updated job |
| `events` | `after` | Up to 100 owned events in sequence order |

HTTP failures use an appropriate non-2xx status and `{error:string}`. Invocations are durable asynchronous jobs; CLI `--wait` offers synchronous waiting over the same job. Result responses omit the original input to avoid duplicating uploaded files. Requests are bounded to 8 MiB; serialized results to 6 MiB. Native file input/output is capped at 4 MiB, represented by `{file:{name,mime_type,data_base64},mime_type}`. Text/links use `{text,mime_type}`. Larger streaming artifacts are future work.

## Local control and activation

`control.sock` lives in a mode-0700 data directory, is mode 0600, and checks the peer uid. Each connection sends one bounded JSON line and receives `{ok:true,value:...}` or `{ok:false,error:...}`. Control operations are `register`, `approve`, `pair`, `devices`, `revoke`, and `rpc`; the Rust `local::Control` type is the exact contract.

Manifests contain `id`, `title`, `description`, `accepts`, inline `input_schema`/`output_schema`, `execution`, and `timeout_seconds`. Unknown fields and schema references are rejected. Registering new content resets the capability to `pending`; re-registering identical content preserves status. Approval requires the exact revision. Device grants do not automatically follow revised capabilities. Re-pair to grant newly approved revisions in v0.

Execution is either `echo`, with no external effect, or a trusted host `command` with an absolute program, fixed argument vector, and SHA-256 of that executable. JSON input goes exclusively to stdin; stdout must be one JSON result. No user input is interpolated into arguments or a shell. Executable bytes are checked before running; changing an executable invalidates the binding. Commands require explicit `--allow-host-execution` approval.

**This is an operator-trusted command runner, not an OS sandbox.** Approved programs have the daemon user's privileges. Fixed arguments can reference mutable files/dependencies; hashing the entrypoint does not freeze the environment. Agents with the same uid already share the operator's trust boundary and can use the control socket; a uid is not a per-agent publisher identity. Do not mount this socket into untrusted Oqto workspaces. OS isolation and distinct publisher/service identities must precede that integration.

## Persistence and failure semantics

SQLite tables: capabilities (current manifest/revision/status), devices (key/revocation), grants (device × capability revision), tickets (hashed enrollment + grant snapshot), nonces, jobs (manifest snapshot/input/status/result/error/owner/deduplication key), and events (monotonic sequence + job/owner/status). Schema migration 001 is transactional; WAL is enabled. An exclusive daemon lock prevents two supervisors from running this queue concurrently.

Job states: `queued → running → succeeded|failed`; queued/running jobs can become `cancelled`. Workers recheck current approval and grants before execution. Revocation cancels the device's outstanding jobs. There are two workers by default, configurable up to 16, with a 1,000-job outstanding limit. Execution time and output size are bounded. Unix process groups are killed on timeout/cancellation; arbitrary programs capable of detaching remain part of the trusted-host boundary.

A restart marks interrupted running jobs failed and leaves queued jobs available. There is no automatic side-effect retry and no exactly-once claim. A duplicate `(owner,idempotency_key)` returns the same job only when capability, revision, and input match. Poll events after the last received sequence; fetch job state as authoritative after reconnect. Completion notifications are hints, not the result store.

The iOS app polls while active and on return, can request local notifications for observed completion, and retrieves/share-saves results. APNs background delivery is not implemented or advertised as working.

## Examples and extension points

- `examples/capabilities/echo.json`: runnable text/file round trip.
- `examples/adapters/ffmpeg_wav.py`: runnable trusted-host media conversion to bounded mono WAV, with a protocol/demuxer allowlist.
- Transcription: a JSON wrapper decodes the audio, calls the configured transcription service, and returns `{text:...}`.
- Prompt an agent session: a wrapper addresses a specific existing session and submits shared text. The session service must authorize that target.
- ComfyUI: a wrapper enqueues a configured workflow, polls its job, and returns an image artifact. The endpoint/workflow selection belongs to reviewed configuration.

The latter three are adapter contracts, not claims of installed or configured services. Agent review may be added as advisory analysis of a pending revision; it must never override deterministic approval/grant checks. Persistent registration is implemented; process-leased capabilities, push delivery, remote executors, large-artifact storage, and Android are tracked extensions.

## Network candidates

Enrollment tickets retain `url` as the primary HTTPS origin and may include `urls`, an array of alternate origins under the same DER certificate pin. The server advertises at most eight discovered addresses from active interfaces. Native clients validate every origin and probe pinned HTTPS health endpoints before choosing one; enrollment and signed RPC bodies are submitted once, after discovery. Mesh providers need no dedicated adapter because their interfaces expose normal routed IP addresses. Interface discovery does not override VPN access policies or firewall rules.
