//! Plain data types for what we read from GitHub. These are what the rest of
//! the app works with; `query.rs` maps GraphQL JSON onto them.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub type Time = DateTime<Utc>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CiState {
    Success,
    Failure,
    Pending,
    #[default]
    None,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CiCounts {
    pub state: CiState,
    pub passed: u32,
    pub failed: u32,
    pub pending: u32,
    pub total: u32,
}

/// What the list query returns for each PR.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrSummary {
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub author: String,
    pub url: String,
    pub body: String,
    pub is_draft: bool,
    pub state: PrState,
    pub created_at: Time,
    pub updated_at: Time,
    pub head_sha: String,
    pub base_sha: String,
    pub head_ref: String,
    pub base_ref: String,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    pub labels: Vec<String>,
    pub review_decision: Option<String>,
    pub ci: CiCounts,
    /// Came from the `review-requested:@me` search.
    pub requested: bool,
}

impl PrSummary {
    pub fn id(&self) -> String {
        pr_id(&self.repo, self.number)
    }
}

pub fn pr_id(repo: &str, number: u64) -> String {
    format!("{repo}#{number}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PrState {
    #[default]
    Open,
    Closed,
    Merged,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckContext {
    pub name: String,
    pub state: CiState,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Review {
    pub id: u64,
    pub author: String,
    /// PENDING, COMMENTED, APPROVED, `CHANGES_REQUESTED`, DISMISSED
    pub state: String,
    pub body: String,
    pub commit_oid: Option<String>,
    pub submitted_at: Option<Time>,
    pub created_at: Time,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewComment {
    pub id: u64,
    pub author: String,
    pub body: String,
    pub created_at: Time,
    pub url: String,
    pub commit_oid: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewThread {
    pub path: String,
    pub line: Option<u64>,
    pub original_line: Option<u64>,
    pub is_outdated: bool,
    pub comments: Vec<ReviewComment>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssueComment {
    pub author: String,
    pub body: String,
    pub created_at: Time,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabelEvent {
    pub label: String,
    pub actor: String,
    pub created_at: Time,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reviewer {
    pub name: String,
    /// "requested", or the state of their latest review.
    pub state: String,
}

/// What the detail query returns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrDetail {
    pub summary: PrSummary,
    pub checks: Vec<CheckContext>,
    pub review_requests: Vec<String>,
    pub reviews: Vec<Review>,
    pub threads: Vec<ReviewThread>,
    pub comments: Vec<IssueComment>,
    pub head_committed_at: Option<Time>,
    pub head_first_check_suite_at: Option<Time>,
    pub label_events: Vec<LabelEvent>,
    pub fetched_at: Time,
}

impl PrDetail {
    /// Reviewers and their states for the Overview tab.
    pub fn reviewers(&self) -> Vec<Reviewer> {
        let mut out: Vec<Reviewer> = vec![];
        for r in &self.review_requests {
            out.push(Reviewer { name: r.clone(), state: "requested".into() });
        }
        let mut reviews: Vec<&Review> = self.reviews.iter().filter(|r| r.state != "PENDING").collect();
        reviews.sort_by_key(|r| r.submitted_at.unwrap_or(r.created_at));
        for r in reviews {
            match out.iter_mut().find(|o| o.name == r.author) {
                Some(o) if o.state != "requested" => o.state.clone_from(&r.state),
                Some(_) => {}
                None => out.push(Reviewer { name: r.author.clone(), state: r.state.clone() }),
            }
        }
        out
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RateLimit {
    pub cost: i64,
    pub remaining: i64,
    pub reset_at: Option<Time>,
}
