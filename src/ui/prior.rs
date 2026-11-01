//! Prior review tab: what the CI agentic reviewer and I already said.

use dioxus::prelude::*;

use crate::markdown;
use crate::service::Ctx;
use crate::state::{self, SaidItem, Source};

#[component]
pub fn Prior(id: String) -> Element {
    let ctx = use_context::<Ctx>();
    let Some(e) = ctx.entry(&id) else { return rsx! {} };
    let Some(cfg) = ctx.config() else { return rsx! {} };
    let Some(d) = e.detail.as_ref() else { return rsx! { div { class: "muted", "Loading…" } } };
    let said = state::already_said(d, &cfg);
    let bot: Vec<SaidItem> = said.iter().filter(|s| s.source == Source::Bot).cloned().collect();
    let me: Vec<SaidItem> = said.iter().filter(|s| s.source == Source::Me).cloned().collect();
    rsx! {
        div { class: "h3", "CI agentic reviewer ({bot.len()})" }
        SaidList { items: bot, head: d.summary.head_sha.clone() }
        div { class: "h3", "Me ({me.len()})" }
        SaidList { items: me, head: d.summary.head_sha.clone() }
    }
}

#[component]
fn SaidList(items: Vec<SaidItem>, head: String) -> Element {
    if items.is_empty() {
        return rsx! { div { class: "muted", "Nothing yet." } };
    }
    rsx! {
        div { class: "card",
            for (i, s) in items.iter().enumerate() {
                SaidView { key: "{i}", item: s.clone(), head: head.clone() }
            }
        }
    }
}

#[component]
fn SaidView(item: SaidItem, head: String) -> Element {
    let body = item.body.clone();
    let html = use_memo(use_reactive!(|body| markdown::render(&body)));
    let when = item.created_at.with_timezone(&chrono::Local).format("%b %-d %H:%M").to_string();
    let commit = item.commit.as_deref().map(|c| c.chars().take(7).collect::<String>());
    let loc = match (&item.path, item.line) {
        (Some(p), Some(l)) => Some(format!("{p}:{l}")),
        (Some(p), None) => Some(p.clone()),
        _ => None,
    };
    let url = item.url.clone();
    rsx! {
        div { class: "said",
            div { class: "said-meta",
                strong { "@{item.author}" }
                span { "{item.kind}" }
                if let Some(l) = loc { span { class: "mono", "{l}" } }
                if let Some(c) = commit { span { class: "mono faint", "{c}" } }
                if item.outdated { span { class: "chip warn", "Outdated" } }
                span { class: "faint", "{when}" }
                if !url.is_empty() {
                    a { href: "{url}", style: "margin-left: auto", "View" }
                }
            }
            div { class: "said-body markdown-body", dangerous_inner_html: "{html}" }
        }
    }
}
