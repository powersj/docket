//! The sidebar: filter bar, sections and rows (design section 10.2).

use dioxus::prelude::*;

use super::commands::{self, dom_id};
use super::icons::{CiGlyph, Icon};
use crate::service::{self as svc, Ctx, Popup, PopupKind};
use crate::state::{self, Row, Section};

#[derive(Clone, Copy)]
pub struct ListClock(pub Signal<chrono::DateTime<chrono::Utc>>);

#[component]
pub fn Sidebar() -> Element {
    let ctx = use_context::<Ctx>();
    let mut filter = ctx.filter;
    let refresh = ctx.refresh.read().clone();
    let Some(cfg) = ctx.config() else { return rsx! {} };

    let sections: Vec<Section> = {
        let prs = ctx.prs.read();
        let queue = ctx.queue.read();
        let rows: Vec<Row> =
            prs.values().map(|e| Row { summary: &e.summary, rec: &e.rec, queued: queue.contains(&e.rec.id) }).collect();
        state::sections(&rows, &cfg, ctx.ui.read().show_hidden, &filter.read())
    };
    let total: usize = sections.iter().map(|s| s.ids.len() + s.bot_ids.len()).sum();
    let selected = ctx.selected.read().clone();
    let active_desc = selected.as_deref().map(|s| format!("row-{}", dom_id(s))).unwrap_or_default();
    let stale = refresh.last_error.is_some();
    let list_class = if *ctx.list_focused.read() { "list focused" } else { "list" };

    rsx! {
        nav { class: "sidebar", "aria-label": "Pull requests",
            div { class: "topbar", "data-drag-region": "true",
                label { class: "filter",
                    Icon { name: "search", size: 13 }
                    input {
                        id: "filter",
                        r#type: "search",
                        placeholder: "Filter",
                        "aria-label": "Filter pull requests",
                        value: "{filter}",
                        oninput: move |e| filter.set(e.value()),
                        onkeydown: move |e| {
                            if e.key() == Key::Escape {
                                filter.set(String::new());
                            }
                        },
                    }
                }
                button {
                    class: "icon-btn",
                    title: "Refresh (⌘R)",
                    "aria-label": "Refresh",
                    disabled: refresh.running,
                    onclick: move |_| { spawn(svc::refresh(ctx, true)); },
                    Icon { name: "refresh" }
                }
            }
            div {
                id: "list",
                class: "{list_class}",
                role: "listbox",
                tabindex: "0",
                "aria-label": "Pull requests",
                "aria-activedescendant": "{active_desc}",
                style: if stale { "opacity: 0.7" } else { "" },
                if !refresh.loaded_once && ctx.prs.read().is_empty() {
                    for _ in 0..6 {
                        div { class: "skeleton", div {} div {} }
                    }
                } else if total == 0 && !filter.read().is_empty() {
                    div { class: "empty",
                        span { "No matches" }
                        button { class: "link-btn", onclick: move |_| filter.set(String::new()), "Clear filter" }
                    }
                } else if total == 0 {
                    div { class: "empty",
                        h2 { "All caught up" }
                        span { "Nothing needs a review right now." }
                    }
                } else {
                    for s in sections {
                        if !s.ids.is_empty() || !s.bot_ids.is_empty() || s.key == "needs" {
                            SectionView { key: "{s.key}", section: s.clone() }
                        }
                    }
                }
            }
            Splitter {}
        }
    }
}

#[component]
fn Splitter() -> Element {
    let mut ctx = use_context::<Ctx>();
    let mut dragging = use_signal(|| false);
    rsx! {
        div {
            class: "splitter",
            onmousedown: move |e| {
                e.prevent_default();
                dragging.set(true);
            },
            if dragging() {
                div {
                    style: "position: fixed; inset: 0; z-index: 100; cursor: col-resize;",
                    onmousemove: move |e| {
                        let x = e.client_coordinates().x.clamp(240.0, 640.0);
                        ctx.ui.write().sidebar_width = x;
                    },
                    onmouseup: move |_| {
                        dragging.set(false);
                        svc::save_ui_prefs(ctx);
                    },
                }
            }
        }
    }
}

#[component]
fn SectionView(section: Section) -> Element {
    let mut ctx = use_context::<Ctx>();
    let skey = section.key.clone();
    let collapsed = ctx.ui.read().collapsed.contains(&skey);
    let bots_open = ctx.ui.read().expanded_bots.contains(&skey);
    let n = section.ids.len() + section.bot_ids.len();
    let count = if section.starred > 0 { format!("{n} · {}★", section.starred) } else { n.to_string() };
    let k1 = skey.clone();
    let k2 = skey.clone();
    let chev = if collapsed { "chev-right" } else { "chev-down" };
    let bots_chev = if bots_open { "chev-down" } else { "chev-right" };
    let bots_label = format!(" {} bot PRs", section.bot_ids.len());
    rsx! {
        div { role: "group", "aria-label": "{section.title}",
            button {
                class: "section-head",
                "aria-expanded": "{!collapsed}",
                onclick: move |_| {
                    {
                        let mut ui = ctx.ui.write();
                        if !ui.collapsed.remove(&k1) {
                            ui.collapsed.insert(k1.clone());
                        }
                    }
                    svc::save_ui_prefs(ctx);
                },
                Icon { name: chev, size: 11 }
                span { {section.title.clone()} }
                span { class: "count", "{count}" }
            }
            if !collapsed {
                for id in section.ids.iter() {
                    PrRow { key: "{id}", id: id.clone(), cross_repo: section.cross_repo }
                }
                if !section.bot_ids.is_empty() {
                    button {
                        class: "bot-group",
                        "aria-expanded": "{bots_open}",
                        onclick: move |_| {
                            let mut ui = ctx.ui.write();
                            if !ui.expanded_bots.remove(&k2) {
                                ui.expanded_bots.insert(k2.clone());
                            }
                        },
                        Icon { name: bots_chev, size: 11 }
                        {bots_label}
                    }
                    if bots_open {
                        for id in section.bot_ids.iter() {
                            PrRow { key: "{id}", id: id.clone(), cross_repo: section.cross_repo }
                        }
                    }
                }
            }
        }
    }
}

fn chip_class(stage: state::Stage, timed_out: bool) -> &'static str {
    use state::Stage::{DraftReady, Error, Interrupted, Posted, ReReviewNeeded};
    match stage {
        _ if timed_out => "warn",
        DraftReady | ReReviewNeeded => "attn",
        Error(_) | Interrupted(_) => "bad",
        Posted => "ok",
        _ => "",
    }
}

#[component]
fn PrRow(id: String, cross_repo: bool) -> Element {
    let mut ctx = use_context::<Ctx>();
    let clock = use_context::<ListClock>().0;
    let Some(e) = ctx.prs.read().get(&id).cloned() else { return rsx! {} };
    let now = clock();
    let s = &e.summary;
    let sel = ctx.selected.read().as_deref() == Some(id.as_str());
    let compact = ctx.ui.read().compact;
    let queued = svc::queue_position(ctx, &id);
    let chip = state::next_action(&e.rec, queued, now);
    let chip_cls = chip_class(e.rec.stage, e.rec.agentic_timed_out());
    let age = state::age(s.updated_at, now);
    let created = s.created_at.with_timezone(&chrono::Local).format("Created %b %-d, %Y").to_string();
    let tooltip: String = s.body.lines().filter(|l| !l.trim().is_empty()).take(2).collect::<Vec<_>>().join("\n");
    let tooltip = if tooltip.is_empty() { s.title.clone() } else { tooltip };
    let num = if cross_repo {
        format!("{}#{}", s.repo.rsplit('/').next().unwrap_or(&s.repo), s.number)
    } else {
        format!("#{}", s.number)
    };
    let class = format!(
        "row{}{}{}",
        if sel { " selected" } else { "" },
        if compact { " compact" } else { "" },
        if e.rec.stage.is_hidden() || e.rec.closed { " dimmed" } else { "" }
    );
    let draft = if s.is_draft { " · Draft" } else { "" };
    let id1 = id.clone();
    let id2 = id.clone();
    rsx! {
        div {
            id: "row-{dom_id(&id)}",
            class: "{class}",
            role: "option",
            "aria-selected": "{sel}",
            title: "{tooltip}",
            onclick: move |_| svc::select(ctx, &id1),
            oncontextmenu: move |ev| {
                ev.prevent_default();
                svc::select(ctx, &id2);
                let p = ev.client_coordinates();
                ctx.popup.set(Some(Popup { id: id2.clone(), x: p.x, y: p.y, kind: PopupKind::Row }));
            },
            if s.requested {
                span { class: "star", title: "Review requested", Icon { name: "star", size: 11 } }
            }
            if compact {
                div { class: "row-line1",
                    span { class: "row-title", "{s.title}" }
                    CiGlyph { ci: s.ci.clone() }
                    span { class: "chip {chip_cls}", "{chip}" }
                    span { class: "row-age muted", title: "{created}", "{age}" }
                }
            } else {
                div { class: "row-line1",
                    span { class: "row-title", "{s.title}" }
                    span { class: "row-age muted", title: "{created}", "{age}" }
                }
                div { class: "row-line2",
                    span { class: "meta muted",
                        "{num} @{s.author} "
                        span { class: "plus num", "+{s.additions}" }
                        " "
                        span { class: "minus num", "−{s.deletions}" }
                        "{draft}"
                    }
                    CiGlyph { ci: s.ci.clone() }
                    span { class: "chip {chip_cls}", "{chip}" }
                }
            }
        }
    }
}

pub fn row_menu_items(ctx: Ctx, id: &str) -> Vec<(&'static str, &'static str, bool)> {
    let e = ctx.entry(id);
    let active = e.as_ref().is_some_and(|e| e.rec.stage.is_active());
    let ignored = e.as_ref().is_some_and(|e| e.rec.stage == state::Stage::Ignored);
    vec![
        ("start", "Start review", !active),
        (if ignored { "unignore" } else { "ignore" }, if ignored { "Unignore" } else { "Ignore" }, !active),
        ("snooze", "Snooze…", !active),
        ("open", "Open in GitHub", true),
        ("-", "", true),
        ("watch", "Watch this repo", true),
        ("hide_author", "Hide this author", true),
    ]
}

pub fn row_menu_action(mut ctx: Ctx, id: &str, action: &str) {
    ctx.popup.set(None);
    let Some(e) = ctx.entry(id) else { return };
    let path = crate::config::paths::config_file();
    match action {
        "start" => commands::request_start(ctx, id),
        "ignore" => svc::ignore(ctx, id),
        "unignore" => svc::unignore(ctx, id),
        "snooze" => ctx.sheet.set(Some(svc::Sheet::Snooze(id.to_string()))),
        "open" => svc::open_url(&e.summary.url),
        "watch" => match crate::config::edit::add_watch(&path, &e.summary.repo) {
            Ok(()) => {
                ctx.toast(format!("Watching {}", e.summary.repo), false, None);
                spawn(svc::refresh(ctx, false));
            }
            Err(err) => ctx.toast(format!("Couldn't update config: {err}"), true, None),
        },
        "hide_author" => match crate::config::edit::hide_author(&path, &e.summary.repo, &e.summary.author) {
            Ok(()) => {
                ctx.toast(format!("Hid @{} in {}", e.summary.author, e.summary.repo), false, None);
                spawn(svc::refresh(ctx, false));
            }
            Err(err) => ctx.toast(format!("Couldn't update config: {err}"), true, None),
        },
        "rerun" => commands::request_start(ctx, id),
        "rerun_agentic" => {
            spawn(svc::rerun_agentic(ctx, id.to_string()));
        }
        "discard" => svc::discard(ctx, id),
        _ => {}
    }
}
