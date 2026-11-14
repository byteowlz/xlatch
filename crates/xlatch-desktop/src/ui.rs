//! Incumbent management shell in Rune; transport and palette I/O stay off its event loop.
mod fixtures;
mod icons;
mod layout;
#[cfg(test)]
mod tests;

use crate::appearance::{self, Appearance};
use rune::{AppHandle, Color, Image, ImageCache, TextEdit, Tx, Waker};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
use xlatch_core::{
    capability::{Capability, Job, Request},
    local::Control,
};
use xlatch_desktop::{Connection, Draft, Snapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Actions,
    Activity,
    Connection,
    Settings,
}
impl Page {
    #[cfg(test)]
    const ALL: [(Self, &'static str); 4] = [
        (Self::Actions, "Actions"),
        (Self::Activity, "Activity"),
        (Self::Connection, "Connection"),
        (Self::Settings, "Settings"),
    ];
    const fn title(self) -> &'static str {
        match self {
            Self::Actions => "Actions",
            Self::Activity => "Activity",
            Self::Connection => "Connection",
            Self::Settings => "Settings",
        }
    }
}

enum Reply {
    Snapshot(anyhow::Result<Snapshot>),
    Call(anyhow::Result<Value>),
    Appearance {
        choice: String,
        dark: bool,
        value: anyhow::Result<(xlatch_desktop::palette::Palette, String)>,
    },
}

pub struct Desktop {
    connection: Connection,
    snapshot: Option<Snapshot>,
    page: Page,
    selected: Option<Capability>,
    draft: Option<Draft>,
    result: Option<Value>,
    show_result: bool,
    pub preview_controls: bool,
    native_error: Option<String>,
    #[cfg(target_os = "macos")]
    native_window: Option<rune::WindowHandle>,
    #[cfg(target_os = "macos")]
    native_tint: Option<Color>,
    error: Option<String>,
    busy: bool,
    search: TextEdit,
    payload: TextEdit,
    address: TextEdit,
    confirm_approval: bool,
    host_consent: bool,
    raw_input: bool,
    show_contract: bool,
    appearance: Appearance,
    theme_at: f64,
    theme_pending: bool,
    system_dark: Arc<AtomicBool>,
    sender: mpsc::Sender<Reply>,
    receiver: mpsc::Receiver<Reply>,
    waker: Option<Waker>,
    images: Option<ImageCache>,
    icons: HashMap<(String, bool), Image>,
    fixture: bool,
    refresh_pending: bool,
    tray_requested: bool,
    pub handle: Option<AppHandle<Self>>,
    #[cfg(all(feature = "tray", any(target_os = "macos", target_os = "windows")))]
    tray: Option<crate::tray::Tray>,
}

impl Desktop {
    pub fn new(
        connection: Connection,
        choice: String,
        quick: bool,
        fixture: bool,
        tray: bool,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        let address = TextEdit::new(connection.path().to_string_lossy().into_owned());
        let mut search = TextEdit::new("");
        search.focused = quick;
        Self {
            connection,
            snapshot: None,
            page: Page::Actions,
            selected: None,
            draft: None,
            result: None,
            show_result: false,
            preview_controls: false,
            native_error: None,
            #[cfg(target_os = "macos")]
            native_window: None,
            #[cfg(target_os = "macos")]
            native_tint: None,
            error: None,
            busy: false,
            search,
            payload: TextEdit::new(""),
            address,
            confirm_approval: false,
            host_consent: false,
            raw_input: false,
            show_contract: false,
            appearance: Appearance::new(choice),
            theme_at: -2.,
            theme_pending: false,
            system_dark: Arc::new(AtomicBool::new(false)),
            sender,
            receiver,
            waker: None,
            images: None,
            icons: HashMap::new(),
            fixture,
            refresh_pending: !fixture,
            tray_requested: tray,
            handle: None,
            #[cfg(all(feature = "tray", any(target_os = "macos", target_os = "windows")))]
            tray: None,
        }
    }

    /// Safe tooling initializer: never connects, captures or reads user theme/config files.
    pub fn fixture() -> Self {
        let mut state = Self::new(
            Connection::Local("/synthetic/xlatch".into()),
            "dark".into(),
            false,
            true,
            false,
        );
        fixtures::actions(&mut state);
        state.preview_controls = true;
        state
    }

    pub fn setup(&mut self, waker: Waker) {
        self.images = Some(ImageCache::new(waker.clone()));
        self.waker = Some(waker);
    }

    fn work(&mut self, request: Option<Control>) {
        if self.busy {
            return;
        }
        if self.fixture {
            fixtures::request(self, request);
            return;
        }
        self.busy = true;
        self.error = None;
        let connection = self.connection.clone();
        let sender = self.sender.clone();
        let waker = self.waker.clone();
        let worker = std::thread::Builder::new()
            .name("xlatch-client".into())
            .spawn(move || {
                let reply = request.map_or_else(
                    || Reply::Snapshot(connection.snapshot()),
                    |request| Reply::Call(connection.call(request)),
                );
                let _ = sender.send(reply);
                if let Some(waker) = waker {
                    waker();
                }
            });
        if let Err(error) = worker {
            self.busy = false;
            self.error = Some(format!("Cannot start client worker: {error}"));
        }
    }

    fn request(&mut self, request: Control) {
        if self.busy {
            return;
        }
        self.result = None;
        self.show_result = matches!(
            &request,
            Control::Rpc {
                request: Request::Job { .. }
            }
        );
        self.work(Some(request));
    }

    fn select(&mut self, capability: Capability) {
        if self.busy {
            return;
        }
        self.selected = Some(capability);
        self.raw_input = false;
        self.show_contract = false;
        self.draft = None;
        self.result = None;
        self.confirm_approval = false;
        self.host_consent = false;
        self.payload.replace_text("");
    }

    fn search_first(&mut self) {
        let first = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .capabilities
                    .iter()
                    .find(|action| xlatch_desktop::matches(action, &self.search.text))
            })
            .cloned();
        if let Some(action) = first {
            self.select(action);
            self.search.focused = false;
            self.payload.focused = true;
        }
    }

    fn invoke(&mut self) {
        if self.busy || self.snapshot.is_none() {
            return;
        }
        let Some(capability) = &self.selected else {
            return;
        };
        let input = if self.raw_input {
            self.payload.text.clone()
        } else {
            json!({"text":self.payload.text,"mime_type":"text/plain"}).to_string()
        };
        match Draft::prepare(capability, &input, self.draft.as_ref()) {
            Ok(draft) => {
                if self
                    .draft
                    .as_ref()
                    .is_some_and(|previous| previous.id != draft.id)
                {
                    self.error=Some("Input changed. Choose New run explicitly before sending changed content; the previous run's identity is retained.".into());
                    return;
                }
                let request = draft.request();
                self.draft = Some(draft);
                self.request(request);
            }
            Err(error) => self.error = Some(format!("{error:#}")),
        }
    }

    fn paste(&mut self, tx: &mut Tx) {
        if self.busy {
            return;
        }
        tx.read_clipboard(|state: &mut Self, content, _| {
            if content.text().is_empty() {
                state.error = Some("The clipboard does not contain text.".into());
                return;
            }
            let input = if state.raw_input {
                json!({"text":content.text(),"mime_type":"text/plain"}).to_string()
            } else {
                content.text().into()
            };
            state.payload.replace_text(input);
            state.result = None;
        });
    }

    fn connect(&mut self) {
        if self.busy {
            return;
        }
        let path = PathBuf::from(&self.address.text);
        if !path.is_absolute() {
            self.error = Some("Use an absolute directory path.".into());
            return;
        }
        self.connection = if self.connection.is_paired() {
            Connection::Paired(path)
        } else {
            Connection::Local(path)
        };
        self.selected = None;
        self.snapshot = None;
        self.draft = None;
        self.result = None;
        self.work(None);
    }

    fn receive(&mut self, reply: Reply) {
        match reply {
            Reply::Snapshot(response) => {
                self.busy = false;
                match response {
                    Ok(snapshot) => {
                        self.error = None;
                        if self
                            .selected
                            .as_ref()
                            .is_some_and(|selected| !snapshot.capabilities.contains(selected))
                        {
                            self.selected = None;
                            self.error=Some("The selected action changed. Select it again to review the new revision.".into());
                        }
                        self.snapshot = Some(snapshot);
                    }
                    Err(error) => {
                        self.error = Some(format!("{error:#}"));
                        self.snapshot = None;
                    }
                }
            }
            Reply::Call(response) => {
                self.busy = false;
                match response {
                    Ok(value) => {
                        self.result = Some(value);
                        self.work(None);
                    }
                    Err(error) => self.error = Some(format!("{error:#}")),
                }
            }
            Reply::Appearance {
                choice,
                dark,
                value,
            } => {
                self.theme_pending = false;
                if self.appearance.choice == choice
                    && self.system_dark.load(Ordering::Relaxed) == dark
                {
                    self.appearance.update(value);
                } else {
                    self.theme_at = -2.;
                }
            }
        }
    }

    fn theme(&mut self, now: f64) {
        if self.fixture || self.theme_pending || now - self.theme_at < 2. {
            return;
        }
        self.theme_at = now;
        self.theme_pending = true;
        let choice = self.appearance.choice.clone();
        let dark = self.system_dark.load(Ordering::Relaxed);
        let sender = self.sender.clone();
        let waker = self.waker.clone();
        let worker = std::thread::Builder::new()
            .name("xlatch-appearance".into())
            .spawn(move || {
                let value = appearance::resolve(&choice, dark);
                let _ = sender.send(Reply::Appearance {
                    choice,
                    dark,
                    value,
                });
                if let Some(waker) = waker {
                    waker();
                }
            });
        if let Err(error) = worker {
            self.theme_pending = false;
            self.appearance.status = format!("Keeping previous theme: {error}");
        }
    }

    pub const fn has_tray(&self) -> bool {
        #[cfg(all(feature = "tray", any(target_os = "macos", target_os = "windows")))]
        {
            self.tray.is_some()
        }
        #[cfg(not(all(feature = "tray", any(target_os = "macos", target_os = "windows"))))]
        {
            let _ = self;
            false
        }
    }

    fn tray(&mut self) -> bool {
        let mut changed = false;
        if self.tray_requested {
            self.tray_requested = false;
            changed = true;
            #[cfg(all(feature = "tray", any(target_os = "macos", target_os = "windows")))]
            match crate::tray::Tray::new() {
                Ok(tray) => self.tray = Some(tray),
                Err(error) => self.error = Some(format!("Tray unavailable: {error:#}")),
            }
            #[cfg(not(all(feature = "tray", any(target_os = "macos", target_os = "windows"))))]
            {
                self.error=Some("Tray support is unavailable in this build. The management window remains usable.".into());
            }
        }
        #[cfg(all(feature = "tray", any(target_os = "macos", target_os = "windows")))]
        while let Some(action) = crate::tray::next_action() {
            changed = true;
            if let Some(handle) = &self.handle {
                if action == "quit" {
                    handle.quit();
                    continue;
                }
                handle.open();
            }
            match action.as_str() {
                "quick" => {
                    self.page = Page::Actions;
                    self.search.focused = true;
                    self.payload.focused = false;
                }
                "activity" => {
                    self.page = Page::Activity;
                    self.work(None);
                }
                _ => {}
            }
        }
        changed
    }

    fn icons(&mut self) {
        let (Some(images), Some(snapshot)) = (&self.images, &self.snapshot) else {
            return;
        };
        for capability in &snapshot.capabilities {
            for dark in [false, true] {
                let key = (capability.revision.clone(), dark);
                if self.icons.contains_key(&key) {
                    continue;
                }
                if let Some(bytes) = capability
                    .manifest
                    .icon
                    .as_ref()
                    .and_then(|icon| icon.png_bytes_for(dark).ok())
                {
                    self.icons.insert(key, images.load(bytes));
                }
            }
        }
        self.icons.retain(|(revision, _), _| {
            snapshot
                .capabilities
                .iter()
                .any(|capability| &capability.revision == revision)
        });
    }

    #[cfg(target_os = "macos")]
    pub fn native_window_ready(&mut self, window: rune::WindowHandle) {
        self.native_window = Some(window);
        self.native_tint = None;
    }

    #[cfg(target_os = "macos")]
    fn native_chrome(&mut self) -> bool {
        let Some(window) = &self.native_window else {
            return false;
        };
        let Ok(paint) = self
            .appearance
            .paint(self.system_dark.load(Ordering::Relaxed))
        else {
            return false;
        };
        if self.native_tint == Some(paint.fg) {
            return false;
        }
        match crate::native_window::apply(window, paint.fg) {
            Ok(()) => {
                self.native_tint = Some(paint.fg);
                self.native_error = None;
                true
            }
            Err(error) => {
                let message = format!("Native window controls: {error:#}");
                let changed = self.native_error.as_ref() != Some(&message);
                self.native_error = Some(message);
                changed
            }
        }
    }

    pub fn poll(&mut self, now: f64, tx: &mut Tx) -> bool {
        let refreshed = self.refresh_pending;
        if refreshed {
            self.refresh_pending = false;
            self.work(None);
        }
        let mut changed = self.tray() || refreshed;
        while let Ok(reply) = self.receiver.try_recv() {
            self.receive(reply);
            changed = true;
        }
        self.theme(now);
        self.icons();
        #[cfg(target_os = "macos")]
        {
            changed |= self.native_chrome();
        }
        if !self.fixture || self.has_tray() {
            tx.wake_after(Duration::from_millis(if self.has_tray() {
                150
            } else {
                2000
            }));
        }
        changed
    }
}

pub use layout::{on_key, view};

pub const TITLEBAR_HEIGHT: f32 = 52.;
pub const WINDOW_RADIUS: f32 = 12.;

type Scene = (&'static str, fn(&mut Desktop));
pub const SCENES: &[Scene] = &[
    ("actions", fixtures::actions),
    ("activity", fixtures::activity),
    ("connection", fixtures::connection),
    ("settings", fixtures::settings),
    ("pending", fixtures::pending),
    ("paired", fixtures::paired),
    ("offline", fixtures::offline),
    ("selected", fixtures::selected),
    ("busy", fixtures::busy),
    ("unknown", fixtures::unknown),
    ("empty", fixtures::empty),
    ("long", fixtures::long),
    ("contract", fixtures::contract),
    ("result", fixtures::result),
];
