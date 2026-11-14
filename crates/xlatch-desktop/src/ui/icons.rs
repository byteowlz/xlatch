//! Small native vector marks preserving the incumbent navigation concepts.
use rune::{Color, El, Icon, icon};

static ACTIONS: Icon = Icon {
    name: "actions",
    view: 24.,
    body: r#"<rect x="3" y="3" width="7" height="7" rx="1" fill="none" stroke="currentColor" stroke-width="2"/><rect x="14" y="3" width="7" height="7" rx="1" fill="none" stroke="currentColor" stroke-width="2"/><rect x="3" y="14" width="7" height="7" rx="1" fill="none" stroke="currentColor" stroke-width="2"/><rect x="14" y="14" width="7" height="7" rx="1" fill="none" stroke="currentColor" stroke-width="2"/>"#,
};
static ACTIVITY: Icon = Icon {
    name: "activity",
    view: 24.,
    body: r#"<path d="M2 12h5l3-7 4 14 3-7h5" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>"#,
};
static CONNECTION: Icon = Icon {
    name: "connection",
    view: 24.,
    body: r#"<rect x="3" y="3" width="18" height="7" rx="2" fill="none" stroke="currentColor" stroke-width="2"/><rect x="3" y="14" width="18" height="7" rx="2" fill="none" stroke="currentColor" stroke-width="2"/><circle cx="7" cy="6.5" r="1"/><circle cx="7" cy="17.5" r="1"/>"#,
};
static SETTINGS: Icon = Icon {
    name: "settings",
    view: 24.,
    body: r#"<circle cx="12" cy="12" r="7" fill="none" stroke="currentColor" stroke-width="2"/><circle cx="12" cy="12" r="2.5" fill="none" stroke="currentColor" stroke-width="2"/><path d="M12 2v3m0 14v3M2 12h3m14 0h3M5 5l2 2m10 10 2 2M5 19l2-2M17 7l2-2" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>"#,
};

static REFRESH: Icon = Icon {
    name: "refresh",
    view: 24.,
    body: r#"<path d="M20 10a8 8 0 1 0-2 8M20 4v6h-6" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>"#,
};
static CONTRACT: Icon = Icon {
    name: "contract",
    view: 24.,
    body: r#"<rect x="5" y="3" width="14" height="18" rx="2" fill="none" stroke="currentColor" stroke-width="2"/><path d="M9 8h6M9 12h6M9 16h4" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>"#,
};
static COPY: Icon = Icon {
    name: "copy",
    view: 24.,
    body: r#"<rect x="8" y="8" width="12" height="13" rx="2" fill="none" stroke="currentColor" stroke-width="2"/><path d="M15 8V3H3v13h5" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"/>"#,
};
static CLOSE: Icon = Icon {
    name: "close",
    view: 24.,
    body: r#"<path d="m6 6 12 12M6 18 18 6" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>"#,
};
static MINIMIZE: Icon = Icon {
    name: "minimize",
    view: 24.,
    body: r#"<path d="M5 12h14" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>"#,
};
static ZOOM: Icon = Icon {
    name: "zoom",
    view: 24.,
    body: r#"<path d="M5 11V5h6M13 19h6v-6M5 5l6 6m2 2 6 6" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

pub(super) fn symbol(name: &str, color: Color) -> El {
    let mark = match name {
        "settings" => &SETTINGS,
        "refresh" => &REFRESH,
        "contract" => &CONTRACT,
        "copy" => &COPY,
        "close" => &CLOSE,
        "minimize" => &MINIMIZE,
        "zoom" => &ZOOM,
        "activity" => &ACTIVITY,
        "connection" => &CONNECTION,
        _ => &ACTIONS,
    };
    icon(mark, 16.).color(color).a11y_hidden()
}
