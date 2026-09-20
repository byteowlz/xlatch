# Action icons

Add an icon while registering a manifest:

```sh
xlatch register action.json --icon action.svg
xlatch register action.json --icon action.png
```

Import produces a portable PNG no larger than 128 × 128 pixels, preserving proportions and transparency. The image is embedded in the approved manifest and cached with the target, so clients need no image server or access to the source file. Keep the original SVG in your integration repository.

Raw API manifests may include `"icon": {"png_base64": "BASE64_PNG_BYTES"}`. PNG bytes are limited to 128 KiB and 256 pixels per axis. The field is optional; omitting it preserves existing manifest serialization and revisions. Adding or changing an icon changes the revision and follows the normal approval/grant flow, including protected mode.

PNG import accepts sources up to 1 MiB and 4096 pixels per axis. SVG import accepts up to 64 KiB and 1024 XML nodes. Use outlined shapes, gradients and clip paths. Text, filters, embedded images, scripts, event handlers, external references and reuse elements are unsupported. Image resolvers are disabled, so imports cannot retrieve remote or local resources. Convert text to paths before exporting.

Icons appear inside the xlatch iOS share extension and action/approval lists, Android action lists, and the GPUI action list. Missing or malformed cached icons fall back to a generic glyph. Original colors are retained; include sufficient contrast for both light and dark backgrounds. These are target icons within xlatch, not dynamically installed top-level iOS share extensions or custom Shortcuts glyphs.
