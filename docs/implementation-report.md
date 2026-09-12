# CrossLatch build report

Implemented in `/Users/tommyfalkowski/byteowlz/xlatch`. Govnr was not dispatched or modified.

## Available now

- Rust CLI, HTTPS API daemon, and a real MCP adapter. Built executables are in `target/debug/`.
- Typed manifests, pending registration, exact-revision approval, private Unix control socket, and explicit device grants.
- QR enrollment with single-use expiry, pinned TLS, Ed25519 device signatures, persistent replay protection, Keychain storage, and revocation.
- SQLite jobs/events, idempotent submission, bounded workers/output, cancellation/timeouts, restart recovery, and owned result retrieval.
- Native SwiftUI iOS app and share extension: pairing, cached action discovery, text/file submission, Activity, results, file preview/save/share, and explicit connection/error states.
- Runnable echo and FFmpeg-to-WAV examples. Transcription, agent-session, and ComfyUI integrations have documented contracts and tracked implementation work.

Open `ios/XLatch.xcodeproj` in Xcode. `docs/testing.md` contains reproducible commands; `docs/protocol.md` defines the wire format and security boundary. The simulator app is installed and paired to the temporary test server on port 7443. That server uses `/private/tmp/xlatch-live`; permanent use should start a daemon with the documented normal data directory and pair again.

## Verification

- 14 Rust tests passed: existing path/schema checks plus approval changes, input validation, key proof, replay/clock skew, revocation, ownership, idempotency, persistence, recovery, remote-reference rejection, command consent, timeout, and cancellation.
- Five iOS tests passed. The live test used actual pinned HTTPS enrollment, signed invocation/result retrieval, and rejected an incorrect certificate pin.
- Rust formatting and strict Clippy passed for the four affected crates and their test targets.
- CLI echo round trip passed.
- FFmpeg executed through the queue and returned a validated mono 16 kHz WAV (32,078 bytes).
- MCP initialization and capability discovery passed against the running daemon.
- Simulator Actions screen visually inspected. Manual system share-sheet interaction could not be completed after the Mac locked.
- Comparative Ripwire quality delta was unavailable because this scaffold has no git HEAD or pre-change quality baseline. No comparative quality pass is claimed.

## iPhone installation verified

CrossLatch is installed on the physical iPhone: `com.byteowlz.xlatch`, version 1.0, build 1, verified with devicectl. The Xcode GUI account was signed in; the earlier command-line No Accounts error did not reflect that state. Physical camera QR enrollment and share-sheet/result acceptance remain to be verified.

## Oqto

`docs/oqto-integration.md` places integration behind Oqto’s runner-side Gate. Future grants must preserve Account/Service Identity, Workspace, and OS Principal distinctions. The current local socket/MCP operator authority must not be exposed to untrusted Apps. Typed revisions, owned jobs, and transport-neutral operations provide the common foundation.

## Tracker

Epic: `xltch-gkte`. Protocol/server/security/jobs tasks `.1`–`.4` are closed with evidence; iOS/device acceptance `.5`–`.6` remain in progress.

- `xltch-1m54`: physical iPhone provisioning and share-sheet acceptance.
- `xltch-xccz`: APNs completion delivery.
- `xltch-fv2w`: Oqto identity/Gate integration.
- `xltch-19dm`: publisher identity, leases, and advisory review.
- `xltch-sk7e`: transcription, agent-session, and ComfyUI adapters.
- `xltch-dphj`: streaming artifacts and retention.
- `xltch-x587`: Windows control transport and release-matrix validation.

Four issues filed in `/Users/tommyfalkowski/byteowlz/templates`:

- `tmpl-d9wd`: incomplete scaffold substitution (crate imports and identity).
- `tmpl-h7h7`: unsafe unauthenticated config endpoint/permissive CORS defaults.
- `tmpl-30fy`: rendered-template pull-request CI and release gates.
- `tmpl-8adx`: concise, accurate AGENTS.md and enforceable guardrails.

Existing unrelated scaffold files were preserved. No commits, pushes, or releases were made.
