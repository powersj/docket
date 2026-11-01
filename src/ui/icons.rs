//! Inline SVG glyphs. Status glyphs differ by shape as well as colour.

use dioxus::prelude::*;

use crate::github::model::{CiCounts, CiState};

#[component]
pub fn CiGlyph(ci: CiCounts) -> Element {
    let (class, label) = match ci.state {
        CiState::Success => ("ok", format!("CI passing, {} of {}", ci.passed, ci.total)),
        CiState::Failure => ("bad", format!("CI failing, {} of {}", ci.failed, ci.total)),
        CiState::Pending => ("pending", format!("CI pending, {} of {}", ci.pending, ci.total)),
        CiState::None => ("none", "No CI".to_string()),
    };
    rsx! {
        svg { class: "glyph {class}", width: "12", height: "12", view_box: "0 0 12 12", role: "img", "aria-label": "{label}",
            title { "{label}" }
            match ci.state {
                CiState::Success => rsx! {
                    circle { cx: "6", cy: "6", r: "6", fill: "currentColor" }
                    path { d: "M3.4 6.2l1.7 1.7 3.5-3.6", fill: "none", stroke: "var(--bg)", stroke_width: "1.6", stroke_linecap: "round", stroke_linejoin: "round" }
                },
                CiState::Failure => rsx! {
                    path { d: "M3 3l6 6M9 3l-6 6", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round" }
                },
                CiState::Pending => rsx! {
                    circle { cx: "6", cy: "6", r: "4.6", fill: "none", stroke: "currentColor", stroke_width: "1.8" }
                },
                CiState::None => rsx! {
                    path { d: "M3 6h6", stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round" }
                },
            }
        }
    }
}

#[component]
pub fn Icon(name: &'static str, size: Option<u32>) -> Element {
    let s = size.unwrap_or(14);
    let d = match name {
        "refresh" => "M13 8a5 5 0 1 1-1.46-3.54M13 2.5V5h-2.5",
        "more" => "",
        "search" => "M7 12A5 5 0 1 0 7 2a5 5 0 0 0 0 10zM14 14l-3.5-3.5",
        "chev-down" => "M4 6l4 4 4-4",
        "chev-right" => "M6 4l4 4-4 4",
        "edit" => "M10.5 2.5l3 3L6 13H3v-3z",
        "revert" => "M3 7h7a3 3 0 0 1 0 6H7M3 7l3-3M3 7l3 3",
        "close" => "M4 4l8 8M12 4l-8 8",
        "external" => "M9 3h4v4M13 3L7 9M11 9v4H3V5h4",
        "warn" => "M8 2l6.5 11.5h-13zM8 6.5v3M8 11.5v.2",
        "plus" => "M8 3v10M3 8h10",
        "check" => "M3 8.5l3 3 7-7",
        _ => "",
    };
    rsx! {
        svg { class: "glyph", width: "{s}", height: "{s}", view_box: "0 0 16 16", "aria-hidden": "true",
            if name == "more" {
                circle { cx: "3.5", cy: "8", r: "1.3", fill: "currentColor" }
                circle { cx: "8", cy: "8", r: "1.3", fill: "currentColor" }
                circle { cx: "12.5", cy: "8", r: "1.3", fill: "currentColor" }
            } else if name == "star" {
                path { d: "M8 1.8l1.9 3.9 4.2.6-3 3 .7 4.2L8 11.5l-3.8 2 .7-4.2-3-3 4.2-.6z", fill: "currentColor" }
            } else {
                path { d: "{d}", fill: "none", stroke: "currentColor", stroke_width: "1.5", stroke_linecap: "round", stroke_linejoin: "round" }
            }
        }
    }
}
