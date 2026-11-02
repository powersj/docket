//! GFM rendering (comrak, raw HTML and unsafe links stripped) with class-based
//! syntect highlighting so light/dark themes switch via CSS.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::sync::OnceLock;

use comrak::adapters::SyntaxHighlighterAdapter;
use comrak::{Options, markdown_to_html_with_plugins, options::Plugins};
use syntect::html::{ClassStyle, ClassedHTMLGenerator, css_for_theme_with_class_style};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

const CLASS_STYLE: ClassStyle = ClassStyle::SpacedPrefixed { prefix: "sx-" };

fn syntaxes() -> &'static SyntaxSet {
    static S: OnceLock<SyntaxSet> = OnceLock::new();
    S.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn find_syntax(lang: Option<&str>) -> &'static SyntaxReference {
    let ss = syntaxes();
    let lang = lang.unwrap_or("").split_whitespace().next().unwrap_or("");
    let alias = match lang {
        "golang" => "go",
        "sh" | "shell" | "zsh" | "console" => "bash",
        "yml" => "yaml",
        "ts" | "tsx" | "typescript" => "js",
        "rs" => "rust",
        "py" => "python",
        "dockerfile" => "Dockerfile",
        other => other,
    };
    ss.find_syntax_by_token(alias)
        .or_else(|| ss.find_syntax_by_extension(alias))
        .unwrap_or_else(|| ss.find_syntax_plain_text())
}

pub fn syntax_for_path(path: &str) -> &'static SyntaxReference {
    let ss = syntaxes();
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.').map_or(name, |(_, e)| e);
    ss.find_syntax_by_extension(ext)
        .or_else(|| ss.find_syntax_by_extension(name))
        .unwrap_or_else(|| ss.find_syntax_plain_text())
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub fn highlight(code: &str, syntax: &SyntaxReference) -> String {
    let mut g = ClassedHTMLGenerator::new_with_class_style(syntax, syntaxes(), CLASS_STYLE);
    for line in LinesWithEndings::from(code) {
        if g.parse_html_for_line_which_includes_newline(line).is_err() {
            return esc(code);
        }
    }
    g.finalize()
}

/// Highlight each line separately but with shared parse state, for diff hunks.
pub fn highlight_lines(lines: &[&str], path: &str) -> Vec<String> {
    let syntax = syntax_for_path(path);
    lines
        .iter()
        .map(|l| {
            let mut g = ClassedHTMLGenerator::new_with_class_style(syntax, syntaxes(), CLASS_STYLE);
            match g.parse_html_for_line_which_includes_newline(&format!("{l}\n")) {
                Ok(()) => g.finalize().trim_end_matches('\n').to_string(),
                Err(_) => esc(l),
            }
        })
        .collect()
}

struct Highlighter;

impl SyntaxHighlighterAdapter for Highlighter {
    fn write_highlighted(&self, out: &mut dyn fmt::Write, lang: Option<&str>, code: &str) -> fmt::Result {
        out.write_str(&highlight(code, find_syntax(lang)))
    }

    fn write_pre_tag(&self, out: &mut dyn fmt::Write, attrs: HashMap<&'static str, Cow<'_, str>>) -> fmt::Result {
        write_tag(out, "pre", &attrs)
    }

    fn write_code_tag(&self, out: &mut dyn fmt::Write, attrs: HashMap<&'static str, Cow<'_, str>>) -> fmt::Result {
        write_tag(out, "code", &attrs)
    }
}

fn write_tag(out: &mut dyn fmt::Write, tag: &str, attrs: &HashMap<&'static str, Cow<'_, str>>) -> fmt::Result {
    write!(out, "<{tag}")?;
    let mut keys: Vec<_> = attrs.keys().collect();
    keys.sort();
    for k in keys {
        write!(out, " {k}=\"{}\"", esc(&attrs[k]))?;
    }
    write!(out, ">")
}

fn options() -> Options<'static> {
    let mut o = Options::default();
    o.extension.strikethrough = true;
    o.extension.table = true;
    o.extension.autolink = true;
    o.extension.tasklist = true;
    o.render.r#unsafe = false;
    o.render.github_pre_lang = true;
    o
}

/// Render untrusted markdown to safe HTML.
pub fn render(md: &str) -> String {
    let opts = options();
    let mut plugins = Plugins::default();
    let h = Highlighter;
    plugins.render.codefence_syntax_highlighter = Some(&h);
    // With `unsafe` off, comrak drops raw HTML and javascript:/vbscript:/data: links.
    markdown_to_html_with_plugins(md, &opts, &plugins)
}

/// CSS for both syntax themes, switched by prefers-color-scheme.
pub fn syntax_css() -> String {
    let themes = syntect::highlighting::ThemeSet::load_defaults();
    let light = css_for_theme_with_class_style(&themes.themes["InspiredGitHub"], CLASS_STYLE).unwrap_or_default();
    let dark = css_for_theme_with_class_style(&themes.themes["base16-ocean.dark"], CLASS_STYLE).unwrap_or_default();
    // Themes set their own backgrounds on the root class; let our CSS own those.
    let strip_bg = |css: String| css.replace("background-color", "--sx-bg");
    format!("{}\n@media (prefers-color-scheme: dark) {{\n{}\n}}\n", strip_bg(light), strip_bg(dark))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_raw_html_and_js_links() {
        let html = render("<script>alert(1)</script>\n\n[x](javascript:alert(1)) <img src=x onerror=alert(1)>");
        assert!(!html.contains("<script"));
        assert!(!html.contains("javascript:"));
        assert!(!html.contains("<img"));
    }

    #[test]
    fn gfm_features() {
        let html = render("| a | b |\n|---|---|\n| 1 | 2 |\n\n- [x] done\n\n~~gone~~ https://example.com");
        assert!(html.contains("<table>"));
        assert!(html.contains("checkbox"));
        assert!(html.contains("<del>"));
        assert!(html.contains("href=\"https://example.com\""));
    }

    #[test]
    fn code_is_highlighted_with_classes() {
        let html = render("```go\nfunc main() {}\n```\n");
        assert!(html.contains("sx-"), "{html}");
        assert!(html.contains("lang=\"go\""), "{html}");
        let lines = highlight_lines(&["fn x() {}", "<b>"], "a.rs");
        assert_eq!(lines.len(), 2);
        assert!(!lines[1].contains("<b>") && lines[1].contains("&lt;"));
        assert!(syntax_css().contains("prefers-color-scheme"));
    }

    /// Every HTML tag in `html`, as (lowercase name, lowercase attribute text).
    fn tags(html: &str) -> Vec<(String, String)> {
        let mut out = vec![];
        let mut rest = html;
        while let Some(start) = rest.find('<') {
            let Some(len) = rest[start..].find('>') else { break };
            let inner = rest[start + 1..start + len].trim_start_matches('/').to_lowercase();
            let (name, attrs) = inner.split_once(char::is_whitespace).unwrap_or((&inner, ""));
            out.push((name.to_string(), attrs.to_string()));
            rest = &rest[start + len + 1..];
        }
        out
    }

    // Property test: render() turns untrusted PR text into HTML for the
    // webview. Whatever mix of XSS payloads and markdown proptest builds, the
    // output must contain no script-capable tags, event handlers, or
    // javascript:/vbscript:/data: URLs.
    proptest::proptest! {
        #[test]
        fn rendered_markdown_has_no_script_vectors(
            parts in proptest::collection::vec(
                proptest::prop_oneof![
                    proptest::strategy::Just("<script>alert(1)</script>"),
                    proptest::strategy::Just("<img src=x onerror=alert(1)>"),
                    proptest::strategy::Just("<svg onload=alert(1)>"),
                    proptest::strategy::Just("<iframe src=javascript:alert(1)>"),
                    proptest::strategy::Just("[x](javascript:alert(1))"),
                    proptest::strategy::Just("[x](JaVaScRiPt:alert(1))"),
                    proptest::strategy::Just("[x](vbscript:msgbox(1))"),
                    proptest::strategy::Just("[x](data:text/html,<script>alert(1)</script>)"),
                    proptest::strategy::Just("![i](javascript:alert(1))"),
                    proptest::strategy::Just("<a href=\"javascript:alert(1)\">x</a>"),
                    proptest::strategy::Just("[x](&#106;avascript:alert(1))"),
                    proptest::strategy::Just("```html\n<script>x</script>\n```"),
                    proptest::strategy::Just("`<b>`"),
                    proptest::strategy::Just("\n\n"),
                    proptest::strategy::Just(" "),
                    proptest::strategy::Just("| a |\n|---|\n| <b> |"),
                ],
                1..10,
            )
        ) {
            let html = render(&parts.concat());
            for (name, attrs) in tags(&html) {
                let bad_tag = ["script", "iframe", "svg", "object", "embed", "style", "math"];
                proptest::prop_assert!(!bad_tag.contains(&name.as_str()), "<{name}> in {html}");
                proptest::prop_assert!(!attrs.split_whitespace().any(|a| a.starts_with("on")), "handler in {html}");
                for url_attr in ["href=\"", "src=\""] {
                    if let Some(v) = attrs.split(url_attr).nth(1) {
                        let v = v.trim_start();
                        for scheme in ["javascript:", "vbscript:", "data:"] {
                            proptest::prop_assert!(!v.starts_with(scheme), "{scheme} URL in {html}");
                        }
                    }
                }
            }
        }
    }
}
