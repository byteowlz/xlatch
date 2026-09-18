//! Management shell; transport work runs off the GPUI event loop.
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    px,
};
use serde_json::{Value, json};
use std::path::PathBuf;
use xlatch_core::{
    capability::{Capability, Execution, Job, Request},
    local::Control,
};
use xlatch_desktop::{Draft, Snapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Actions,
    Activity,
    Connection,
    Settings,
}

pub struct Desktop {
    directory: PathBuf,
    snapshot: Option<Snapshot>,
    page: Page,
    selected: Option<Capability>,
    draft: Option<Draft>,
    result: Option<Value>,
    error: Option<String>,
    busy: bool,
    search: Entity<InputState>,
    payload: Entity<TextareaState>,
    address: Entity<InputState>,
    confirm_approval: bool,
    host_consent: bool,
    raw_input: bool,
    show_contract: bool,
    _subscriptions: Vec<Subscription>,
    #[cfg(all(feature = "tray", any(target_os = "macos", target_os = "windows")))]
    tray: Option<crate::tray::Tray>,
}

impl Desktop {
    pub fn new(
        directory: PathBuf,
        quick: bool,
        tray: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Find an action by name or purpose…")
        });
        let payload = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Write or paste content for the selected action…")
        });
        let address = cx.new(|cx| {
            InputState::new(window, cx).default_value(directory.to_string_lossy().into_owned())
        });
        let subscription = cx.subscribe_in(
            &search,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let query = this.search.read(cx).value();
                    let first = this
                        .snapshot
                        .as_ref()
                        .and_then(|snapshot| {
                            snapshot
                                .capabilities
                                .iter()
                                .find(|action| xlatch_desktop::matches(action, &query))
                        })
                        .cloned();
                    if let Some(action) = first {
                        this.select(action, window, cx);
                        this.payload.update(cx, |input, cx| input.focus(window, cx));
                    }
                }
                cx.notify();
            },
        );
        if quick {
            search.update(cx, |input, cx| input.focus(window, cx));
        }
        let mut app = Self {
            directory,
            snapshot: None,
            page: Page::Actions,
            selected: None,
            draft: None,
            result: None,
            error: None,
            busy: false,
            search,
            payload,
            address,
            confirm_approval: false,
            host_consent: false,
            raw_input: false,
            show_contract: false,
            _subscriptions: vec![subscription],
            #[cfg(all(feature = "tray", any(target_os = "macos", target_os = "windows")))]
            tray: None,
        };
        app.refresh(cx);
        app.install_tray(tray, window, cx);
        let has_tray = {
            #[cfg(all(feature = "tray", any(target_os = "macos", target_os = "windows")))]
            {
                app.tray.is_some()
            }
            #[cfg(not(all(feature = "tray", any(target_os = "macos", target_os = "windows"))))]
            {
                false
            }
        };
        window.on_window_should_close(cx, move |_, cx| {
            if has_tray {
                cx.hide();
            } else {
                cx.quit();
            }
            false
        });
        app
    }

    fn install_tray(&mut self, enabled: bool, window: &Window, cx: &Context<'_, Self>) {
        if !enabled {
            return;
        }
        #[cfg(all(feature = "tray", any(target_os = "macos", target_os = "windows")))]
        {
            match crate::tray::Tray::new() {
                Ok(tray) => self.tray = Some(tray),
                Err(error) => {
                    self.error = Some(format!("Tray unavailable: {error:#}"));
                    return;
                }
            }
            cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(150))
                        .await;
                    while let Some(action) = crate::tray::next_action() {
                        let result = this.update_in(cx, |this, window, cx| {
                            this.tray_action(&action, window, cx);
                            cx.notify();
                        });
                        if result.is_err() {
                            return;
                        }
                    }
                    if this.upgrade().is_none() {
                        return;
                    }
                }
            })
            .detach();
        }
        #[cfg(not(all(feature = "tray", any(target_os = "macos", target_os = "windows"))))]
        {
            let _ = (window, cx);
            self.error = Some(
                "Tray support is unavailable in this build. The management window remains usable."
                    .into(),
            );
        }
    }

    #[cfg(all(feature = "tray", any(target_os = "macos", target_os = "windows")))]
    fn tray_action(&mut self, action: &str, window: &mut Window, cx: &mut Context<'_, Self>) {
        if action == "quit" {
            cx.quit();
            return;
        }
        match action {
            "activity" => {
                self.page = Page::Activity;
                self.refresh(cx);
            }
            "quick" => {
                self.page = Page::Actions;
                self.search.update(cx, |input, cx| input.focus(window, cx));
            }
            _ => {}
        }
        cx.activate(true);
        window.activate_window();
    }

    fn refresh(&mut self, cx: &mut Context<'_, Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        let directory = self.directory.clone();
        cx.spawn(async move |this, cx| {
            let response = cx.background_spawn(async move { xlatch_desktop::snapshot(&directory) }).await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match response {
                    Ok(snapshot) => {
                        this.error = None;
                        if let Some(selected) = &this.selected
                            && !snapshot.capabilities.iter().any(|c| c == selected) {
                                this.selected = None;
                                this.error = Some("The selected action changed. Select it again to review the new revision.".into());
                        }
                        this.snapshot = Some(snapshot);
                    },
                    Err(error) => { this.error = Some(format!("{error:#}")); this.snapshot = None; }
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    fn request(&mut self, request: Control, cx: &mut Context<'_, Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        self.result = None;
        let directory = self.directory.clone();
        cx.spawn(async move |this, cx| {
            let response = cx
                .background_spawn(async move { xlatch_desktop::call(&directory, request) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match response {
                    Ok(value) => {
                        this.result = Some(value);
                        this.refresh(cx);
                    }
                    Err(error) => this.error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn select(&mut self, capability: Capability, window: &mut Window, cx: &mut Context<'_, Self>) {
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
        self.payload
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    fn invoke(&mut self, cx: &mut Context<'_, Self>) {
        let Some(capability) = &self.selected else {
            return;
        };
        let text = self.payload.read(cx).value();
        let input = if self.raw_input {
            text.to_string()
        } else {
            json!({"text":text.as_str(),"mime_type":"text/plain"}).to_string()
        };
        match Draft::prepare(capability, &input, self.draft.as_ref()) {
            Ok(draft) => {
                let request = draft.request();
                self.draft = Some(draft);
                self.request(request, cx);
            }
            Err(error) => self.error = Some(format!("{error:#}")),
        }
        cx.notify();
    }

    fn paste(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let input = if self.raw_input {
                json!({"text":text,"mime_type":"text/plain"}).to_string()
            } else {
                text
            };
            self.payload
                .update(cx, |state, cx| state.set_value(input, window, cx));
            self.draft = None;
            self.result = None;
        } else {
            self.error = Some("The clipboard does not contain text.".into());
        }
        cx.notify();
    }

    fn sidebar(&self, cx: &Context<'_, Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let pages = [
            (
                Page::Actions,
                "Actions",
                gpui_kit::assets::IconName::LayoutGrid,
            ),
            (
                Page::Activity,
                "Activity",
                gpui_kit::assets::IconName::Activity,
            ),
            (
                Page::Connection,
                "Connection",
                gpui_kit::assets::IconName::Server,
            ),
            (
                Page::Settings,
                "Settings",
                gpui_kit::assets::IconName::Settings,
            ),
        ];
        v_flex()
            .w(px(200.))
            .flex_shrink_0()
            .h_full()
            .p_4()
            .gap_2()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .child(
                div()
                    .text_2xl()
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .mb_4()
                    .child("xlatch"),
            )
            .children(
                pages
                    .into_iter()
                    .enumerate()
                    .map(|(i, (page, label, icon))| {
                        Button::new(("nav", i))
                            .ghost()
                            .w_full()
                            .justify_start()
                            .icon(gpui_kit::component::Icon::new(icon))
                            .label(label)
                            .bg(if self.page == page {
                                theme.sidebar_accent
                            } else {
                                theme.sidebar
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.page = page;
                                cx.notify();
                            }))
                    }),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Local operator"),
            )
            .child(div().text_sm().child(if self.snapshot.is_some() {
                "Connected"
            } else {
                "Not connected"
            }))
    }

    fn actions(&self, cx: &Context<'_, Self>) -> impl IntoElement + use<> {
        let query = self.search.read(cx).value();
        let actions = self
            .snapshot
            .as_ref()
            .map(|s| s.capabilities.clone())
            .unwrap_or_default();
        let theme = cx.theme().clone();
        h_flex()
            .flex_1()
            .min_h_0()
            .items_start()
            .gap_6()
            .child(
                v_flex()
                    .w(px(250.))
                    .flex_shrink_0()
                    .h_full()
                    .gap_3()
                    .child(Input::new(&self.search))
                    .child(
                        div()
                            .id("action-list")
                            .overflow_y_scroll()
                            .flex_1()
                            .children(
                                actions
                                    .into_iter()
                                    .filter(|c| xlatch_desktop::matches(c, &query))
                                    .map(|capability| {
                                        let selected = self.selected.as_ref().is_some_and(|s| {
                                            s.manifest.id == capability.manifest.id
                                        });
                                        v_flex()
                                            .id(SharedString::from(capability.manifest.id.clone()))
                                            .px_3()
                                            .py_3()
                                            .gap_1()
                                            .mb_1()
                                            .rounded(cx.theme().radius * 1.5)
                                            .cursor_pointer()
                                            .hover(|style| style.bg(theme.list_hover))
                                            .bg(if selected {
                                                theme.secondary
                                            } else {
                                                theme.background
                                            })
                                            .child(SharedString::from(
                                                capability.manifest.title.clone(),
                                            ))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(theme.muted_foreground)
                                                    .child(SharedString::from(format!(
                                                        "{} · {}",
                                                        capability.manifest.id, capability.status
                                                    ))),
                                            )
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.select(capability.clone(), window, cx);
                                            }))
                                    }),
                            ),
                    ),
            )
            .child(self.action_detail(cx))
    }

    fn action_detail(&self, cx: &Context<'_, Self>) -> gpui_kit::AnyElement {
        let Some(action) = &self.selected else {
            return v_flex()
                .flex_1()
                .gap_3()
                .pt_8()
                .child(div().text_xl().child("Choose an action"))
                .min_w_0()
                .child(div().text_color(cx.theme().muted_foreground).child("Choose where to send your content. Search by name or purpose, then press Enter."))
                .child(div().text_sm().text_color(cx.theme().muted_foreground).child("Command / Ctrl + K opens search. Pending actions need your review."))
                .into_any_element();
        };
        let theme = cx.theme().clone();
        let needs_host_consent = matches!(action.manifest.execution, Execution::Command { .. });
        v_flex().id("action-detail").flex_1().min_w_0().h_full().overflow_y_scroll().gap_3().whitespace_normal()
            .child(div().text_2xl().font_weight(gpui_kit::FontWeight::SEMIBOLD).child(SharedString::from(action.manifest.title.clone())))
            .child(div().w_full().text_color(theme.muted_foreground).child(SharedString::from(action.manifest.description.clone())))
            .child(div().id("revision").overflow_x_scroll().text_xs().text_color(theme.muted_foreground).child(SharedString::from(format!("Revision {}",action.revision))))
            .child(div().text_sm().child(SharedString::from(format!("Accepts {}",action.manifest.accepts.join(", ")))))
            .when(action.status == "active", |view| view
                .child(h_flex().flex_wrap().gap_2().child(div().flex_1().child(if self.raw_input { "JSON input" } else { "Text to send" }))
                    .child(Button::new("input-mode").small().label(if self.raw_input { "Use text" } else { "Use JSON" }).on_click(cx.listener(|this,_,_,cx| {this.raw_input = !this.raw_input;this.draft=None;cx.notify();})))
                    .child(Button::new("paste").small().label("Paste text").on_click(cx.listener(|this,_,window,cx| this.paste(window,cx))))
                    .child(Button::new("new-run").small().label("New run").on_click(cx.listener(|this,_,_,cx| {this.draft=None;this.result=None;cx.notify();}))))
                .child(Textarea::new(&self.payload).h(px(160.)))
                .child(Button::new("invoke").primary().label(if self.draft.is_some() {"Retry / retrieve same run"} else {"Run action"}).disabled(self.busy || self.snapshot.is_none()).on_click(cx.listener(|this,_,_,cx| this.invoke(cx))))
                .child(div().text_xs().text_color(theme.muted_foreground).child("Retries keep the same job identity. Choose New run to execute again.")))
            .child(Button::new("contract").small().label(if self.show_contract { "Hide contract" } else { "Inspect contract & execution" }).on_click(cx.listener(|this,_,_,cx| {this.show_contract = !this.show_contract;cx.notify();})))
            .when(self.show_contract || self.confirm_approval, |view| view.child(div().id("manifest").w_full().p_3().bg(theme.sidebar).rounded(theme.radius * 1.5).font_family(theme.mono_font_family.clone()).max_h(px(260.)).overflow_y_scroll().overflow_x_scroll().text_xs().child(SharedString::from(serde_json::to_string_pretty(&action.manifest).unwrap_or_default()))))
            .when(action.status != "active", |view| view
                .child(Button::new("review").label("Review activation").disabled(self.busy).on_click(cx.listener(|this,_,_,cx| {this.confirm_approval=true;cx.notify();})))
                .when(self.confirm_approval, |view| view
                    .child("Approve this exact revision? Command actions run with the executor’s OS permissions. Protected servers require approval from an enrolled phone.")
                    .when(needs_host_consent, |view| view.child(Button::new("host-consent").label(if self.host_consent {"Host execution acknowledged"} else {"Acknowledge host execution"}).on_click(cx.listener(|this,_,_,cx| {this.host_consent = !this.host_consent;cx.notify();}))))
                    .child(Button::new("approve").label("Approve revision").disabled(self.busy || (needs_host_consent && !self.host_consent)).on_click(cx.listener(|this,_,_,cx| {
                        if let Some(action)=&this.selected {
                            let request=Control::Approve{id:action.manifest.id.clone(),revision:action.revision.clone(),allow_host_execution:this.host_consent};
                            this.confirm_approval=false;this.request(request,cx);
                        }
                    })))))
            .into_any_element()
    }

    fn activity(&self, cx: &Context<'_, Self>) -> impl IntoElement + use<> {
        let jobs = self
            .snapshot
            .as_ref()
            .map(|s| s.jobs.clone())
            .unwrap_or_default();
        v_flex()
            .id("activity")
            .flex_1()
            .overflow_y_scroll()
            .gap_2()
            .when(jobs.is_empty(), |view| {
                view.child("No recent jobs. Refresh after running an action.")
            })
            .children(jobs.into_iter().map(|job| Self::job_row(job, cx)))
    }

    fn job_row(job: Job, cx: &Context<'_, Self>) -> impl IntoElement + use<> {
        let id = job.id.clone();
        let cancellable = matches!(job.status.as_str(), "queued" | "running");
        h_flex()
            .p_3()
            .gap_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                v_flex()
                    .flex_1()
                    .child(SharedString::from(job.capability_id))
                    .child(
                        div()
                            .text_xs()
                            .child(SharedString::from(format!("{} · {}", job.status, job.id))),
                    ),
            )
            .child(
                Button::new(SharedString::from(format!("result-{id}")))
                    .small()
                    .label("Result")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.request(
                            Control::Rpc {
                                request: Request::Job { id: id.clone() },
                            },
                            cx,
                        );
                    })),
            )
            .when(cancellable, |view| {
                view.child(
                    Button::new(SharedString::from(format!("cancel-{}", job.id)))
                        .small()
                        .label("Cancel")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.request(
                                Control::Rpc {
                                    request: Request::Cancel { id: job.id.clone() },
                                },
                                cx,
                            );
                        })),
                )
            })
    }

    fn connection(&self, cx: &Context<'_, Self>) -> impl IntoElement + use<> {
        v_flex().gap_4().max_w(px(620.))
            .child("Local daemon connection")
            .child("Use the same data directory as xlatch service run, or the configured control directory. This app does not start or stop the daemon automatically.")
            .child(Input::new(&self.address))
            .child(Button::new("connect").label("Connect").disabled(self.busy).on_click(cx.listener(|this,_,_,cx| {
                let path=PathBuf::from(this.address.read(cx).value().as_str());
                if !path.is_absolute() {this.error=Some("Use an absolute directory path.".into());cx.notify();return;}
                this.directory=path;this.selected=None;this.snapshot=None;this.draft=None;this.result=None;this.refresh(cx);
            })))
            .child("Authority: local operator. The server enforces protected-mode restrictions; the desktop app cannot bypass phone approval.")
            .child("Paired remote connections are not available in this first desktop build.")
    }

    fn settings(cx: &Context<'_, Self>) -> impl IntoElement + use<> {
        let appearance = cx.global::<crate::appearance::Appearance>();
        v_flex().gap_4().max_w(px(620.))
            .child(div().text_lg().font_weight(gpui_kit::FontWeight::SEMIBOLD).child("Appearance"))
            .child(h_flex().gap_2().children([("auto","Follow system"),("light","Light"),("dark","Dark")].into_iter().map(|(choice,label)| {
                Button::new(choice).label(label).when(appearance.choice == choice, Button::primary).on_click(cx.listener(move |_,_,_,cx| crate::appearance::select(choice,cx)))
            })))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(appearance.status.clone()))
            .child("Automatic mode follows xlatch/theme.toml or theme.json, then Omarchy’s active palette, then system light/dark. Theme files reload automatically.")
            .child(div().pt_4().text_lg().font_weight(gpui_kit::FontWeight::SEMIBOLD).child("Quick launch"))
            .child("Press Command/Ctrl+K to jump to action search. Use --quick when launching from omni or a system shortcut.")
            .child("Optional tray")
            .child("Start with --tray in a tray-enabled build for quick access to search and activity. Closing the window does not stop the server.")
            .child("Context stays explicit")
            .child("Paste text only reads the clipboard when clicked. ctx captures and omni shortcuts will connect through reviewed inputs; no background capture is enabled.")
    }
}

impl Render for Desktop {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        h_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .text_sm()
            .on_key_down(
                cx.listener(|this, event: &gpui_kit::KeyDownEvent, window, cx| {
                    if (event.keystroke.modifiers.platform || event.keystroke.modifiers.control)
                        && event.keystroke.key == "k"
                    {
                        this.page = Page::Actions;
                        this.search.update(cx, |state, cx| state.focus(window, cx));
                        cx.notify();
                    }
                }),
            )
            .child(self.sidebar(cx))
            .child(
                v_flex()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .p_6()
                    .gap_4()
                    .child(
                        h_flex()
                            .gap_3()
                            .child(div().flex_1().text_xl().child(match self.page {
                                Page::Actions => "Actions",
                                Page::Activity => "Activity",
                                Page::Connection => "Connection",
                                Page::Settings => "Settings",
                            }))
                            .child(
                                Button::new("refresh")
                                    .label(if self.busy { "Working…" } else { "Refresh" })
                                    .disabled(self.busy)
                                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                            ),
                    )
                    .when_some(self.error.clone(), |view, error| {
                        view.child(
                            div()
                                .p_3()
                                .border_1()
                                .border_color(theme.danger)
                                .text_color(theme.danger)
                                .child(error),
                        )
                    })
                    .child(match self.page {
                        Page::Actions => self.actions(cx).into_any_element(),
                        Page::Activity => self.activity(cx).into_any_element(),
                        Page::Connection => self.connection(cx).into_any_element(),
                        Page::Settings => Self::settings(cx).into_any_element(),
                    })
                    .when_some(self.result.clone(), |view, result| {
                        view.child(
                            v_flex()
                                .gap_2()
                                .max_h(px(200.))
                                .child(h_flex().gap_3().child("Server response").child(
                                    Button::new("copy-result").small().label("Copy").on_click(
                                        cx.listener(|this, _, _, cx| {
                                            if let Some(value) = &this.result {
                                                cx.write_to_clipboard(
                                                    gpui_kit::ClipboardItem::new_string(
                                                        value.to_string(),
                                                    ),
                                                );
                                            }
                                        }),
                                    ),
                                ))
                                .child(
                                    div()
                                        .id("response")
                                        .p_3()
                                        .bg(theme.sidebar)
                                        .font_family(theme.mono_font_family.clone())
                                        .overflow_y_scroll()
                                        .overflow_x_scroll()
                                        .text_xs()
                                        .child(SharedString::from(
                                            serde_json::to_string_pretty(&result)
                                                .unwrap_or_default(),
                                        )),
                                ),
                        )
                    }),
            )
    }
}
