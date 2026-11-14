//! Incumbent palette resolution projected into Rune's linear color roles.
use anyhow::{Context as _, Result};
use rune::Color;
use std::path::PathBuf;
use xlatch_desktop::palette::{Palette, candidates};

pub struct Appearance {
    pub choice: String,
    pub status: String,
    applied: Option<Palette>,
}

impl Appearance {
    pub fn new(choice: String) -> Self {
        Self {
            choice,
            status: "Loading appearance…".into(),
            applied: None,
        }
    }

    pub fn update(&mut self, result: Result<(Palette, String)>) {
        match result {
            Ok((palette, source)) => {
                self.status = format!("{} · {source}", palette.name);
                self.applied = Some(palette);
            }
            Err(error) => self.status = format!("Keeping previous theme: {error:#}"),
        }
    }

    pub fn paint(&self, system_dark: bool) -> Result<Paint> {
        let palette = match &self.applied {
            Some(palette) => palette.clone(),
            None => builtin(house_choice(&self.choice) || (self.choice == "auto" && system_dark))?,
        };
        Ok(Paint::new(&palette))
    }
}

/// All normal UI colors/radii come from the incumbent semantic mapping.
#[derive(Clone, Copy)]
pub struct Paint {
    pub bg: Color,
    pub sidebar: Color,
    pub line: Color,
    pub control: Color,
    pub hover: Color,
    pub fg: Color,
    pub muted: Color,
    pub primary: Color,
    pub danger: Color,
    pub warning: Color,
    pub info: Color,
    pub radius: f32,
    pub dark: bool,
}

impl Paint {
    fn new(palette: &Palette) -> Self {
        let s = palette.slots;
        Self {
            bg: color(s[0]),
            sidebar: color(s[17]),
            line: color(s[1]),
            control: color(s[1]),
            hover: color(s[2]),
            fg: color(s[5]),
            muted: color(s[4]),
            primary: color(s[11]),
            danger: color(s[8]),
            warning: color(s[10]),
            info: color(s[13]),
            radius: f32::from(palette.radius) / 2.,
            dark: palette.dark,
        }
    }

    pub fn status(self, status: &str) -> Color {
        match status {
            "succeeded" => self.primary,
            "failed" => self.danger,
            "running" => self.warning,
            "queued" => self.info,
            _ => self.muted,
        }
    }
}

pub fn color(rgb: u32) -> Color {
    let channel = |shift: u32| {
        let value = ((rgb >> shift) & 255_u32) as f32 / 255.;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    Color {
        r: channel(16),
        g: channel(8),
        b: channel(0),
        a: 1.,
    }
}

/// `house` (the default) and its `dark` alias select byteowlz 2 House.
fn house_choice(choice: &str) -> bool {
    matches!(choice, "house" | "dark")
}

/// The bundled scheme: House for dark, xlatch's light palette until House ships
/// a light scheme.
pub fn builtin(dark: bool) -> Result<Palette> {
    if dark {
        Palette::house()
    } else {
        Palette::parse(include_str!("../themes/light.json"), true)
    }
}

pub fn resolve(choice: &str, system_dark: bool) -> Result<(Palette, String)> {
    if !matches!(choice, "auto" | "light" | "dark" | "house") {
        return Ok((Palette::load(std::path::Path::new(choice))?, choice.into()));
    }
    if choice == "auto" {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| dirs::home_dir().map(|p| p.join(".config")))
            .context("Cannot locate configuration directory")?;
        for path in candidates(&config) {
            match std::fs::symlink_metadata(&path) {
                Ok(_) => {
                    return Ok((
                        Palette::load(&path).with_context(|| path.display().to_string())?,
                        path.display().to_string(),
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok((
        builtin(house_choice(choice) || (choice == "auto" && system_dark))?,
        if choice == "auto" {
            "system appearance".into()
        } else {
            "manual selection".into()
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rune_projection_preserves_roles_radius_and_last_valid_palette() -> Result<()> {
        let palette = builtin(true)?;
        let paint = Paint::new(&palette);
        anyhow::ensure!(paint.sidebar == color(0x0014_1416) && paint.primary == color(0x0032_d74b));
        anyhow::ensure!(paint.radius.to_bits() == 4_f32.to_bits());
        let mut appearance = Appearance::new("dark".into());
        appearance.update(Ok((palette, "fixture".into())));
        appearance.update(Err(anyhow::anyhow!("malformed fixture")));
        anyhow::ensure!(appearance.paint(false)?.primary == paint.primary);
        anyhow::ensure!(appearance.status.contains("Keeping previous theme"));
        let light = Appearance::new("auto".into()).paint(false)?;
        anyhow::ensure!(!light.dark && paint.dark);
        let house = Appearance::new("house".into()).paint(false)?;
        anyhow::ensure!(house.dark && house.bg == color(0) && house.primary == paint.primary);
        Ok(())
    }
}
