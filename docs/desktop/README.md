# Desktop management

The optional GPUI app starts with a full management window: searchable actions,
explicit revision review, text/JSON input, recent jobs, cancellation and results.
Command/Ctrl-K focuses action search; Enter selects the first match for composition. Clipboard text is read only when requested.
The daemon runs independently; quitting the app does not stop it.

```sh
just desktop
just desktop-tray --control-dir /absolute/path/to/daemon/data
just desktop --quick
just desktop-bundle # macOS development .app; not a notarized release
```

The default workspace build remains headless. Desktop builds use the `desktop`
feature; `tray` additionally compiles native menu support. The first transport is
the existing Unix control socket. It has local operator authority in ordinary
mode and cannot override protected-mode authorization. Paired HTTPS, device/grant
management, Windows transport, Linux tray and release packaging remain tracked
work. macOS is the initial validation platform; Windows/Linux parity is not yet
claimed. The tray currently navigates to actions and activity; it does not run
arbitrary commands. Without the tray, closing the window exits the GUI.

An uncertain invocation retains its exact revision, payload and idempotency key
for retry within the open window. New run explicitly discards that identity.
This is not a durable desktop outbox. The app never opens the server database.

## Adjacent tools

- `oqto-desktop`: reuse the GPUI kit and separation of native presentation from
  transport/domain logic. Do not copy its application shell or session model.
- `tray`: preserve the existing tool for now. Migrate useful service operations
  into reviewed, typed xlatch capabilities with presence and status. Importing a
  service definition must not grant permission to execute its shell commands.
- `ctx`: owns explicit context capture and provenance. The desktop composer
  should consume a versioned context bundle, preview it and send only after user
  confirmation. No background clipboard or browser scraping in xlatch.
- `omni`: optional entry point to quick launch. Keep action discovery/invocation
  in xlatch, with a future authenticated paired client and existing-instance
  activation. Do not depend on Omni's current implementation.
- `oqto`: consumes the same capability protocol; desktop presentation must not
  introduce another registry or authorization policy.

Appearance follows Base16/Base24, Omarchy or the system's light/dark mode, with
live reload and explicit overrides in Settings. See [theming](THEMING.md) for
palette formats and the Tinty hook.
