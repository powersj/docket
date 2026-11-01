//! The post confirmation sheet (design section 9).

use dioxus::prelude::*;

use crate::service::{self as svc, Ctx};

#[component]
pub fn ConfirmSheet() -> Element {
    let ctx = use_context::<Ctx>();
    let Some(c) = ctx.confirm.read().clone() else { return rsx! {} };
    let Some(e) = ctx.entry(&c.pr_id) else { return rsx! {} };
    let Some(cfg) = ctx.config() else { return rsx! {} };
    let head =
        ctx.drafts.read().get(&c.pr_id).map(|d| d.head_sha.chars().take(7).collect::<String>()).unwrap_or_default();
    let blocked = c.pending_review.is_some()
        || c.checking
        || c.posting
        || (c.error.is_some() && !c.unprocessable && !ctx.preflight.read().can_write());
    let paras =
        if c.summary_paras == 1 { "1 paragraph".to_string() } else { format!("{} paragraphs", c.summary_paras) };
    let excluded = if c.excluded > 0 {
        format!(
            ", {} excluded{}",
            c.excluded,
            c.excluded_reason.as_ref().map(|r| format!(" ({r})")).unwrap_or_default()
        )
    } else {
        String::new()
    };
    let mut confirm = ctx.confirm;
    let moved_text = c.head_moved.as_ref().map(|cmp| {
        let n = cmp.titles.len();
        let titles = if n > 0 { format!(" ({})", cmp.titles.join(", ")) } else { String::new() };
        format!(
            "⚠ PR has {} since this review{titles}. The review is posted on the commit it was written for.",
            if n == 0 {
                "new commits".to_string()
            } else if n == 1 {
                "1 new commit".to_string()
            } else {
                format!("{n} new commits")
            }
        )
    });
    let posting = c.posting;

    rsx! {
        div {
            class: "scrim",
            onclick: move |_| {
                if !posting {
                    confirm.set(None);
                }
            },
            div {
                class: "sheet",
                role: "dialog",
                "aria-modal": "true",
                "aria-labelledby": "confirm-title",
                onclick: move |ev| ev.stop_propagation(),
                h2 { id: "confirm-title",
                    "Post review as @{cfg.github.login} on {e.summary.repo} #{e.summary.number} @ "
                    span { class: "mono", "{head}" }
                }
                div { class: "row-kv",
                    label { r#for: "verdict", "Verdict" }
                    select {
                        id: "verdict",
                        disabled: c.posting,
                        onchange: move |ev| {
                            if let Some(x) = confirm.write().as_mut() {
                                x.verdict = ev.value();
                            }
                        },
                        option { value: "COMMENT", selected: c.verdict == "COMMENT", "Comment" }
                        option { value: "APPROVE", selected: c.verdict == "APPROVE", disabled: c.own_pr, "Approve" }
                        option { value: "REQUEST_CHANGES", selected: c.verdict == "REQUEST_CHANGES", disabled: c.own_pr, "Request changes" }
                    }
                    if c.own_pr {
                        span { class: "muted", "Your own PR: Comment only." }
                    }
                }
                div { class: "row-kv muted", "Summary: {paras} · Inline: {c.included} included{excluded}" }
                if c.checking {
                    div { class: "row-kv muted", "Checking GitHub…" }
                }
                if let Some(text) = moved_text.clone() {
                    div { class: "warn-box", "{text}" }
                }
                if let Some(url) = c.pending_review.clone() {
                    div { class: "bad-box",
                        "You have a pending review on GitHub; submit or discard it there before posting. "
                        button { class: "link-btn", onclick: move |_| svc::open_url(&url), "Open it" }
                        " · "
                        button {
                            class: "link-btn",
                            onclick: move |_| {
                                let id = c.pr_id.clone();
                                spawn(svc::open_confirm(ctx, id));
                            },
                            "Check again"
                        }
                    }
                }
                if let Some(err) = c.error.clone() {
                    div { class: "bad-box selectable", "{err}" }
                }
                div { class: "sheet-actions",
                    button { class: "btn", disabled: c.posting, onclick: move |_| confirm.set(None), "Cancel" }
                    span { class: "spacer" }
                    if c.unprocessable {
                        button {
                            class: "btn",
                            disabled: c.posting,
                            onclick: move |_| { spawn(svc::post(ctx, false, true)); },
                            "Post summary + valid comments only"
                        }
                    }
                    button {
                        class: "btn",
                        disabled: blocked,
                        title: "Leave the review pending so you can finish it on GitHub",
                        onclick: move |_| { spawn(svc::post(ctx, true, false)); },
                        "Post as pending"
                    }
                    button {
                        class: "btn primary",
                        disabled: blocked,
                        onclick: move |_| { spawn(svc::post(ctx, false, false)); },
                        if c.posting { "Posting…" } else { "Post" }
                        span { class: "kbd", "⌘↩" }
                    }
                }
            }
        }
    }
}
