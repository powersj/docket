//! The status card shown above the tabs, by stage (design section 10.3).

use dioxus::prelude::*;

use super::commands;
use crate::github::model::PrState;
use crate::service::{self as svc, Ctx};
use crate::state::{self, Phase, Stage};

#[derive(Clone, Copy)]
pub struct SecondClock(pub Signal<chrono::DateTime<chrono::Utc>>);

fn mmss(d: chrono::Duration) -> String {
    let s = d.num_seconds().max(0);
    if s >= 60 { format!("{}m{:02}s", s / 60, s % 60) } else { format!("{s}s") }
}

#[component]
pub fn StatusCard(id: String) -> Element {
    let ctx = use_context::<Ctx>();
    let now = use_context::<SecondClock>().0();
    let mut show_log = use_signal(|| false);
    let Some(e) = ctx.entry(&id) else { return rsx! {} };
    let Some(cfg) = ctx.config() else { return rsx! {} };
    let head_moved = ctx.head_moved.read().get(&id).cloned();
    let draft_head = ctx.drafts.read().get(&id).map(|d| d.head_sha.clone());
    let run = ctx.runs.read().get(&id).cloned();
    let id_ = id.clone();
    let short = |sha: &str| sha.chars().take(7).collect::<String>();

    if e.rec.closed {
        let what = if e.summary.state == PrState::Merged { "merged" } else { "closed" };
        return rsx! {
            div { class: "status-card",
                span { class: "msg", "This PR was {what}. Any draft is kept; reopening restores where you left off." }
            }
        };
    }

    let card = match e.rec.stage {
        Stage::AwaitingAgentic => {
            let detail = e.rec.stage_detail.clone().unwrap_or_default();
            let elapsed = now - e.rec.stage_updated_at;
            let total = chrono::Duration::from_std(cfg.agentic.wait_timeout).unwrap_or_default();
            #[expect(clippy::cast_precision_loss, reason = "a progress bar; seconds are far below 2^52")]
            let pct = (elapsed.num_seconds() as f64 / total.num_seconds().max(1) as f64 * 100.0).clamp(0.0, 100.0);
            let checked = e.detail_checked_at.map_or_else(|| "not yet".into(), |t| mmss(now - t));
            let (a, b, c) = (id.clone(), id.clone(), id.clone());
            if let Some(msg) = detail.strip_prefix("unavailable:") {
                rsx! {
                    div { class: "status-card",
                        span { class: "msg", "The agentic review label isn't available here ({msg}). You can continue without it." }
                        div { class: "actions",
                            button { class: "btn small primary", onclick: move |_| svc::continue_without(ctx, &a), "Continue" }
                        }
                    }
                }
            } else if e.rec.agentic_timed_out() {
                // The bot reviewed an earlier commit but not this one.
                let older = e.detail.as_ref().is_some_and(|d| state::agentic_done(d, &cfg, None));
                let msg = if older {
                    "No agentic review on the latest commit.".to_string()
                } else {
                    format!(
                        "No agentic review after {}. The bot may post nothing when it has no findings.",
                        humantime::format_duration(cfg.agentic.wait_timeout)
                    )
                };
                let d = id.clone();
                rsx! {
                    div { class: "status-card warn",
                        span { class: "msg", "{msg}" }
                        div { class: "actions",
                            button { class: "btn small", onclick: move |_| svc::keep_waiting(ctx, &a), "Keep waiting" }
                            button { class: "btn small", onclick: move |_| { spawn(svc::rerun_agentic(ctx, b.clone())); }, "Re-run agentic review" }
                            button { class: "btn small primary", onclick: move |_| svc::continue_without(ctx, &c), "Continue without it" }
                            button { class: "btn small", onclick: move |_| svc::cancel(ctx, &d), "Cancel" }
                        }
                    }
                }
            } else {
                let mins = elapsed.num_minutes();
                let tmins = total.num_minutes();
                let head = short(&e.summary.head_sha);
                let d = id.clone();
                rsx! {
                    div { class: "status-card",
                        span { class: "msg",
                            "Waiting for agentic reviewer on "
                            span { class: "mono", "{head}" }
                            span { class: "progress", role: "progressbar", "aria-valuenow": "{pct:.0}", "aria-valuemin": "0", "aria-valuemax": "100",
                                span { style: "width: {pct:.0}%" }
                            }
                            span { class: "num", "{mins}m/{tmins}m" }
                            span { class: "muted", " · checked {checked} ago" }
                        }
                        div { class: "actions",
                            button {
                                class: "btn small",
                                onclick: move |_| {
                                    let id = a.clone();
                                    spawn(async move {
                                        if let Some(cfg) = ctx.config() {
                                            svc::check_agentic(ctx, &id, &cfg).await;
                                        }
                                    });
                                },
                                "Check now"
                            }
                            button { class: "btn small", onclick: move |_| { spawn(svc::rerun_agentic(ctx, b.clone())); }, "Re-run agentic" }
                            button { class: "btn small", onclick: move |_| svc::continue_without(ctx, &c), "Continue without it" }
                            button { class: "btn small", onclick: move |_| svc::cancel(ctx, &d), "Cancel" }
                        }
                    }
                }
            }
        }
        Stage::Ready if svc::queue_position(ctx, &id).is_some() => {
            let pos = state::ordinal(svc::queue_position(ctx, &id).unwrap_or(1));
            rsx! {
                div { class: "status-card",
                    span { class: "msg", "Queued ({pos}) — waiting for a free agent slot." }
                    div { class: "actions",
                        button { class: "btn small", onclick: move |_| svc::cancel(ctx, &id_), "Cancel" }
                    }
                }
            }
        }
        Stage::ClaudeReviewing | Stage::Polishing => {
            let rv = run.clone();
            let line = rv.as_ref().map_or_else(|| "Starting…".into(), |r| r.line.clone());
            let el = rv.as_ref().map(|r| mmss(now - r.started)).unwrap_or_default();
            let cost = rv.as_ref().and_then(|r| r.cost).map(|c| format!(" · ${c:.2}")).unwrap_or_default();
            let what = if e.rec.stage == Stage::Polishing { "Polishing" } else { "Reviewing" };
            rsx! {
                div { class: "status-card", "aria-live": "polite",
                    span { class: "msg",
                        strong { "{what}: " }
                        "{line}"
                        span { class: "muted num", " · {el}{cost}" }
                    }
                    div { class: "actions",
                        button { class: "btn small", onclick: move |_| show_log.toggle(), if show_log() { "▾ Hide log" } else { "▸ Show log" } }
                        button { class: "btn small", title: "Cancel (⌘.)", onclick: move |_| svc::cancel(ctx, &id_), "Cancel" span { class: "kbd", "⌘." } }
                    }
                    if show_log() {
                        RunLog { id: id.clone() }
                    }
                }
            }
        }
        Stage::Error(p) => {
            let msg = e.rec.stage_detail.clone().unwrap_or_else(|| "Something went wrong.".into());
            let what = match p {
                Phase::AwaitingAgentic => "Agentic step failed",
                Phase::ClaudeReviewing => "Review failed",
                Phase::Polishing => "Polish failed",
            };
            let b = id.clone();
            rsx! {
                div { class: "status-card bad", role: "alert",
                    span { class: "msg", strong { "{what}: " } span { class: "selectable", "{msg}" } }
                    div { class: "actions",
                        button { class: "btn small primary", onclick: move |_| svc::retry(ctx, &id_), "Retry" }
                        button { class: "btn small", onclick: move |_| show_log.toggle(), "View log" }
                        button { class: "btn small", onclick: move |_| svc::discard(ctx, &b), "Discard" }
                    }
                    if show_log() {
                        RunLog { id: id.clone() }
                    }
                }
            }
        }
        Stage::Interrupted(p) => {
            let what = if p == Phase::Polishing { "polish" } else { "review" };
            let b = id.clone();
            rsx! {
                div { class: "status-card warn",
                    span { class: "msg", "The {what} run was interrupted when the app quit." }
                    div { class: "actions",
                        button { class: "btn small primary", onclick: move |_| svc::retry(ctx, &id_), "Retry" }
                        button { class: "btn small", onclick: move |_| svc::discard(ctx, &b), "Discard" }
                    }
                }
            }
        }
        _ => rsx! {},
    };

    let moved = match (head_moved, draft_head) {
        (Some(cmp), Some(old))
            if matches!(e.rec.stage, Stage::ClaudeReviewing | Stage::Polishing | Stage::DraftReady)
                && old != e.summary.head_sha =>
        {
            let n = cmp.titles.len();
            let commits_text = format!("+{n} commit{} since review", if n == 1 { "" } else { "s" });
            let titles = cmp.titles.join(", ");
            let url = format!("https://github.com/{}/compare/{}...{}", e.summary.repo, old, e.summary.head_sha);
            let (a, b) = (id.clone(), id.clone());
            let mut hm = ctx.head_moved;
            rsx! {
                div { class: "status-card warn",
                    span { class: "msg",
                        "{commits_text}"
                        if !titles.is_empty() { ": {titles} " }
                        button { class: "link-btn", onclick: move |_| svc::open_url(&url), "[view]" }
                        span { class: "muted", " Comments on changed files are marked “may be outdated”." }
                    }
                    div { class: "actions",
                        button { class: "btn small", onclick: move |_| commands::request_start(ctx, &a), "Re-run" }
                        button { class: "btn small", onclick: move |_| { hm.write().remove(&b); }, "Keep draft" }
                    }
                }
            }
        }
        _ => rsx! {},
    };

    rsx! {
        {card}
        {moved}
    }
}

#[component]
pub fn RunLog(id: String) -> Element {
    let ctx = use_context::<Ctx>();
    let run = ctx.runs.read().get(&id).cloned();
    let last = use_resource({
        let id = id.clone();
        move || {
            let id = id.clone();
            async move { svc::svc().store.last_run(&id).ok().flatten() }
        }
    });
    let stored = last.read().clone().flatten();
    rsx! {
        div { class: "log", role: "log",
            match run {
                Some(ref r) if !r.log.is_empty() => rsx! {
                    for (i, l) in r.log.iter().enumerate() {
                        div { key: "{i}", class: if l.kind == "tool" { "tool" } else { "" },
                            span { class: "t", {l.at.with_timezone(&chrono::Local).format("%H:%M:%S").to_string()} }
                            "{l.text}"
                        }
                    }
                },
                _ => rsx! {
                    if let Some(s) = stored.clone() {
                        div { "Last run: {s.kind}, {s.status}" }
                        if let Some(err) = s.error.clone() { div { "Error: {err}" } }
                        if let Some(c) = s.cost_usd { div { "Cost: ${c:.2} · {s.turns.unwrap_or(0)} turns" } }
                    } else {
                        div { "No log for this PR yet." }
                    }
                },
            }
            if let Some(p) = run.and_then(|r| r.log_path).or(stored.and_then(|s| s.log_path)) {
                div { style: "margin-top: 6px",
                    button { class: "link-btn", onclick: move |_| svc::reveal(std::path::Path::new(&p)), "Reveal full log in Finder" }
                }
            }
        }
    }
}
