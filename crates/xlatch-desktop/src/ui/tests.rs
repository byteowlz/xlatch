//! Rune runtime/input proofs, separate from pinned HTTPS protocol and native AX proof.
use super::*;
use anyhow::{Context as _, Result, ensure};
use rune::{Env, Key, KeyEvent, Mods, Runtime};

fn runtime() -> Runtime<Desktop> {
    let mut runtime = Runtime::new(Desktop::fixture(), view, (1100., 760.), Env::new(true, 1.));
    runtime.on_key = Some(on_key);
    runtime.frame(0.);
    runtime
}

fn click(runtime: &mut Runtime<Desktop>, key: &str) -> Result<()> {
    let (x, y) = runtime
        .find(key)
        .with_context(|| format!("missing control {key}"))?;
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
    runtime.frame(0.);
    Ok(())
}

#[test]
fn pages_search_composition_and_uncertain_identity_survive_rune_input() -> Result<()> {
    let mut runtime = runtime();
    for (page, title) in Page::ALL {
        click(&mut runtime, &format!("nav-{title}"))?;
        ensure!(runtime.state.page == page);
    }
    runtime.key(KeyEvent {
        key: Key::Command('k'),
        mods: Mods {
            cmd: true,
            ..Mods::default()
        },
    });
    runtime.frame(0.);
    ensure!(runtime.state.page == Page::Actions);
    ensure!(
        runtime
            .elements()
            .iter()
            .any(|element| element.key.as_deref() == Some("search") && element.focused)
    );
    runtime.key(KeyEvent {
        key: Key::Text("echo".into()),
        mods: Mods::default(),
    });
    runtime.key(KeyEvent {
        key: Key::Enter,
        mods: Mods::default(),
    });
    runtime.frame(0.);
    ensure!(
        runtime
            .state
            .selected
            .as_ref()
            .is_some_and(|action| action.manifest.id == "echo")
    );
    ensure!(
        runtime
            .elements()
            .iter()
            .any(|element| element.key.as_deref() == Some("payload") && element.focused)
    );
    runtime.key(KeyEvent {
        key: Key::Text("Synthetic input".into()),
        mods: Mods::default(),
    });
    runtime.frame(0.);
    click(&mut runtime, "invoke")?;
    let draft = runtime
        .state
        .draft
        .clone()
        .context("missing reviewed draft")?;
    ensure!(draft.input == json!({"text":"Synthetic input","mime_type":"text/plain"}));
    ensure!(
        runtime
            .state
            .result
            .as_ref()
            .is_some_and(|result| result["status"] == "succeeded")
    );
    click(&mut runtime, "invoke")?;
    ensure!(
        runtime
            .state
            .draft
            .as_ref()
            .is_some_and(|retry| retry.id == draft.id)
    );
    runtime.update(|s, _| s.payload.replace_text("Edited input"));
    runtime.frame(0.);
    click(&mut runtime, "invoke")?;
    ensure!(
        runtime
            .state
            .error
            .as_deref()
            .is_some_and(|error| error.contains("New run"))
    );
    ensure!(
        runtime
            .state
            .draft
            .as_ref()
            .is_some_and(|retry| retry.id == draft.id)
    );
    click(&mut runtime, "new-run")?;
    click(&mut runtime, "invoke")?;
    ensure!(
        runtime
            .state
            .draft
            .as_ref()
            .is_some_and(|next| next.id != draft.id)
    );
    Ok(())
}

#[test]
fn paired_pending_offline_and_activity_controls_retain_authority() -> Result<()> {
    let mut runtime = runtime();
    runtime.update(|s, _| fixtures::pending(s));
    runtime.frame(0.);
    ensure!(runtime.find("invoke").is_none());
    click(&mut runtime, "review")?;
    ensure!(
        runtime
            .elements()
            .iter()
            .any(|element| element.key.as_deref() == Some("approve"))
    );
    runtime.update(|s, _| s.connection = Connection::Paired("/synthetic/paired.toml".into()));
    runtime.frame(0.);
    ensure!(runtime.find("review").is_none() && runtime.find("approve").is_none());
    runtime.update(|s, _| fixtures::offline(s));
    runtime.frame(0.);
    ensure!(runtime.find("error").is_some());
    runtime.update(|s, _| fixtures::activity(s));
    runtime.frame(0.);
    click(&mut runtime, "cancel-synthetic-job-0")?;
    ensure!(
        runtime
            .state
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.jobs[0].status == "cancelled")
    );
    ensure!(runtime.find("cancel-synthetic-job-0").is_none());
    click(&mut runtime, "result-synthetic-job-2")?;
    ensure!(
        runtime
            .state
            .result
            .as_ref()
            .is_some_and(|result| result["status"] == "succeeded")
    );
    Ok(())
}

#[test]
fn narrow_long_editor_has_an_ancestor_clip_and_shortcut_keeps_reviewed_identity() -> Result<()> {
    let mut state = Desktop::fixture();
    fixtures::long(&mut state);
    let mut runtime = Runtime::new(state, view, (760., 540.), Env::new(true, 1.));
    runtime.on_key = Some(on_key);
    runtime.frame(0.);
    let elements = runtime.elements();
    let viewport = elements
        .iter()
        .find(|element| element.key.as_deref() == Some("editor-viewport"))
        .context("missing editor viewport")?;
    let editor = elements
        .iter()
        .find(|element| element.key.as_deref() == Some("payload"))
        .context("missing editor")?;
    ensure!(
        editor.path.starts_with(&format!("{}/", viewport.path)),
        "the text must be clipped by an ancestor, not its own scroll style"
    );
    ensure!(viewport.rect[1] + viewport.rect[3] < 540.);
    let (x, y) = (viewport.rect[0] + 20., viewport.rect[1] + 20.);
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
    runtime.frame(0.);
    runtime.key(KeyEvent {
        key: Key::End,
        mods: Mods::default(),
    });
    runtime.frame(0.);
    ensure!(runtime.state.payload.text.len() > 10000);
    runtime.key(KeyEvent {
        key: Key::Enter,
        mods: Mods {
            cmd: true,
            ..Mods::default()
        },
    });
    runtime.frame(0.);
    let first = runtime
        .state
        .draft
        .as_ref()
        .context("shortcut did not invoke")?
        .id
        .clone();
    runtime.key(KeyEvent {
        key: Key::Enter,
        mods: Mods {
            cmd: true,
            ..Mods::default()
        },
    });
    runtime.frame(0.);
    ensure!(
        runtime
            .state
            .draft
            .as_ref()
            .is_some_and(|draft| draft.id == first)
    );
    click(&mut runtime, "inspect-result")?;
    ensure!(runtime.state.page == Page::Activity && runtime.state.show_result);
    click(&mut runtime, "nav-Actions")?;
    ensure!(
        runtime.state.payload.text.len() > 10000
            && runtime
                .state
                .draft
                .as_ref()
                .is_some_and(|draft| draft.id == first)
    );
    Ok(())
}

#[test]
fn editing_json_settings_and_navigation_keep_the_draft() -> Result<()> {
    let mut runtime = runtime();
    click(&mut runtime, "action-echo")?;
    click(&mut runtime, "input-mode")?;
    runtime.update(|s, _| {
        s.payload
            .replace_text(r#"{"text":"JSON fixture","mime_type":"text/plain"}"#);
    });
    runtime.frame(0.);
    click(&mut runtime, "invoke")?;
    let id = runtime
        .state
        .draft
        .as_ref()
        .with_context(|| {
            format!(
                "draft missing; raw={} input={:?} error={:?}",
                runtime.state.raw_input, runtime.state.payload.text, runtime.state.error
            )
        })?
        .id
        .clone();
    click(&mut runtime, "nav-Settings")?;
    click(&mut runtime, "light")?;
    ensure!(runtime.state.appearance.choice == "light");
    click(&mut runtime, "nav-Actions")?;
    ensure!(runtime.state.raw_input && runtime.state.payload.text.contains("JSON fixture"));
    ensure!(
        runtime
            .state
            .draft
            .as_ref()
            .is_some_and(|draft| draft.id == id)
    );
    let elements = runtime.elements();
    ensure!(
        elements
            .iter()
            .any(|element| element.key.as_deref() == Some("payload") && element.role == "textbox")
    );
    ensure!(
        elements
            .iter()
            .any(|element| element.key.as_deref() == Some("invoke") && element.role == "button")
    );
    Ok(())
}
