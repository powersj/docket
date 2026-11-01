//! Stylesheets and helper scripts injected into the page head.

pub const APP_CSS: &str = include_str!("../../assets/app.css");
pub const MARKDOWN_CSS: &str = include_str!("../../assets/github-markdown.css");
pub const EDITOR_JS: &str = include_str!("../../assets/editor.js");

pub fn head() -> String {
    format!(
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<style>{APP_CSS}</style>\n<style>{MARKDOWN_CSS}</style>\n<style>{}</style>\n<script>{EDITOR_JS}</script>",
        crate::markdown::syntax_css()
    )
}
