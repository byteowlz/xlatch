//! Byteowlz slot/role contract. Theme files are data, never executable hooks.
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// Fully validated palette; Base16 additions follow design-system derivation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    /// User-visible scheme name.
    pub name: String,
    /// Scheme luminosity, independent of OS appearance.
    pub dark: bool,
    /// base00 through base17, in hexadecimal order.
    pub slots: [u32; 24],
    /// Root radius dial; controls derive proportional radii from it.
    pub radius: u16,
}

#[derive(Deserialize)]
struct Scheme {
    name: String,
    system: String,
    #[serde(alias = "variant")]
    mode: String,
    #[serde(alias = "palette")]
    slots: BTreeMap<String, String>,
    #[serde(default = "default_radius")]
    radius: u16,
}
const fn default_radius() -> u16 {
    8
}

/// Parse six-digit sRGB, as used by tinted and Omarchy exports.
/// # Errors
/// Rejects malformed colors rather than partially applying a scheme.
pub fn color(value: &str) -> Result<u32> {
    let hex = value.trim_start_matches('#');
    ensure!(
        hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()),
        "Expected six-digit sRGB color, got {value:?}"
    );
    Ok(u32::from_str_radix(hex, 16)?)
}

/// Mix sRGB channels with an integer percentage, rounding to nearest byte.
#[must_use]
pub fn mix(from: u32, to: u32, percent: u32) -> u32 {
    let amount = percent.min(100);
    [0, 8, 16].into_iter().fold(0, |out, shift| {
        let a = (from >> shift) & 255;
        let b = (to >> shift) & 255;
        out | (((a * (100 - amount) + b * amount + 50) / 100) << shift)
    })
}

impl Palette {
    /// Parse design-system or tinted-shaped JSON/TOML, or Omarchy colors TOML.
    /// # Errors
    /// Rejects incomplete Base24, invalid modes, colors and radius values.
    pub fn parse(body: &str, json: bool) -> Result<Self> {
        let value: serde_json::Value = if json {
            serde_json::from_str(body)?
        } else {
            serde_json::to_value(toml::from_str::<toml::Value>(body)?)?
        };
        if value.get("background").is_some() {
            return Self::omarchy(&value);
        }
        let scheme: Scheme = serde_json::from_value(value)?;
        ensure!(
            matches!(scheme.system.as_str(), "base16" | "base24"),
            "system must be base16 or base24"
        );
        ensure!(
            matches!(scheme.mode.as_str(), "dark" | "light"),
            "mode must be dark or light"
        );
        ensure!(scheme.radius <= 24, "radius must be between 0 and 24");
        let mut slots = [0; 24];
        let required = if scheme.system == "base24" { 24 } else { 16 };
        for (i, slot) in slots.iter_mut().enumerate().take(required) {
            let key = format!("base{i:02X}");
            *slot = color(
                scheme
                    .slots
                    .get(&key)
                    .with_context(|| format!("Missing {key}"))?,
            )?;
        }
        if required == 16 {
            slots[16] = mix(slots[0], 0, 18);
            slots[17] = mix(slots[0], 0, 34);
            for (index, source) in [8, 10, 11, 12, 13, 14].into_iter().enumerate() {
                slots[index + 18] = mix(slots[source], 0x00ff_ffff, 22);
            }
            for (index, slot) in slots.iter_mut().enumerate().skip(16) {
                if let Some(value) = scheme.slots.get(&format!("base{index:02X}")) {
                    *slot = color(value)?;
                }
            }
        }
        Ok(Self {
            name: scheme.name,
            dark: scheme.mode == "dark",
            slots,
            radius: scheme.radius,
        })
    }

    fn omarchy(value: &serde_json::Value) -> Result<Self> {
        let get = |key: &str| -> Result<u32> {
            color(
                value[key]
                    .as_str()
                    .with_context(|| format!("Missing Omarchy {key}"))?,
            )
        };
        let background = get("background")?;
        let foreground = get("foreground")?;
        let mut slots = [background; 24];
        slots[1] = mix(background, foreground, 6);
        slots[2] = mix(background, foreground, 12);
        slots[3] = mix(background, foreground, 35);
        slots[4] = mix(background, foreground, 75);
        slots[5] = foreground;
        slots[6] = foreground;
        slots[7] = foreground;
        for (index, named, ansi) in [
            (8, "red", "color1"),
            (9, "orange", "color3"),
            (10, "yellow", "color3"),
            (11, "green", "color2"),
            (12, "cyan", "color6"),
            (13, "blue", "color4"),
            (14, "magenta", "color5"),
            (15, "brown", "color1"),
        ] {
            slots[index] = get(if value.get(named).is_some() {
                named
            } else {
                ansi
            })?;
        }
        slots[16] = mix(background, 0, 18);
        slots[17] = mix(background, 0, 34);
        for (index, source, named, ansi) in [
            (18, 8, "bright_red", "color9"),
            (19, 10, "bright_yellow", "color11"),
            (20, 11, "bright_green", "color10"),
            (21, 12, "bright_cyan", "color14"),
            (22, 13, "bright_blue", "color12"),
            (23, 14, "bright_magenta", "color13"),
        ] {
            slots[index] = if value.get(named).is_some() {
                get(named)?
            } else if value.get(ansi).is_some() {
                get(ansi)?
            } else {
                mix(slots[source], 0x00ff_ffff, 22)
            };
        }
        for (index, key) in [
            (1, "lighter_background"),
            (2, "selection"),
            (16, "dark_background"),
            (17, "darker_background"),
            (6, "light_foreground"),
            (7, "bright_foreground"),
        ] {
            if value.get(key).is_some() {
                slots[index] = get(key)?;
            }
        }
        let dark = match value["mode"].as_str() {
            Some("dark") => true,
            Some("light") => false,
            None => luminance(background) < luminance(foreground),
            _ => bail!("Omarchy mode must be dark or light"),
        };
        Ok(Self {
            name: "Omarchy".into(),
            dark,
            slots,
            radius: 8,
        })
    }

    /// Read a bounded regular theme file (symlinks to regular files are supported).
    /// # Errors
    /// Reports I/O or validation failures, preserving the caller's previous theme.
    pub fn load(path: &Path) -> Result<Self> {
        use std::io::Read as _;
        let file = std::fs::File::open(path)?;
        ensure!(file.metadata()?.is_file(), "Theme must be a regular file");
        let mut body = String::new();
        file.take(65_537).read_to_string(&mut body)?;
        ensure!(body.len() <= 65_536, "Theme exceeds 64 KiB");
        Self::parse(&body, path.extension().is_some_and(|e| e == "json"))
    }
}

fn luminance(color: u32) -> f64 {
    [16, 8, 0]
        .into_iter()
        .zip([0.2126, 0.7152, 0.0722])
        .map(|(shift, weight)| {
            let channel = f64::from((color >> shift) & 255) / 255.;
            weight
                * if channel <= 0.04045 {
                    channel / 12.92
                } else {
                    ((channel + 0.055) / 1.055).powf(2.4)
                }
        })
        .sum()
}

/// Highest-contrast black or white foreground for filled actions.
#[must_use]
pub fn on_color(color: u32) -> u32 {
    if luminance(color) > 0.179 {
        0
    } else {
        0x00ff_ffff
    }
}

/// Candidate precedence for automatic theming; resolve paths afresh on each reload.
#[must_use]
pub fn candidates(config: &Path) -> Vec<PathBuf> {
    vec![
        config.join("xlatch/theme.toml"),
        config.join("xlatch/theme.json"),
        config.join("omarchy/current/theme/colors.toml"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn base16_matches_reference_derivation_and_preserves_explicit_slots() -> Result<()> {
        let mut value: serde_json::Value =
            serde_json::from_str(include_str!("../themes/dark.json"))?;
        value["system"] = "base16".into();
        let mut palette = value["slots"].as_object().context("slots")?.clone();
        for index in 16..24 {
            palette.remove(&format!("base{index:02X}"));
        }
        palette.insert("base17".into(), "#abcdef".into());
        value["slots"] = serde_json::Value::Object(palette);
        let result = Palette::parse(&value.to_string(), true)?;
        ensure!(result.slots[16] == mix(result.slots[0], 0, 18));
        ensure!(result.slots[17] == mix(result.slots[0], 0, 34));
        ensure!(result.slots[18] == mix(result.slots[8], 0x00ff_ffff, 22));
        ensure!(result.slots[23] == 0x00ab_cdef);
        value["system"] = "base24".into();
        ensure!(Palette::parse(&value.to_string(), true).is_err());
        Ok(())
    }
    #[test]
    fn invalid_palette_is_rejected_whole() -> Result<()> {
        let mut value: serde_json::Value =
            serde_json::from_str(include_str!("../themes/light.json"))?;
        value["slots"]["base08"] = "#notrgb".into();
        ensure!(Palette::parse(&value.to_string(), true).is_err());
        ensure!(color("#1234567").is_err());
        ensure!(color("#000000")? == 0);
        Ok(())
    }
    #[test]
    fn house_text_meets_contrast_floor() -> Result<()> {
        for body in [
            include_str!("../themes/dark.json"),
            include_str!("../themes/light.json"),
        ] {
            let p = Palette::parse(body, true)?;
            for foreground in [p.slots[4], p.slots[5]] {
                for background in [p.slots[0], p.slots[1], p.slots[17]] {
                    let a = luminance(foreground);
                    let b = luminance(background);
                    ensure!(
                        (a.max(b) + 0.05) / (a.min(b) + 0.05) >= 4.5,
                        "{} text contrast",
                        p.name
                    );
                }
            }
        }
        Ok(())
    }
    #[test]
    fn omarchy_named_and_legacy_palettes_are_supported() -> Result<()> {
        let mut value = serde_json::json!({"background":"#101010","foreground":"#dddddd"});
        for index in 0..16 {
            value[format!("color{index}")] = "#778899".into();
        }
        let legacy = Palette::parse(&value.to_string(), true)?;
        ensure!(legacy.dark && legacy.slots[13] == 0x0077_8899);
        value["green"] = "#123456".into();
        value["darker_background"] = "#050505".into();
        let modern = Palette::parse(&value.to_string(), true)?;
        ensure!(modern.slots[11] == 0x0012_3456 && modern.slots[17] == 0x0005_0505);
        Ok(())
    }
    #[cfg(unix)]
    #[test]
    fn load_follows_atomic_symlink_changes() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let light = directory.path().join("light.json");
        let dark = directory.path().join("dark.json");
        let current = directory.path().join("current.json");
        std::fs::write(&light, include_str!("../themes/light.json"))?;
        std::fs::write(&dark, include_str!("../themes/dark.json"))?;
        std::os::unix::fs::symlink(&dark, &current)?;
        ensure!(Palette::load(&current)?.dark);
        let next = directory.path().join("next");
        std::os::unix::fs::symlink(&light, &next)?;
        std::fs::rename(next, &current)?;
        ensure!(!Palette::load(&current)?.dark);
        Ok(())
    }
}
