//! R2 Operate prototype: integrated chrome, adjacent discovery and a roomy composer.
//! Palette and all effects come from the existing model/transport; no source picker.
use super::{Desktop, Page, TITLEBAR_HEIGHT, WINDOW_RADIUS, icons};
use crate::appearance::Paint;
use rune::{
    Color, Cx, El, Key, KeyEvent, ScrollSpec, TextEdit, Tx, col, el, row, text, text_input,
    tok::{Family, TextStyle},
};
use xlatch_core::{
    capability::{Capability, Job, Request},
    local::Control,
};

// Shared byteowlz desktop language (fdbk toolbar, faceman dock): a quiet black
// ground, floating rounded surfaces, pill controls, a 12/13-point type scale and
// white-on-black selection. Colours are House/xlatch roles only; panel corners still
// derive from xlatch's own radius dial, controls are pills sized to their height.
const BODY: f32 = 13.;
const META: f32 = 12.;
const CONTROL: f32 = 30.;

fn label(value: impl Into<String>, size: f32, color: Color) -> El {
    text(value)
        .font(TextStyle::new(size, 400.))
        .color(color)
        .wrap()
}
fn heading(value: &str, size: f32, p: Paint) -> El {
    text(value)
        .font(TextStyle::new(size, 600.))
        .color(p.fg)
        .wrap()
}
/// Corner radius of a floating surface, from the palette's radius dial (8 -> 12).
fn panel(p: Paint) -> f32 {
    p.radius * 3.
}
/// A floating surface: one step up from the ground, hairline edge, rounded.
fn surface(p: Paint) -> El {
    col()
        .fill(p.sidebar)
        .border(1., p.fg.alpha(0.06))
        .radius(panel(p))
}
fn dot(color: Color) -> El {
    el().size(7., 7.).radius(3.5).fill(color).a11y_hidden()
}
const fn scroll() -> ScrollSpec {
    ScrollSpec {
        stick_bottom: false,
    }
}

/// Shared pill chrome: a labelled, focusable capsule sized to its height.
fn pill(key: &str, title: &str, height: f32, fill: Color, hover: Color, ink: Color) -> El {
    el().key(key)
        .role("button")
        .label(title)
        .h(height)
        .px(if height < CONTROL { 12. } else { 14. })
        .center()
        .radius(height / 2.)
        .fill(fill)
        .hover(hover)
        .child(
            text(title)
                .font(TextStyle::new(BODY, 500.))
                .color(ink)
                .single_line()
                .a11y_hidden(),
        )
}
/// Pill button. Primary is the inverted chip (foreground fill), as in faceman/fdbk;
/// colour accents stay for status, not for every call to action.
fn button(
    key: &str,
    title: &str,
    p: Paint,
    enabled: bool,
    primary: bool,
    action: impl Fn(&mut Desktop, &mut Tx) + 'static,
) -> El {
    let (fill, hover, ink) = if primary {
        (p.fg, p.fg.alpha(0.86), p.bg)
    } else {
        (p.control, p.hover, p.fg)
    };
    pill(key, title, CONTROL, fill, hover, ink)
        .focusable(enabled)
        .disabled(!enabled)
        .opacity(if enabled { 1. } else { 0.4 })
        .focus_ring(2., 2., p.fg)
        .on_press_tx(move |state, tx| {
            if enabled {
                action(state, tx);
            }
        })
}
fn icon_button(
    key: &str,
    title: &str,
    glyph: &str,
    p: Paint,
    enabled: bool,
    action: impl Fn(&mut Desktop, &mut Tx) + 'static,
) -> El {
    el().key(key)
        .role("button")
        .label(title)
        .help(title)
        .focusable(enabled)
        .disabled(!enabled)
        .size(CONTROL, CONTROL)
        .center()
        .radius(CONTROL / 2.)
        .hover(p.hover)
        .focus_ring(2., 2., p.fg)
        .opacity(if enabled { 1. } else { 0.4 })
        .on_press_tx(move |state, tx| {
            if enabled {
                action(state, tx);
            }
        })
        .child(icons::symbol(glyph, p.muted))
}
/// One option of a segmented pill group; the chosen one is the inverted chip.
fn segment(
    key: &str,
    title: &str,
    selected: bool,
    p: Paint,
    action: impl Fn(&mut Desktop, &mut Tx) + 'static,
) -> El {
    let (fill, ink) = if selected {
        (p.fg, p.bg)
    } else {
        (p.fg.alpha(0.), p.muted)
    };
    pill(
        key,
        title,
        CONTROL - 6.,
        fill,
        if selected { p.fg } else { p.hover },
        ink,
    )
    .checked(selected)
    .focusable(true)
    .focus_ring(2., 1., p.fg)
    .on_press_tx(action)
}
fn segments(p: Paint) -> El {
    row()
        .gap(2.)
        .padding(3.)
        .center_y()
        .radius(CONTROL / 2.)
        .fill(p.control)
}
fn field(edit: &TextEdit, key: &str, title: &str, p: Paint) -> El {
    text_input(edit, TextStyle::new(BODY, 400.))
        .key(key)
        .label(title)
        .color(p.fg)
        .caret(p.fg, p.hover)
        .placeholder(title, p.muted)
        .px(14.)
        .py(8.)
        .radius(CONTROL / 2.)
        .fill(p.control)
        .border(1., p.control)
        .focus_ring(2., 1., p.fg.alpha(0.5))
        .min_w(0.)
        .on_input_key(on_key)
}

/// Offscreen stand-in for the native controls, at the same geometry
/// (`xlatch_window_controls::Layout`): 24-point targets 2 points apart from the
/// header's 16-point leading padding.
fn window_preview(p: Paint) -> El {
    let mut controls = row().key("window-controls-preview").gap(2.).center_y();
    for (key, title, glyph) in [
        ("window-close", "Close", "close"),
        ("window-minimize", "Minimize", "minimize"),
        ("window-zoom", "Full Screen", "zoom"),
    ] {
        controls = controls.child(
            icon_button(key, title, glyph, p, true, move |_, tx| match key {
                "window-close" => tx.close_window(),
                "window-minimize" => tx.minimize(),
                _ => tx.toggle_maximized(),
            })
            .size(24., 24.)
            .radius(12.)
            .focusable(false),
        );
    }
    controls
}

fn header(s: &Desktop, p: Paint, cx: &Cx<Desktop>) -> El {
    let insets = cx.chrome_insets();
    // No filled title bar: the chrome sits on the window's own ground, as in faceman.
    let mut header = row()
        .key("app-header")
        .h(TITLEBAR_HEIGHT)
        .w_full()
        .px(16.)
        .gap(12.)
        .center_y();
    if cfg!(target_os = "macos") {
        if s.preview_controls {
            header = header.child(window_preview(p));
        } else {
            // The app-owned native controls sit over this reserved, non-dragging space;
            // the stock traffic lights are hidden, so their inset no longer applies.
            #[cfg(target_os = "macos")]
            let reserved = crate::native_window::layout().end() as f32 - 16.;
            #[cfg(not(target_os = "macos"))]
            let reserved = 0.;
            header = header.child(el().w(reserved).h(32.).a11y_hidden());
        }
    }
    header = header.child(heading("xlatch", 14., p).window_drag().pl(4.).pr(4.));
    let mut nav = segments(p).key("nav");
    for (page, title) in [(Page::Actions, "Actions"), (Page::Activity, "Activity")] {
        nav = nav.child(segment(
            &format!("nav-{title}"),
            title,
            s.page == page,
            p,
            move |s, _| s.page = page,
        ));
    }
    header = header.child(nav).child(
        el().grow(1.)
            .min_w(12.)
            .h(TITLEBAR_HEIGHT)
            .window_drag()
            .a11y_hidden(),
    );
    let authority = if s.connection.is_paired() {
        "Paired"
    } else {
        "Local"
    };
    let (state, tone) = if s.snapshot.is_some() {
        ("Connected", p.primary)
    } else {
        ("Offline", p.muted)
    };
    let title = format!("{authority} · {state}");
    let selected = s.page == Page::Connection;
    header = header
        .child(
            row()
                .key("nav-Connection")
                .role("button")
                .label(&title)
                .help("Connection and authority")
                .checked(selected)
                .focusable(true)
                .h(CONTROL)
                .px(12.)
                .gap(8.)
                .center_y()
                .radius(CONTROL / 2.)
                .fill(if selected { p.control } else { p.fg.alpha(0.) })
                .hover(p.hover)
                .focus_ring(2., 2., p.fg)
                .on_press(|s: &mut Desktop| s.page = Page::Connection)
                .child(dot(tone))
                .child(label(&title, META, p.muted).single_line().a11y_hidden()),
        )
        .child(
            icon_button("nav-Settings", "Settings", "settings", p, true, |s, _| {
                s.page = Page::Settings;
            })
            .fill(if s.page == Page::Settings {
                p.control
            } else {
                p.fg.alpha(0.)
            })
            .checked(s.page == Page::Settings),
        );
    if insets.trailing > 0. {
        header = header.child(el().w((insets.trailing - 16.).max(0.)).a11y_hidden());
    }
    header
}

fn discovery(s: &Desktop, p: Paint) -> El {
    let mut list = col()
        .key("action-list")
        .grow(1.)
        .min_h(0.)
        .gap(2.)
        .scroll(scroll());
    let mut count = 0;
    if let Some(snapshot) = &s.snapshot {
        for capability in snapshot
            .capabilities
            .iter()
            .filter(|capability| xlatch_desktop::matches(capability, &s.search.text))
        {
            count += 1;
            list = list.child(action_row(s, capability, p));
        }
    }
    if count == 0 {
        list = list.child(
            label(
                if s.busy {
                    "Loading actions…"
                } else if s.snapshot.is_none() {
                    "Not connected. Refresh or open Connection."
                } else if s.search.text.trim().is_empty() {
                    "No actions granted yet."
                } else {
                    "No matches."
                },
                META,
                p.muted,
            )
            .px(8.)
            .py(10.),
        );
    }
    surface(p)
        .key("discovery")
        .w_frac(0.27)
        .min_w(224.)
        .max_w(280.)
        .h_full()
        .min_h(0.)
        .gap(10.)
        .padding(10.)
        .child(
            row()
                .center_y()
                .gap(8.)
                .pl(6.)
                .child(heading("Actions", BODY, p).grow(1.))
                .child(icon_button(
                    "refresh",
                    if s.busy { "Working…" } else { "Refresh" },
                    "refresh",
                    p,
                    !s.busy,
                    |s, _| s.work(None),
                )),
        )
        .child(
            field(&s.search, "search", "Find an action…", p)
                .single_line()
                .on_edit(|s: &mut Desktop| &mut s.search)
                .on_submit(|s: &mut Desktop, _| s.search_first())
                .help("Search by name or purpose. Command/Ctrl+K"),
        )
        .child(list)
}
fn action_row(s: &Desktop, capability: &Capability, p: Paint) -> El {
    let selected = s
        .selected
        .as_ref()
        .is_some_and(|action| action.manifest.id == capability.manifest.id);
    let chosen = capability.clone();
    let artwork = s
        .icons
        .get(&(capability.revision.clone(), p.dark))
        .map_or_else(
            || icons::symbol("action", if selected { p.fg } else { p.muted }),
            |image| {
                el().size(18., 18.)
                    .pixels_fit(image.pixels(), rune::SurfaceFit::Fit)
                    .a11y_hidden()
            },
        );
    // Purpose first; the identifier and an inactive status are secondary details.
    let detail = if capability.manifest.description.trim().is_empty() {
        capability.manifest.id.clone()
    } else {
        capability.manifest.description.clone()
    };
    let mut text_column = col().grow(1.).min_w(0.).gap(1.).child(
        text(&capability.manifest.title)
            .font(TextStyle::new(BODY, 500.))
            .color(p.fg)
            .single_line()
            .truncate(),
    );
    text_column = text_column.child(label(detail, META, p.muted).single_line().truncate());
    if capability.status != "active" {
        text_column = text_column.child(label(&capability.status, META, p.warning).single_line());
    }
    row()
        .key(format!("action-{}", capability.manifest.id))
        .role("button")
        .label(&capability.manifest.title)
        .checked(selected)
        .focusable(!s.busy)
        .disabled(s.busy)
        .px(10.)
        .py(8.)
        .gap(10.)
        .center_y()
        .radius(panel(p) - 2.)
        .fill(if selected {
            p.control
        } else {
            p.sidebar.alpha(0.)
        })
        .hover(p.hover)
        .focus_ring(2., 0., p.fg)
        .on_press(move |s: &mut Desktop| s.select(chosen.clone()))
        .child(artwork)
        .child(text_column)
}
fn empty_detail(s: &Desktop, p: Paint) -> El {
    col()
        .key("action-overview")
        .grow(1.)
        .min_w(0.)
        .h_full()
        .center()
        .gap(10.)
        .px(24.)
        .child(heading("Choose an action", 18., p))
        .child(
            label(
                "Pick or search for an action on the left. Nothing is sent until you run it.",
                BODY,
                p.muted,
            )
            .max_w(360.)
            .align(rune::Align::Center),
        )
        .child(
            button(
                "overview-connection",
                if s.snapshot.is_some() {
                    "Review connection"
                } else {
                    "Set up connection"
                },
                p,
                true,
                false,
                |s, _| s.page = Page::Connection,
            )
            .mt(6.),
        )
}
/// A small rounded tag for secondary facts (MIME, short revision).
fn chip(value: impl Into<String>, p: Paint, mono: bool) -> El {
    let style = if mono {
        TextStyle::new(11., 400.).family(Family::SystemMono)
    } else {
        TextStyle::new(META, 500.)
    };
    el().h(22.)
        .px(9.)
        .center()
        .radius(11.)
        .fill(p.control)
        .child(text(value).font(style).color(p.muted).single_line())
}
fn detail(s: &Desktop, p: Paint) -> El {
    let Some(action) = &s.selected else {
        return empty_detail(s, p);
    };
    let short = action.revision.chars().take(8).collect::<String>();
    let accepts = action.manifest.accepts.join(", ");
    let mut panel = col()
        .key("action-detail")
        .grow(1.)
        .min_w(0.)
        .min_h(0.)
        .h_full()
        .gap(12.)
        .pt(4.)
        .child(
            row()
                .gap(12.)
                .items_start()
                .child(heading(&action.manifest.title, 20., p).grow(1.).min_w(0.))
                .child(icon_button(
                    "contract",
                    if s.show_contract {
                        "Back to content"
                    } else {
                        "Inspect contract & execution"
                    },
                    "contract",
                    p,
                    true,
                    |s, _| s.show_contract = !s.show_contract,
                )),
        )
        .child(
            label(&action.manifest.description, BODY, p.muted)
                .max_lines(2)
                .help_when_truncated(&action.manifest.description),
        )
        .child(
            // The exact revision stays one hover (and the contract view) away.
            row()
                .gap(6.)
                .flex_wrap()
                .child(chip(&accepts, p, false).help(format!("Accepts {accepts}")))
                .child(
                    chip(format!("rev {short}"), p, true)
                        .key("revision")
                        .label(format!("Revision {}", action.revision))
                        .help(format!("Revision {}", action.revision)),
                ),
        );
    if action.status == "active" && !s.show_contract {
        panel = panel.child(composer(s, p));
    } else {
        let mut inspect = col().grow(1.).min_h(0.).gap(14.).scroll(scroll());
        if s.show_contract || s.confirm_approval {
            inspect = inspect
                .child(
                    text(format!("Revision {}", action.revision))
                        .font(TextStyle::new(11., 400.).family(Family::SystemMono))
                        .color(p.muted)
                        .wrap_anywhere()
                        .selectable(true),
                )
                .child(code(
                    "manifest",
                    &serde_json::to_string_pretty(&action.manifest).unwrap_or_default(),
                    p,
                    500.,
                ));
        }
        if action.status != "active" && !s.connection.is_paired() {
            inspect = inspect.child(approval(s, action, p));
        }
        panel = panel.child(inspect);
    }
    panel
}
fn editor(s: &Desktop, p: Paint) -> El {
    col()
        .key("editor-viewport")
        .role("label")
        .label("Content editor")
        .h(260.)
        .grow(1.)
        .min_h(120.)
        .radius(panel(p))
        .border(
            1.,
            if s.payload.focused {
                p.fg.alpha(0.35)
            } else {
                p.fg.alpha(0.06)
            },
        )
        .fill(p.sidebar)
        .scroll(scroll())
        .clip_rounded(panel(p))
        .child(
            field(
                &s.payload,
                "payload",
                "Write or paste content for the selected action…",
                p,
            )
            .w_full()
            .min_h(260.)
            .padding(14.)
            .radius(panel(p))
            // The viewport paints the ground and border; an opaque field scrolled
            // under its edges would cover the border.
            .fill(p.bg.alpha(0.))
            .border(0., p.line)
            .focus_ring(0., 0., p.fg)
            .on_edit(|s: &mut Desktop| &mut s.payload)
            .on_submit(|s: &mut Desktop, _| s.payload.insert("\n")),
        )
}

fn composer(s: &Desktop, p: Paint) -> El {
    let mut modes = segments(p).key("input-modes");
    for (raw, title) in [(false, "Text"), (true, "JSON")] {
        let key = if raw == s.raw_input {
            "input-mode-current"
        } else {
            "input-mode"
        };
        modes = modes.child(segment(key, title, raw == s.raw_input, p, move |s, _| {
            if !s.busy {
                s.raw_input = raw;
            }
        }));
    }
    let mut tools = row()
        .gap(8.)
        .center_y()
        .flex_wrap()
        .child(modes)
        .child(el().grow(1.).a11y_hidden())
        .child(button(
            "paste",
            "Paste text",
            p,
            !s.busy,
            false,
            Desktop::paste,
        ));
    if s.draft.is_some() {
        tools = tools.child(button("new-run", "New run", p, !s.busy, false, |s, _| {
            s.draft = None;
            s.result = None;
        }));
    }
    col()
        .key("composer")
        .grow(1.)
        .min_h(0.)
        .gap(10.)
        .min_w(0.)
        .child(tools)
        .child(editor(s, p))
        .child(
            row()
                .gap(12.)
                .center_y()
                .child(
                    label(
                        if s.draft.is_some() {
                            "Same revision, content and run identity."
                        } else {
                            "Nothing is sent until you run this action."
                        },
                        META,
                        p.muted,
                    )
                    .grow(1.)
                    .min_w(0.),
                )
                .child(
                    button(
                        "invoke",
                        if s.draft.is_some() {
                            "Retry / retrieve same run"
                        } else {
                            "Run action"
                        },
                        p,
                        !s.busy && s.snapshot.is_some(),
                        true,
                        |s, _| s.invoke(),
                    )
                    .shortcut_hint(if cfg!(target_os = "macos") {
                        "Cmd+Enter"
                    } else {
                        "Ctrl+Enter"
                    }),
                ),
        )
}
fn approval(s: &Desktop, action: &Capability, p: Paint) -> El {
    let mut panel = col().gap(14.).child(
        button("review", "Review activation", p, !s.busy, false, |s, _| {
            s.confirm_approval = true;
        })
        .self_start(),
    );
    if s.confirm_approval {
        panel=panel.child(label("Approve this exact revision? Command actions run with the executor’s OS permissions. Protected servers require approval from an enrolled phone.",14.,p.fg));
        if action.manifest.executes_commands() {
            panel = panel.child(
                button(
                    "host-consent",
                    if s.host_consent {
                        "Host execution acknowledged"
                    } else {
                        "Acknowledge host execution"
                    },
                    p,
                    !s.busy,
                    false,
                    |s, _| s.host_consent = !s.host_consent,
                )
                .self_start(),
            );
        }
        panel = panel.child(
            button(
                "approve",
                "Approve revision",
                p,
                !s.busy && (!action.manifest.executes_commands() || s.host_consent),
                true,
                |s, _| {
                    if let Some(action) = &s.selected {
                        let request = Control::Approve {
                            id: action.manifest.id.clone(),
                            revision: action.revision.clone(),
                            allow_host_execution: s.host_consent,
                        };
                        s.confirm_approval = false;
                        s.request(request);
                    }
                },
            )
            .self_start(),
        );
    }
    panel
}

fn activity(s: &Desktop, p: Paint) -> El {
    let jobs = s
        .snapshot
        .as_ref()
        .map_or(&[][..], |snapshot| snapshot.jobs.as_slice());
    let mut list = col()
        .key("activity")
        .grow(1.)
        .min_h(0.)
        .gap(8.)
        .scroll(scroll());
    if jobs.is_empty() {
        list = list.child(label(
            if s.snapshot.is_none() {
                "Recent activity is unavailable. Refresh or review Connection."
            } else {
                "No recent jobs. Refresh after running an action."
            },
            BODY,
            p.muted,
        ));
    }
    for job in jobs {
        list = list.child(job_row(s, job, p));
    }
    col()
        .grow(1.)
        .min_h(0.)
        .gap(16.)
        .child(
            row()
                .center_y()
                .child(heading(Page::Activity.title(), 18., p).grow(1.))
                .child(icon_button(
                    "refresh",
                    "Refresh",
                    "refresh",
                    p,
                    !s.busy,
                    |s, _| s.work(None),
                )),
        )
        .child(list)
}
fn job_row(s: &Desktop, job: &Job, p: Paint) -> El {
    let id = job.id.clone();
    let cancel = id.clone();
    let mut item = row()
        .key(format!("job-{id}"))
        .px(14.)
        .py(10.)
        .gap(12.)
        .center_y()
        .fill(p.sidebar)
        .border(1., p.fg.alpha(0.06))
        .radius(panel(p))
        .child(dot(p.status(&job.status)))
        .child(
            col()
                .grow(1.)
                .min_w(0.)
                .gap(2.)
                .child(
                    text(&job.capability_id)
                        .font(TextStyle::new(BODY, 500.))
                        .color(p.fg)
                        .single_line()
                        .truncate(),
                )
                .child(
                    label(format!("{} · {id}", job.status), META, p.muted)
                        .single_line()
                        .truncate()
                        .help(format!("{} · {id}", job.status)),
                ),
        )
        .child(button(
            &format!("result-{id}"),
            "Result",
            p,
            !s.busy,
            false,
            move |s, _| {
                s.request(Control::Rpc {
                    request: Request::Job { id: id.clone() },
                });
            },
        ));
    if matches!(job.status.as_str(), "queued" | "running") {
        item = item.child(button(
            &format!("cancel-{cancel}"),
            "Cancel",
            p,
            !s.busy,
            false,
            move |s, _| {
                s.request(Control::Rpc {
                    request: Request::Cancel { id: cancel.clone() },
                });
            },
        ));
    }
    item
}
fn connection(s: &Desktop, p: Paint) -> El {
    col().key("connection").grow(1.).min_h(0.).max_w(560.).gap(14.).scroll(scroll()).pt(4.)
        .child(heading("Connection",18.,p))
        .child(heading(if s.connection.is_paired() {"Paired server connection"}else{"Local daemon connection"},BODY,p))
        .child(label(if s.connection.is_paired() {"Use the config file created by xlatch-client pair. Pairing and credential changes are available through the CLI."}else{"Use the same data directory as xlatch service run, or the configured control directory. This app does not start or stop the daemon automatically."},BODY,p.muted))
        .child(field(&s.address,"address","Connection path",p).single_line().on_edit(|s: &mut Desktop|&mut s.address).on_submit(|s: &mut Desktop,_|s.connect()))
        .child(button("connect","Connect",p,!s.busy,true,|s,_|s.connect()).self_start())
        .child(label(if s.connection.is_paired() {"Authority: this paired device only. Grants, enrollment approval and job ownership are enforced by the server."}else{"Authority: local operator. The server enforces protected-mode restrictions; the desktop app cannot bypass phone approval."},BODY,p.muted))
        .child(text("xlatch-desktop --client-config /absolute/path/to/config.toml").font(TextStyle::new(11.,400.).family(Family::SystemMono)).color(p.muted).selectable(true).wrap_anywhere())
}
fn settings(s: &Desktop, p: Paint) -> El {
    let mut choices = segments(p).key("appearance").self_start();
    for (choice, title) in [
        ("house", "House"),
        ("auto", "Follow system"),
        ("light", "Light"),
    ] {
        choices = choices.child(segment(
            choice,
            title,
            s.appearance.choice == choice,
            p,
            move |s, _| {
                s.appearance.choice = choice.into();
                s.theme_at = -2.;
                if s.fixture {
                    s.appearance = super::Appearance::new(choice.into());
                    s.appearance.status = "Bundled fixture palette · no user configuration".into();
                }
            },
        ));
    }
    let section = |title: &str, body: &str| {
        surface(p)
            .gap(6.)
            .padding(14.)
            .child(heading(title, BODY, p))
            .child(label(body, BODY, p.muted))
    };
    col().key("settings").grow(1.).min_h(0.).max_w(560.).gap(12.).scroll(scroll()).pt(4.)
        .child(heading("Settings",18.,p))
        .child(surface(p).gap(10.).padding(14.)
            .child(heading("Appearance",BODY,p)).child(choices)
            .child(label(&s.appearance.status,META,p.muted))
            .child(label("House is the byteowlz 2 default. Follow system uses xlatch/theme.toml or theme.json, then Omarchy’s active palette, then House or Light with the system appearance. Theme files reload automatically.",META,p.muted)))
        .child(section("Quick launch","Command/Ctrl+K opens action search. Command/Ctrl+Enter runs the reviewed action. Use --quick from a launcher or system shortcut."))
        .child(section("Optional tray","Use --tray in a tray-enabled build for quick access to search and activity. Closing the window does not stop the server."))
        .child(section("Context stays explicit","Paste text only reads the clipboard when clicked. No background capture is enabled. Context bundles plus prompts are currently available through the paired CLI, not a new source picker."))
}
fn code(key: &str, value: &str, p: Paint, height: f32) -> El {
    col()
        .key(key)
        .max_h(height)
        .min_h(0.)
        .min_w(0.)
        .padding(14.)
        .fill(p.sidebar)
        .border(1., p.fg.alpha(0.06))
        .radius(panel(p))
        .scroll_xy()
        .child(
            text(value)
                .font(TextStyle::new(12., 400.).family(Family::SystemMono))
                .color(p.fg)
                .selectable(true)
                .selection_color(p.hover),
        )
}
/// The latest server response: status dot, identifiers, and its two actions.
fn receipt_summary(s: &Desktop, status: &str, id: &str, p: Paint) -> El {
    let toggle = if s.show_result {
        "Hide details"
    } else {
        "View result"
    };
    row()
        .gap(12.)
        .center_y()
        .child(dot(p.status(status)))
        .child(
            label(format!("{status}  {id}"), META, p.fg)
                .grow(1.)
                .min_w(0.)
                .single_line()
                .truncate(),
        )
        .child(button("inspect-result", toggle, p, true, false, |s, _| {
            s.show_result = !s.show_result;
            if s.show_result && s.page == Page::Actions {
                s.page = Page::Activity;
            }
        }))
        .child(icon_button(
            "copy-result",
            "Copy result",
            "copy",
            p,
            true,
            |s, tx| {
                if let Some(value) = &s.result {
                    tx.copy(value.to_string());
                }
            },
        ))
}
fn receipt(s: &Desktop, p: Paint) -> El {
    let Some(result) = &s.result else {
        return el();
    };
    let status = result["status"].as_str().unwrap_or("Server response");
    let id = result["id"].as_str().unwrap_or("");
    let pane = surface(p)
        .key("server-response")
        .gap(10.)
        .min_h(0.)
        .px(14.)
        .py(8.)
        .child(receipt_summary(s, status, id, p));
    if !s.show_result {
        return pane;
    }
    let body = serde_json::to_string_pretty(result).unwrap_or_default();
    pane.child(code("response", &body, p, 180.))
}

pub fn view(s: &Desktop, cx: &mut Cx<Desktop>) -> El {
    s.system_dark
        .store(cx.system_dark(), std::sync::atomic::Ordering::Relaxed);
    let p = match s.appearance.paint(cx.system_dark()) {
        Ok(p) => p,
        Err(error) => {
            return text(format!("Cannot load appearance: {error:#}"))
                .color(Color::WHITE)
                .fill(Color::BLACK);
        }
    };
    cx.caption_color(p.fg);
    let mut content = col()
        .key("content")
        .grow(1.)
        .min_h(0.)
        .min_w(0.)
        .px(16.)
        .pb(16.)
        .gap(14.);
    for error in [s.error.as_ref(), s.native_error.as_ref()]
        .into_iter()
        .flatten()
    {
        content = content.child(
            label(error, BODY, p.danger)
                .key("error")
                .px(14.)
                .py(10.)
                .fill(p.danger.alpha(0.12))
                .radius(panel(p))
                .role("alert"),
        );
    }
    let page = match s.page {
        Page::Actions => row()
            .key("actions")
            .grow(1.)
            .min_h(0.)
            .min_w(0.)
            .gap(20.)
            .child(discovery(s, p))
            .child(detail(s, p)),
        Page::Activity => activity(s, p),
        Page::Connection => connection(s, p),
        Page::Settings => settings(s, p),
    };
    content = content.child(page);
    if s.result.is_some() {
        content = content.child(receipt(s, p));
    }
    col()
        .key("window")
        .role("window")
        .label("xlatch")
        .w_full()
        .h_full()
        .fill(p.bg)
        .radius(WINDOW_RADIUS)
        .clip_rounded(WINDOW_RADIUS)
        .instant_changes()
        .child(header(s, p, cx))
        .child(content)
}

pub fn on_key(s: &mut Desktop, event: &KeyEvent, tx: &mut Tx) -> bool {
    let primary = event.mods.cmd || event.mods.ctrl;
    if primary && !event.mods.alt && !event.mods.shift {
        match &event.key {
            Key::Command('q' | 'Q') => {
                tx.quit();
                return true;
            }
            Key::Command('k' | 'K') => {
                s.page = Page::Actions;
                s.search.focused = true;
                s.payload.focused = false;
                s.address.focused = false;
                return true;
            }
            Key::Enter if s.page == Page::Actions => {
                s.invoke();
                return true;
            }
            _ => {}
        }
    }
    false
}
