//! Row types for the store.

use crate::agent::schema::DraftComment;
use crate::github::model::Time;

#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub id: i64,
    pub pr_id: String,
    pub run_id: Option<i64>,
    pub head_sha: String,
    pub merge_base: String,
    pub verdict_suggested: String,
    pub summary_raw: String,
    pub summary_polished: Option<String>,
    pub summary_edited: Option<String>,
    pub summary_warning: Option<String>,
    pub comments: Vec<DraftComment>,
    pub edited: bool,
    pub superseded: bool,
    pub updated_at: Time,
}

impl Draft {
    pub fn summary(&self) -> &str {
        self.summary_edited.as_deref().or(self.summary_polished.as_deref()).unwrap_or(&self.summary_raw)
    }

    pub fn next_comment_id(&self) -> u64 {
        self.comments.iter().map(|c| c.id).max().unwrap_or(0) + 1
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentRun {
    pub id: i64,
    pub pr_id: String,
    pub kind: String,
    pub head_sha: String,
    pub merge_base: Option<String>,
    pub status: String,
    pub started_at: Time,
    pub ended_at: Option<Time>,
    pub cost_usd: Option<f64>,
    pub turns: Option<u32>,
    pub log_path: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PostedRow {
    pub review_id: u64,
    pub pr_id: String,
    pub draft_id: Option<i64>,
    pub head_sha: String,
    pub verdict: Option<String>,
    pub pending: bool,
    pub url: String,
    pub posted_at: Time,
    pub body: String,
}
