//! The review output schema (design section 8.4) and the draft types built from it.

use serde::{Deserialize, Serialize};

pub fn review_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["verdict", "summary", "comments"],
        "properties": {
            "verdict": { "type": "string", "enum": ["COMMENT", "APPROVE", "REQUEST_CHANGES"] },
            "summary": { "type": "string", "description": "Markdown review summary." },
            "comments": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["path", "subject_type", "body"],
                    "properties": {
                        "path": { "type": "string" },
                        "subject_type": { "type": "string", "enum": ["line", "file"] },
                        "line": { "type": ["integer", "null"] },
                        "side": { "type": ["string", "null"], "enum": ["LEFT", "RIGHT", null] },
                        "start_line": { "type": ["integer", "null"] },
                        "start_side": { "type": ["string", "null"], "enum": ["LEFT", "RIGHT", null] },
                        "body": { "type": "string", "description": "Markdown; may include a ```suggestion block." }
                    }
                }
            }
        }
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewOutput {
    pub verdict: String,
    pub summary: String,
    #[serde(default)]
    pub comments: Vec<AgentComment>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentComment {
    pub path: String,
    #[serde(default = "line_type")]
    pub subject_type: String,
    #[serde(default)]
    pub line: Option<u64>,
    #[serde(default)]
    pub side: Option<String>,
    #[serde(default)]
    pub start_line: Option<u64>,
    #[serde(default)]
    pub start_side: Option<String>,
    pub body: String,
}

fn line_type() -> String {
    "line".into()
}

impl AgentComment {
    /// Same anchor (everything but the body).
    pub fn same_anchor(&self, o: &AgentComment) -> bool {
        self.path == o.path
            && self.subject_type == o.subject_type
            && self.line == o.line
            && self.side == o.side
            && self.start_line == o.start_line
            && self.start_side == o.start_side
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DupInfo {
    pub author: String,
    pub commit: Option<String>,
    pub body: String,
    pub url: String,
    pub outdated: bool,
}

/// One inline comment in a draft (stored in `drafts.comments_json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DraftComment {
    pub id: u64,
    pub anchor: AgentComment,
    pub body_raw: String,
    #[serde(default)]
    pub body_polished: Option<String>,
    #[serde(default)]
    pub body_edited: Option<String>,
    pub include: bool,
    #[serde(default)]
    pub invalid: Option<String>,
    #[serde(default)]
    pub dup: Option<DupInfo>,
    #[serde(default)]
    pub polish_warning: Option<String>,
    #[serde(default)]
    pub may_be_outdated: bool,
}

impl DraftComment {
    pub fn body(&self) -> &str {
        self.body_edited.as_deref().or(self.body_polished.as_deref()).unwrap_or(&self.body_raw)
    }
}
