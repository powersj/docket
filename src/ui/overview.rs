//! Overview tab: description, checks, reviewers.

use dioxus::prelude::*;

use super::icons::CiGlyph;
use crate::github::model::{CiCounts, CiState};
use crate::markdown;
use crate::service::Ctx;

#[component]
pub fn Overview(id: String) -> Element {
    let ctx = use_context::<Ctx>();
    let body = ctx
        .entry(&id)
        .map(|e| e.summary.body)
        .filter(|b| !b.trim().is_empty())
        .unwrap_or_else(|| "_No description provided._".into());
    let html = use_memo(use_reactive!(|body| markdown::render(&body)));
    let Some(e) = ctx.entry(&id) else { return rsx! {} };
    let detail = e.detail.clone();
    rsx! {
        div { class: "h3", "Description" }
        div { class: "card", div { class: "card-pad markdown-body", dangerous_inner_html: "{html}" } }

        div { class: "h3", "Checks" }
        match detail.as_ref() {
            None => rsx! { div { class: "muted", "Loading…" } },
            Some(d) if d.checks.is_empty() => rsx! { div { class: "muted", "No checks on the head commit." } },
            Some(d) => rsx! {
                ul { class: "checks",
                    for (i, c) in d.checks.iter().enumerate() {
                        li { key: "{i}",
                            CiGlyph { ci: CiCounts { state: c.state, passed: u32::from(c.state == CiState::Success), failed: u32::from(c.state == CiState::Failure), pending: u32::from(c.state == CiState::Pending), total: 1 } }
                            match c.url.clone() {
                                Some(u) => rsx! { a { href: "{u}", "{c.name}" } },
                                None => rsx! { span { "{c.name}" } },
                            }
                        }
                    }
                }
            },
        }

        div { class: "h3", "Reviewers" }
        match detail.as_ref() {
            None => rsx! { div { class: "muted", "Loading…" } },
            Some(d) => {
                let revs = d.reviewers();
                if revs.is_empty() {
                    rsx! { div { class: "muted", "No reviewers yet." } }
                } else {
                    rsx! {
                        div { class: "kv",
                            for r in revs {
                                Fragment { key: "{r.name}",
                                    span { "@{r.name}" }
                                    span { class: "muted", {review_state(&r.state).to_string()} }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn review_state(s: &str) -> &str {
    match s {
        "requested" => "Requested",
        "APPROVED" => "Approved",
        "CHANGES_REQUESTED" => "Changes requested",
        "COMMENTED" => "Commented",
        "DISMISSED" => "Dismissed",
        other => other,
    }
}
