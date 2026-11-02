//! Inline comment cards with hunk context and re-anchoring (design 10.5).

use std::sync::Arc;

use dioxus::prelude::*;

use super::icons::Icon;
use super::review::MdEditor;
use crate::agent::output::{normalize, validate};
use crate::agent::schema::{AgentComment, DraftComment};
use crate::markdown;
use crate::repo::DiffIndex;
use crate::repo::diff::{DiffLine, LineKind, Side};
use crate::service::{self as svc, Ctx};

fn anchor_label(a: &AgentComment) -> String {
    if a.subject_type == "file" {
        return format!("{} (file)", a.path);
    }
    let side = a.side.as_deref().unwrap_or("RIGHT");
    match (a.start_line, a.line) {
        (Some(s), Some(l)) => format!("{}:{s}–{l} {side}", a.path),
        (None, Some(l)) => format!("{}:{l} {side}", a.path),
        _ => a.path.clone(),
    }
}

/// Re-anchor to a clicked hunk line; ⇧-click extends to a range.
fn reanchor(c: &mut DraftComment, line: &DiffLine, shift: bool, diff: &DiffIndex) {
    let side = if line.kind == LineKind::Del { Side::Left } else { Side::Right };
    let Some(n) = line.on(side) else { return };
    let a = &mut c.anchor;
    let same_side = a.side.as_deref().and_then(Side::parse) == Some(side);
    if shift
        && a.subject_type == "line"
        && same_side
        && let Some(cur_end) = a.line
    {
        let cur_start = a.start_line.unwrap_or(cur_end);
        let (s, e) = (cur_start.min(n), cur_end.max(n));
        a.start_line = (s != e).then_some(s);
        a.line = Some(e);
    } else {
        a.start_line = None;
        a.line = Some(n);
    }
    a.subject_type = "line".into();
    a.side = Some(side.as_str().into());
    a.start_side = None;
    normalize(a);
    let mut probe = a.clone();
    probe.body = c.body().to_string();
    c.invalid = validate(&probe, diff);
    c.include = c.invalid.is_none();
}

fn update_comment(ctx: Ctx, id: &str, cid: u64, f: impl FnOnce(&mut DraftComment, Option<&DiffIndex>)) {
    let Some(mut d) = ctx.drafts.read().get(id).cloned() else { return };
    let diff = ctx.diffs.read().get(id).cloned();
    if let Some(c) = d.comments.iter_mut().find(|c| c.id == cid) {
        f(c, diff.as_deref());
        d.edited = true;
        svc::save_draft(ctx, d);
    }
}

#[component]
pub fn CommentCard(id: String, cid: u64, read_only: bool) -> Element {
    let mut ctx = use_context::<Ctx>();
    let mut dup_open = use_signal(|| false);
    let Some(c) = ctx.drafts.read().get(&id).and_then(|d| d.comments.iter().find(|c| c.id == cid).cloned()) else {
        return rsx! {};
    };
    let diff = ctx.diffs.read().get(&id).cloned();
    let focused = *ctx.focus_card.read() == Some(cid);
    let a = c.anchor.clone();
    let label = anchor_label(&a);
    let suggest = diff.as_ref().and_then(|d| {
        let side = Side::parse(a.side.as_deref()?)?;
        d.text_of(&a.path, side, a.start_line.or(a.line)?, a.line?)
    });
    let class = format!(
        "card{}{}{}",
        if focused { " focused" } else { "" },
        if !c.include || c.invalid.is_some() { " excluded" } else { "" },
        if c.invalid.is_some() { " invalid" } else { "" }
    );
    let polished_differs = c.body_polished.is_some() && c.body() != c.body_raw;
    let (i1, i2, i3, i4, i5) = (id.clone(), id.clone(), id.clone(), id.clone(), id.clone());
    let include_label = if c.include { "Excluded from the review when unchecked" } else { "Include in the review" };

    rsx! {
        div { id: "card-{cid}", class: "{class}", onclick: move |_| ctx.focus_card.set(Some(cid)),
            div { class: "card-head",
                input {
                    r#type: "checkbox",
                    checked: c.include && c.invalid.is_none(),
                    disabled: read_only || c.invalid.is_some(),
                    title: "{include_label} (space)",
                    "aria-label": "Include comment on {label}",
                    onclick: move |ev| ev.stop_propagation(),
                    onchange: move |_| update_comment(ctx, &i1, cid, |c, _| c.include = !c.include),
                }
                span { class: "grow mono", "{label}" }
                if let Some(r) = c.invalid.clone() {
                    span { class: "chip warn", title: "{r}", Icon { name: "warn", size: 11 } "invalid line" }
                }
                if c.may_be_outdated {
                    span { class: "chip warn", title: "This file changed after the review", "may be outdated" }
                }
                if let Some(dup) = c.dup.clone() {
                    button {
                        class: "chip warn",
                        style: "border: 0",
                        "aria-expanded": "{dup_open}",
                        onclick: move |ev| { ev.stop_propagation(); dup_open.toggle(); },
                        Icon { name: "warn", size: 11 }
                        if dup.outdated { "possible dup (outdated) ▾" } else { "possible dup ▾" }
                    }
                }
                if !read_only {
                    button {
                        class: "icon-btn small",
                        title: "Edit (e)",
                        "aria-label": "Edit comment",
                        onclick: move |ev| { ev.stop_propagation(); super::commands::edit_card(ctx, cid); },
                        Icon { name: "edit", size: 13 }
                    }
                    if polished_differs {
                        button {
                            class: "icon-btn small",
                            title: "Revert to original (pre-polish)",
                            "aria-label": "Revert comment to original",
                            onclick: move |ev| {
                                ev.stop_propagation();
                                update_comment(ctx, &i2, cid, |c, _| c.body_edited = Some(c.body_raw.clone()));
                            },
                            Icon { name: "revert", size: 13 }
                        }
                    }
                    button {
                        class: "icon-btn small",
                        title: "Delete comment",
                        "aria-label": "Delete comment",
                        onclick: move |ev| {
                            ev.stop_propagation();
                            let current = ctx.drafts.read().get(&i3).cloned();
                            if let Some(mut d) = current {
                                d.comments.retain(|x| x.id != cid);
                                d.edited = true;
                                svc::save_draft(ctx, d);
                            }
                        },
                        Icon { name: "close", size: 13 }
                    }
                }
            }
            if dup_open() {
                if let Some(dup) = c.dup.clone() {
                    DupView { dup }
                }
            }
            if let Some(w) = c.polish_warning.clone() {
                div { class: "warnline", Icon { name: "warn", size: 12 } "{w}" }
            }
            if a.subject_type != "file" {
                match diff.clone() {
                    Some(d) => rsx! {
                        Hunk {
                            diff: d,
                            anchor: a.clone(),
                            invalid: c.invalid.clone(),
                            read_only,
                            on_line: move |(line, shift): (DiffLine, bool)| {
                                update_comment(ctx, &i4, cid, |c, diff| {
                                    if let Some(diff) = diff {
                                        reanchor(c, &line, shift, diff);
                                    }
                                });
                            },
                        }
                    },
                    None => rsx! { div { class: "hunk-hint", "Diff not loaded yet." } },
                }
            }
            div { class: "card-body",
                MdEditor {
                    editor_id: cid,
                    value: c.body().to_string(),
                    suggest,
                    read_only,
                    on_save: move |v: String| update_comment(ctx, &i5, cid, |c, _| {
                        if c.body() != v {
                            c.body_edited = Some(v);
                        }
                    }),
                }
            }
        }
    }
}

#[component]
fn DupView(dup: crate::agent::schema::DupInfo) -> Element {
    let html = markdown::render(&dup.body);
    let commit = dup.commit.as_deref().map(|c| c.chars().take(7).collect::<String>()).unwrap_or_default();
    let url = dup.url.clone();
    rsx! {
        div { class: "said", style: "background: var(--bg-subtle); border-top: 1px solid var(--border)",
            div { class: "said-meta",
                strong { "@{dup.author}" }
                span { "said earlier" }
                if !commit.is_empty() { span { class: "mono faint", "{commit}" } }
                if dup.outdated { span { class: "chip warn", "Outdated" } }
                if !url.is_empty() {
                    button { class: "link-btn", style: "margin-left: auto", onclick: move |_| svc::open_url(&url), "View" }
                }
            }
            div { class: "said-body markdown-body", dangerous_inner_html: "{html}" }
        }
    }
}

/// Hunk context: 3 lines above and 1 below the anchor. When the anchor isn't
/// in the diff, show the file's hunks so a line can be picked.
#[component]
fn Hunk(
    diff: Arc<DiffIndex>,
    anchor: AgentComment,
    invalid: Option<String>,
    read_only: bool,
    on_line: EventHandler<(DiffLine, bool)>,
) -> Element {
    let side = anchor.side.as_deref().and_then(Side::parse).unwrap_or(Side::Right);
    let ctx_lines = anchor.line.and_then(|l| diff.context(&anchor.path, side, anchor.start_line.unwrap_or(l), l, 3, 1));
    let rows: Vec<(DiffLine, bool)> = match ctx_lines {
        Some(r) if invalid.is_none() => r,
        _ => diff
            .file(&anchor.path)
            .map(|f| f.hunks.iter().flat_map(|h| h.lines.iter().map(|l| (l.clone(), false))).take(400).collect())
            .unwrap_or_default(),
    };
    if rows.is_empty() {
        return rsx! { div { class: "hunk-hint", "No diff lines for this file." } };
    }
    let texts: Vec<&str> = rows.iter().map(|(l, _)| l.text.as_str()).collect();
    let highlighted = markdown::highlight_lines(&texts, &anchor.path);
    let picking = invalid.is_some();
    rsx! {
        if picking && !read_only {
            div { class: "hunk-hint", "Click a line to re-anchor this comment (⇧-click for a range)." }
        }
        div { class: "hunk", style: if picking { "max-height: 320px; overflow-y: auto" } else { "" },
            for (i, ((line, anchored), html)) in rows.into_iter().zip(highlighted).enumerate() {
                {
                    let kind = match line.kind { LineKind::Add => "add", LineKind::Del => "del", LineKind::Ctx => "" };
                    let sign = match line.kind { LineKind::Add => "+", LineKind::Del => "−", LineKind::Ctx => " " };
                    let n = line.new.or(line.old).map(|n| n.to_string()).unwrap_or_default();
                    let cls = format!("hunk-row {kind}{}", if anchored { " anchored" } else { "" });
                    let l2 = line.clone();
                    rsx! {
                        div {
                            key: "{i}",
                            class: "{cls}",
                            title: if read_only { "" } else { "Click to anchor here, ⇧-click for a range" },
                            onclick: move |ev| {
                                ev.stop_propagation();
                                if !read_only {
                                    on_line.call((l2.clone(), ev.modifiers().shift()));
                                }
                            },
                            span { class: "ln num", "{n}" }
                            span { class: "sign", "{sign}" }
                            span { class: "code", dangerous_inner_html: "{html}" }
                            span { class: "mark", if anchored { "◄" } }
                        }
                    }
                }
            }
        }
    }
}

/// Add a comment: pick a changed file, then click a line in its hunks.
#[component]
pub fn AddComment(id: String, on_done: EventHandler<()>) -> Element {
    let ctx = use_context::<Ctx>();
    let mut file = use_signal(String::new);
    let Some(diff) = ctx.diffs.read().get(&id).cloned() else {
        return rsx! { div { class: "card", div { class: "add-comment muted", "The diff isn't loaded yet; run a review first." } } };
    };
    let files: Vec<String> = diff.files.iter().map(|f| f.path.clone()).collect();
    let id1 = id.clone();
    let id2 = id.clone();
    let add = move |id: &str, anchor: AgentComment| {
        let Some(mut d) = ctx.drafts.read().get(id).cloned() else { return };
        let cid = d.next_comment_id();
        d.comments.push(DraftComment {
            id: cid,
            anchor,
            body_raw: String::new(),
            body_polished: None,
            body_edited: Some(String::new()),
            include: true,
            invalid: None,
            dup: None,
            polish_warning: None,
            may_be_outdated: false,
        });
        d.edited = true;
        svc::save_draft(ctx, d);
        super::commands::edit_card(ctx, cid);
    };
    let picked = file.read().clone();
    let anchor = AgentComment {
        path: picked.clone(),
        subject_type: "line".into(),
        line: None,
        side: Some("RIGHT".into()),
        start_line: None,
        start_side: None,
        body: String::new(),
    };
    rsx! {
        div { class: "card",
            div { class: "add-comment",
                span { "File" }
                select {
                    "aria-label": "Changed file",
                    onchange: move |e| file.set(e.value()),
                    option { value: "", "Choose a changed file…" }
                    for f in files {
                        option { key: "{f}", value: "{f}", selected: f == picked, "{f}" }
                    }
                }
                if !picked.is_empty() {
                    button {
                        class: "btn small",
                        onclick: {
                            let p = picked.clone();
                            move |_| {
                                add(&id1, AgentComment { path: p.clone(), subject_type: "file".into(), line: None, side: None, start_line: None, start_side: None, body: String::new() });
                                on_done.call(());
                            }
                        },
                        "File comment"
                    }
                }
                span { style: "flex: 1" }
                button { class: "btn small", onclick: move |_| on_done.call(()), "Cancel" }
            }
            if !picked.is_empty() {
                Hunk {
                    diff: diff.clone(),
                    anchor,
                    invalid: Some("pick".to_string()),
                    read_only: false,
                    on_line: move |(line, _shift): (DiffLine, bool)| {
                        let side = if line.kind == LineKind::Del { Side::Left } else { Side::Right };
                        let a = AgentComment {
                            path: file.read().clone(),
                            subject_type: "line".into(),
                            line: line.on(side),
                            side: Some(side.as_str().into()),
                            start_line: None,
                            start_side: None,
                            body: String::new(),
                        };
                        add(&id2, a);
                        on_done.call(());
                    },
                }
            }
        }
    }
}
