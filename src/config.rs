//! Loading, validating and hot-reloading `config.toml` (design section 12).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

/// The version the sandbox flags (design 8.1) were verified against.
pub const DEFAULT_MIN_CLAUDE_VERSION: &str = "2.1.308";

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(with = "humantime_serde", default = "d_poll")]
    pub poll_interval: Duration,
    #[serde(default = "d_concurrent")]
    pub max_concurrent_reviews: usize,
    #[serde(default = "d_worktree_root")]
    pub worktree_root: String,
    #[serde(default = "d_true")]
    pub auto_unignore_on_new_commits: bool,
    #[serde(with = "humantime_serde", default = "d_log_retention")]
    pub log_retention: Duration,
    pub github: GithubConfig,
    #[serde(default)]
    pub agentic: AgenticConfig,
    #[serde(default)]
    pub claude: ClaudeConfig,
    #[serde(default)]
    pub filters: FiltersConfig,
    #[serde(default)]
    pub watch: Vec<Watch>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GithubConfig {
    pub login: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct AgenticConfig {
    pub label: String,
    pub bot_logins: Vec<String>,
    pub enabled_repos: Vec<String>,
    #[serde(with = "humantime_serde")]
    pub wait_poll_interval: Duration,
    #[serde(with = "humantime_serde")]
    pub wait_timeout: Duration,
}

impl Default for AgenticConfig {
    fn default() -> Self {
        Self {
            label: "agentic-review".into(),
            bot_logins: vec!["ai-reviewer".into()],
            enabled_repos: vec![],
            wait_poll_interval: Duration::from_secs(60),
            wait_timeout: Duration::from_mins(30),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct ClaudeConfig {
    pub binary: String,
    pub min_version: String,
    pub review_skill: String,
    pub writing_skill: String,
    pub review_skill_path: Option<String>,
    pub writing_skill_path: Option<String>,
    pub allowed_tools: Vec<String>,
    pub max_turns: u32,
    pub max_budget_usd: f64,
    #[serde(with = "humantime_serde")]
    pub run_timeout: Duration,
    pub max_inline_diff_lines: usize,
    pub already_said_max_chars: usize,
    pub extra_args: Vec<String>,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            binary: "claude".into(),
            min_version: String::new(),
            review_skill: "pr-reviewer".into(),
            writing_skill: "writing".into(),
            review_skill_path: None,
            writing_skill_path: None,
            allowed_tools: [
                "Read",
                "Grep",
                "Glob",
                "Bash(git diff:*)",
                "Bash(git log:*)",
                "Bash(git show:*)",
                "Bash(git blame:*)",
            ]
            .map(String::from)
            .to_vec(),
            max_turns: 60,
            max_budget_usd: 5.0,
            run_timeout: Duration::from_mins(20),
            max_inline_diff_lines: 3000,
            already_said_max_chars: 20000,
            extra_args: vec![],
        }
    }
}

impl ClaudeConfig {
    pub fn effective_min_version(&self) -> &str {
        if self.min_version.trim().is_empty() { DEFAULT_MIN_CLAUDE_VERSION } else { self.min_version.trim() }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct FiltersConfig {
    pub bot_authors: Vec<String>,
    pub hide_drafts: bool,
}

impl Default for FiltersConfig {
    fn default() -> Self {
        Self {
            bot_authors: ["dependabot", "renovate", "octo-sts", "github-actions"].map(String::from).to_vec(),
            hide_drafts: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Watch {
    pub repo: String,
    #[serde(default)]
    pub authors: Vec<String>,
}

fn d_poll() -> Duration {
    Duration::from_secs(300)
}
fn d_concurrent() -> usize {
    2
}
fn d_worktree_root() -> String {
    "~/Library/Caches/docket/worktrees".into()
}
fn d_true() -> bool {
    true
}
fn d_log_retention() -> Duration {
    Duration::from_hours(720)
}

/// A config parse/validation failure, with a location when we know it.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigError {
    pub path: PathBuf,
    pub line: Option<usize>,
    pub message: String,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.line {
            Some(l) => write!(f, "{}:{}: {}", self.path.display(), l, self.message),
            None => write!(f, "{}: {}", self.path.display(), self.message),
        }
    }
}

impl Config {
    pub fn parse(text: &str, path: &Path) -> Result<Config, ConfigError> {
        let cfg: Config = toml::from_str(text).map_err(|e| ConfigError {
            path: path.to_owned(),
            line: e.span().map(|s| text[..s.start.min(text.len())].lines().count().max(1)),
            message: e.message().to_string(),
        })?;
        cfg.validate().map_err(|message| ConfigError { path: path.to_owned(), line: None, message })?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), String> {
        if self.github.login.trim().is_empty() {
            return Err("github.login must be set".into());
        }
        if self.max_concurrent_reviews == 0 {
            return Err("max_concurrent_reviews must be at least 1".into());
        }
        if self.poll_interval < Duration::from_secs(30) {
            return Err("poll_interval must be at least 30s".into());
        }
        for w in &self.watch {
            if w.repo.split('/').filter(|s| !s.is_empty()).count() != 2 {
                return Err(format!("watch.repo {:?} must be owner/name", w.repo));
            }
        }
        if !self.claude.min_version.trim().is_empty() && semver::Version::parse(self.claude.min_version.trim()).is_err()
        {
            return Err(format!("claude.min_version {:?} is not a version", self.claude.min_version));
        }
        Ok(())
    }

    pub fn worktree_root(&self) -> PathBuf {
        expand_tilde(&self.worktree_root)
    }

    pub fn is_bot_login(&self, login: &str) -> bool {
        self.agentic.bot_logins.iter().any(|b| same_login(b, login))
    }

    pub fn is_bot_author(&self, login: &str) -> bool {
        self.filters.bot_authors.iter().any(|b| same_login(b, login))
    }

    pub fn agentic_enabled_for(&self, repo: &str) -> bool {
        self.agentic.enabled_repos.iter().any(|g| glob_match(&g.to_ascii_lowercase(), &repo.to_ascii_lowercase()))
    }

    /// Whether a PR from `repo` by `author` passes the watch filter.
    pub fn watches(&self, repo: &str, author: &str) -> bool {
        self.watch.iter().any(|w| {
            w.repo.eq_ignore_ascii_case(repo)
                && (w.authors.is_empty() || w.authors.iter().any(|a| same_login(a, author)))
        })
    }
}

/// Logins compare case-insensitively, ignoring an optional `[bot]` suffix
/// (GraphQL omits it, REST includes it).
pub fn normalize_login(login: &str) -> String {
    let l = login.trim().to_ascii_lowercase();
    l.strip_suffix("[bot]").map(str::to_owned).unwrap_or(l)
}

pub fn same_login(a: &str, b: &str) -> bool {
    normalize_login(a) == normalize_login(b)
}

/// `*` matches any run of characters (including `/`), `?` matches one.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(p)
}

pub mod paths {
    use std::path::PathBuf;

    fn home() -> PathBuf {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"))
    }
    pub fn support_dir() -> PathBuf {
        home().join("Library/Application Support/docket")
    }
    pub fn config_file() -> PathBuf {
        support_dir().join("config.toml")
    }
    pub fn db_file() -> PathBuf {
        support_dir().join("docket.db")
    }
    pub fn cache_dir() -> PathBuf {
        home().join("Library/Caches/docket")
    }
    pub fn sandbox_dir() -> PathBuf {
        cache_dir().join("sandbox")
    }
    pub fn log_dir() -> PathBuf {
        home().join("Library/Logs/docket")
    }
}

pub const EXAMPLE_CONFIG: &str = r#"poll_interval = "5m"
max_concurrent_reviews = 2
worktree_root = "~/Library/Caches/docket/worktrees"
auto_unignore_on_new_commits = true
log_retention = "30d"

[github]
login = "your-login"              # writes refused if gh isn't logged in as this account

[agentic]
label = "agentic-review"
bot_logins = ["ai-reviewer"]      # CI agentic reviewer account(s); case-insensitive, [bot] optional
enabled_repos = ["your-org/*"]    # globs; other repos skip the agentic step
wait_poll_interval = "60s"
wait_timeout = "30m"

[claude]
binary = "claude"
min_version = ""                  # empty = built-in minimum
review_skill = "pr-reviewer"
writing_skill = "writing"
max_turns = 60
max_budget_usd = 5.0
run_timeout = "20m"
max_inline_diff_lines = 3000
already_said_max_chars = 20000
extra_args = []                   # appended last; CAN weaken the sandbox

[filters]
bot_authors = ["dependabot", "renovate", "octo-sts", "github-actions"]
hide_drafts = false

[[watch]]
repo = "your-org/your-repo"
authors = []                      # empty = all authors
"#;

/// Holds the last config that parsed, plus the most recent error (if the file
/// on disk is currently broken). Running jobs clone the `Config` they start with.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConfigState {
    pub current: Option<Config>,
    pub error: Option<ConfigError>,
    pub missing: bool,
}

impl ConfigState {
    pub fn reload(&mut self, path: &Path) {
        match std::fs::read_to_string(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.missing = self.current.is_none();
                self.error = None;
            }
            Err(e) => self.error = Some(ConfigError { path: path.into(), line: None, message: e.to_string() }),
            Ok(text) => match Config::parse(&text, path) {
                Ok(c) => {
                    self.current = Some(c);
                    self.error = None;
                    self.missing = false;
                }
                Err(e) => self.error = Some(e),
            },
        }
    }
}

/// Write-back helpers that keep comments and formatting (right-click menu).
pub mod edit {
    use std::path::Path;
    use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table, value};

    fn load(path: &Path) -> anyhow::Result<DocumentMut> {
        Ok(std::fs::read_to_string(path)?.parse::<DocumentMut>()?)
    }

    pub fn add_watch(path: &Path, repo: &str) -> anyhow::Result<()> {
        let mut doc = load(path)?;
        let exists = doc
            .get("watch")
            .and_then(Item::as_array_of_tables)
            .is_some_and(|a| a.iter().any(|t| t.get("repo").and_then(|r| r.as_str()) == Some(repo)));
        if exists {
            return Ok(());
        }
        let mut t = Table::new();
        t["repo"] = value(repo);
        t["authors"] = value(Array::new());
        if let Some(a) = doc.get_mut("watch").and_then(Item::as_array_of_tables_mut) {
            a.push(t);
        } else {
            let mut a = ArrayOfTables::new();
            a.push(t);
            doc["watch"] = Item::ArrayOfTables(a);
        }
        std::fs::write(path, doc.to_string())?;
        Ok(())
    }

    /// Removes `author` from every watch entry for `repo` that lists authors.
    /// A watch with an empty author list (all authors) can't express "all but
    /// one", so the author goes into `filters.hidden_authors`-style bot list
    /// instead: they're collapsed out of sight with the bot PRs.
    pub fn hide_author(path: &Path, repo: &str, author: &str) -> anyhow::Result<()> {
        let mut doc = load(path)?;
        let mut handled = false;
        if let Some(a) = doc.get_mut("watch").and_then(Item::as_array_of_tables_mut) {
            for t in a.iter_mut() {
                if t.get("repo").and_then(|r| r.as_str()).is_some_and(|r| r.eq_ignore_ascii_case(repo))
                    && let Some(arr) = t.get_mut("authors").and_then(Item::as_array_mut)
                    && !arr.is_empty()
                {
                    arr.retain(|v| !v.as_str().is_some_and(|s| super::same_login(s, author)));
                    handled = true;
                }
            }
        }
        if !handled {
            if doc.get("filters").is_none() {
                doc["filters"] = toml_edit::table();
            }
            let filters = doc["filters"].as_table_mut().ok_or_else(|| anyhow::anyhow!("[filters] is not a table"))?;
            if filters.get("bot_authors").is_none() {
                let mut arr = Array::new();
                for d in super::FiltersConfig::default().bot_authors {
                    arr.push(d);
                }
                filters["bot_authors"] = value(arr);
            }
            let arr = filters["bot_authors"]
                .as_array_mut()
                .ok_or_else(|| anyhow::anyhow!("filters.bot_authors is not an array"))?;
            if !arr.iter().any(|v| v.as_str().is_some_and(|s| super::same_login(s, author))) {
                arr.push(author);
            }
        }
        std::fs::write(path, doc.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Config, ConfigError> {
        Config::parse(s, Path::new("config.toml"))
    }

    #[test]
    fn example_parses() {
        let c = parse(EXAMPLE_CONFIG).unwrap();
        assert_eq!(c.poll_interval, Duration::from_secs(300));
        assert_eq!(c.agentic.wait_timeout, Duration::from_mins(30));
        assert_eq!(c.claude.max_turns, 60);
        assert_eq!(c.watch.len(), 1);
    }

    #[test]
    fn defaults_fill_in() {
        let c = parse("[github]\nlogin = \"me\"\n").unwrap();
        assert_eq!(c.max_concurrent_reviews, 2);
        assert_eq!(c.agentic.bot_logins, vec!["ai-reviewer"]);
        assert_eq!(c.claude.allowed_tools.len(), 7);
        assert_eq!(c.claude.effective_min_version(), DEFAULT_MIN_CLAUDE_VERSION);
    }

    #[test]
    fn errors_have_line_numbers() {
        let e = parse("[github]\nlogin = \"me\"\npoll_interval = 5\n").unwrap_err();
        assert!(e.line.is_some(), "{e}");
        let e = parse("[github]\nlogin = \"me\"\n[bogus]\n").unwrap_err();
        assert_eq!(e.line, Some(3), "{e}");
        let e = parse("[github]\nlogin = \"\"\n").unwrap_err();
        assert!(e.message.contains("login"));
    }

    #[test]
    fn bot_login_normalization() {
        let c = parse("[github]\nlogin = \"me\"\n").unwrap();
        assert!(c.is_bot_login("ai-reviewer"));
        assert!(c.is_bot_login("AI-Reviewer[bot]"));
        assert!(!c.is_bot_login("ai-reviewer2"));
        assert!(c.is_bot_author("dependabot[bot]"));
        assert!(c.is_bot_author("Renovate"));
    }

    #[test]
    fn globs() {
        assert!(glob_match("chainguard-dev/*", "chainguard-dev/foo"));
        assert!(!glob_match("chainguard-dev/*", "other/foo"));
        assert!(glob_match("*/foo", "a/foo"));
        assert!(glob_match("a/f?o", "a/foo"));
        assert!(glob_match("*", "x/y"));
        assert!(!glob_match("a/b", "a/bc"));
        let c = parse("[github]\nlogin=\"me\"\n[agentic]\nenabled_repos=[\"Chainguard-dev/*\"]\n").unwrap();
        assert!(c.agentic_enabled_for("chainguard-dev/x"));
    }

    #[test]
    fn watch_filter() {
        let c =
            parse("[github]\nlogin=\"me\"\n[[watch]]\nrepo=\"o/a\"\nauthors=[\"Alice\"]\n[[watch]]\nrepo=\"o/b\"\n")
                .unwrap();
        assert!(c.watches("o/a", "alice"));
        assert!(!c.watches("o/a", "bob"));
        assert!(c.watches("o/b", "anyone"));
        assert!(!c.watches("o/c", "alice"));
    }

    #[test]
    fn last_good_kept_on_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        let mut st = ConfigState::default();
        st.reload(&p);
        assert!(st.missing);
        std::fs::write(&p, "[github]\nlogin=\"me\"\n").unwrap();
        st.reload(&p);
        assert!(st.current.is_some() && st.error.is_none());
        std::fs::write(&p, "[github\n").unwrap();
        st.reload(&p);
        assert!(st.current.is_some() && st.error.is_some());
    }

    #[test]
    fn write_back_keeps_comments() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(
            &p,
            "# top comment\n[github]\nlogin=\"me\" # me\n\n[[watch]]\nrepo=\"o/a\"\nauthors=[\"alice\",\"bob\"]\n",
        )
        .unwrap();
        edit::add_watch(&p, "o/b").unwrap();
        edit::add_watch(&p, "o/b").unwrap();
        edit::hide_author(&p, "o/a", "bob").unwrap();
        edit::hide_author(&p, "o/b", "carol").unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("# top comment") && text.contains("# me"));
        let c = Config::parse(&text, &p).unwrap();
        assert_eq!(c.watch.len(), 2);
        assert_eq!(c.watch[0].authors, vec!["alice"]);
        assert!(c.is_bot_author("carol"));
        assert!(c.is_bot_author("dependabot"));
    }
}
