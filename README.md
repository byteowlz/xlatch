# CrossLatch

Share content from your phone to approved server capabilities. The Rust service provides pinned HTTPS pairing, device grants, typed manifests, durable SQLite jobs, and a private Unix control socket. The native iOS app includes a share extension.

## Install and run

```sh
cargo install --path crates/xlatch-cli
xlatch service run
```

For phone access, supply a reachable LAN or Tailscale origin when first starting the service:

```sh
xlatch service enable --listen 0.0.0.0:7443 --public-url https://YOUR_SERVER_IP:7443
xlatch pair
```

Pairing guides capability selection and prints a terminal QR. An empty registry offers an explicit approval for the built-in content round-trip action. Use `--json` for scripted output or `--qr pair.svg` to export the QR. Application configuration uses JSON or TOML.

## Service lifecycle

```sh
xlatch service run       # foreground
xlatch service enable    # install and start at user login
xlatch service start
xlatch service stop
xlatch service restart
xlatch service status
xlatch service disable   # stop and disable automatic startup
```

`enable --dry-run` prints the service definition without installing it. Linux uses a systemd user service; macOS uses a launch agent. Run `enable` before controlling an installed service. Linux startup without a login requires user lingering configured separately. Pass the same global `--data-dir` when accessing a service using a custom directory. Changing an existing HTTPS identity's hostname requires explicit certificate migration; set the reachable origin before pairing.

The source package is `xlatch` in `crates/xlatch-cli`. The separate `xlatch-api` executable has been removed. MCP remains an adapter in `xlatch-mcp`.

See [testing](docs/testing.md), [protocol](docs/protocol.md), and [Oqto integration](docs/oqto-integration.md).
