//! GraphQL query text and response parsing (pure; tested with fixtures).

use std::fmt::Write as _;

use anyhow::{Context, Result, anyhow};
use serde_json::Value;

use super::model::{
    CheckContext, CiCounts, CiState, IssueComment, LabelEvent, PrDetail, PrState, PrSummary, RateLimit, Review,
    ReviewComment, ReviewThread, Time,
};

const PR_LIST_FIELDS: &str = r"
  number title url body isDraft state merged createdAt updatedAt
  headRefOid baseRefOid headRefName baseRefName additions deletions changedFiles
  author { login }
  repository { nameWithOwner }
  labels(first: 20) { nodes { name } }
  reviewDecision
  commits(last: 1) { nodes { commit { statusCheckRollup { state contexts(first: 1) {
    checkRunCountsByState { state count }
    statusContextCountsByState { state count }
  } } } } }
";

pub const RATE_LIMIT: &str = "rateLimit { cost remaining resetAt }";

/// One search alias per source. `after` cursors are only used for follow-up pages.
pub fn list_query(searches: &[(String, String, Option<String>)]) -> String {
    let mut q = String::from("query {\n");
    for (alias, search, after) in searches {
        let after = after.as_ref().map(|c| format!(", after: {}", json_str(c))).unwrap_or_default();
        let _ = write!(
            q,
            "  {alias}: search(query: {}, type: ISSUE, first: 50{after}) {{\n    issueCount pageInfo {{ hasNextPage endCursor }}\n    nodes {{ ... on PullRequest {{ {PR_LIST_FIELDS} }} }}\n  }}\n",
            json_str(search)
        );
    }
    let _ = write!(q, "  {RATE_LIMIT}\n}}\n");
    q
}

pub fn viewer_query() -> String {
    format!("query {{ viewer {{ login }} {RATE_LIMIT} }}")
}

pub fn repo_probe_query(owner: &str, name: &str) -> String {
    format!("query {{ repository(owner: {}, name: {}) {{ id }} {RATE_LIMIT} }}", json_str(owner), json_str(name))
}

pub fn detail_query(owner: &str, name: &str, number: u64) -> String {
    format!(
        r"query {{
  repository(owner: {owner}, name: {name}) {{
    pullRequest(number: {number}) {{
      {PR_LIST_FIELDS}
      reviewRequests(first: 50) {{ nodes {{ requestedReviewer {{
        ... on User {{ login }} ... on Team {{ slug }} ... on Bot {{ login }} ... on Mannequin {{ login }}
      }} }} }}
      reviewThreads(first: 100) {{ nodes {{
        path line originalLine isOutdated
        comments(first: 50) {{ nodes {{
          databaseId author {{ login }} body createdAt url commit {{ oid }}
        }} }}
      }} }}
      reviews(first: 100) {{ nodes {{
        databaseId author {{ login }} state body submittedAt createdAt url commit {{ oid }}
      }} }}
      comments(first: 100) {{ nodes {{ author {{ login }} body createdAt url }} }}
      timelineItems(itemTypes: [LABELED_EVENT], last: 30) {{ nodes {{
        ... on LabeledEvent {{ createdAt label {{ name }} actor {{ login }} }}
      }} }}
      headCommit: commits(last: 1) {{ nodes {{ commit {{
        oid committedDate
        checkSuites(first: 30) {{ nodes {{ createdAt }} }}
        statusCheckRollup {{ contexts(first: 100) {{ nodes {{
          __typename
          ... on CheckRun {{ name status conclusion detailsUrl }}
          ... on StatusContext {{ context state targetUrl }}
        }} }} }}
      }} }} }}
    }}
  }}
  {RATE_LIMIT}
}}",
        owner = json_str(owner),
        name = json_str(name),
    )
}

fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

/// GraphQL top-level errors. SAML/SSO errors are recognisable by their text.
#[derive(Debug, Clone, PartialEq)]
pub enum GqlError {
    Sso(String),
    Other(String),
}

pub fn check_errors(v: &Value) -> Result<(), GqlError> {
    let Some(errs) = v.get("errors").and_then(Value::as_array) else { return Ok(()) };
    if errs.is_empty() {
        return Ok(());
    }
    let msgs: Vec<String> =
        errs.iter().filter_map(|e| e.get("message").and_then(Value::as_str)).map(String::from).collect();
    let joined = msgs.join("; ");
    let lower = joined.to_ascii_lowercase();
    if lower.contains("saml") || lower.contains("sso") || lower.contains("single sign-on") {
        Err(GqlError::Sso(joined))
    } else {
        Err(GqlError::Other(joined))
    }
}

pub fn parse_rate_limit(v: &Value) -> Option<RateLimit> {
    let r = v.pointer("/data/rateLimit")?;
    Some(RateLimit {
        cost: r.get("cost")?.as_i64()?,
        remaining: r.get("remaining")?.as_i64()?,
        reset_at: r.get("resetAt").and_then(Value::as_str).and_then(parse_time),
    })
}

fn parse_time(s: &str) -> Option<Time> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|t| t.with_timezone(&chrono::Utc))
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn t(v: &Value, k: &str) -> Option<Time> {
    v.get(k).and_then(Value::as_str).and_then(parse_time)
}

fn login(v: &Value) -> String {
    v.pointer("/author/login").and_then(Value::as_str).unwrap_or("ghost").to_string()
}

fn nodes<'a>(v: &'a Value, k: &str) -> impl Iterator<Item = &'a Value> {
    v.get(k)
        .and_then(|c| c.get("nodes"))
        .and_then(Value::as_array)
        .map(|a| a.iter())
        .into_iter()
        .flatten()
        .filter(|n| !n.is_null())
}

fn ci_state(s: &str) -> CiState {
    match s {
        "SUCCESS" | "NEUTRAL" | "SKIPPED" => CiState::Success,
        "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED" | "STARTUP_FAILURE" | "STALE" => {
            CiState::Failure
        }
        "PENDING" | "EXPECTED" | "QUEUED" | "IN_PROGRESS" | "WAITING" | "REQUESTED" => CiState::Pending,
        _ => CiState::None,
    }
}

fn parse_ci(pr: &Value) -> CiCounts {
    let Some(rollup) = pr.pointer("/commits/nodes/0/commit/statusCheckRollup").filter(|r| !r.is_null()) else {
        return CiCounts::default();
    };
    let mut c = CiCounts { state: ci_state(&s(rollup, "state")), ..Default::default() };
    for key in ["checkRunCountsByState", "statusContextCountsByState"] {
        for e in rollup.pointer(&format!("/contexts/{key}")).and_then(Value::as_array).into_iter().flatten() {
            let n = e.get("count").and_then(Value::as_u64).map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX));
            match ci_state(&s(e, "state")) {
                CiState::Success => c.passed += n,
                CiState::Failure => c.failed += n,
                CiState::Pending => c.pending += n,
                CiState::None => {}
            }
            c.total += n;
        }
    }
    c
}

pub fn parse_pr(pr: &Value, requested: bool) -> Result<PrSummary> {
    let state = match (s(pr, "state").as_str(), pr.get("merged").and_then(Value::as_bool).unwrap_or(false)) {
        (_, true) | ("MERGED", _) => PrState::Merged,
        ("CLOSED", _) => PrState::Closed,
        _ => PrState::Open,
    };
    Ok(PrSummary {
        repo: pr
            .pointer("/repository/nameWithOwner")
            .and_then(Value::as_str)
            .context("PR missing repository")?
            .to_string(),
        number: pr.get("number").and_then(Value::as_u64).context("PR missing number")?,
        title: s(pr, "title"),
        author: login(pr),
        url: s(pr, "url"),
        body: s(pr, "body"),
        is_draft: pr.get("isDraft").and_then(Value::as_bool).unwrap_or(false),
        state,
        created_at: t(pr, "createdAt").context("PR missing createdAt")?,
        updated_at: t(pr, "updatedAt").context("PR missing updatedAt")?,
        head_sha: s(pr, "headRefOid"),
        base_sha: s(pr, "baseRefOid"),
        head_ref: s(pr, "headRefName"),
        base_ref: s(pr, "baseRefName"),
        additions: pr.get("additions").and_then(Value::as_u64).unwrap_or(0),
        deletions: pr.get("deletions").and_then(Value::as_u64).unwrap_or(0),
        changed_files: pr.get("changedFiles").and_then(Value::as_u64).unwrap_or(0),
        labels: nodes(pr, "labels").map(|l| s(l, "name")).collect(),
        review_decision: pr.get("reviewDecision").and_then(Value::as_str).map(String::from),
        ci: parse_ci(pr),
        requested,
    })
}

pub struct SearchPage {
    pub prs: Vec<PrSummary>,
    pub next_cursor: Option<String>,
}

pub fn parse_search(v: &Value, alias: &str, requested: bool) -> Result<SearchPage> {
    let sr = v.pointer(&format!("/data/{alias}")).ok_or_else(|| anyhow!("missing search result {alias}"))?;
    let mut prs = vec![];
    for n in sr.get("nodes").and_then(Value::as_array).into_iter().flatten() {
        // Non-PR search hits come back as empty objects.
        if n.get("number").is_some() {
            prs.push(parse_pr(n, requested)?);
        }
    }
    let next_cursor = if sr.pointer("/pageInfo/hasNextPage").and_then(Value::as_bool).unwrap_or(false) {
        sr.pointer("/pageInfo/endCursor").and_then(Value::as_str).map(String::from)
    } else {
        None
    };
    Ok(SearchPage { prs, next_cursor })
}

pub fn parse_detail(v: &Value, requested: bool) -> Result<PrDetail> {
    let pr = v
        .pointer("/data/repository/pullRequest")
        .filter(|p| !p.is_null())
        .ok_or_else(|| anyhow!("pull request not found"))?;
    let summary = parse_pr(pr, requested)?;
    let head = pr.pointer("/headCommit/nodes/0/commit");

    let checks = head
        .and_then(|h| h.pointer("/statusCheckRollup/contexts/nodes"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|c| {
            if s(c, "__typename") == "StatusContext" {
                CheckContext {
                    name: s(c, "context"),
                    state: ci_state(&s(c, "state")),
                    url: c.get("targetUrl").and_then(Value::as_str).map(String::from),
                }
            } else {
                let conclusion = s(c, "conclusion");
                let state = if conclusion.is_empty() { ci_state(&s(c, "status")) } else { ci_state(&conclusion) };
                CheckContext {
                    name: s(c, "name"),
                    state,
                    url: c.get("detailsUrl").and_then(Value::as_str).map(String::from),
                }
            }
        })
        .collect();

    let review_requests = nodes(pr, "reviewRequests")
        .filter_map(|n| n.get("requestedReviewer"))
        .filter_map(|r| r.get("login").or_else(|| r.get("slug")).and_then(Value::as_str).map(String::from))
        .collect();

    let reviews = nodes(pr, "reviews")
        .map(|r| Review {
            id: r.get("databaseId").and_then(Value::as_u64).unwrap_or(0),
            author: login(r),
            state: s(r, "state"),
            body: s(r, "body"),
            commit_oid: r.pointer("/commit/oid").and_then(Value::as_str).map(String::from),
            submitted_at: t(r, "submittedAt"),
            created_at: t(r, "createdAt").unwrap_or_else(chrono::Utc::now),
            url: s(r, "url"),
        })
        .collect();

    let threads = nodes(pr, "reviewThreads")
        .map(|th| ReviewThread {
            path: s(th, "path"),
            line: th.get("line").and_then(Value::as_u64),
            original_line: th.get("originalLine").and_then(Value::as_u64),
            is_outdated: th.get("isOutdated").and_then(Value::as_bool).unwrap_or(false),
            comments: nodes(th, "comments")
                .map(|c| ReviewComment {
                    id: c.get("databaseId").and_then(Value::as_u64).unwrap_or(0),
                    author: login(c),
                    body: s(c, "body"),
                    created_at: t(c, "createdAt").unwrap_or_else(chrono::Utc::now),
                    url: s(c, "url"),
                    commit_oid: c.pointer("/commit/oid").and_then(Value::as_str).map(String::from),
                })
                .collect(),
        })
        .collect();

    let comments = nodes(pr, "comments")
        .map(|c| IssueComment {
            author: login(c),
            body: s(c, "body"),
            created_at: t(c, "createdAt").unwrap_or_else(chrono::Utc::now),
            url: s(c, "url"),
        })
        .collect();

    let label_events = nodes(pr, "timelineItems")
        .filter(|e| e.get("label").is_some())
        .map(|e| LabelEvent {
            label: e.pointer("/label/name").and_then(Value::as_str).unwrap_or_default().into(),
            actor: e.pointer("/actor/login").and_then(Value::as_str).unwrap_or_default().into(),
            created_at: t(e, "createdAt").unwrap_or_else(chrono::Utc::now),
        })
        .collect();

    Ok(PrDetail {
        summary,
        checks,
        review_requests,
        reviews,
        threads,
        comments,
        head_committed_at: head.and_then(|h| t(h, "committedDate")),
        head_first_check_suite_at: head.and_then(|h| nodes(h, "checkSuites").filter_map(|n| t(n, "createdAt")).min()),
        label_events,
        fetched_at: chrono::Utc::now(),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    pub fn pr_json(repo: &str, number: u64, author: &str, head: &str) -> Value {
        json!({
            "number": number, "title": format!("PR {number}"), "url": format!("https://github.com/{repo}/pull/{number}"),
            "body": "Line one\nLine two\nLine three", "isDraft": false, "state": "OPEN", "merged": false,
            "createdAt": "2026-10-01T00:00:00Z", "updatedAt": "2026-10-30T00:00:00Z",
            "headRefOid": head, "baseRefOid": "base000", "headRefName": "fix/x", "baseRefName": "main",
            "additions": 120, "deletions": 34, "changedFiles": 6,
            "author": {"login": author}, "repository": {"nameWithOwner": repo},
            "labels": {"nodes": [{"name": "agentic-review"}]}, "reviewDecision": "CHANGES_REQUESTED",
            "commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": "FAILURE", "contexts": {
                "checkRunCountsByState": [{"state": "SUCCESS", "count": 12}, {"state": "FAILURE", "count": 2}],
                "statusContextCountsByState": [{"state": "PENDING", "count": 1}]
            }}}}]}
        })
    }

    #[test]
    fn list_query_has_aliases_and_rate_limit() {
        let q = list_query(&[
            ("w0".into(), "repo:o/a is:pr is:open".into(), None),
            ("req".into(), "is:pr is:open review-requested:@me".into(), Some("abc".into())),
        ]);
        assert!(q.contains("w0: search(query: \"repo:o/a is:pr is:open\""));
        assert!(q.contains("after: \"abc\""));
        assert!(q.contains("rateLimit { cost remaining resetAt }"));
    }

    #[test]
    fn parses_search_page() {
        let v = json!({"data": {
            "w0": {"issueCount": 2, "pageInfo": {"hasNextPage": true, "endCursor": "c1"},
                   "nodes": [pr_json("o/a", 1, "alice", "h1"), {}]},
            "rateLimit": {"cost": 1, "remaining": 4999, "resetAt": "2026-10-31T00:00:00Z"}
        }});
        let page = parse_search(&v, "w0", false).unwrap();
        assert_eq!(page.prs.len(), 1);
        assert_eq!(page.next_cursor.as_deref(), Some("c1"));
        let pr = &page.prs[0];
        assert_eq!(pr.id(), "o/a#1");
        assert_eq!(pr.ci.state, CiState::Failure);
        assert_eq!((pr.ci.passed, pr.ci.failed, pr.ci.pending, pr.ci.total), (12, 2, 1, 15));
        assert_eq!(pr.labels, vec!["agentic-review"]);
        assert_eq!(parse_rate_limit(&v).unwrap().remaining, 4999);
    }

    #[test]
    fn detects_sso_errors() {
        let v = json!({"errors": [{"message": "Resource protected by organization SAML enforcement."}]});
        assert!(matches!(check_errors(&v), Err(GqlError::Sso(_))));
        let v = json!({"errors": [{"message": "Something else"}]});
        assert!(matches!(check_errors(&v), Err(GqlError::Other(_))));
        assert!(check_errors(&json!({"data": {}})).is_ok());
    }

    pub fn detail_json() -> Value {
        let mut pr = pr_json("o/a", 7, "alice", "head1");
        let extra = json!({
            "reviewRequests": {"nodes": [{"requestedReviewer": {"login": "me"}}, {"requestedReviewer": {"slug": "team-x"}}]},
            "reviewThreads": {"nodes": [{
                "path": "pkg/retry.go", "line": 42, "originalLine": 40, "isOutdated": false,
                "comments": {"nodes": [{"databaseId": 11, "author": {"login": "ai-reviewer"}, "body": "Backoff never resets",
                    "createdAt": "2026-10-30T10:00:00Z", "url": "u1", "commit": {"oid": "head1"}}]}
            }]},
            "reviews": {"nodes": [
                {"databaseId": 5, "author": {"login": "me"}, "state": "PENDING", "body": "", "submittedAt": null,
                 "createdAt": "2026-10-30T11:00:00Z", "url": "r5", "commit": {"oid": "head1"}},
                {"databaseId": 6, "author": {"login": "bob"}, "state": "APPROVED", "body": "lgtm",
                 "submittedAt": "2026-10-29T11:00:00Z", "createdAt": "2026-10-29T11:00:00Z", "url": "r6", "commit": {"oid": "old"}}
            ]},
            "comments": {"nodes": [{"author": {"login": "carol"}, "body": "hi", "createdAt": "2026-10-28T00:00:00Z", "url": "c1"}]},
            "timelineItems": {"nodes": [{"createdAt": "2026-10-30T09:00:00Z", "label": {"name": "agentic-review"}, "actor": {"login": "me"}}, {}]},
            "headCommit": {"nodes": [{"commit": {
                "oid": "head1", "committedDate": "2026-10-29T00:00:00Z",
                "checkSuites": {"nodes": [{"createdAt": "2026-10-30T08:05:00Z"}, {"createdAt": "2026-10-30T08:00:00Z"}]},
                "statusCheckRollup": {"contexts": {"nodes": [
                    {"__typename": "CheckRun", "name": "build", "status": "COMPLETED", "conclusion": "SUCCESS", "detailsUrl": "d"},
                    {"__typename": "CheckRun", "name": "lint", "status": "IN_PROGRESS", "conclusion": null, "detailsUrl": null},
                    {"__typename": "StatusContext", "context": "ci/x", "state": "FAILURE", "targetUrl": "t"}
                ]}}
            }}]}
        });
        for (k, v) in extra.as_object().unwrap() {
            pr[k] = v.clone();
        }
        json!({"data": {"repository": {"pullRequest": pr}, "rateLimit": {"cost": 1, "remaining": 10, "resetAt": null}}})
    }

    #[test]
    fn parses_detail() {
        let d = parse_detail(&detail_json(), true).unwrap();
        assert!(d.summary.requested);
        assert_eq!(d.checks.len(), 3);
        assert_eq!(d.checks[1].state, CiState::Pending);
        assert_eq!(d.checks[2].state, CiState::Failure);
        assert_eq!(d.review_requests, vec!["me", "team-x"]);
        assert_eq!(d.reviews.len(), 2);
        assert_eq!(d.threads[0].comments[0].author, "ai-reviewer");
        assert_eq!(d.label_events.len(), 1);
        assert_eq!(d.head_first_check_suite_at.unwrap().to_rfc3339(), "2026-10-30T08:00:00+00:00");
        let revs = d.reviewers();
        assert!(revs.iter().any(|r| r.name == "bob" && r.state == "APPROVED"));
        assert!(revs.iter().any(|r| r.name == "me" && r.state == "requested"));
    }
}
