# xlatch for Android

Native Kotlin/Compose client for Android 11+ (API 30). The app uses the existing
Rust server protocol; there is no separate Android server or central account.

## Build and install

Install Java 17 and Android SDK platform 35/build-tools 35.0.0. Accept Google's SDK
license using the SDK manager. Set `ANDROID_HOME` to that SDK, or set `sdk.dir`
in the ignored `local.properties` file. Then:

```sh
./gradlew :protocol:test :app:assembleDebug :app:lintDebug
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

The Gradle wrapper pins 8.11.1 and its distribution SHA-256. AGP, Kotlin and all
other direct dependencies have fixed versions. Debug signing is for testing;
release signing and Play distribution are not configured or claimed complete.

`just android-test`, `just android-build` and `just install-android` run the common
commands from the repository root. Installation requires one authorized device;
use `ANDROID_SERIAL` to select a specific device.

## Implemented flows

- Scan or paste an expiring pairing QR; keep independent device keys and server
  connections. Refresh discovers the exact revisions granted to this phone.
- Receive Android text/URL/file shares, including up to eight files per share,
  and choose a MIME-compatible action. Files are read while the content URI grant
  is valid and copied into encrypted storage on submission. Files are capped at
  4 MiB each, consistent with the current server protocol.
- Queue content locally, inspect delivery status/progress, retry paused shares
  with the same identity, and enable/disable individual actions on this phone.
- List jobs, cancel queued/running jobs, view/copy results. Background result
  polling can post generic completion notifications after permission is granted;
  Android controls timing. This is not instant push and does not require Google
  Play Services or an xlatch relay.
- Bootstrap an approver with the server-issued code; review pending enrollment
  payloads, exact action revisions and selected grant recipients. Approve/reject
  with a biometric-bound P-256 signature. The complete payload being signed is
  shown before authentication.

## Security and delivery boundaries

QR certificate pins authenticate first contact. A remembered TLS key pin takes
precedence and supports certificate renewal. TLS validity, hostname verification,
HTTPS-only origins and redirect rejection remain enforced. Read-only health
probes select an address; each signed POST is sent once. No trust-all manager or
permissive hostname verifier is installed. Certificate tests cover wrong pins,
wrong hostnames, renewed certificates and key-pin precedence.

Ordinary Ed25519 seeds and local data are AES-GCM encrypted with an Android
Keystore key requiring an unlocked device. Backups are disabled. The Ed25519 key
is software signing material protected at rest, not a claimed hardware Ed25519
key. Approval keys are separate, non-exportable P-256 Keystore keys, require
strong biometrics for every signing operation, and must report hardware backing.
Unsupported devices fail explicitly; device PIN is not an approval fallback.
Hardware provenance is not remotely attested by the existing server protocol.
Changing enrolled biometrics may invalidate the approval key; existing recovery
limitations from the server/iOS protocol still apply.

Outbox payloads bind the paired device, certificate identity, exact action
revision and stable idempotency key. Delivery rechecks server grants and local
enablement. TLS/auth/revision failures pause instead of downgrading trust.
Transport failures retry with fresh signed envelopes and the same job identity.
Limits: 50 local records / 64 MiB, seven-day queued lifetime. Expired content is
paused for review; sent content bytes are removed after acceptance. Removing a
local receipt cannot undo accepted server work. The app keeps delivered receipts
until removed; Android may delay or stop background work.

## Verification and remaining acceptance

`:protocol:test` runs on Java without an Android SDK. It validates RFC 8032
Ed25519 vectors, exact envelope bytes, unsafe-origin rejection and real TLS
handshakes. Set `XLATCH_TEST_TICKET` to an expiring ticket file for an isolated
server with `echo` granted to run the opt-in Rust daemon round trip. Never put
real pairing secrets in source or logs.

The debug APK compiles with SDK 35 and passes Android lint (no errors); its APK
signature is verified. Emulator and real-device testing remain outstanding, so
this is a development build, not a tested mobile release. Hardware biometric flows
require a real compatible Android device. Remaining parity work includes file
result export, identity-migration UI, signed recovery/revocation flows beyond the
current protocol, notification deep links, Android theme-file import, launcher
shortcuts/widgets and release distribution. The parent TRX remains open until
these acceptance boundaries are recorded and core device flows are verified.
