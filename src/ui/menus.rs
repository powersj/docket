//! Native menus. A custom menu replaces Dioxus's default, so the Edit menu is
//! rebuilt here, or ⌘C/⌘V/⌘Z stop working in text fields.

use std::cell::RefCell;
use std::collections::HashMap;

use dioxus::desktop::muda::accelerator::{Accelerator, Code, Modifiers};
use dioxus::desktop::muda::{AboutMetadata, CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};

use crate::state::Stage;

thread_local! {
    static ITEMS: RefCell<HashMap<&'static str, MenuItem>> = RefCell::new(HashMap::new());
    static CHECKS: RefCell<HashMap<&'static str, CheckMenuItem>> = RefCell::new(HashMap::new());
}

#[expect(clippy::unnecessary_wraps, reason = "every caller passes it straight to an `Option<Accelerator>` parameter")]
fn acc(m: Modifiers, c: Code) -> Option<Accelerator> {
    Some(Accelerator::new(Some(m), c))
}

const CMD: Modifiers = Modifiers::SUPER;

fn item(id: &'static str, text: &str, a: Option<Accelerator>) -> MenuItem {
    let i = MenuItem::with_id(id, text, true, a);
    ITEMS.with(|m| m.borrow_mut().insert(id, i.clone()));
    i
}

fn check(id: &'static str, text: &str, a: Option<Accelerator>) -> CheckMenuItem {
    let i = CheckMenuItem::with_id(id, text, true, false, a);
    CHECKS.with(|m| m.borrow_mut().insert(id, i.clone()));
    i
}

pub fn build() -> Menu {
    let menu = Menu::new();
    let sep = PredefinedMenuItem::separator;

    let app = Submenu::with_items(
        "Reviewer",
        true,
        &[
            &PredefinedMenuItem::about(
                None,
                Some(AboutMetadata {
                    name: Some("Reviewer".into()),
                    version: Some(env!("CARGO_PKG_VERSION").into()),
                    ..Default::default()
                }),
            ),
            &sep(),
            &item("reveal_config", "Reveal Config in Finder", acc(CMD, Code::Comma)),
            &sep(),
            &PredefinedMenuItem::services(None),
            &sep(),
            &PredefinedMenuItem::hide(None),
            &PredefinedMenuItem::hide_others(None),
            &PredefinedMenuItem::show_all(None),
            &sep(),
            &PredefinedMenuItem::quit(None),
        ],
    )
    .unwrap();

    let edit = Submenu::with_items(
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(None),
            &PredefinedMenuItem::redo(None),
            &sep(),
            &PredefinedMenuItem::cut(None),
            &PredefinedMenuItem::copy(None),
            &PredefinedMenuItem::paste(None),
            &PredefinedMenuItem::select_all(None),
        ],
    )
    .unwrap();

    let view = Submenu::with_items(
        "View",
        true,
        &[
            &item("refresh", "Refresh", acc(CMD, Code::KeyR)),
            &item("filter", "Filter", acc(CMD, Code::KeyF)),
            &sep(),
            &item("tab1", "Overview", acc(CMD, Code::Digit1)),
            &item("tab2", "Prior Review", acc(CMD, Code::Digit2)),
            &item("tab3", "Review", acc(CMD, Code::Digit3)),
            &sep(),
            &check("compact", "Compact Rows", None),
            &check("split", "Split Preview", None),
            &item("write_preview", "Toggle Write / Preview", acc(CMD | Modifiers::SHIFT, Code::KeyP)),
            &check("show_hidden", "Show Ignored & Snoozed", None),
            &sep(),
            &item("zoom_in", "Zoom In", acc(CMD, Code::Equal)),
            &item("zoom_out", "Zoom Out", acc(CMD, Code::Minus)),
            &item("zoom_reset", "Actual Size", acc(CMD, Code::Digit0)),
            &sep(),
            &PredefinedMenuItem::fullscreen(None),
        ],
    )
    .unwrap();

    let snooze = Submenu::with_items(
        "Snooze",
        true,
        &[
            &item("snooze_tomorrow", "Tomorrow 9am", None),
            &item("snooze_monday", "Monday 9am", None),
            &item("snooze_week", "Next Week", None),
            &item("snooze_custom", "Custom…", None),
        ],
    )
    .unwrap();

    let pr = Submenu::with_items(
        "PR",
        true,
        &[
            &item("start", "Start Review", None),
            &item("rerun", "Re-run Review", None),
            &item("cancel", "Cancel Run", acc(CMD, Code::Period)),
            &sep(),
            &item("post", "Post…", acc(CMD, Code::Enter)),
            &sep(),
            &item("ignore", "Ignore", None),
            &snooze,
            &sep(),
            &item("open", "Open in GitHub", None),
        ],
    )
    .unwrap();

    let window = Submenu::with_items(
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(None),
            &PredefinedMenuItem::maximize(None),
            &sep(),
            &PredefinedMenuItem::bring_all_to_front(None),
        ],
    )
    .unwrap();
    window.set_as_windows_menu_for_nsapp();

    let help = Submenu::with_items("Help", true, &[&item("shortcuts", "Keyboard Shortcuts", None)]).unwrap();
    help.set_as_help_menu_for_nsapp();

    menu.append_items(&[&app, &edit, &view, &pr, &window, &help]).unwrap();
    menu
}

fn set(id: &str, enabled: bool) {
    ITEMS.with(|m| {
        if let Some(i) = m.borrow().get(id) {
            i.set_enabled(enabled);
        }
    });
}

pub fn set_checked(id: &str, on: bool) {
    CHECKS.with(|m| {
        if let Some(i) = m.borrow().get(id) {
            i.set_checked(on);
        }
    });
}

/// Enable PR menu items for the selected PR's stage.
pub fn update_for(stage: Option<Stage>, closed: bool, has_draft: bool) {
    let has = stage.is_some();
    let open = has && !closed;
    let active = stage.is_some_and(super::super::state::Stage::is_active);
    set("start", open && !active && !matches!(stage, Some(Stage::DraftReady | Stage::Posted | Stage::ReReviewNeeded)));
    set("rerun", open && !active && has_draft);
    set("cancel", active);
    set("post", open && has_draft && matches!(stage, Some(Stage::DraftReady)));
    set("ignore", has && !active);
    for s in ["snooze_tomorrow", "snooze_monday", "snooze_week", "snooze_custom"] {
        set(s, has && !active);
    }
    set("open", has);
    for t in ["tab1", "tab2", "tab3"] {
        set(t, has);
    }
}
