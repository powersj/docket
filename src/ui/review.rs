//! Review tab: summary editor, inline comment cards, run log (design 10.4).

use dioxus::prelude::*;

use super::comment_card::{AddComment, CommentCard};
use super::icons::Icon;
use super::status_card::RunLog;
use crate::markdown;
use crate::service::{self as svc, Ctx};
use crate::state::Stage;

/// A Write|Preview markdown editor with a debounced (500 ms) save.
#[component]
pub fn MdEditor(
    editor_id: u64,
    value: String,
    on_save: EventHandler<String>,
    #[props(default)] suggest: Option<String>,
    #[props(default)] placeholder: Option<String>,
    #[props(default)] read_only: bool,
) -> Element {
    let ctx = use_context::<Ctx>();
    let mut text = use_signal(|| value.clone());
    let mut last_prop = use_signal(|| value.clone());
    let mut generation = use_signal(|| 0u64);
    // Outside changes (revert, re-run) replace the text.
    if *last_prop.peek() != value {
        last_prop.set(value.clone());
        text.set(value.clone());
    }
    let html = use_memo(move || markdown::render(&text.read()));
    let preview = ctx.preview_on.read().contains(&editor_id) || read_only;
    let mut preview_on = ctx.preview_on;
    let empty = text.read().trim().is_empty();
    let split_class = if preview { "md-split preview" } else { "md-split" };

    rsx! {
        div { class: "editor",
            if !read_only {
                div { style: "position: absolute; right: 8px; top: -30px;",
                    div { class: "editor-tabs", role: "tablist", "aria-label": "Editor mode",
                        button {
                            class: if !preview { "active" } else { "" },
                            role: "tab",
                            "aria-selected": "{!preview}",
                            onclick: move |_| { preview_on.write().remove(&editor_id); },
                            "Write"
                        }
                        button {
                            class: if preview { "active" } else { "" },
                            role: "tab",
                            "aria-selected": "{preview}",
                            title: "⌘⇧P",
                            onclick: move |_| { preview_on.write().insert(editor_id); },
                            "Preview"
                        }
                    }
                }
            }
            div { class: "{split_class}",
                if !read_only {
                    textarea {
                        id: "editor-{editor_id}",
                        class: "md-editor md-input",
                        spellcheck: "true",
                        placeholder: placeholder.clone().unwrap_or_else(|| "Write markdown…".into()),
                        "data-suggest": suggest.clone().unwrap_or_default(),
                        value: "{text}",
                        oninput: move |e| {
                            let v = e.value();
                            text.set(v.clone());
                            let g = *generation.peek() + 1;
                            generation.set(g);
                            spawn(async move {
                                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                                if *generation.peek() == g {
                                    on_save.call(v);
                                }
                            });
                        },
                        onblur: move |_| {
                            let v = text.peek().clone();
                            if v != *last_prop.peek() {
                                let g = *generation.peek() + 1;
                                generation.set(g);
                                on_save.call(v);
                            }
                        },
                    }
                }
                if empty {
                    div { class: "md-preview markdown-body empty-preview", "Nothing to preview" }
                } else {
                    div { class: "md-preview markdown-body", dangerous_inner_html: "{html}" }
                }
            }
        }
    }
}

#[component]
pub fn ReviewTab(id: String) -> Element {
    let mut ctx = use_context::<Ctx>();
    let log_open = use_signal(|| false);
    let mut adding = use_signal(|| false);
    let Some(e) = ctx.entry(&id) else { return rsx! {} };
    let draft = ctx.drafts.read().get(&id).cloned();
    let force_split = ctx.ui.read().split_preview;

    let Some(d) = draft else {
        let msg = match e.rec.stage {
            Stage::ClaudeReviewing => "Claude is reviewing. The draft appears here when it's done.",
            Stage::AwaitingAgentic => "Waiting for the CI agentic reviewer first.",
            Stage::Ready => "Starting soon.",
            _ => "No draft yet. Start review to generate one.",
        };
        return rsx! {
            div { class: "empty", style: "min-height: 200px", span { "{msg}" } }
            RunLogToggle { id: id.clone(), open: log_open }
        };
    };

    let posted = ctx.posted_drafts.read().get(&d.id).cloned();
    // Edits are fine during a polish run: polishing only fills `*_polished`.
    let read_only = posted.is_some();
    let focused = *ctx.focus_card.read() == Some(0);
    let suggests = match d.verdict_suggested.as_str() {
        "APPROVE" => "Approve",
        "REQUEST_CHANGES" => "Request changes",
        _ => "Comment",
    };
    let total = d.comments.len();
    let included = d.comments.iter().filter(|c| c.include && c.invalid.is_none()).count();
    let summary = d.summary().to_string();
    let polished_differs = d.summary_polished.is_some() && d.summary() != d.summary_raw;
    let d_save = d.clone();
    let d_revert = d.clone();
    let class = if force_split { "force-split" } else { "" };

    rsx! {
        div { class: "{class}",
            if let Some(p) = posted.clone() {
                div { class: "status-card", style: "margin: 0 0 12px",
                    span { class: "msg", "{posted_text(&p)}" }
                    div { class: "actions",
                        button {
                            class: "btn small",
                            onclick: move |_| svc::open_url(&p.url),
                            Icon { name: "external", size: 12 }
                            "Open on GitHub"
                        }
                    }
                }
            }
            div {
                id: "card-0",
                class: if focused { "card focused" } else { "card" },
                onclick: move |_| ctx.focus_card.set(Some(0)),
                div { class: "card-head",
                    span { class: "grow", strong { "Summary" } }
                    span { class: "chip", title: "The agent's verdict is only a suggestion; posting defaults to Comment.", "Claude suggests: {suggests}" }
                    if polished_differs && !read_only {
                        button {
                            class: "icon-btn small",
                            title: "Revert to original (pre-polish)",
                            "aria-label": "Revert summary to original",
                            onclick: move |ev| {
                                ev.stop_propagation();
                                let mut d = d_revert.clone();
                                d.summary_edited = Some(d.summary_raw.clone());
                                d.edited = true;
                                svc::save_draft(ctx, d);
                            },
                            Icon { name: "revert", size: 13 }
                        }
                    }
                }
                if let Some(w) = d.summary_warning.clone() {
                    div { class: "warnline", Icon { name: "warn", size: 12 } "{w}" }
                }
                div { style: "padding-top: 0; position: relative; margin-top: 0;",
                    MdEditor {
                        editor_id: 0,
                        value: summary,
                        read_only,
                        placeholder: "Review summary…".to_string(),
                        on_save: move |v: String| {
                            let mut d = ctx.drafts.read().get(&d_save.pr_id).cloned().unwrap_or(d_save.clone());
                            if d.summary() != v {
                                d.summary_edited = Some(v);
                                d.edited = true;
                                svc::save_draft(ctx, d);
                            }
                        },
                    }
                }
            }

            div { class: "h3", style: "display: flex; align-items: center; gap: 8px; margin-top: 22px;",
                span { "Inline comments ({total})" }
                span { class: "faint", "· {included} included" }
                span { style: "flex: 1" }
                if !read_only {
                    button { class: "btn small", onclick: move |_| adding.toggle(), Icon { name: "plus", size: 12 } "Add comment" }
                }
            }
            if adding() {
                AddComment { id: id.clone(), on_done: move |()| adding.set(false) }
            }
            if d.comments.is_empty() && !adding() {
                div { class: "muted", "No inline comments." }
            }
            for c in d.comments.iter() {
                CommentCard { key: "{c.id}", id: id.clone(), cid: c.id, read_only }
            }
            RunLogToggle { id: id.clone(), open: log_open }
        }
    }
}

fn posted_text(p: &crate::store::PostedRow) -> String {
    let what = if p.pending { "Saved as a pending review" } else { "Posted" };
    let when = p.posted_at.with_timezone(&chrono::Local).format("%b %-d %H:%M");
    let verdict = p.verdict.as_ref().map(|v| format!(" as {}", v.to_lowercase().replace('_', " "))).unwrap_or_default();
    format!("{what} {when}{verdict}. This draft is read-only.")
}

#[component]
fn RunLogToggle(id: String, open: Signal<bool>) -> Element {
    let mut open = open;
    rsx! {
        div { class: "h3", style: "margin-top: 26px",
            button { class: "link-btn", onclick: move |_| open.toggle(), if open() { "▾ Run log" } else { "▸ Run log" } }
        }
        if open() {
            RunLog { id: id.clone() }
        }
    }
}
