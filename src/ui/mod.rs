//! Dioxus UI (design section 10).

pub mod app;
pub mod commands;
pub mod comment_card;
pub mod confirm_sheet;
pub mod detail;
pub mod drag;
pub mod icons;
pub mod menus;
pub mod overview;
pub mod pr_list;
pub mod prior;
pub mod review;
pub mod status_card;
pub mod styles;

pub use app::App;

#[cfg(test)]
mod snapshot;
