//! Blobless clones per repo and a detached worktree per PR. All git operations
//! on a repo run under that repo's lock.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use base64::Engine;
use tokio::process::Command;

use super::diff::DiffIndex;
use crate::sync::MutexExt;

/// The result of preparing a PR for a run.
#[derive(Debug, Clone)]
pub struct Checkout {
    pub worktree: PathBuf,
    pub head: String,
    pub base: String,
    pub merge_base: String,
    pub diff_text: String,
    pub diff: DiffIndex,
}

pub struct Repos {
    root: PathBuf,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl Repos {
    pub fn new(root: PathBuf) -> Self {
        Self { root, locks: Mutex::new(HashMap::new()) }
    }

    fn lock_for(&self, repo: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.locks.locked().entry(repo.to_string()).or_default().clone()
    }

    pub fn repo_dir(&self, repo: &str) -> PathBuf {
        self.root.join(repo).join(".repo")
    }

    pub fn worktree_dir(&self, repo: &str, number: u64) -> PathBuf {
        self.root.join(repo).join(format!("pr-{number}"))
    }

    /// Clone if needed, fetch head + base, make a clean worktree at head, and
    /// compute the one diff (`git diff <merge-base> <head>`) used everywhere.
    pub async fn prepare(
        &self,
        repo: &str,
        number: u64,
        head: &str,
        base: &str,
        token: Option<&str>,
    ) -> Result<Checkout> {
        let lock = self.lock_for(repo);
        let _g = lock.lock().await;
        let git = Git { token: token.map(String::from) };
        let dir = self.repo_dir(repo);
        if !dir.join("HEAD").exists() && !dir.join(".git").exists() {
            if let Some(parent) = dir.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let url = format!("https://github.com/{repo}.git");
            git.run(None, &["clone", "--filter=blob:none", "--no-checkout", "--bare", &url, path_str(&dir)?]).await?;
        }
        git.run(Some(&dir), &["fetch", "--no-tags", "origin", head, base]).await.context("fetching PR commits")?;
        let mb = git.run(Some(&dir), &["merge-base", base, head]).await?.trim().to_string();

        let wt = self.worktree_dir(repo, number);
        if wt.exists() {
            // Reuse it if git still knows about it, otherwise start over.
            if git.run(Some(&wt), &["rev-parse", "--git-dir"]).await.is_err() {
                std::fs::remove_dir_all(&wt).ok();
                git.run(Some(&dir), &["worktree", "prune"]).await.ok();
            }
        }
        if wt.exists() {
            git.run(Some(&wt), &["checkout", "--detach", "--force", head]).await?;
        } else {
            git.run(Some(&dir), &["worktree", "add", "--detach", "--force", path_str(&wt)?, head]).await?;
        }
        git.run(Some(&wt), &["reset", "--hard", head]).await?;
        git.run(Some(&wt), &["clean", "-fdx"]).await?;

        let diff_text = git
            .run(Some(&dir), &["-c", "core.quotePath=false", "diff", "--no-color", "--no-ext-diff", "-M", &mb, head])
            .await?;
        let diff = DiffIndex::parse(&diff_text);
        Ok(Checkout { worktree: wt, head: head.into(), base: base.into(), merge_base: mb, diff_text, diff })
    }

    /// Diff only, for a worktree we already prepared (used to rebuild the hunk
    /// index for a stored draft).
    pub async fn diff(&self, repo: &str, mb: &str, head: &str) -> Result<DiffIndex> {
        let lock = self.lock_for(repo);
        let _g = lock.lock().await;
        let git = Git { token: None };
        let out = git
            .run(
                Some(&self.repo_dir(repo)),
                &["-c", "core.quotePath=false", "diff", "--no-color", "--no-ext-diff", "-M", mb, head],
            )
            .await?;
        Ok(DiffIndex::parse(&out))
    }

    pub async fn remove_worktree(&self, repo: &str, number: u64) -> Result<()> {
        let wt = self.worktree_dir(repo, number);
        if !wt.exists() {
            return Ok(());
        }
        let lock = self.lock_for(repo);
        let _g = lock.lock().await;
        let git = Git { token: None };
        let dir = self.repo_dir(repo);
        if git.run(Some(&dir), &["worktree", "remove", "--force", path_str(&wt)?]).await.is_err() {
            std::fs::remove_dir_all(&wt).ok();
            git.run(Some(&dir), &["worktree", "prune"]).await.ok();
        }
        Ok(())
    }

    /// Remove worktrees for PRs not in `keep` (ids like `owner/repo#n`).
    pub async fn gc(&self, keep: &std::collections::HashSet<String>) {
        let Ok(owners) = std::fs::read_dir(&self.root) else { return };
        for owner in owners.flatten() {
            let Ok(repos) = std::fs::read_dir(owner.path()) else { continue };
            for r in repos.flatten() {
                let repo = format!("{}/{}", owner.file_name().to_string_lossy(), r.file_name().to_string_lossy());
                let Ok(entries) = std::fs::read_dir(r.path()) else { continue };
                for e in entries.flatten() {
                    let name = e.file_name().to_string_lossy().to_string();
                    if let Some(n) = name.strip_prefix("pr-").and_then(|n| n.parse::<u64>().ok())
                        && !keep.contains(&crate::github::model::pr_id(&repo, n))
                    {
                        tracing::info!("gc worktree {repo}#{n}");
                        self.remove_worktree(&repo, n).await.ok();
                    }
                }
            }
        }
    }
}

fn path_str(p: &Path) -> Result<&str> {
    p.to_str().context("non-UTF-8 path")
}

struct Git {
    token: Option<String>,
}

impl Git {
    async fn run(&self, cwd: Option<&Path>, args: &[&str]) -> Result<String> {
        let mut cmd = Command::new("git");
        if let Some(c) = cwd {
            cmd.current_dir(c);
        }
        cmd.args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_LFS_SKIP_SMUDGE", "1")
            // Never run hooks or filters from the repo under review.
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .kill_on_drop(true);
        let mut n = 0;
        let mut cfg = |k: &str, v: &str| {
            cmd.env(format!("GIT_CONFIG_KEY_{n}"), k).env(format!("GIT_CONFIG_VALUE_{n}"), v);
            n += 1;
        };
        cfg("core.hooksPath", "/dev/null");
        cfg("core.fsmonitor", "false");
        if let Some(t) = &self.token {
            // Auth via env, never argv, so it doesn't show in `ps`.
            let basic = base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{t}"));
            cfg("http.https://github.com/.extraheader", &format!("AUTHORIZATION: basic {basic}"));
        }
        cmd.env("GIT_CONFIG_COUNT", n.to_string());
        let out = cmd.output().await.context("running git")?;
        if !out.status.success() {
            bail!("git {} failed: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Builds a local "remote", then drives `prepare()` against a clone of it by
    /// pointing the bare repo's origin at the local path.
    #[tokio::test]
    async fn prepare_makes_clean_worktree_and_diff() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        sh(&src, &["init", "-q", "-b", "main"]);
        std::fs::write(src.join("a.txt"), "one\ntwo\n").unwrap();
        sh(&src, &["add", "."]);
        sh(&src, &["commit", "-qm", "base"]);
        let base = sh(&src, &["rev-parse", "HEAD"]);
        std::fs::write(src.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        sh(&src, &["commit", "-qam", "head"]);
        let head = sh(&src, &["rev-parse", "HEAD"]);

        let root = tmp.path().join("wt");
        let repos = Repos::new(root.clone());
        let bare = repos.repo_dir("o/r");
        std::fs::create_dir_all(bare.parent().unwrap()).unwrap();
        sh(tmp.path(), &["clone", "-q", "--bare", src.to_str().unwrap(), bare.to_str().unwrap()]);

        let co = repos.prepare("o/r", 1, &head, &base, None).await.unwrap();
        assert_eq!(co.merge_base, base);
        assert_eq!(std::fs::read_to_string(co.worktree.join("a.txt")).unwrap(), "one\nTWO\nthree\n");
        let f = co.diff.file("a.txt").unwrap();
        assert_eq!((f.additions, f.deletions), (2, 1));

        // Junk in the worktree is removed on the next prepare.
        std::fs::write(co.worktree.join("junk"), "x").unwrap();
        let co = repos.prepare("o/r", 1, &head, &base, None).await.unwrap();
        assert!(!co.worktree.join("junk").exists());

        repos.gc(&std::collections::HashSet::default()).await;
        assert!(!co.worktree.exists());
    }
}
