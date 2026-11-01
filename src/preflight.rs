//! gh / claude / skills / SSO checks (design section 4). Results gate writes
//! and agent runs.

use std::path::{Path, PathBuf};

use tokio::process::Command;

use crate::config::{Config, expand_tilde};
use crate::github::{GitHub, SsoError};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Check {
    pub ok: bool,
    /// Short footer text, e.g. "gh: powersj".
    pub summary: String,
    /// Why it failed, and how to fix it.
    pub problem: Option<String>,
    pub fix: Option<String>,
}

impl Check {
    fn ok(summary: impl Into<String>) -> Self {
        Self { ok: true, summary: summary.into(), problem: None, fix: None }
    }
    fn fail(summary: impl Into<String>, problem: impl Into<String>, fix: impl Into<String>) -> Self {
        Self { ok: false, summary: summary.into(), problem: Some(problem.into()), fix: Some(fix.into()) }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Report {
    pub gh: Check,
    /// SSO failure for an org, if any.
    pub sso: Option<String>,
    pub claude: Check,
    pub skills: Check,
    pub checked_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl Report {
    pub fn can_write(&self) -> bool {
        self.gh.ok && self.sso.is_none()
    }
    pub fn can_run_agent(&self) -> bool {
        self.claude.ok && self.skills.ok
    }
    pub fn fresh(&self) -> bool {
        self.checked_at.is_some_and(|t| chrono::Utc::now() - t < chrono::Duration::seconds(120))
    }
}

pub async fn check_gh(cfg: &Config, gh: &dyn GitHub) -> (Check, Option<String>) {
    let login = &cfg.github.login;
    if let Err(e) = crate::github::gh_token(login).await {
        return (
            Check::fail(
                format!("gh: {login}"),
                e.to_string(),
                format!(
                    "Install the GitHub CLI and run `gh auth login --hostname github.com`, then `gh auth switch --user {login}` if you use several accounts."
                ),
            ),
            None,
        );
    }
    gh.reset_token();
    match gh.viewer_login().await {
        Ok(v) if crate::config::same_login(&v, login) => {}
        Ok(v) => {
            return (
                Check::fail(
                    format!("gh: {login}"),
                    format!("gh token belongs to @{v}, not @{login}"),
                    format!("Run `gh auth login` as {login}, or fix github.login in config.toml."),
                ),
                None,
            );
        }
        Err(e) => {
            return (
                Check::fail(
                    format!("gh: {login}"),
                    e.to_string(),
                    "Check your network and `gh auth status`.".to_string(),
                ),
                None,
            );
        }
    }
    let sso = match cfg.watch.first() {
        Some(w) => match gh.probe_repo(&w.repo).await {
            Err(e) if e.downcast_ref::<SsoError>().is_some() => {
                let org = w.repo.split('/').next().unwrap_or(&w.repo).to_string();
                Some(org)
            }
            _ => None,
        },
        None => None,
    };
    (Check::ok(format!("gh: {login}")), sso)
}

pub fn parse_claude_version(out: &str) -> Option<semver::Version> {
    let tok = out.split_whitespace().next()?;
    semver::Version::parse(tok.trim_start_matches('v')).ok()
}

pub async fn check_claude(cfg: &Config) -> Check {
    let bin = &cfg.claude.binary;
    let min = cfg.claude.effective_min_version();
    let out = match Command::new(bin).arg("--version").output().await {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        Ok(o) => {
            return Check::fail(
                "claude",
                format!("`{bin} --version` failed: {}", String::from_utf8_lossy(&o.stderr).trim()),
                "Reinstall Claude Code.",
            );
        }
        Err(e) => {
            return Check::fail(
                "claude",
                format!("can't run {bin}: {e}"),
                "Install Claude Code, or set claude.binary in config.toml.",
            );
        }
    };
    let Some(v) = parse_claude_version(&out) else {
        return Check::fail(
            "claude",
            format!("unrecognised version output {:?}", out.trim()),
            "Set claude.binary to the Claude Code CLI.",
        );
    };
    if let Ok(minv) = semver::Version::parse(min)
        && v < minv
    {
        return Check::fail(format!("claude {v}"), format!("claude {v} is older than {minv}"), "Run `claude update`.");
    }
    match Command::new(bin).args(["auth", "status"]).output().await {
        Ok(o) if o.status.success() => Check::ok(format!("claude: ok ({v})")),
        _ => Check::fail(
            "claude: logged out",
            "`claude auth status` reports you're logged out",
            "Run `claude` and log in, or `claude auth login`.",
        ),
    }
}

fn skill_file(name: &str, explicit: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        let p = expand_tilde(p);
        let p = if p.is_dir() { p.join("SKILL.md") } else { p };
        return p.exists().then_some(p);
    }
    let home = dirs::home_dir()?;
    if let Some((plugin, skill)) = name.split_once(':') {
        return plugin_skill_file(&home.join(".claude/plugins/installed_plugins.json"), plugin, skill);
    }
    let p = home.join(".claude/skills").join(name).join("SKILL.md");
    p.exists().then_some(p)
}

/// `plugin:skill` → `<installPath>/skills/<skill>/SKILL.md` for a user-scope install.
fn plugin_skill_file(installed: &Path, plugin: &str, skill: &str) -> Option<PathBuf> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(installed).ok()?).ok()?;
    let plugins = v.get("plugins")?.as_object()?;
    plugins
        .iter()
        .filter(|(k, _)| k.split('@').next() == Some(plugin))
        .flat_map(|(_, installs)| installs.as_array().into_iter().flatten())
        .filter(|i| i.get("scope").and_then(|s| s.as_str()) == Some("user"))
        .filter_map(|i| i.get("installPath").and_then(|p| p.as_str()))
        .map(|p| PathBuf::from(p).join("skills").join(skill).join("SKILL.md"))
        .find(|p| p.exists())
}

/// Plugin skills are listed namespaced (`plugin:skill`) and configured the same way.
fn listed(name: &str, slash: &[String]) -> bool {
    slash.iter().any(|s| s == name)
}

/// `cached` is the slash-command list from the most recent init event.
pub fn check_skills(cfg: &Config, cached: Option<&[String]>) -> Check {
    let mut missing = vec![];
    for (name, path) in [
        (&cfg.claude.review_skill, cfg.claude.review_skill_path.as_deref()),
        (&cfg.claude.writing_skill, cfg.claude.writing_skill_path.as_deref()),
    ] {
        let found = skill_file(name, path).is_some() || cached.is_some_and(|c| listed(name, c));
        if !found {
            missing.push(name.clone());
        }
    }
    if missing.is_empty() {
        Check::ok("skills: ok")
    } else {
        Check::fail(
            "skills: missing",
            format!("skill(s) not found: {}", missing.join(", ")),
            "Install them under ~/.claude/skills/<name>/SKILL.md, or set claude.review_skill / writing_skill (or *_skill_path) in config.toml.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parsing() {
        assert_eq!(parse_claude_version("2.1.308 (Claude Code)\n").unwrap(), semver::Version::new(2, 1, 308));
        assert!(parse_claude_version("garbage").is_none());
    }

    #[test]
    fn skill_lookup_uses_cache_and_namespaces() {
        let cfg = Config::parse(
            "[github]\nlogin=\"me\"\n[claude]\nreview_skill=\"definitely-not-installed-x\"\nwriting_skill=\"plug:writer\"\n",
            Path::new("c"),
        )
        .unwrap();
        assert!(!check_skills(&cfg, None).ok);
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path().join("p");
        std::fs::create_dir_all(install.join("skills/writer")).unwrap();
        std::fs::write(install.join("skills/writer/SKILL.md"), "x").unwrap();
        let installed = dir.path().join("installed.json");
        std::fs::write(
            &installed,
            serde_json::json!({"plugins": {"plug@market": [{"scope": "user", "installPath": install}]}}).to_string(),
        )
        .unwrap();
        assert!(plugin_skill_file(&installed, "plug", "writer").is_some());
        assert!(plugin_skill_file(&installed, "plug", "other").is_none());

        let cache = vec!["definitely-not-installed-x".to_string(), "plug:writer".to_string()];
        assert!(check_skills(&cfg, Some(&cache)).ok);
        let partial = vec!["definitely-not-installed-x".to_string()];
        let c = check_skills(&cfg, Some(&partial));
        assert!(!c.ok && c.problem.unwrap().contains("plug:writer"));
    }
}
