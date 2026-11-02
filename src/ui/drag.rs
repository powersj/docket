//! Native window dragging from the top bars.
//!
//! WKWebView ignores the CSS drag-region property, and starting a drag from a
//! Dioxus mousedown handler is unreliable: the event reaches Rust after AppKit
//! has moved on, so the drag often never begins ("Window move completed
//! without beginning"). Instead the page reports where its drag regions and
//! the controls inside them are, and a local AppKit event monitor starts the
//! drag synchronously with the real mouse-down.

use std::ptr::NonNull;
use std::sync::{Mutex, Once};

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSEvent, NSEventMask};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// Rects in CSS pixels, as the page reports them.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Regions {
    pub drag: Vec<Rect>,
    pub no_drag: Vec<Rect>,
}

impl Regions {
    pub fn hit(&self, x: f64, y: f64) -> bool {
        self.drag.iter().any(|r| r.contains(x, y)) && !self.no_drag.iter().any(|r| r.contains(x, y))
    }
}

static REGIONS: Mutex<(Regions, f64)> = Mutex::new((Regions { drag: vec![], no_drag: vec![] }, 1.0));

pub fn set_regions(r: Regions) {
    REGIONS.lock().unwrap().0 = r;
}

/// The webview zoom level: CSS pixels × zoom = window points.
pub fn set_zoom(z: f64) {
    REGIONS.lock().unwrap().1 = z;
}

/// Reports drag regions (and the controls inside them) whenever layout
/// changes. While a sheet or popup is open nothing drags.
pub const REGIONS_JS: &str = r"
const controls = 'button, input, select, textarea, a, label, [data-no-drag]';
function rect(el) { const r = el.getBoundingClientRect(); return {x: r.left, y: r.top, w: r.width, h: r.height}; }
let last = '';
function report() {
  const overlay = document.querySelector('.scrim, .popup');
  const regions = overlay ? [] : [...document.querySelectorAll('[data-drag-region]')];
  const drag = regions.map(rect);
  const no_drag = regions.flatMap(r => [...r.querySelectorAll(controls)].map(rect))
    .concat([...document.querySelectorAll('.splitter')].map(rect));
  const msg = JSON.stringify({drag, no_drag});
  if (msg !== last) { last = msg; dioxus.send({drag, no_drag}); }
}
let queued = false;
function schedule() { if (!queued) { queued = true; requestAnimationFrame(() => { queued = false; report(); }); } }
new MutationObserver(schedule).observe(document.body, {childList: true, subtree: true, attributes: true});
window.addEventListener('resize', schedule);
report();
await new Promise(() => {});
";

/// Install the mouse-down monitor once, on the main thread.
pub fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let block = RcBlock::new(|ev: NonNull<NSEvent>| -> *mut NSEvent {
            let Some(mtm) = MainThreadMarker::new() else { return ev.as_ptr() };
            // SAFETY: AppKit hands the monitor a valid event.
            let e = unsafe { ev.as_ref() };
            let Some(win) = e.window(mtm) else { return ev.as_ptr() };
            let Some(view) = win.contentView() else { return ev.as_ptr() };
            let height = view.frame().size.height;
            let p = e.locationInWindow();
            let hit = {
                let g = REGIONS.lock().unwrap();
                let zoom = if g.1 > 0.0 { g.1 } else { 1.0 };
                // Window coordinates start at the bottom left; CSS at the top left.
                g.0.hit(p.x / zoom, (height - p.y) / zoom)
            };
            if !hit {
                return ev.as_ptr();
            }
            if e.clickCount() == 2 {
                win.performZoom(None);
            } else {
                win.performWindowDragWithEvent(e);
            }
            // Handled: the webview doesn't see this click.
            std::ptr::null_mut()
        });
        // SAFETY: called on the main thread; the block returns the event or null.
        let monitor =
            unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::LeftMouseDown, &block) };
        // The monitor lives for the life of the app.
        std::mem::forget(monitor);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_inside_regions_are_excluded() {
        let r = Regions {
            drag: vec![Rect { x: 0.0, y: 0.0, w: 300.0, h: 52.0 }],
            no_drag: vec![Rect { x: 250.0, y: 12.0, w: 28.0, h: 28.0 }],
        };
        assert!(r.hit(100.0, 20.0));
        assert!(!r.hit(260.0, 20.0), "button");
        assert!(!r.hit(100.0, 60.0), "below the bar");
        assert!(!Regions::default().hit(1.0, 1.0));
    }
}
