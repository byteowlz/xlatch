//! Native projection of byteowlz palette roles, plus automatic file reloads.
use anyhow::{Context as _, Result};
use gpui_kit::component::{Theme, ThemeMode, ThemeRegistry};
use gpui_kit::{App, Global};
use serde_json::{Value, json};
use std::path::PathBuf;
use xlatch_desktop::palette::{Palette, candidates, mix, on_color};

#[derive(Clone)]
pub struct Appearance {
    pub choice: String,
    pub status: String,
    applied: Option<Palette>,
}
impl Global for Appearance {}

pub fn init(choice: String, cx: &mut App) {
    let initial = resolve(
        if ThemeMode::from(cx.window_appearance()).is_dark() {
            "dark"
        } else {
            "light"
        },
        false,
    )
    .ok()
    .map(|(palette, _)| palette);
    if let Some(palette) = &initial
        && let Err(error) = apply(palette, cx)
    {
        log::error!("Default theme: {error:#}");
    }
    cx.set_global(Appearance {
        choice,
        status: "Loading appearance…".into(),
        applied: initial,
    });
    cx.spawn(async move |cx| {
        loop {
            let input = cx.update(|cx| {
                (
                    cx.global::<Appearance>().choice.clone(),
                    ThemeMode::from(cx.window_appearance()).is_dark(),
                )
            });
            let (choice, dark) = input;
            let expected_choice = choice.clone();
            let result = cx
                .background_executor()
                .spawn(async move { resolve(&choice, dark) })
                .await;
            cx.update(|cx| {
                if cx.global::<Appearance>().choice != expected_choice {
                    return;
                }
                update(result, cx);
            });
            cx.background_executor()
                .timer(std::time::Duration::from_secs(2))
                .await;
        }
    })
    .detach();
}

fn update(result: Result<(Palette, String)>, cx: &mut App) {
    let previous_status = cx.global::<Appearance>().status.clone();
    match result {
        Ok((palette, source)) => {
            if cx.global::<Appearance>().applied.as_ref() != Some(&palette)
                || Theme::global(cx).background
                    != gpui_kit::Hsla::from(gpui_kit::rgb(palette.slots[0]))
            {
                if let Err(error) = apply(&palette, cx) {
                    cx.global_mut::<Appearance>().status = format!("Theme not applied: {error:#}");
                    return;
                }
                cx.global_mut::<Appearance>().applied = Some(palette.clone());
            }
            cx.global_mut::<Appearance>().status = format!("{} · {source}", palette.name);
        }
        Err(error) => {
            cx.global_mut::<Appearance>().status = format!("Keeping previous theme: {error:#}");
        }
    }
    if cx.global::<Appearance>().status != previous_status {
        cx.refresh_windows();
    }
}

pub fn select(choice: &str, cx: &mut App) {
    cx.global_mut::<Appearance>().choice = choice.into();
    cx.refresh_windows();
}

fn resolve(choice: &str, system_dark: bool) -> Result<(Palette, String)> {
    if !matches!(choice, "auto" | "light" | "dark") {
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
    let dark = choice == "dark" || (choice == "auto" && system_dark);
    let body = if dark {
        include_str!("../themes/dark.json")
    } else {
        include_str!("../themes/light.json")
    };
    Ok((
        Palette::parse(body, true)?,
        if choice == "auto" {
            "system appearance".into()
        } else {
            "manual selection".into()
        },
    ))
}

fn theme_document(palette: &Palette) -> Value {
    let s = &palette.slots;
    let mut colors = serde_json::Map::new();
    for (keys, index) in [
        (
            "background,title_bar.background,tab_bar.background,list.background,table.background",
            0,
        ),
        (
            "border,input.border,popover.background,accordion.background,group_box.background,window.border,sidebar.border,title_bar.border,table.row_border,tab.border",
            1,
        ),
        (
            "secondary.hover.background,secondary.active.background,secondary.background,accent.background,muted.background,list.active.background,list.hover.background,tab.active.background,selection.background,button.hover.background,button.active.background,sidebar.accent.background",
            2,
        ),
        (
            "muted.foreground,scrollbar.thumb.background,scrollbar.thumb.hover.background",
            4,
        ),
        (
            "foreground,caret,accent.foreground,secondary.foreground,popover.foreground,sidebar.foreground,sidebar.accent.foreground,tab.foreground,tab.active.foreground,button.foreground",
            5,
        ),
        ("primary.background,ring,sidebar.primary.background", 11),
        ("sidebar.background", 17),
        ("button.background", 1),
    ] {
        for key in keys.split(',') {
            colors.insert(key.into(), json!(format!("#{:06x}", s[index])));
        }
    }
    for (role, index, bright) in [
        ("primary", 11, 20),
        ("success", 11, 20),
        ("danger", 8, 18),
        ("warning", 10, 19),
        ("info", 13, 22),
    ] {
        for (suffix, color) in [
            ("background", s[index]),
            ("foreground", on_color(s[index])),
            ("hover.background", mix(s[index], s[5], 8)),
            ("active.background", s[bright]),
        ] {
            colors.insert(format!("{role}.{suffix}"), json!(format!("#{color:06x}")));
        }
    }
    json!({"name":"xlatch","themes":[{"name":"xlatch current","mode":if palette.dark {"dark"} else {"light"},"radius":palette.radius / 2,"radius.lg":palette.radius,"font.size":14,"shadow":false,"colors":colors}]})
}

fn apply(palette: &Palette, cx: &mut App) -> Result<()> {
    ThemeRegistry::global_mut(cx).load_themes_from_str(&theme_document(palette).to_string())?;
    let config = ThemeRegistry::global(cx)
        .themes()
        .get("xlatch current")
        .cloned()
        .context("Theme registration failed")?;
    let mode = config.mode;
    let theme = Theme::global_mut(cx);
    if mode.is_dark() {
        theme.dark_theme = config;
    } else {
        theme.light_theme = config;
    }
    Theme::change(mode, None, cx);
    cx.refresh_windows();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_theme_schema_keeps_palette_roles_and_radius() -> Result<()> {
        let palette = Palette::parse(include_str!("../themes/dark.json"), true)?;
        let set: gpui_kit::component::ThemeSet = serde_json::from_value(theme_document(&palette))?;
        let theme = set.themes.first().context("theme")?;
        anyhow::ensure!(theme.colors.sidebar.as_deref() == Some("#0e1310"));
        anyhow::ensure!(theme.colors.primary.as_deref() == Some("#77bd9d"));
        anyhow::ensure!(theme.radius == Some(4) && theme.radius_lg == Some(8));
        Ok(())
    }
}
