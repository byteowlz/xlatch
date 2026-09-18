# Desktop appearance

xlatch follows byteowlz/design-system's slot → semantic role → native widget
contract. Its own light/dark palettes keep the green identity. Theme colors also
cover buttons, inputs, caret, selection, sidebar, scrollbars and status states;
the shared GPUI base theme is synchronized. Filled status/action text uses the
higher-contrast black or white foreground. Imported palette text colors remain
author-controlled; a poor third-party palette may still have low contrast.

`--theme auto` (default) checks these in order under `$XDG_CONFIG_HOME`, falling
back to `~/.config`:

1. `xlatch/theme.toml`
2. `xlatch/theme.json`
3. `omarchy/current/theme/colors.toml`
4. xlatch light/dark, following the OS appearance.

`--theme /absolute/path/theme.json` selects a file explicitly. `--theme dark` or
`--theme light` ignores external palettes. Settings shows the actual source and
provides session-only Auto/Light/Dark overrides. Startup flags are unchanged.
Files are checked every two seconds off the UI thread, resolving symlinks anew;
invalid updates retain the last valid palette and show an error in Settings.
No shell hooks are executed by the app. The file limit is 64 KiB.

## Base16 and Base24

Accept JSON or TOML with `name`, `system` (`base16` or `base24`), `mode` (`light`
or `dark`) and a `slots` table of six-digit sRGB colors. Tinted's `variant` and
`palette` field names are accepted aliases. No YAML or CSS color expression
parser is included. See the shipped JSON files in `crates/xlatch-desktop/themes`.

Base24 requires all `base00`–`base17`. Base16 requires `base00`–`base0F`; missing
backgrounds mix base00 toward black by 18% and 34%, and missing bright accents
mix their source toward white by 22%, matching the design-system's TypeScript
reference. Explicit extra slots win. When selecting a community scheme, prefer
its authored Base24 twin; the app does not download or search a scheme catalog.
An optional integer `radius` (0–24, default 8) controls shape. Native controls use
half the dial, list surfaces three quarters, and large surfaces the full dial.
Zero keeps all of those square.

## Omarchy

Reads the active `colors.toml`, including current named colors and legacy
`color0`–`color15` palettes. Background and foreground levels use semantic fields
when available, with derived levels otherwise. Green remains the design-system
primary role (base0B); an arbitrary Omarchy accent does not redefine status green.
Switching the `current/theme` symlink is detected automatically. No Omarchy
commands, hooks or theme scripts are run. Native Linux acceptance is still needed.

## Tinted / Tinty

Tinty 0.29+ exposes palette components to hooks. Add this to the top-level `hooks`
array in your existing Tinty TOML configuration, using your checkout path:

```toml
hooks = ["python3 /absolute/path/to/xlatch/scripts/tinty-xlatch.py"]
```

The hook validates all expected colors, then atomically writes
`$XDG_CONFIG_HOME/xlatch/theme.json`. The running app reloads it in Auto mode.
An existing `xlatch/theme.toml` takes precedence. No user configuration is changed
automatically by building xlatch. The hook does not pick or apply a theme itself.

References: byteowlz/design-system `spec/roles.md`, `spec/base16-policy.md`,
`spec/radius.md`; [Tinty hook variables](https://github.com/tinted-theming/tinty#hooks)
and [Omarchy palette](https://github.com/omacom/omarchy/blob/master/themes/tokyo-night/colors.toml).
