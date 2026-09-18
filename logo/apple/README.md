# Apple icon exports

Generated from the approved SVGs in `logo/SVG/`, without changing the logo or wordmark.

- `ios/xlatch_white_on_black_1024.png`: current iOS/iPadOS app icon. Opaque RGB, 1024×1024, square background. Xcode generates the smaller sizes from the existing universal asset entry.
- `ios/xlatch_black_on_white_1024.png`: alternative default artwork, not automatically configured as an appearance variant.
- `macos/xlatch_*.icns`: traditional macOS/Tauri bundle icons, with the standard 16/32/128/256/512 point images at 1× and 2×.
- `macos/xlatch_*_1024.png`: square macOS masters with transparent margins around the rounded tile.
- `macos/xlatch_*.iconset/`: source PNGs used by `iconutil`.

These are conventional asset-catalog/ICNS exports, not layered Icon Composer documents. No App Store submission validation has been performed. The macOS wordmark remains faithful to the source and will be small at Dock sizes.

## Update the Illustrator template

1. Use RGB document mode and an sRGB export workflow. Use pixel units.
2. Make dedicated **1024×1024 px artboards**, with integer positions and dimensions. Keep the original design proportions when moving/scaling artwork.
3. **iOS flat icon:** extend an opaque background to all four edges. Do not draw rounded corners, outer transparent margins, or an outer drop shadow. iOS masks the square. Ensure the final exported PNG has no alpha channel, even if every pixel looks opaque.
4. **Traditional macOS/Tauri icon:** use a separate square artboard with a transparent background and a rounded tile inside it. These exports fit the existing tile into a centered 896×896 area (64px nominal margins). That padding is an optical choice, not a mandatory Apple pixel measurement. Do not flatten these corners onto white or black.
5. Export **artboards**, not selected artwork bounds: File → Export → Export for Screens → Artboards → PNG at **1×**. With Export As, enable **Use Artboards** and select **72 ppi** for a pixel-sized artboard. A 1024px artboard at 300 ppi would export roughly 4267px; the original 800px artboards at 300 ppi explain the roughly 3334px exports.
6. Export SVG source using artboard bounds too. Verify a square `viewBox` and avoid fractional extra canvas. The current SVG artboards are 1024×1024. Illustrator’s current `1x` PNGs are 1025×1025; check artboard X/Y pixel alignment and export bounds. This script renders the SVGs at exact target dimensions, so those PNGs are not used.
7. Keep logo artwork, background, and any effects on separate layers for future Icon Composer work. Its layered workflow differs from the flat PNG/ICNS pipeline; do not bake platform masks into foreground layers.

The `.ai` source/template has not been modified. Apply these settings in Illustrator, then regenerate exports.

## Rebuild

On macOS with `rsvg-convert` (librsvg), ImageMagick, and `iconutil` installed:

```sh
./scripts/export-apple-icons.sh
```

This regenerates both color alternatives and copies the white-on-black iOS asset into the app. It does not install or publish anything. Use the macOS `.icns` in Tauri's `bundle.icon` configuration; keep other platform icon entries as needed.

References: [Apple app icon configuration](https://developer.apple.com/documentation/xcode/configuring-your-app-icon), [Apple iconset sizes](https://developer.apple.com/library/archive/documentation/Xcode/Reference/xcode_ref-Asset_Catalog_Format/IconSetType.html), [Adobe Export for Screens](https://helpx.adobe.com/ca/illustrator/desktop/save-and-export/export-files-to-different-formats/export-for-screens.html).
