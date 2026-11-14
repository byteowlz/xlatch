# xlatch justfile (Rust workspace)

# Default recipe - show help
default:
    @just --list

# === Development ===

# Install all binaries
install:
    cargo install --path crates/xlatch-cli

# Install all binaries from workspace
install-all:
    @for crate in $(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.targets[] | .kind[] == "bin") | .manifest_path | split("/") | .[-2]'); do \
        echo "Installing $crate..."; \
        cargo install --path crates/$crate; \
    done

# Install a specific crate
install-crate CRATE:
    cargo install --path crates/{{CRATE}}

# Uninstall all binaries
uninstall:
    @for crate in $(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.targets[] | .kind[] == "bin") | .name'); do \
        echo "Uninstalling $crate..."; \
        cargo uninstall $crate 2>/dev/null || true; \
    done

# === Building ===

# Debug build (all crates)
build:
    cargo build --workspace

# Release build (all crates)
build-release:
    cargo build --workspace --release

# Build a specific crate
build-crate CRATE:
    cargo build -p {{CRATE}}

# Build with all features
build-all:
    cargo build --workspace --all-features

# Fast compile check
check:
    cargo check --workspace

# Check a specific crate
check-crate CRATE:
    cargo check -p {{CRATE}}

# Clean build artifacts
clean:
    cargo clean

# === Testing ===

# Run all tests
test:
    cargo test --workspace

# Run tests for a specific crate
test-crate CRATE:
    cargo test -p {{CRATE}}

# Run tests with all features
test-all:
    cargo test --workspace --all-features

# Run tests verbosely
test-v:
    cargo test --workspace -- --nocapture

# Run a specific test
test-one TEST:
    cargo test --workspace {{TEST}}

# === Code Quality ===

# Format all code
fmt:
    cargo fmt --all

# Check formatting
fmt-check:
    cargo fmt --all -- --check

# Run clippy on all crates
clippy:
    cargo clippy --workspace -- -D warnings

# Alias for clippy
lint: clippy

# Run ast-grep guardrails for Rust sources
lint-rust-ai-guardrails:
    sg scan --config .ast-grep/sgconfig.yml

# Clippy on a specific crate
clippy-crate CRATE:
    cargo clippy -p {{CRATE}} -- -D warnings

# Auto-fix clippy warnings
fix:
    cargo clippy --workspace --fix --allow-dirty

# Run all checks
check-all: fmt-check clippy lint-rust-ai-guardrails test

# === Config Generation ===

# Generate config.toml and schema from Rust structs
generate-config:
    cargo run -p xlatch-core --example generate_config

# Validate that examples/ config files are up to date
validate-config:
    cargo test -p xlatch-core validate_examples_are_up_to_date

# === Documentation ===

# Generate docs for all crates
docs:
    cargo doc --workspace --no-deps

# Generate and open docs
docs-open:
    cargo doc --workspace --no-deps --open

# Docs for a specific crate
docs-crate CRATE:
    cargo doc -p {{CRATE}} --no-deps --open

# === Dependencies ===

# Update all dependencies
update:
    cargo update

# Check for outdated dependencies
outdated:
    cargo outdated --workspace

# === Workspace Info ===

# List all crates in workspace
list:
    @cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | "\(.name) (\(.version))"'

# List binary crates
list-bins:
    @cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.targets[] | .kind[] == "bin") | .name'

# List library crates
list-libs:
    @cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.targets[] | .kind[] == "lib") | .name'

# === Release ===

# Release build and show binary sizes
release: build-release
    @echo "Binary sizes:"
    @find target/release -maxdepth 1 -type f -perm +111 ! -name "*.d" -exec ls -lh {} \; 2>/dev/null || true

# Tag and push a release
release-tag VERSION:
    git tag v{{VERSION}}
    git push --tags

# Set up GitHub secrets for automated releases (requires byt)
setup-secrets:
    byt secrets setup xlatch

# Build and install the iOS app; optionally specify a device name or ID.
install-iphone device="":
    python3 scripts/install-iphone.py {{quote(device)}}

# Run the paired desktop CLI; no local daemon or GUI is required.
client *args:
    cargo run --locked -p xlatch-client -- {{args}}

# Prove the optional client contract with isolated pinned HTTPS fixtures.
client-check:
    cargo check -p xlatch-client -p xlatch-desktop
    cargo clippy -p xlatch-client --all-targets -- -D warnings
    cargo test -p xlatch-client -p xlatch-desktop

# Agent-visible operations use the same paired substrate as the desktop app.
agent-check: client-check

# Open the optional native management window (daemon runs independently).
desktop *args:
    cargo run --locked -p xlatch-desktop --features desktop -- {{args}}

# Open the management window with its optional native tray.
desktop-tray *args:
    cargo run --locked -p xlatch-desktop --features tray -- --tray {{args}}

# Verify the shared backend, Rune input model and tray-enabled desktop build.
desktop-check:
    cargo check --locked -p xlatch-desktop --features desktop
    cargo test --locked -p xlatch-desktop --features tray
    cargo clippy --locked -p xlatch-desktop --all-targets --features tray -- -D warnings

# Render synthetic fixtures only; pixels/semantic trees are not native AX proof.
desktop-render SCENE="actions" THEME="dark" SIZE="1100x760":
    mkdir -p target/desktop-review
    cargo run --locked -p xlatch-desktop --features desktop -- --fixture --theme {{quote(THEME)}} -- --scene {{quote(SCENE)}} --size {{quote(SIZE)}} --snap {{quote("target/desktop-review/" + SCENE + "-" + THEME + "-" + SIZE + ".png")}} --tree {{quote("target/desktop-review/" + SCENE + "-" + THEME + "-" + SIZE + ".json")}}

# Write session-bridge manifests bound to the built adapter (ADR 0008). Pass HERDR="" to omit herdr.
sessions-manifests HC HERDR OUT="target/sessions-manifests":
    cargo build --locked --release -p xlatch-sessions
    target/release/xlatch-sessions --hc {{quote(HC)}} {{ if HERDR != "" { "--herdr " + quote(HERDR) } else { "" } }} manifests --out {{quote(OUT)}}

# Copy Design Studio's generated House theme byte-for-byte (never hand-edit it).
sync-house-theme CONSUMER="xlatch-desktop":
    cp {{quote("../design-studio/generated/" + CONSUMER + "/house.rune.json")}} crates/xlatch-desktop/themes/house.rune.json
    shasum -a 256 crates/xlatch-desktop/themes/house.rune.json

# Build a local, ad-hoc signed macOS bundle with the xlatch icon.
desktop-bundle:
    bash scripts/bundle-desktop-macos.sh

# Android protocol tests run without an Android SDK.
android-test:
    cd android && ./gradlew :protocol:test

# Build and lint the Android debug APK (requires Android SDK 35).
android-build:
    cd android && ./gradlew :protocol:test :app:assembleDebug :app:lintDebug

# Install on the connected Android device; ANDROID_SERIAL selects among devices.
install-android: android-build
    adb install -r android/app/build/outputs/apk/debug/app-debug.apk
