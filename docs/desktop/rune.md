# Rune desktop renderer

The optional desktop uses Rune at the workspace-pinned revision. The default
workspace remains headless. `xlatch-desktop` retains the shared `Connection`,
`Snapshot` and `Draft` substrate; paired HTTPS uses `xlatch-client`, never an
operator fallback. No second graphical renderer is maintained.

```sh
just desktop-check
just desktop-render actions dark 1100x760
just desktop-render selected light 760x540
```

`desktop-check` compiles the renderer and runs desktop transport/model/input tests
and strict all-target Clippy. `desktop-render` writes a synthetic PNG and semantic
JSON tree to `target/desktop-review/`. Scenes include all four pages, selected,
pending, paired, offline, busy, unknown, empty, long-input, contract and result states.

Tooling flags follow `--` and require `--fixture`. Fixture mode does not load
credentials, connect, capture context or read user theme files. Ordinary text
editing remains local. Clipboard reads happen only through explicit paste.
Background threads perform transport calls and palette-file resolution; the UI
keeps exact selected revisions and invocation identity across uncertain replies.
Changed content requires explicit New run before another invocation.

The original Base16/Base24/Omarchy palette parser and semantic roles remain
shared. Invalid updates retain the last valid palette. Rune's implicit mount and
reflow fades are disabled for this operational surface so newly revealed controls
are immediately usable. Runtime tests change external state with `Runtime::update`;
`frame(0)` alone does not invalidate a cached view. Long editors need an ancestor
scroll/clip viewport: scrolling a text field itself does not clip its own glyphs.

## R2 shell and platform boundaries

The R2 prototype integrates navigation and window controls in one header, places
discovery beside a larger editor, and keeps contract inspection secondary.
Command/Ctrl+K focuses search; Command/Ctrl+Enter runs the reviewed action.
Results stay compact until inspected; inspecting an action receipt opens Activity
without losing editor content or the invocation key. No new context-source picker
is introduced.

macOS uses Rune's app-drawn titlebar with **app-owned** window controls from
`xlatch-window-controls`: three real `NSButton`s (close, minimize, full screen) in
the window's frame view, pinned top-leading at the same geometry as the offscreen
preview (`Layout`: 24-point targets, 2 points apart, 16 points in). Each forwards
`performClick:` to its hidden stock counterpart, so close vetoes/tray handling,
minimize and the green button's full-screen behavior stay native. The window's
accessibility close/minimize/zoom/full-screen relationships point at the new
controls, and the stock buttons are hidden, so exactly one group is visible and
accessible; dropping the controls restores the stock ones.

Do not restyle the stock traffic-light cells: they keep drawing their coloured
backgrounds under any replacement image (rejected on a real window). The adapter
is the workspace's only unsafe code (control targets, superview lookup, AX
constants and relationships), reviewed per operation; the desktop crate itself
still forbids unsafe. Native verification must still prove appearance, hover,
actions, AX, focus, full screen and close/tray behavior on a real window.

Windows uses Rune's client-drawn caption area, preserving native resize/snap
behavior and platform caption controls. The pinned renderer retains the system
frame on Linux; Linux frameless parity is an explicit unresolved platform gap,
not a claim made by a macOS prototype. Do not force `borderless = true` merely to
hide it: the pinned Rune minimize path explicitly rejects borderless windows.
System scrollbars likewise remain a platform-native theming exception.

Headless pixels, the semantic tree, scripted Runtime input, native window
interactions/accessibility, signed installation and human acceptance are distinct
proof. Renderer tests do not establish native platform parity, Session delivery,
new source-picker approval or a durable desktop outbox. Platform chrome and
scrollbar differences need explicit native evidence rather than being hidden.
