//! One place that maps menu items, keys and buttons to actions.

use dioxus::prelude::*;
use serde::Deserialize;

use crate::service::{self as svc, Ctx, Sheet, ToastAction};
use crate::state::{self, Row, Stage};

#[derive(Debug, Deserialize)]
pub struct KeyMsg {
    pub key: String,
}

/// The keyboard bridge: single-letter keys only when focus isn't in a text
/// field or the filter box.
pub const KEY_JS: &str = r#"
document.addEventListener('keydown', (e) => {
  const t = e.target;
  const editable = t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT' || t.isContentEditable);
  if (e.metaKey || e.ctrlKey || e.altKey) {
    if (!editable && e.metaKey && !e.shiftKey && e.key === 'z') { e.preventDefault(); dioxus.send({key: 'undo'}); }
    return;
  }
  if (editable) {
    if (e.key === 'Escape') { t.blur(); dioxus.send({key: 'leave-editor'}); }
    return;
  }
  const keys = ['j','k','Enter','r','i','s','o','[',']',' ','e','Escape','?','ArrowDown','ArrowUp'];
  if (keys.includes(e.key)) { e.preventDefault(); dioxus.send({key: e.key}); }
});
document.addEventListener('focusin', (e) => {
  const inList = !!(e.target && e.target.closest && e.target.closest('.list'));
  dioxus.send({key: inList ? 'focus-list' : 'focus-other'});
});
await new Promise(() => {});
"#;

pub fn default_tab(stage: Stage) -> usize {
    match stage {
        Stage::ClaudeReviewing
        | Stage::Polishing
        | Stage::DraftReady
        | Stage::Posted
        | Stage::ReReviewNeeded
        | Stage::Error(_)
        | Stage::Interrupted(_) => 2,
        _ => 0,
    }
}

pub fn current_tab(ctx: Ctx, id: &str, stage: Stage) -> usize {
    match ctx.ui.read().tab.get(id) {
        Some((s, t)) if *s == stage => *t,
        _ => default_tab(stage),
    }
}

pub fn set_tab(mut ctx: Ctx, n: usize) {
    let Some(id) = ctx.selected.read().clone() else { return };
    let Some(e) = ctx.entry(&id) else { return };
    ctx.ui.write().tab.insert(id, (e.rec.stage, n));
}

/// PR ids in the order the list shows them (collapsed groups skipped).
pub fn visible_ids(ctx: Ctx) -> Vec<String> {
    let Some(cfg) = ctx.config() else { return vec![] };
    let prs = ctx.prs.read();
    let queue = ctx.queue.read();
    let rows: Vec<Row> =
        prs.values().map(|e| Row { summary: &e.summary, rec: &e.rec, queued: queue.contains(&e.rec.id) }).collect();
    let ui = ctx.ui.read();
    let mut out = vec![];
    for s in state::sections(&rows, &cfg, ui.show_hidden, &ctx.filter.read()) {
        if ui.collapsed.contains(&s.key) {
            continue;
        }
        out.extend(s.ids.iter().cloned());
        if ui.expanded_bots.contains(&s.key) {
            out.extend(s.bot_ids.iter().cloned());
        }
    }
    out
}

fn move_selection(ctx: Ctx, delta: i32) {
    let ids = visible_ids(ctx);
    if ids.is_empty() {
        return;
    }
    let cur = ctx.selected.read().clone();
    let idx = cur.and_then(|c| ids.iter().position(|i| *i == c));
    let next = match idx {
        Some(i) => (i as i32 + delta).clamp(0, ids.len() as i32 - 1) as usize,
        None => 0,
    };
    svc::select(ctx, &ids[next]);
    document::eval(&format!(
        "const el = document.getElementById('row-{}'); if (el) el.scrollIntoView({{block: 'nearest'}});",
        dom_id(&ids[next])
    ));
}

pub fn dom_id(pr_id: &str) -> String {
    pr_id.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

fn selected(ctx: Ctx) -> Option<String> {
    ctx.selected.read().clone()
}

/// Start review / re-run, asking first if it would replace an edited draft.
pub fn request_start(mut ctx: Ctx, id: &str) {
    let edited = ctx.drafts.read().get(id).is_some_and(|d| d.edited)
        && ctx.entry(id).is_some_and(|e| !matches!(e.rec.stage, Stage::Posted | Stage::ReReviewNeeded));
    if edited {
        ctx.sheet.set(Some(Sheet::ReplaceDraft(id.to_string())));
    } else {
        spawn(svc::start_review(ctx, id.to_string()));
    }
}

/// The one primary action for the stage (section 10.3).
pub fn primary_label(stage: Stage, queued: bool) -> Option<&'static str> {
    Some(match stage {
        Stage::New | Stage::Triage | Stage::Ignored | Stage::Snoozed => "Start review",
        Stage::Ready if queued => return None,
        Stage::Ready => "Start review",
        Stage::AwaitingAgentic => "Continue without it",
        Stage::DraftReady => "Post",
        Stage::Posted | Stage::ReReviewNeeded => "Re-review",
        Stage::Error(_) | Stage::Interrupted(_) => "Retry",
        Stage::ClaudeReviewing | Stage::Polishing => return None,
    })
}

pub fn primary(ctx: Ctx, id: &str) {
    let Some(e) = ctx.entry(id) else { return };
    match e.rec.stage {
        Stage::AwaitingAgentic => svc::continue_without(ctx, id),
        Stage::DraftReady => {
            spawn(svc::open_confirm(ctx, id.to_string()));
        }
        Stage::Error(_) | Stage::Interrupted(_) => svc::retry(ctx, id),
        Stage::ClaudeReviewing | Stage::Polishing => {}
        _ => request_start(ctx, id),
    }
}

fn cards(ctx: Ctx, id: &str) -> Vec<u64> {
    let mut v = vec![0];
    if let Some(d) = ctx.drafts.read().get(id) {
        v.extend(d.comments.iter().map(|c| c.id));
    }
    v
}

fn move_card(mut ctx: Ctx, delta: i32) {
    let Some(id) = selected(ctx) else { return };
    let cs = cards(ctx, &id);
    let cur = *ctx.focus_card.read();
    let i = cur.and_then(|c| cs.iter().position(|x| *x == c));
    let n = match i {
        Some(i) => (i as i32 + delta).clamp(0, cs.len() as i32 - 1) as usize,
        None => 0,
    };
    ctx.focus_card.set(Some(cs[n]));
    set_tab(ctx, 2);
    document::eval(&format!(
        "const el = document.getElementById('card-{}'); if (el) el.scrollIntoView({{block: 'nearest'}});",
        cs[n]
    ));
}

pub fn toggle_include(ctx: Ctx, id: &str, cid: u64) {
    let Some(mut d) = ctx.drafts.read().get(id).cloned() else { return };
    if let Some(c) = d.comments.iter_mut().find(|c| c.id == cid) {
        c.include = !c.include;
        svc::save_draft(ctx, d);
    }
}

pub fn edit_card(mut ctx: Ctx, cid: u64) {
    ctx.preview_on.write().remove(&cid);
    ctx.focus_card.set(Some(cid));
    set_tab(ctx, 2);
    document::eval(&format!(
        "setTimeout(() => {{ const el = document.getElementById('editor-{cid}'); if (el) el.focus(); }}, 30);"
    ));
}

fn toggle_write_preview(mut ctx: Ctx) {
    let cid = ctx.focus_card.read().unwrap_or(0);
    let mut p = ctx.preview_on.write();
    if !p.remove(&cid) {
        p.insert(cid);
    }
}

pub fn snooze_cmd(mut ctx: Ctx, id: &str, preset: &str) {
    if preset == "custom" {
        ctx.sheet.set(Some(Sheet::Snooze(id.to_string())));
    } else {
        svc::snooze(ctx, id, svc::snooze_preset(preset));
    }
}

pub fn open_github(ctx: Ctx, id: &str) {
    if let Some(e) = ctx.entry(id) {
        svc::open_url(&e.summary.url);
    }
}

fn zoom(mut ctx: Ctx, f: impl Fn(f64) -> f64) {
    let z = f(ctx.ui.read().zoom).clamp(0.6, 2.0);
    ctx.ui.write().zoom = (z * 100.0).round() / 100.0;
    svc::save_ui_prefs(ctx);
}

pub fn menu(mut ctx: Ctx, mid: &str) {
    let sel = selected(ctx);
    match mid {
        "refresh" => {
            spawn(svc::refresh(ctx, true));
        }
        "filter" => {
            document::eval("const f = document.getElementById('filter'); if (f) { f.focus(); f.select(); }");
        }
        "tab1" => set_tab(ctx, 0),
        "tab2" => set_tab(ctx, 1),
        "tab3" => set_tab(ctx, 2),
        "compact" => {
            let v = !ctx.ui.read().compact;
            ctx.ui.write().compact = v;
            super::menus::set_checked("compact", v);
            svc::save_ui_prefs(ctx);
        }
        "split" => {
            let v = !ctx.ui.read().split_preview;
            ctx.ui.write().split_preview = v;
            super::menus::set_checked("split", v);
            svc::save_ui_prefs(ctx);
        }
        "show_hidden" => {
            let v = !ctx.ui.read().show_hidden;
            ctx.ui.write().show_hidden = v;
            super::menus::set_checked("show_hidden", v);
        }
        "write_preview" => toggle_write_preview(ctx),
        "zoom_in" => zoom(ctx, |z| z + 0.1),
        "zoom_out" => zoom(ctx, |z| z - 0.1),
        "zoom_reset" => zoom(ctx, |_| 1.0),
        "shortcuts" => ctx.sheet.set(Some(Sheet::Shortcuts)),
        "reveal_config" => {
            let p = crate::config::paths::config_file();
            if p.exists() {
                svc::reveal(&p);
            } else {
                svc::reveal(&crate::config::paths::support_dir());
            }
        }
        _ => {
            let Some(id) = sel else { return };
            match mid {
                "start" => request_start(ctx, &id),
                "rerun" => request_start(ctx, &id),
                "cancel" => svc::cancel(ctx, &id),
                "post" => {
                    if ctx.confirm.read().as_ref().is_some_and(|c| c.pr_id == id) {
                        spawn(svc::post(ctx, false, false));
                    } else if ctx.entry(&id).is_some_and(|e| e.rec.stage == Stage::DraftReady) {
                        spawn(svc::open_confirm(ctx, id));
                    }
                }
                "ignore" => svc::ignore(ctx, &id),
                "snooze_tomorrow" => snooze_cmd(ctx, &id, "tomorrow"),
                "snooze_monday" => snooze_cmd(ctx, &id, "monday"),
                "snooze_week" => snooze_cmd(ctx, &id, "week"),
                "snooze_custom" => snooze_cmd(ctx, &id, "custom"),
                "open" => open_github(ctx, &id),
                _ => {}
            }
        }
    }
}

pub fn key(mut ctx: Ctx, k: KeyMsg) {
    let sel = selected(ctx);
    match k.key.as_str() {
        "focus-list" => ctx.list_focused.set(true),
        "focus-other" => ctx.list_focused.set(false),
        "j" | "ArrowDown" => move_selection(ctx, 1),
        "k" | "ArrowUp" => move_selection(ctx, -1),
        "Enter" => {
            if sel.is_some() {
                ctx.list_focused.set(false);
                document::eval("const d = document.getElementById('detail'); if (d) d.focus();");
                if ctx.focus_card.read().is_none() {
                    ctx.focus_card.set(Some(0));
                }
            }
        }
        "?" => ctx.sheet.set(Some(Sheet::Shortcuts)),
        "Escape" => {
            if ctx.popup.read().is_some() {
                ctx.popup.set(None);
            } else if ctx.sheet.read().is_some() {
                ctx.sheet.set(None);
            } else if ctx.confirm.read().as_ref().is_some_and(|c| !c.posting) {
                ctx.confirm.set(None);
            } else {
                ctx.focus_card.set(None);
                document::eval("const l = document.getElementById('list'); if (l) l.focus();");
            }
        }
        "leave-editor" => {
            document::eval("const d = document.getElementById('detail'); if (d) d.focus();");
        }
        "undo" => {
            let undo = ctx.toasts.read().iter().rev().find_map(|t| match &t.action {
                Some((_, ToastAction::UndoIgnore(id))) => Some((t.id, id.clone())),
                _ => None,
            });
            if let Some((tid, id)) = undo {
                svc::unignore(ctx, &id);
                ctx.toasts.write().retain(|t| t.id != tid);
            }
        }
        "[" => move_card(ctx, -1),
        "]" => move_card(ctx, 1),
        other => {
            let Some(id) = sel else { return };
            match other {
                "r" => request_start(ctx, &id),
                "i" => svc::ignore(ctx, &id),
                "s" => ctx.sheet.set(Some(Sheet::Snooze(id))),
                "o" => open_github(ctx, &id),
                " " => {
                    let focused = *ctx.focus_card.read();
                    if let Some(c) = focused.filter(|c| *c != 0) {
                        toggle_include(ctx, &id, c);
                    }
                }
                "e" => {
                    let focused = ctx.focus_card.read().unwrap_or(0);
                    edit_card(ctx, focused);
                }
                _ => {}
            }
        }
    }
}
