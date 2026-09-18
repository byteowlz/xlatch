//! Native xlatch management app. The daemon remains an independent process.
#[cfg(all(feature = "tray", any(target_os = "macos", target_os = "windows")))]
mod tray;
mod ui;

use clap::Parser;
use gpui_kit::component::Root;
use gpui_kit::{AppContext as _, WindowBounds, WindowOptions, px, size};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(about = "Native xlatch management and action launcher")]
struct Options {
    /// Directory containing the local daemon's control.sock.
    #[arg(long)]
    control_dir: Option<PathBuf>,
    /// Start with the action search focused.
    #[arg(long)]
    quick: bool,
    /// Show an optional system tray icon (requires the tray build feature).
    #[arg(long)]
    tray: bool,
}

fn main() -> anyhow::Result<()> {
    env_logger::try_init()?;
    let options = Options::parse();
    let directory = options
        .control_dir
        .map_or_else(xlatch_core::paths::default_data_dir, Ok)?;
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            {
                let theme = gpui_kit::component::Theme::global_mut(cx);
                theme.colors.primary = gpui_kit::rgb(0x001f_6e57).into();
                theme.colors.primary_hover = gpui_kit::rgb(0x0018_5943).into();
                theme.colors.primary_active = gpui_kit::rgb(0x0012_4735).into();
                theme.colors.primary_foreground = gpui_kit::rgb(0x00ff_ffff).into();
            }
            gpui_kit::component::Theme::sync_base(cx);
            let bounds = WindowBounds::centered(size(px(1100.), px(760.)), cx);
            cx.spawn(async move |cx| {
                let result = cx.open_window(
                    WindowOptions {
                        window_bounds: Some(bounds),
                        window_min_size: Some(size(px(860.), px(580.))),
                        titlebar: Some(gpui_kit::TitlebarOptions {
                            title: Some("xlatch".into()),
                            ..Default::default()
                        }),
                        app_id: Some("com.byteowlz.xlatch.desktop".into()),
                        ..Default::default()
                    },
                    |window, cx| {
                        let view = cx.new(|cx| {
                            ui::Desktop::new(directory, options.quick, options.tray, window, cx)
                        });
                        cx.new(|cx| Root::new(view, window, cx))
                    },
                );
                if let Err(error) = result {
                    log::error!("Cannot open xlatch: {error:#}");
                    cx.update(|cx| cx.quit());
                }
            })
            .detach();
        });
    Ok(())
}
