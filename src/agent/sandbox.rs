//! The sandboxed `claude` invocation (design section 8.1). Every flag here is
//! deliberate; `claude.extra_args` is appended last and can weaken them.

use std::path::Path;

use crate::config::ClaudeConfig;

/// Env vars removed from the agent's environment.
pub fn scrub_env(vars: impl Iterator<Item = (String, String)>) -> Vec<String> {
    // Claude's own credentials must survive or the CLI can't authenticate.
    const KEEP: &[&str] = &["CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_AUTH_TOKEN"];
    vars.map(|(k, _)| k)
        .filter(|k| {
            !KEEP.contains(&k.as_str())
                && (matches!(k.as_str(), "GH_TOKEN" | "GITHUB_TOKEN" | "GH_ENTERPRISE_TOKEN" | "SSH_AUTH_SOCK")
                    || k.ends_with("_TOKEN"))
        })
        .collect()
}

/// The allowlist, plus `git -C <worktree> ...` forms of each git rule since the
/// agent's cwd is an empty sandbox, not the repo.
pub fn allowed_tools(cfg: &ClaudeConfig, worktree: &Path) -> Vec<String> {
    let mut out = cfg.allowed_tools.clone();
    for t in &cfg.allowed_tools {
        if let Some(rest) = t.strip_prefix("Bash(git ") {
            out.push(format!("Bash(git -C {} {rest}", worktree.display()));
        }
    }
    out
}

fn common(cfg: &ClaudeConfig, schema: &str) -> Vec<String> {
    [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--json-schema",
        schema,
        "--no-session-persistence",
        "--setting-sources",
        "user",
        "--strict-mcp-config",
        "--mcp-config",
        r#"{"mcpServers":{}}"#,
        "--permission-mode",
        "dontAsk",
        "--append-system-prompt",
        super::prompt::SYSTEM_APPEND,
        "--max-turns",
    ]
    .into_iter()
    .map(String::from)
    .chain([cfg.max_turns.to_string(), "--max-budget-usd".into(), format!("{:.2}", cfg.max_budget_usd)])
    .collect()
}

pub fn review_args(cfg: &ClaudeConfig, schema: &str, worktree: &Path) -> Vec<String> {
    let mut a = common(cfg, schema);
    a.push("--add-dir".into());
    a.push(worktree.display().to_string());
    a.push("--tools".into());
    a.push("Read,Grep,Glob,Bash".into());
    a.push("--allowedTools".into());
    a.extend(allowed_tools(cfg, worktree));
    a.extend(cfg.extra_args.iter().cloned());
    a
}

pub fn polish_args(cfg: &ClaudeConfig, schema: &str) -> Vec<String> {
    let mut a = common(cfg, schema);
    // No tools at all. The skill is expanded from the prompt's leading /<skill>.
    a.push("--tools".into());
    a.push(String::new());
    a.extend(cfg.extra_args.iter().cloned());
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs_tokens_but_keeps_claude_auth() {
        let vars = [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "NPM_TOKEN",
            "SSH_AUTH_SOCK",
            "PATH",
            "HOME",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "TOKENIZER",
        ]
        .map(|k| (k.to_string(), "v".to_string()));
        let removed = scrub_env(vars.into_iter());
        assert_eq!(removed, vec!["GH_TOKEN", "GITHUB_TOKEN", "NPM_TOKEN", "SSH_AUTH_SOCK"]);
    }

    #[test]
    fn review_flags() {
        let cfg = ClaudeConfig::default();
        let a = review_args(&cfg, "{}", Path::new("/wt/pr-1"));
        let joined = a.join(" ");
        for must in [
            "--setting-sources user",
            "--strict-mcp-config",
            "--permission-mode dontAsk",
            "--tools Read,Grep,Glob,Bash",
            "--add-dir /wt/pr-1",
            "Bash(git -C /wt/pr-1 diff:*)",
            "--max-turns 60",
            "--no-session-persistence",
        ] {
            assert!(joined.contains(must), "missing {must}");
        }
        assert!(!joined.contains("bypassPermissions"));
        let p = polish_args(&cfg, "{}");
        let i = p.iter().position(|x| x == "--tools").unwrap();
        assert_eq!(p[i + 1], "");
        assert!(!p.iter().any(|x| x == "--add-dir"));
    }
}
