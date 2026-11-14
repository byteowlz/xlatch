//! Native Rune management app. The daemon remains an independent process.
mod appearance;
#[cfg(target_os = "macos")]
mod native_window;
#[cfg(all(feature = "tray", any(target_os = "macos", target_os = "windows")))]
mod tray;
mod ui;

use anyhow::ensure;
use clap::Parser;
use rune::{App, AppSpec, Titlebar, WindowOptions};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(version, about = "Native xlatch management and action launcher")]
struct Options {
    /// Directory containing the local daemon's control.sock.
    #[arg(long, conflicts_with = "client_config")]
    control_dir: Option<PathBuf>,
    /// Use a paired HTTPS identity, not local authority.
    #[arg(long)]
    client_config: Option<PathBuf>,
    /// Start with action search focused.
    #[arg(long)]
    quick: bool,
    /// Show the optional system tray icon (requires the tray build feature).
    #[arg(long)]
    tray: bool,
    /// house (byteowlz 2 default; `dark` is an alias), auto, light, or an
    /// absolute JSON/TOML palette or Studio Rune theme path.
    #[arg(long, default_value = "house")]
    theme: String,
    /// Synthetic review mode: no transports, user theme files or capture sources.
    #[arg(long)]
    fixture: bool,
    /// Rune tooling flags after --; require --fixture to avoid private captures.
    #[arg(last = true)]
    rune: Vec<String>,
}

impl Options {
    fn connection(&self) -> anyhow::Result<xlatch_desktop::Connection> {
        if self.fixture {
            return Ok(if self.client_config.is_some() {
                xlatch_desktop::Connection::Paired("/synthetic/xlatch-client/config.toml".into())
            } else {
                xlatch_desktop::Connection::Local("/synthetic/xlatch".into())
            });
        }
        self.client_config.as_ref().map_or_else(
            || {
                self.control_dir
                    .clone()
                    .map_or_else(xlatch_core::paths::default_data_dir, Ok)
                    .map(xlatch_desktop::Connection::Local)
            },
            |config| Ok(xlatch_desktop::Connection::Paired(config.clone())),
        )
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::try_init()?;
    let options = Options::parse();
    ensure!(
        options.rune.is_empty() || options.fixture,
        "Rune review tooling requires --fixture; private/live captures are not enabled"
    );
    let headless = !options.rune.is_empty();
    let connection = options.connection()?;
    let mut state = ui::Desktop::new(
        connection,
        options.theme,
        options.quick,
        options.fixture,
        options.tray && !headless,
    );
    if options.fixture {
        ui::SCENES[0].1(&mut state);
    }
    state.preview_controls = headless;
    rune::tooling::set_app_id("com.byteowlz.xlatch.desktop");
    let mut spec = AppSpec::new("xlatch", (1040., 720.), ui::Desktop::fixture, ui::view);
    spec.min_size = Some((760., 540.));
    spec.families = ("system-ui", "ui-monospace");
    spec.scenes = ui::SCENES;
    spec.setup = Some(ui::Desktop::setup);
    spec.poll = Some(ui::Desktop::poll);
    spec.on_key = Some(ui::on_key);
    let mut app = App::new(spec)
        .args(options.rune)
        .titlebar(Titlebar::new(ui::TITLEBAR_HEIGHT))
        .window_options(WindowOptions {
            transparent: true,
            ..WindowOptions::default()
        })
        .tab_visits_controls(true)
        .standard_menus()
        .on_close(|state, tx| {
            if !state.has_tray() {
                tx.quit();
            }
        });
    #[cfg(target_os = "macos")]
    {
        let handle = app.handle();
        app = app.on_window(move |window| {
            let window = window.clone();
            handle.update(move |state, _| state.native_window_ready(window));
        });
    }
    state.handle = Some(app.handle());
    app = app.init(move || state);
    if options.tray && !headless {
        app = app.keep_running();
    }
    app.run();
    Ok(())
}
