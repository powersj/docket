//! Root component: global state, polling loops, keyboard/menu bridges, and
//! window-level overlays.

use std::time::Duration;

use dioxus::desktop::tao::event::{Event as TaoEvent, WindowEvent};
use dioxus::desktop::{use_muda_event_handler, use_wry_event_handler, window};
use dioxus::prelude::*;

use super::commands::{self, KEY_JS, KeyMsg};
use super::confirm_sheet::ConfirmSheet;
use super::detail::{Detail, more_items};
use super::pr_list::{ListClock, Sidebar, row_menu_action, row_menu_items};
use super::status_card::SecondClock;
use crate::config::{EXAMPLE_CONFIG, paths};
use crate::service::{self as svc, Ctx, PopupKind, Sheet, ToastAction};

/// Put the blur view behind the webview. This must run after the webview is
/// created: wry replaces the window's content view when it attaches, which
/// would discard a blur view added earlier.
fn apply_vibrancy() -> bool {
    use window_vibrancy::{NSVisualEffectMaterial, apply_vibrancy};
    match apply_vibrancy(&*window().window, NSVisualEffectMaterial::Sidebar, None, None) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("vibrancy unavailable, using a solid sidebar: {e}");
            false
        }
    }
}

#[component]
pub fn App() -> Element {
    let ctx = use_hook(|| {
        let c = Ctx::new();
        svc::reload_config(c);
        svc::load_ui_prefs(c);
        svc::startup(c);
        c
    });
    use_context_provider(|| ctx);
    let vibrant_on = use_hook(apply_vibrancy);
    let mut list_clock = use_signal(chrono::Utc::now);
    let mut sec_clock = use_signal(chrono::Utc::now);
    use_context_provider(|| ListClock(list_clock));
    use_context_provider(|| SecondClock(sec_clock));

    use_future(move || async move {
        let mut n = 0u64;
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            n += 1;
            // Only re-render the status card every second while something is live.
            if ctx.runs.read().values().any(|r| r.active) || ctx.prs.read().values().any(|e| e.rec.stage.is_active()) {
                sec_clock.set(chrono::Utc::now());
            }
            if n.is_multiple_of(30) {
                list_clock.set(chrono::Utc::now());
            }
        }
    });

    // List polling.
    use_future(move || async move {
        loop {
            svc::refresh(ctx, false).await;
            let wait = ctx.config().map(|c| c.poll_interval).unwrap_or(Duration::from_secs(3));
            tokio::time::sleep(wait).await;
        }
    });

    // Agentic waits poll more often than the list.
    use_future(move || async move {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            svc::agentic_tick(ctx).await;
        }
    });

    // Window dragging from the top bars (see drag.rs).
    use_hook(super::drag::install);
    use_future(move || async move {
        let mut ev = document::eval(super::drag::REGIONS_JS);
        while let Ok(r) = ev.recv::<super::drag::Regions>().await {
            super::drag::set_regions(r);
        }
    });

    // Keyboard bridge.
    use_future(move || async move {
        let mut ev = document::eval(KEY_JS);
        while let Ok(k) = ev.recv::<KeyMsg>().await {
            commands::key(ctx, k);
        }
    });

    use_muda_event_handler(move |e| commands::menu(ctx, &e.id.0));

    // Window frame: remember position and size.
    use_wry_event_handler(move |event, _| {
        if let TaoEvent::WindowEvent { event: WindowEvent::Moved(_) | WindowEvent::Resized(_), .. } = event {
            let w = window();
            let scale = w.window.scale_factor();
            if let (Ok(pos), size) = (w.window.outer_position(), w.window.inner_size()) {
                let (p, s) = (pos.to_logical::<f64>(scale), size.to_logical::<f64>(scale));
                if !w.window.is_minimized() && s.width > 100.0 {
                    svc::svc().store.ui_set("window", &format!("{},{},{},{}", p.x, p.y, s.width, s.height));
                }
            }
        }
    });

    // Menu items follow the selected PR's stage.
    use_effect(move || {
        let sel = ctx.selected.read().clone();
        let e = sel.as_ref().and_then(|id| ctx.prs.read().get(id).cloned());
        let has_draft = sel.as_ref().is_some_and(|id| ctx.drafts.read().contains_key(id));
        super::menus::update_for(e.as_ref().map(|e| e.rec.stage), e.as_ref().is_some_and(|e| e.rec.closed), has_draft);
    });

    use_effect(move || {
        let ui = ctx.ui.read();
        window().set_zoom_level(ui.zoom);
        super::drag::set_zoom(ui.zoom);
        super::menus::set_checked("compact", ui.compact);
        super::menus::set_checked("split", ui.split_preview);
        super::menus::set_checked("show_hidden", ui.show_hidden);
    });

    let cfg = ctx.cfg.read().clone();
    let vibrant = if vibrant_on { "vibrant" } else { "" };
    if cfg.current.is_none() {
        return rsx! {
            div { class: "app", style: "grid-template-columns: 1fr; position: relative;",
                div { class: "title-strip", "data-drag-region": "true" }
                FirstRun {}
            }
        };
    }
    let w = ctx.ui.read().sidebar_width;
    rsx! {
        div {
            class: "app {vibrant}",
            style: "--sidebar-w: {w}px",
            onclick: move |_| {
                let mut p = ctx.popup;
                if p.read().is_some() {
                    p.set(None);
                }
            },
            Sidebar {}
            Detail {}
            Footer {}
            ConfirmSheet {}
            SheetView {}
            PopupMenu {}
            Toasts {}
            div { class: "sr-only", "aria-live": "polite", {live_text(ctx)} }
        }
    }
}

fn live_text(ctx: Ctx) -> String {
    let sel = ctx.selected.read().clone();
    sel.and_then(|id| ctx.entry(&id))
        .map(|e| format!("{}: {}", e.summary.title, e.rec.stage.label()))
        .unwrap_or_default()
}

#[component]
fn Footer() -> Element {
    let mut ctx = use_context::<Ctx>();
    let clock = use_context::<ListClock>().0;
    let r = ctx.preflight.read().clone();
    let refresh = ctx.refresh.read().clone();
    let active = ctx.runs.read().values().filter(|r| r.active).count();
    let queued = ctx.queue.read().len();
    let now = clock();
    let checked = r.checked_at.is_some();
    let dot = |ok: bool| {
        if !checked {
            "dot unknown"
        } else if ok {
            "dot"
        } else {
            "dot bad"
        }
    };
    let gh_ok = r.gh.ok && r.sso.is_none();
    let gh_text = if r.gh.summary.is_empty() {
        ctx.config().map(|c| format!("gh: {}", c.github.login)).unwrap_or_default()
    } else {
        r.gh.summary.clone()
    };
    let claude_text = if !r.claude.ok || !checked {
        r.claude.summary.clone()
    } else if !r.skills.ok {
        "claude: skills missing".into()
    } else {
        "claude: ok".into()
    };
    let claude_text = if claude_text.is_empty() { "claude".to_string() } else { claude_text };
    let refreshed = match (&refresh.last_error, refresh.last_ok) {
        _ if refresh.running => "refreshing…".to_string(),
        (Some((t, _)), _) => format!("refresh failed {} ago", crate::state::age(*t, now)),
        (None, Some(t)) => format!("refreshed {} ago", crate::state::age(t, now)),
        _ => "not refreshed yet".into(),
    };
    let err = refresh.last_error.as_ref().map(|(_, e)| e.clone());
    let runs_text = format!(
        "{active} run{} active{}",
        if active == 1 { "" } else { "s" },
        if queued > 0 { format!(", {queued} queued") } else { String::new() }
    );
    rsx! {
        footer { class: "footer",
            button { class: "foot-item", title: "GitHub status", onclick: move |_| ctx.sheet.set(Some(Sheet::Check("gh"))),
                span { class: dot(gh_ok) } "{gh_text}"
            }
            button { class: "foot-item", title: "Claude status", onclick: move |_| ctx.sheet.set(Some(Sheet::Check("claude"))),
                span { class: dot(r.claude.ok && r.skills.ok) } "{claude_text}"
            }
            span { class: "faint", "·" }
            span { title: err.clone().unwrap_or_default(), "{refreshed}" }
            if err.is_some() {
                button { class: "link-btn", onclick: move |_| { spawn(svc::refresh(ctx, true)); }, "Retry" }
            }
            if active > 0 || queued > 0 {
                span { class: "faint", "·" }
                span { "{runs_text}" }
            }
            span { style: "flex: 1" }
            button { class: "foot-item", title: "Keyboard shortcuts", onclick: move |_| ctx.sheet.set(Some(Sheet::Shortcuts)), span { class: "kbd", "?" } }
        }
    }
}

#[component]
fn PopupMenu() -> Element {
    let ctx = use_context::<Ctx>();
    let Some(p) = ctx.popup.read().clone() else { return rsx! {} };
    let items = match p.kind {
        PopupKind::Row => row_menu_items(ctx, &p.id),
        PopupKind::More => more_items(ctx, &p.id),
    };
    let x = p.x.max(8.0);
    let y = p.y.max(8.0);
    rsx! {
        div { class: "popup", role: "menu", style: "left: {x}px; top: {y}px", onclick: move |e| e.stop_propagation(),
            for (i, (action, label, enabled)) in items.into_iter().enumerate() {
                if action == "-" {
                    hr { key: "{i}" }
                } else {
                    button {
                        key: "{i}",
                        role: "menuitem",
                        disabled: !enabled,
                        onclick: {
                            let id = p.id.clone();
                            move |_| row_menu_action(ctx, &id, action)
                        },
                        "{label}"
                    }
                }
            }
        }
    }
}

#[component]
fn SheetView() -> Element {
    let ctx = use_context::<Ctx>();
    let mut custom = use_signal(String::new);
    let Some(sheet) = ctx.sheet.read().clone() else { return rsx! {} };
    let mut sheet_sig = ctx.sheet;
    let body = match sheet {
        Sheet::Shortcuts => rsx! {
            h2 { "Keyboard shortcuts" }
            div { class: "cheats",
                for (k, v) in SHORTCUTS {
                    Fragment { key: "{k}",
                        span { class: "kbd", style: "justify-self: start", "{k}" }
                        span { "{v}" }
                    }
                }
            }
            div { class: "sheet-actions", span { class: "spacer" } button { class: "btn primary", onclick: move |_| sheet_sig.set(None), "Done" } }
        },
        Sheet::ReplaceDraft(id) => rsx! {
            h2 { "Replace edited draft?" }
            p { "You've edited this draft. Re-running creates a new draft; the edited one is kept in history." }
            div { class: "sheet-actions",
                span { class: "spacer" }
                button { class: "btn", onclick: move |_| sheet_sig.set(None), "Cancel" }
                button {
                    class: "btn primary",
                    onclick: move |_| {
                        sheet_sig.set(None);
                        spawn(svc::start_review(ctx, id.clone()));
                    },
                    "Re-run"
                }
            }
        },
        Sheet::Check(which) => {
            let r = ctx.preflight.read().clone();
            let checks: Vec<crate::preflight::Check> =
                if which == "gh" { vec![r.gh.clone()] } else { vec![r.claude.clone(), r.skills.clone()] };
            let sso = if which == "gh" { r.sso.clone() } else { None };
            rsx! {
                h2 { if which == "gh" { "GitHub" } else { "Claude" } }
                if r.checked_at.is_none() {
                    p { class: "muted", "Not checked yet." }
                }
                for (i, c) in checks.into_iter().enumerate() {
                    div { key: "{i}", style: "margin: 8px 0",
                        div { span { class: if c.ok { "dot" } else { "dot bad" } } " " strong { "{c.summary}" } }
                        if let Some(p) = c.problem { p { class: "selectable", "{p}" } }
                        if let Some(f) = c.fix { p { class: "muted selectable", "Fix: {f}" } }
                    }
                }
                if let Some(org) = sso {
                    p { class: "selectable", "Token not SSO-authorized for {org}. Authorize the gh token for that org on GitHub, then refresh." }
                }
                div { class: "sheet-actions",
                    span { class: "spacer" }
                    button { class: "btn", onclick: move |_| { spawn(async move { svc::run_preflight(ctx, true).await; }); }, "Check again" }
                    button { class: "btn primary", onclick: move |_| sheet_sig.set(None), "Done" }
                }
            }
        }
        Sheet::Snooze(id) => {
            let presets = [("tomorrow", "Tomorrow 9am"), ("monday", "Monday 9am"), ("week", "Next week")];
            rsx! {
                h2 { "Snooze until…" }
                div { style: "display: flex; gap: 8px; flex-wrap: wrap",
                    for (k, label) in presets {
                        button {
                            key: "{k}",
                            class: "btn",
                            onclick: {
                                let id = id.clone();
                                move |_| {
                                    sheet_sig.set(None);
                                    svc::snooze(ctx, &id, svc::snooze_preset(k));
                                }
                            },
                            "{label}"
                        }
                    }
                }
                div { class: "row-kv", style: "margin-top: 14px",
                    label { r#for: "snooze-custom", "Custom" }
                    input { id: "snooze-custom", r#type: "datetime-local", value: "{custom}", oninput: move |e| custom.set(e.value()) }
                    button {
                        class: "btn",
                        disabled: parse_local(&custom.read()).is_none(),
                        onclick: move |_| {
                            if let Some(t) = parse_local(&custom.read()) {
                                sheet_sig.set(None);
                                svc::snooze(ctx, &id, t);
                            }
                        },
                        "Snooze"
                    }
                }
                div { class: "sheet-actions", span { class: "spacer" } button { class: "btn", onclick: move |_| sheet_sig.set(None), "Cancel" } }
            }
        }
    };
    rsx! {
        div { class: "scrim", onclick: move |_| sheet_sig.set(None),
            div { class: "sheet", role: "dialog", "aria-modal": "true", onclick: move |e| e.stop_propagation(), {body} }
        }
    }
}

fn parse_local(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::TimeZone;
    let n = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M").ok()?;
    let t = chrono::Local.from_local_datetime(&n).single()?.with_timezone(&chrono::Utc);
    (t > chrono::Utc::now()).then_some(t)
}

const SHORTCUTS: &[(&str, &str)] = &[
    ("j / k", "Next / previous PR"),
    ("Enter", "Focus detail"),
    ("r", "Start review / re-run"),
    ("i", "Ignore (undo with ⌘Z)"),
    ("s", "Snooze"),
    ("o", "Open in GitHub"),
    ("[ / ]", "Previous / next comment card"),
    ("space", "Toggle include on the focused card"),
    ("e", "Edit focused card or summary"),
    ("Esc", "Leave editor"),
    ("⌘1–⌘3", "Switch tab"),
    ("⌘↩", "Open post confirmation / confirm"),
    ("⌘.", "Cancel agent run"),
    ("⌘R", "Refresh"),
    ("⌘F", "Focus filter"),
    ("⌘⇧P", "Write / preview toggle"),
    ("⌘B ⌘I ⌘K", "Bold, italic, link (in editors)"),
    ("⌘⇧S", "Insert suggestion (in comment editors)"),
    ("⌘+ / ⌘−", "Zoom"),
    ("?", "This cheat sheet"),
];

#[component]
fn Toasts() -> Element {
    let ctx = use_context::<Ctx>();
    let toasts = ctx.toasts.read().clone();
    rsx! {
        div { class: "toasts", "aria-live": "polite",
            for t in toasts {
                div { key: "{t.id}", class: if t.error { "toast error" } else { "toast" }, role: if t.error { "alert" } else { "status" },
                    span { class: "selectable", "{t.text}" }
                    if let Some((label, action)) = t.action.clone() {
                        button {
                            class: "btn small",
                            onclick: move |_| {
                                let mut toasts = ctx.toasts;
                                match &action {
                                    ToastAction::OpenUrl(u) => svc::open_url(u),
                                    ToastAction::UndoIgnore(id) => svc::unignore(ctx, id),
                                }
                                toasts.write().retain(|x| x.id != t.id);
                            },
                            "{label}"
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn FirstRun() -> Element {
    let ctx = use_context::<Ctx>();
    let path = paths::config_file();
    let shown = path.display().to_string();
    let err = ctx.cfg.read().error.clone();
    rsx! {
        div { class: "first-run",
            h1 { "Welcome to Reviewer" }
            p { class: "muted", "Create a config file to tell Reviewer which repos to watch and which GitHub account to use." }
            p { "Save this as " span { class: "mono selectable", "{shown}" } " and edit it:" }
            pre { "{EXAMPLE_CONFIG}" }
            if let Some(e) = err {
                p { class: "selectable", style: "color: var(--bad)", "{e}" }
            }
            div { style: "display: flex; gap: 8px",
                button {
                    class: "btn primary",
                    onclick: move |_| {
                        let p = paths::config_file();
                        if !p.exists() {
                            std::fs::create_dir_all(paths::support_dir()).ok();
                            std::fs::write(&p, EXAMPLE_CONFIG).ok();
                        }
                        svc::reveal(&p);
                    },
                    "Create example and reveal in Finder"
                }
                button { class: "btn", onclick: move |_| svc::reload_config(ctx), "I've saved it — reload" }
            }
            p { class: "faint", style: "margin-top: 18px", "Reviewer also needs the GitHub CLI (gh) logged in as that account, and Claude Code." }
        }
    }
}
