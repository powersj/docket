//! GitHub access: GraphQL reads and a few REST writes over `reqwest`, behind a
//! trait so the rest of the app can be tested with recorded fixtures.

pub mod model;
pub mod mutate;
pub mod query;

use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use serde_json::{Value, json};

use model::*;
use mutate::*;

/// Downcast target for reads that failed because the token isn't SSO-authorized.
#[derive(Debug, thiserror::Error)]
#[error("token not SSO-authorized: {0}")]
pub struct SsoError(pub String);

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Compare {
    pub titles: Vec<String>,
    pub files: Vec<String>,
}

/// A source the list query searches.
#[derive(Debug, Clone)]
pub struct Search {
    pub alias: String,
    pub query: String,
    pub requested: bool,
}

#[async_trait]
pub trait GitHub: Send + Sync {
    async fn viewer_login(&self) -> Result<String>;
    async fn probe_repo(&self, repo: &str) -> Result<()>;
    async fn list(&self, searches: &[Search]) -> Result<Vec<PrSummary>>;
    async fn detail(&self, repo: &str, number: u64, requested: bool) -> Result<PrDetail>;
    async fn add_label(&self, repo: &str, number: u64, label: &str) -> Result<(), WriteError>;
    async fn remove_label(&self, repo: &str, number: u64, label: &str) -> Result<(), WriteError>;
    async fn post_review(&self, repo: &str, number: u64, review: &ReviewPayload) -> Result<PostedReview, WriteError>;
    async fn list_reviews(&self, repo: &str, number: u64) -> Result<Vec<PostedReview>, WriteError>;
    /// Commit titles and changed paths between two commits.
    async fn compare(&self, repo: &str, from: &str, to: &str) -> Result<Compare>;
    fn rate_limit(&self) -> Option<RateLimit>;
    /// Forget any cached token so the next call asks `gh` again.
    fn reset_token(&self);
}

pub fn searches_for(cfg: &crate::config::Config) -> Vec<Search> {
    let mut out: Vec<Search> = cfg
        .watch
        .iter()
        .enumerate()
        .map(|(i, w)| Search {
            alias: format!("w{i}"),
            query: format!("repo:{} is:pr is:open", w.repo),
            requested: false,
        })
        .collect();
    out.push(Search {
        alias: "requested".into(),
        query: "is:pr is:open archived:false review-requested:@me".into(),
        requested: true,
    });
    out
}

/// Ask `gh` for the token of a specific account.
pub async fn gh_token(login: &str) -> Result<String> {
    let out = tokio::process::Command::new("gh")
        .args(["auth", "token", "--hostname", "github.com", "--user", login])
        .output()
        .await
        .context("running gh (is the GitHub CLI installed?)")?;
    if !out.status.success() {
        bail!("gh auth token failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let tok = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if tok.is_empty() {
        bail!("gh returned an empty token");
    }
    Ok(tok)
}

pub struct HttpGitHub {
    http: reqwest::Client,
    login: String,
    token: Mutex<Option<String>>,
    rate: Mutex<Option<RateLimit>>,
    api: String,
}

impl HttpGitHub {
    pub fn new(login: &str) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("reviewer/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(60))
            .build()
            .expect("http client");
        Self {
            http,
            login: login.to_string(),
            token: Mutex::new(None),
            rate: Mutex::new(None),
            api: "https://api.github.com".into(),
        }
    }

    async fn token(&self) -> Result<String> {
        if let Some(t) = self.token.lock().unwrap().clone() {
            return Ok(t);
        }
        let t = gh_token(&self.login).await?;
        *self.token.lock().unwrap() = Some(t.clone());
        Ok(t)
    }

    async fn graphql(&self, q: &str) -> Result<Value> {
        let tok = self.token().await?;
        let resp = self
            .http
            .post(format!("{}/graphql", self.api))
            .bearer_auth(tok)
            .json(&json!({ "query": q }))
            .send()
            .await
            .context("GitHub GraphQL request")?;
        let status = resp.status();
        let v: Value = resp.json().await.context("decoding GraphQL response")?;
        if status == reqwest::StatusCode::UNAUTHORIZED {
            self.reset_token();
            bail!("GitHub rejected the token (401)");
        }
        if let Some(rl) = query::parse_rate_limit(&v) {
            tracing::debug!(cost = rl.cost, remaining = rl.remaining, "GraphQL rate limit");
            *self.rate.lock().unwrap() = Some(rl);
        }
        match query::check_errors(&v) {
            Ok(()) => {}
            Err(query::GqlError::Sso(m)) => return Err(SsoError(m).into()),
            // Partial data with errors (e.g. one inaccessible repo) is still usable.
            Err(query::GqlError::Other(m)) if v.get("data").is_some_and(|d| !d.is_null()) => {
                tracing::warn!("GraphQL partial errors: {m}");
            }
            Err(query::GqlError::Other(m)) => bail!("GitHub GraphQL error: {m}"),
        }
        if !status.is_success() {
            bail!("GitHub GraphQL HTTP {status}");
        }
        Ok(v)
    }

    async fn rest(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Value, WriteError> {
        let tok = self.token().await.map_err(|e| WriteError::Transport(e.to_string()))?;
        let mut req = self
            .http
            .request(method, format!("{}{}", self.api, path))
            .bearer_auth(tok)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await.map_err(|e| WriteError::Transport(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.map_err(|e| WriteError::Transport(e.to_string()))?;
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if (200..300).contains(&status) {
            return Ok(v);
        }
        let mut message = v.get("message").and_then(Value::as_str).unwrap_or(&text).to_string();
        if let Some(errs) = v.get("errors").and_then(Value::as_array) {
            let details: Vec<String> = errs
                .iter()
                .map(|e| {
                    e.as_str()
                        .map(String::from)
                        .or_else(|| e.get("message").and_then(Value::as_str).map(String::from))
                        .unwrap_or_else(|| e.to_string())
                })
                .collect();
            if !details.is_empty() {
                message = format!("{message}: {}", details.join("; "));
            }
        }
        Err(match status {
            422 => WriteError::Unprocessable(message),
            401 => {
                self.reset_token();
                WriteError::Forbidden { status, message }
            }
            403 | 404 => WriteError::Forbidden { status, message },
            _ => WriteError::Other { status, message },
        })
    }
}

fn split_repo(repo: &str) -> Result<(&str, &str)> {
    repo.split_once('/').ok_or_else(|| anyhow!("bad repo {repo:?}"))
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[async_trait]
impl GitHub for HttpGitHub {
    async fn viewer_login(&self) -> Result<String> {
        let v = self.graphql(&query::viewer_query()).await?;
        v.pointer("/data/viewer/login").and_then(Value::as_str).map(String::from).context("viewer.login missing")
    }

    async fn probe_repo(&self, repo: &str) -> Result<()> {
        let (o, n) = split_repo(repo)?;
        self.graphql(&query::repo_probe_query(o, n)).await.map(|_| ())
    }

    async fn list(&self, searches: &[Search]) -> Result<Vec<PrSummary>> {
        let mut out: Vec<PrSummary> = vec![];
        let first: Vec<_> = searches.iter().map(|s| (s.alias.clone(), s.query.clone(), None)).collect();
        let v = self.graphql(&query::list_query(&first)).await?;
        let mut follow: Vec<(&Search, String)> = vec![];
        for s in searches {
            let page = query::parse_search(&v, &s.alias, s.requested)?;
            out.extend(page.prs);
            if let Some(c) = page.next_cursor {
                follow.push((s, c));
            }
        }
        // Search caps at 1000 results: 20 pages of 50.
        for (s, mut cursor) in follow {
            for _ in 1..20 {
                let v = self
                    .graphql(&query::list_query(&[(s.alias.clone(), s.query.clone(), Some(cursor.clone()))]))
                    .await?;
                let page = query::parse_search(&v, &s.alias, s.requested)?;
                out.extend(page.prs);
                match page.next_cursor {
                    Some(c) => cursor = c,
                    None => break,
                }
            }
        }
        Ok(merge_sources(out))
    }

    async fn detail(&self, repo: &str, number: u64, requested: bool) -> Result<PrDetail> {
        let (o, n) = split_repo(repo)?;
        let v = self.graphql(&query::detail_query(o, n, number)).await?;
        query::parse_detail(&v, requested)
    }

    async fn add_label(&self, repo: &str, number: u64, label: &str) -> Result<(), WriteError> {
        self.rest(
            reqwest::Method::POST,
            &format!("/repos/{repo}/issues/{number}/labels"),
            Some(&json!({ "labels": [label] })),
        )
        .await
        .map(|_| ())
    }

    async fn remove_label(&self, repo: &str, number: u64, label: &str) -> Result<(), WriteError> {
        match self
            .rest(reqwest::Method::DELETE, &format!("/repos/{repo}/issues/{number}/labels/{}", enc(label)), None)
            .await
        {
            // Already absent is fine.
            Err(WriteError::Forbidden { status: 404, .. }) => Ok(()),
            r => r.map(|_| ()),
        }
    }

    async fn post_review(&self, repo: &str, number: u64, review: &ReviewPayload) -> Result<PostedReview, WriteError> {
        let body = serde_json::to_value(review).map_err(|e| WriteError::Transport(e.to_string()))?;
        let v = self.rest(reqwest::Method::POST, &format!("/repos/{repo}/pulls/{number}/reviews"), Some(&body)).await?;
        serde_json::from_value(v).map_err(|e| WriteError::Transport(format!("decoding review: {e}")))
    }

    async fn list_reviews(&self, repo: &str, number: u64) -> Result<Vec<PostedReview>, WriteError> {
        let v = self
            .rest(reqwest::Method::GET, &format!("/repos/{repo}/pulls/{number}/reviews?per_page=100"), None)
            .await?;
        serde_json::from_value(v).map_err(|e| WriteError::Transport(format!("decoding reviews: {e}")))
    }

    async fn compare(&self, repo: &str, from: &str, to: &str) -> Result<Compare> {
        let v = self
            .rest(reqwest::Method::GET, &format!("/repos/{repo}/compare/{from}...{to}"), None)
            .await
            .map_err(|e| anyhow!("{e}"))?;
        let list = |k: &str, f: &dyn Fn(&Value) -> Option<String>| -> Vec<String> {
            v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(f).collect()).unwrap_or_default()
        };
        Ok(Compare {
            titles: list("commits", &|c| {
                c.pointer("/commit/message").and_then(Value::as_str).map(|m| m.lines().next().unwrap_or("").to_string())
            }),
            files: list("files", &|f| f.get("filename").and_then(Value::as_str).map(String::from)),
        })
    }

    fn rate_limit(&self) -> Option<RateLimit> {
        *self.rate.lock().unwrap()
    }

    fn reset_token(&self) {
        *self.token.lock().unwrap() = None;
    }
}

/// Post a review without ever posting it twice: if the request may have
/// reached GitHub, look for it on that commit before reporting failure.
pub async fn post_review_once(
    gh: &dyn GitHub,
    repo: &str,
    number: u64,
    login: &str,
    review: &ReviewPayload,
) -> Result<PostedReview, WriteError> {
    match gh.post_review(repo, number, review).await {
        Ok(r) => Ok(r),
        Err(err) if err.maybe_applied() => match gh.list_reviews(repo, number).await {
            Ok(list) => find_matching(&list, login, &review.commit_id, &review.body).cloned().ok_or(err),
            Err(_) => Err(err),
        },
        Err(err) => Err(err),
    }
}

/// A PR can come from a watch search and the requested search; keep one, starred.
pub fn merge_sources(prs: Vec<PrSummary>) -> Vec<PrSummary> {
    let mut map: std::collections::BTreeMap<String, PrSummary> = Default::default();
    for p in prs {
        let id = p.id();
        match map.get_mut(&id) {
            Some(existing) => existing.requested |= p.requested,
            None => {
                map.insert(id, p);
            }
        }
    }
    map.into_values().collect()
}

#[cfg(test)]
pub mod fake {
    //! An in-memory GitHub for tests.
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    pub struct FakeGitHub {
        pub login: String,
        pub prs: Mutex<Vec<PrSummary>>,
        pub details: Mutex<HashMap<String, PrDetail>>,
        pub labels_added: Mutex<Vec<(String, u64, String)>>,
        pub labels_removed: Mutex<Vec<(String, u64, String)>>,
        pub posted: Mutex<Vec<ReviewPayload>>,
        pub existing_reviews: Mutex<Vec<PostedReview>>,
        pub label_error: Mutex<Option<u16>>,
        pub post_error: Mutex<Option<WriteError>>,
        pub compare: Mutex<Compare>,
    }

    #[async_trait]
    impl GitHub for FakeGitHub {
        async fn viewer_login(&self) -> Result<String> {
            Ok(self.login.clone())
        }
        async fn probe_repo(&self, _repo: &str) -> Result<()> {
            Ok(())
        }
        async fn list(&self, _s: &[Search]) -> Result<Vec<PrSummary>> {
            Ok(self.prs.lock().unwrap().clone())
        }
        async fn detail(&self, repo: &str, number: u64, _r: bool) -> Result<PrDetail> {
            self.details.lock().unwrap().get(&pr_id(repo, number)).cloned().context("no fixture")
        }
        async fn add_label(&self, repo: &str, number: u64, label: &str) -> Result<(), WriteError> {
            if let Some(status) = *self.label_error.lock().unwrap() {
                return Err(WriteError::Forbidden { status, message: "nope".into() });
            }
            self.labels_added.lock().unwrap().push((repo.into(), number, label.into()));
            Ok(())
        }
        async fn remove_label(&self, repo: &str, number: u64, label: &str) -> Result<(), WriteError> {
            self.labels_removed.lock().unwrap().push((repo.into(), number, label.into()));
            Ok(())
        }
        async fn post_review(&self, _repo: &str, _n: u64, review: &ReviewPayload) -> Result<PostedReview, WriteError> {
            if let Some(e) = self.post_error.lock().unwrap().take() {
                return Err(e);
            }
            self.posted.lock().unwrap().push(review.clone());
            Ok(PostedReview {
                id: 99,
                html_url: "https://github.com/x".into(),
                body: Some(review.body.clone()),
                commit_id: Some(review.commit_id.clone()),
                user: Some(RestUser { login: self.login.clone() }),
            })
        }
        async fn list_reviews(&self, _repo: &str, _n: u64) -> Result<Vec<PostedReview>, WriteError> {
            Ok(self.existing_reviews.lock().unwrap().clone())
        }
        async fn compare(&self, _repo: &str, _from: &str, _to: &str) -> Result<Compare> {
            Ok(self.compare.lock().unwrap().clone())
        }
        fn rate_limit(&self) -> Option<RateLimit> {
            None
        }
        fn reset_token(&self) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_keeps_star() {
        let v = serde_json::json!({"data": {"a": {"pageInfo": {"hasNextPage": false}, "nodes": [query::tests::pr_json("o/a", 1, "x", "h")]}}});
        let a = query::parse_search(&v, "a", false).unwrap().prs;
        let b = query::parse_search(&v, "a", true).unwrap().prs;
        let m = merge_sources([a, b].concat());
        assert_eq!(m.len(), 1);
        assert!(m[0].requested);
    }

    #[tokio::test]
    async fn post_is_idempotent_after_transport_error() {
        let gh = fake::FakeGitHub { login: "me".into(), ..Default::default() };
        let review = ReviewPayload {
            commit_id: "abc".into(),
            body: "hello".into(),
            event: Some("COMMENT".into()),
            comments: vec![],
        };

        // The POST "fails" but actually landed: find it instead of reposting.
        *gh.post_error.lock().unwrap() = Some(WriteError::Transport("timeout".into()));
        *gh.existing_reviews.lock().unwrap() = vec![PostedReview {
            id: 5,
            html_url: "u".into(),
            body: Some("hello".into()),
            commit_id: Some("abc".into()),
            user: Some(RestUser { login: "me".into() }),
        }];
        let r = post_review_once(&gh, "o/a", 1, "me", &review).await.unwrap();
        assert_eq!(r.id, 5);
        assert!(gh.posted.lock().unwrap().is_empty());

        // It didn't land: the error comes back, nothing is reposted.
        gh.existing_reviews.lock().unwrap().clear();
        *gh.post_error.lock().unwrap() = Some(WriteError::Transport("timeout".into()));
        assert!(post_review_once(&gh, "o/a", 1, "me", &review).await.is_err());
        assert!(gh.posted.lock().unwrap().is_empty());

        // 422s are reported as-is.
        *gh.post_error.lock().unwrap() = Some(WriteError::Unprocessable("line must be part of the diff".into()));
        assert!(matches!(post_review_once(&gh, "o/a", 1, "me", &review).await, Err(WriteError::Unprocessable(_))));

        assert_eq!(post_review_once(&gh, "o/a", 1, "me", &review).await.unwrap().id, 99);
        assert_eq!(gh.posted.lock().unwrap().len(), 1);
    }

    #[test]
    fn url_encoding() {
        assert_eq!(enc("agentic review/x"), "agentic%20review%2Fx");
    }
}
