//! REST write payloads (design section 9).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ReviewCommentPayload {
    pub path: String,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_side: Option<String>,
    pub subject_type: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ReviewPayload {
    pub commit_id: String,
    pub body: String,
    /// `None` posts a PENDING review I can finish on GitHub.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    pub comments: Vec<ReviewCommentPayload>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PostedReview {
    pub id: u64,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub commit_id: Option<String>,
    pub user: Option<RestUser>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RestUser {
    pub login: String,
}

/// Errors from a write. `Unprocessable` carries GitHub's 422 message.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error("GitHub rejected the request (422): {0}")]
    Unprocessable(String),
    #[error("not permitted ({status}): {message}")]
    Forbidden { status: u16, message: String },
    #[error("request did not complete: {0}")]
    Transport(String),
    #[error("GitHub error ({status}): {message}")]
    Other { status: u16, message: String },
}

impl WriteError {
    /// Transport errors and 5xx may have reached GitHub; check before retrying.
    pub fn maybe_applied(&self) -> bool {
        matches!(self, WriteError::Transport(_)) || matches!(self, WriteError::Other { status, .. } if *status >= 500)
    }
}

/// Find a review I already posted with this body on this commit (idempotency check).
pub fn find_matching<'a>(
    reviews: &'a [PostedReview],
    login: &str,
    commit_id: &str,
    body: &str,
) -> Option<&'a PostedReview> {
    reviews.iter().find(|r| {
        r.user.as_ref().is_some_and(|u| crate::config::same_login(&u.login, login))
            && r.commit_id.as_deref() == Some(commit_id)
            && r.body.as_deref().unwrap_or("").trim() == body.trim()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_omits_event() {
        let p = ReviewPayload { commit_id: "abc".into(), body: "b".into(), event: None, comments: vec![] };
        let v = serde_json::to_value(&p).unwrap();
        assert!(v.get("event").is_none());
        let c = ReviewCommentPayload {
            path: "a".into(),
            body: "x".into(),
            line: None,
            side: None,
            start_line: None,
            start_side: None,
            subject_type: "file".into(),
        };
        let v = serde_json::to_value(&c).unwrap();
        assert!(v.get("line").is_none());
        assert_eq!(v["subject_type"], "file");
    }

    #[test]
    fn idempotency_match() {
        let reviews: Vec<PostedReview> = serde_json::from_str(
            r#"[{"id":1,"html_url":"u","body":"hello ","commit_id":"abc","user":{"login":"Me"}},
                {"id":2,"html_url":"u","body":"other","commit_id":"abc","user":{"login":"me"}}]"#,
        )
        .unwrap();
        assert_eq!(find_matching(&reviews, "me", "abc", "hello").unwrap().id, 1);
        assert!(find_matching(&reviews, "me", "def", "hello").is_none());
    }
}
