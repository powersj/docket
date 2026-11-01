//! The detail pane: toolbar, header, status card and tabs (design section 10.3).

use dioxus::prelude::*;

use super::commands::{self, current_tab, primary_label};
use super::icons::{CiGlyph, Icon};
use super::overview::Overview;
use super::prior::Prior;
use super::review::ReviewTab;
use super::status_card::StatusCard;
use crate::github::model::{CiState, PrState};
use crate::service::{Ctx, Popup, PopupKind};
use crate::state;

#[component]
pub fn Detail() -> Element {
    let ctx = use_context::<Ctx>();
    let sel = ctx.selected.read().clone();
    let entry = sel.as_ref().and_then(|id| ctx.entry(id));
    let title = entry.as_ref().map(|e| e.summary.title.clone()).unwrap_or_default();

    rsx! {
        main { id: "detail", class: "detail", tabindex: "-1", "aria-label": "Pull request detail",
            div { class: "topbar", "data-drag-region": "true",
                div { class: "toolbar-title", "{title}" }
                if let Some(e) = entry.as_ref() {
                    ToolbarActions { id: e.rec.id.clone() }
                }
            }
            Banners {}
            match entry {
                None => rsx! {
                    div { class: "empty",
                        h2 { "No pull request selected" }
                        span { "Pick one from the list, or press " span { class: "kbd", "j" } " to start at the top." }
                    }
                },
                Some(e) => rsx! { DetailBody { key: "{e.rec.id}", id: e.rec.id.clone() } },
            }
        }
    }
}

#[component]
fn ToolbarActions(id: String) -> Element {
    let mut ctx = use_context::<Ctx>();
    let Some(e) = ctx.entry(&id) else { return rsx! {} };
    let queued = svc_queued(ctx, &id);
    let label = primary_label(e.rec.stage, queued);
    let report = ctx.preflight.read().clone();
    let disabled = e.rec.closed
        || match e.rec.stage {
            state::Stage::DraftReady => !report.can_write(),
            state::Stage::AwaitingAgentic | state::Stage::Error(_) | state::Stage::Interrupted(_) => false,
            _ => !report.can_run_agent(),
        };
    let id1 = id.clone();
    let id2 = id.clone();
    let post_hint = e.rec.stage == state::Stage::DraftReady;
    rsx! {
        if let Some(l) = label {
            button {
                class: "btn primary",
                disabled,
                onclick: move |_| commands::primary(ctx, &id1),
                "{l}"
                if post_hint { span { class: "kbd", "⌘↩" } }
            }
        }
        button {
            class: "icon-btn",
            title: "More actions",
            "aria-label": "More actions",
            "aria-haspopup": "menu",
            onclick: move |ev| {
                let p = ev.client_coordinates();
                ctx.popup.set(Some(Popup { id: id2.clone(), x: p.x - 200.0, y: p.y + 14.0, kind: PopupKind::More }));
            },
            Icon { name: "more", size: 16 }
        }
    }
}

fn svc_queued(ctx: Ctx, id: &str) -> bool {
    crate::service::queue_position(ctx, id).is_some()
}

#[component]
fn Banners() -> Element {
    let ctx = use_context::<Ctx>();
    let cfg = ctx.cfg.read().clone();
    let report = ctx.preflight.read().clone();
    rsx! {
        if let Some(err) = cfg.error.clone() {
            div { class: "banner bad", role: "alert",
                span { style: "flex: 1", "Config error — using the last good config. {err}" }
                button { class: "btn small", onclick: move |_| crate::service::reveal(&err.path), "Reveal in Finder" }
            }
        }
        if let Some(org) = report.sso.clone() {
            div { class: "banner bad", role: "alert",
                span { style: "flex: 1", "Token not SSO-authorized for {org}. Label changes and posting are disabled." }
                button { class: "btn small", onclick: move |_| crate::service::open_url("https://github.com/settings/tokens"), "Authorize…" }
            }
        }
        if report.checked_at.is_some() && !report.gh.ok {
            div { class: "banner bad", role: "alert",
                span { style: "flex: 1", "GitHub: {report.gh.problem.clone().unwrap_or_default()} Label changes and posting are disabled." }
            }
        }
        if report.checked_at.is_some() && !report.claude.ok {
            div { class: "banner", role: "alert",
                span { style: "flex: 1", "Claude: {report.claude.problem.clone().unwrap_or_default()} Start review is disabled." }
            }
        } else if report.checked_at.is_some() && !report.skills.ok {
            div { class: "banner", role: "alert",
                span { style: "flex: 1", "Skills: {report.skills.problem.clone().unwrap_or_default()} Start review is disabled." }
            }
        }
    }
}

#[component]
fn DetailBody(id: String) -> Element {
    let ctx = use_context::<Ctx>();
    let Some(e) = ctx.entry(&id) else { return rsx! {} };
    let tab = current_tab(ctx, &id, e.rec.stage);
    let tabs = ["Overview", "Prior review", "Review"];
    rsx! {
        div { class: "detail-scroll",
            Header { id: id.clone() }
            StatusCard { id: id.clone() }
            div { class: "tabs", role: "tablist",
                for (i, t) in tabs.iter().enumerate() {
                    button {
                        key: "{t}",
                        class: if i == tab { "tab active" } else { "tab" },
                        role: "tab",
                        "aria-selected": "{i == tab}",
                        onclick: move |_| commands::set_tab(ctx, i),
                        "{t}"
                        span { class: "kbd", "⌘{i + 1}" }
                    }
                }
            }
            div { class: "tab-body", role: "tabpanel",
                match tab {
                    0 => rsx! { Overview { id: id.clone() } },
                    1 => rsx! { Prior { id: id.clone() } },
                    _ => rsx! { ReviewTab { id: id.clone() } },
                }
            }
        }
    }
}

#[component]
fn Header(id: String) -> Element {
    let ctx = use_context::<Ctx>();
    let clock = use_context::<super::pr_list::ListClock>().0;
    let Some(e) = ctx.entry(&id) else { return rsx! {} };
    let Some(cfg) = ctx.config() else { return rsx! {} };
    let s = &e.summary;
    let updated = state::age(s.updated_at, clock());
    let ci = &s.ci;
    let ci_text = match ci.state {
        CiState::Success => format!("CI {}/{}", ci.passed, ci.total),
        CiState::Failure => format!("CI failing {}/{}", ci.failed, ci.total),
        CiState::Pending => format!("CI pending {}/{}", ci.pending, ci.total),
        CiState::None => "No CI".into(),
    };
    let decision = match s.review_decision.as_deref() {
        Some("APPROVED") => "Approved",
        Some("CHANGES_REQUESTED") => "Changes req.",
        Some("REVIEW_REQUIRED") => "Review required",
        _ => "No decision",
    };
    let mine =
        state::my_status_text(s.requested, state::my_review(e.detail.as_ref(), &e.rec, &s.head_sha, &cfg.github.login));
    let has_draft = ctx.drafts.read().contains_key(&id);
    let stage = if e.rec.closed {
        match s.state {
            PrState::Merged => "Merged".to_string(),
            _ => "Closed".to_string(),
        }
    } else {
        format!(
            "{}{}",
            e.rec.stage.label(),
            if has_draft && e.rec.stage != state::Stage::DraftReady { " · Draft" } else { "" }
        )
    };
    let repo = s.repo.clone();
    rsx! {
        div { class: "detail-head",
            div { class: "detail-meta",
                a { href: "{s.url}", "{repo} #{s.number}" }
                span { class: "sep", "·" }
                span { "@{s.author}" }
                span { class: "sep", "·" }
                span { "updated {updated} ago" }
                if s.is_draft {
                    span { class: "sep", "·" }
                    span { class: "chip", "Draft" }
                }
            }
            div { class: "detail-meta", style: "margin-top: 2px",
                span { class: "mono", "{s.base_ref} ← {s.head_ref}" }
                span { class: "sep", "·" }
                span { class: "plus num", "+{s.additions}" }
                span { class: "minus num", "−{s.deletions}" }
                span { class: "sep", "·" }
                span { "{s.changed_files} files" }
            }
            div { class: "detail-status", "aria-live": "polite",
                button {
                    class: "ci-toggle",
                    title: "Show checks",
                    onclick: move |_| commands::set_tab(ctx, 0),
                    CiGlyph { ci: ci.clone() }
                    "{ci_text}"
                }
                span { class: "sep faint", "·" }
                span { "{decision}" }
                span { class: "sep faint", "·" }
                span { "Me: {mine}" }
                span { class: "sep faint", "·" }
                span { class: "stage-pill", "{stage}" }
            }
        }
    }
}

/// Items for the ⋯ menu, mirrored in the native PR menu.
pub fn more_items(ctx: Ctx, id: &str) -> Vec<(&'static str, &'static str, bool)> {
    let Some(e) = ctx.entry(id) else { return vec![] };
    let active = e.rec.stage.is_active();
    let has_draft = ctx.drafts.read().contains_key(id);
    let mut v = vec![];
    if !matches!(e.rec.stage, state::Stage::New | state::Stage::Triage) {
        v.push(("rerun", "Re-run review", !active && has_draft));
    }
    if e.rec.stage == state::Stage::AwaitingAgentic {
        v.push(("rerun_agentic", "Re-run agentic review", true));
    }
    if matches!(e.rec.stage, state::Stage::Interrupted(_) | state::Stage::Error(_)) {
        v.push(("discard", "Discard", true));
    }
    if e.rec.stage == state::Stage::Ignored {
        v.push(("unignore", "Unignore", true));
    } else {
        v.push(("ignore", "Ignore", !active));
    }
    v.push(("snooze", "Snooze…", !active));
    v.push(("-", "", true));
    v.push(("open", "Open in GitHub", true));
    v
}
