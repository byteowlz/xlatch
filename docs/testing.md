# Run and test CrossLatch

From the xlatch repository:

```sh
cargo build -p xlatch -p xlatch-mcp
cargo test -p xlatch-core -p xlatch -p xlatch-mcp
cargo fmt --all -- --check
cargo clippy -p xlatch-core -p xlatch -p xlatch-mcp --all-targets
```

The v0 service targets macOS/Linux because local control requires Unix peer credentials. Windows local transport is not implemented. Android is not part of this first client build.

Start a server with a stable LAN address reachable by the phone (substitute your address):

```sh
./target/debug/xlatch service run --listen 0.0.0.0:7443 --public-url https://YOUR-LAN-IP:7443
```

Data uses the existing XDG resolver (`~/.local/share/xlatch` by default on macOS/Linux). Both binaries accept `--data-dir PATH` for an isolated instance. TLS certificate/key and SQLite live there; don't commit or share that directory. Keep the daemon running in a terminal while testing. `serve` runs in the foreground and can be supervised by launchd/systemd; it does not install a background service automatically.

In a second terminal:

```sh
./target/debug/xlatch register examples/capabilities/echo.json
./target/debug/xlatch approve echo --revision REVISION_FROM_REGISTER
./target/debug/xlatch invoke echo --revision REVISION_FROM_REGISTER --input examples/hello.json --wait
./target/debug/xlatch pair --capability echo --qr /tmp/xlatch-pair.svg
```

Review the registered manifest before approval. Display the generated QR within five minutes; the same command prints a JSON code usable with the app's Paste pairing code option. Each ticket enrolls one device. Treat it as a credential while valid. The app shows only the granted active revisions.

For the FFmpeg example, install Python 3 and FFmpeg, then generate and review a manifest:

```sh
python3 scripts/command-manifest.py examples/adapters/ffmpeg_wav.py --id audio-wav --title 'Convert to WAV' --description 'Convert up to two minutes of media to mono 16 kHz WAV on this host.' --accept 'audio/*' --accept 'video/*' > /tmp/xlatch-ffmpeg.json
./target/debug/xlatch register /tmp/xlatch-ffmpeg.json
./target/debug/xlatch approve audio-wav --revision REVIEWED_REVISION --allow-host-execution
./target/debug/xlatch pair --capability echo --capability audio-wav --qr /tmp/xlatch-pair.svg
```

The manifest generator supplies generic object schemas; tighten them to the exact adapter input/output contract before sharing beyond personal testing. The program is trusted host code, not isolated code.

## iOS

Open `ios/XLatch.xcodeproj` in Xcode. The project includes a native SwiftUI app, share extension, shared Keychain/app-group entitlements, QR scanner, and tests. `ios/project.json` is the XcodeGen source.

Simulator tests require entitlements: use normal simulator/ad-hoc signing, not `CODE_SIGNING_ALLOWED=NO`, for Keychain coverage. A fresh enrollment JSON in the test environment variable `XLATCH_TEST_TICKET` enables the live HTTPS test; without it that one test is explicitly skipped. Never commit this value. Protocol-only tests require no server.

For your iPhone, Xcode needs a signed-in developer account for the selected team and provisioning profiles for both `com.byteowlz.xlatch` and `.share`, with the app group and Keychain group. A registered device and installed signing certificate alone do not create these profiles. Choose the connected unlocked phone and Run, or use `xcodebuild -allowProvisioningUpdates` followed by `xcrun devicectl device install app` once signing succeeds.

Test flows: scan/paste pairing → choose Return shared content → send text → view Activity/result. Then share a URL or small file from another app to CrossLatch. Try a wrong/expired pairing code, disconnected server, unsupported input, and device revocation. Background pushes are not yet available; reopen the app for pending results.
