//! Parsing agent output: stream-json events, the structured result, comment
//! validation against the hunk index, dup flags, and polish verification.

use anyhow::{Result, anyhow};
use serde_json::Value;

use super::schema::{AgentComment, DraftComment, DupInfo, ReviewOutput};
use crate::repo::diff::{DiffIndex, Side};
use crate::state::SaidItem;

#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    Init { slash_commands: Vec<String>, model: String },
    Text(String),
    Tool { name: String, summary: String },
    Result(RunResult),
    Other,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunResult {
    pub structured: Option<Value>,
    pub text: String,
    pub cost_usd: f64,
    pub turns: u32,
    pub is_error: bool,
    pub subtype: String,
    pub duration_ms: u64,
}

fn short(s: &str, n: usize) -> String {
    let s = s.lines().next().unwrap_or_default();
    if s.chars().count() > n { format!("{}…", s.chars().take(n).collect::<String>()) } else { s.to_string() }
}

pub fn tool_summary(name: &str, input: &Value, strip: Option<&str>) -> String {
    let get = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let rel = |p: String| match strip {
        Some(prefix) => p.strip_prefix(prefix).map(|r| r.trim_start_matches('/').to_string()).unwrap_or(p),
        None => p,
    };
    match name {
        "Read" => format!("Reading {}", rel(get("file_path"))),
        "Grep" => format!("Searching for {}", short(&get("pattern"), 60)),
        "Glob" => format!("Listing {}", short(&get("pattern"), 60)),
        "Bash" => format!("Running {}", short(&rel(get("command")).replace(strip.unwrap_or("\u{0}"), ""), 80)),
        "Skill" => format!("Using skill {}", get("skill")),
        "StructuredOutput" => "Writing review".into(),
        other => other.to_string(),
    }
}

pub fn parse_stream_line(line: &str, strip: Option<&str>) -> Vec<StreamEvent> {
    let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { return vec![] };
    match v.get("type").and_then(Value::as_str) {
        Some("system") if v.get("subtype").and_then(Value::as_str) == Some("init") => vec![StreamEvent::Init {
            slash_commands: v
                .get("slash_commands")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
                .unwrap_or_default(),
            model: v.get("model").and_then(Value::as_str).unwrap_or_default().into(),
        }],
        Some("assistant") => v
            .pointer("/message/content")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|c| match c.get("type").and_then(Value::as_str) {
                        Some("text") => c.get("text").and_then(Value::as_str).map(|t| StreamEvent::Text(t.to_string())),
                        Some("tool_use") => {
                            let name = c.get("name").and_then(Value::as_str).unwrap_or("tool").to_string();
                            let summary = tool_summary(&name, c.get("input").unwrap_or(&Value::Null), strip);
                            Some(StreamEvent::Tool { name, summary })
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        Some("result") => vec![StreamEvent::Result(RunResult {
            structured: v.get("structured_output").filter(|s| !s.is_null()).cloned(),
            text: v.get("result").and_then(Value::as_str).unwrap_or_default().into(),
            cost_usd: v.get("total_cost_usd").and_then(Value::as_f64).unwrap_or(0.0),
            turns: v.get("num_turns").and_then(Value::as_u64).map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX)),
            is_error: v.get("is_error").and_then(Value::as_bool).unwrap_or(false),
            subtype: v.get("subtype").and_then(Value::as_str).unwrap_or_default().into(),
            duration_ms: v.get("duration_ms").and_then(Value::as_u64).unwrap_or(0),
        })],
        _ => vec![StreamEvent::Other],
    }
}

/// `structured_output`, falling back to the last fenced JSON block in the text.
pub fn extract_review(r: &RunResult) -> Result<ReviewOutput> {
    if let Some(s) = &r.structured {
        return serde_json::from_value(s.clone())
            .map_err(|e| anyhow!("structured output didn't match the schema: {e}"));
    }
    for block in fenced_blocks(&r.text).iter().rev() {
        let (info, body) = split_fence(block);
        if (info.is_empty() || info.starts_with("json"))
            && let Ok(out) = serde_json::from_str::<ReviewOutput>(body)
        {
            return Ok(out);
        }
    }
    if let Ok(out) = serde_json::from_str::<ReviewOutput>(r.text.trim()) {
        return Ok(out);
    }
    Err(anyhow!("agent returned no structured review"))
}

/// Fenced code blocks (`` ``` `` or `~~~`), each including its fence lines.
pub fn fenced_blocks(md: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur: Option<(char, usize, String)> = None;
    for line in md.split_inclusive('\n') {
        let trimmed = line.trim_start_matches(' ');
        let indent = line.len() - trimmed.len();
        let fence = |t: &str| -> Option<(char, usize)> {
            let c = t.chars().next()?;
            if c != '`' && c != '~' {
                return None;
            }
            let n = t.chars().take_while(|&x| x == c).count();
            (n >= 3).then_some((c, n))
        };
        match &mut cur {
            None => {
                if indent <= 3
                    && let Some((c, n)) = fence(trimmed)
                {
                    cur = Some((c, n, line.to_string()));
                }
            }
            Some((c, n, buf)) => {
                buf.push_str(line);
                if indent <= 3
                    && let Some((c2, n2)) = fence(trimmed)
                    && c2 == *c
                    && n2 >= *n
                    && trimmed.trim_end().chars().all(|x| x == c2)
                {
                    out.push(std::mem::take(buf));
                    cur = None;
                }
            }
        }
    }
    if let Some((_, _, buf)) = cur {
        out.push(buf);
    }
    out
}

/// (info string, body) of a fenced block.
pub fn split_fence(block: &str) -> (&str, &str) {
    let (first, rest) = block.split_once('\n').unwrap_or((block, ""));
    let info = first.trim().trim_start_matches(['`', '~']).trim();
    let body = rest
        .rsplit_once('\n')
        .map_or("", |(b, last)| if last.trim().is_empty() { b.rsplit_once('\n').map_or("", |x| x.0) } else { b });
    (info, body)
}

fn suggestion_lines(body: &str) -> Option<usize> {
    fenced_blocks(body)
        .iter()
        .map(|b| split_fence(b))
        .filter(|(info, _)| info.split_whitespace().next() == Some("suggestion"))
        .map(|(_, b)| if b.is_empty() { 0 } else { b.lines().count() })
        .max()
}

/// Fill in harmless omissions before validation: a missing side means RIGHT;
/// a range without a start side uses the end side; a one-line "range" is a line.
pub fn normalize(c: &mut AgentComment) {
    if c.subject_type == "file" {
        c.line = None;
        c.side = None;
        c.start_line = None;
        c.start_side = None;
        return;
    }
    if c.side.is_none() {
        c.side = Some("RIGHT".into());
    }
    if c.start_line.is_some() && c.start_line == c.line {
        c.start_line = None;
        c.start_side = None;
    }
    if c.start_line.is_some() && c.start_side.is_none() {
        c.start_side = c.side.clone();
    }
    if c.start_line.is_none() {
        c.start_side = None;
    }
}

/// Why a comment can't be posted as anchored, or None if it's fine.
pub fn validate(c: &AgentComment, diff: &DiffIndex) -> Option<String> {
    let Some(f) = diff.file(&c.path) else { return Some(format!("{} isn't in the diff", c.path)) };
    match c.subject_type.as_str() {
        "file" => return None,
        "line" => {}
        other => return Some(format!("unknown subject_type {other:?}")),
    }
    if f.binary || f.hunks.is_empty() {
        return Some("file has no diff hunks; only file-level comments are possible".into());
    }
    let Some(line) = c.line else { return Some("line comment without a line".into()) };
    let Some(side) = c.side.as_deref().and_then(Side::parse) else { return Some("side must be LEFT or RIGHT".into()) };
    if f.status == crate::repo::diff::FileStatus::Deleted && side == Side::Right {
        return Some("deleted file: comments must be on the LEFT".into());
    }
    let Some(hunk) = diff.hunk_of(&c.path, side, line) else {
        return Some(match side {
            Side::Right => format!("line {line} isn't an added or context line in the diff"),
            Side::Left => format!("line {line} isn't a deleted or context line in the diff"),
        });
    };
    match c.start_line {
        Some(start) => {
            let Some(ss) = c.start_side.as_deref().and_then(Side::parse) else {
                return Some("multi-line comment needs start_side".into());
            };
            if start >= line {
                return Some(format!("start_line {start} must be before line {line}"));
            }
            match diff.hunk_of(&c.path, ss, start) {
                Some(h) if h == hunk => {}
                Some(_) => return Some("start_line and line are in different hunks".into()),
                None => return Some(format!("start_line {start} isn't in the diff")),
            }
        }
        None => {
            if suggestion_lines(&c.body).is_some_and(|n| n > 1) {
                return Some("multi-line suggestion needs a line range (start_line)".into());
            }
        }
    }
    None
}

/// ⚠ possible dup: an already-said comment on the same path within ±3 lines
/// (non-outdated threads), or on the same path at all (outdated threads).
pub fn dup_of(c: &AgentComment, said: &[SaidItem]) -> Option<DupInfo> {
    let same_path: Vec<&SaidItem> = said.iter().filter(|s| s.path.as_deref() == Some(c.path.as_str())).collect();
    let near = same_path.iter().find(|s| {
        !s.outdated
            && match (c.line, s.line) {
                (Some(a), Some(b)) => a.abs_diff(b) <= 3,
                // File-level on either side: same path is close enough.
                _ => true,
            }
    });
    let hit = near.map(|s| (s, false)).or_else(|| same_path.iter().find(|s| s.outdated).map(|s| (s, true)))?;
    Some(DupInfo {
        author: hit.0.author.clone(),
        commit: hit.0.commit.clone(),
        body: hit.0.body.clone(),
        url: hit.0.url.clone(),
        outdated: hit.1,
    })
}

pub fn build_draft_comments(out: &ReviewOutput, diff: &DiffIndex, said: &[SaidItem]) -> Vec<DraftComment> {
    out.comments
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut a = c.clone();
            normalize(&mut a);
            let invalid = validate(&a, diff);
            DraftComment {
                id: i as u64 + 1,
                dup: dup_of(&a, said),
                body_raw: a.body.clone(),
                include: invalid.is_none(),
                invalid,
                anchor: a,
                body_polished: None,
                body_edited: None,
                polish_warning: None,
                may_be_outdated: false,
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct PolishCheck {
    pub summary: Result<String, String>,
    /// Per original comment: the polished body, or why it was rejected.
    pub comments: Vec<Result<String, String>>,
}

pub fn verify_polish(orig: &ReviewOutput, polished: &ReviewOutput) -> PolishCheck {
    let blocks_match = |a: &str, b: &str| fenced_blocks(a) == fenced_blocks(b);
    let summary = if blocks_match(&orig.summary, &polished.summary) {
        Ok(polished.summary.clone())
    } else {
        Err("polish changed a code block in the summary; kept the original".into())
    };
    let comments = if orig.comments.len() != polished.comments.len() {
        let msg = format!(
            "polish returned {} comments instead of {}; kept the originals",
            polished.comments.len(),
            orig.comments.len()
        );
        orig.comments.iter().map(|_| Err(msg.clone())).collect()
    } else {
        orig.comments
            .iter()
            .zip(&polished.comments)
            .map(|(o, p)| {
                if !o.same_anchor(p) {
                    Err("polish moved this comment; kept the original".into())
                } else if !blocks_match(&o.body, &p.body) {
                    Err("polish changed a code block; kept the original".into())
                } else {
                    Ok(p.body.clone())
                }
            })
            .collect()
    };
    PolishCheck { summary, comments }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Source;

    const DIFF: &str = "diff --git a/pkg/retry.go b/pkg/retry.go
--- a/pkg/retry.go
+++ b/pkg/retry.go
@@ -38,4 +38,6 @@
 	delay := base
-	delay = delay * 2
+	delay *= 2
+	time.Sleep(delay)
+	log.Print(x)
 	return
 }
@@ -90,2 +92,2 @@
-	a := 1
+	a := 2
 	return a
diff --git a/old.txt b/new.txt
similarity index 100%
rename from old.txt
rename to new.txt
diff --git a/gone.go b/gone.go
deleted file mode 100644
--- a/gone.go
+++ /dev/null
@@ -1,1 +0,0 @@
-package gone
";

    fn c(path: &str, line: Option<u64>, side: &str, start: Option<u64>, body: &str) -> AgentComment {
        AgentComment {
            path: path.into(),
            subject_type: "line".into(),
            line,
            side: Some(side.into()),
            start_line: start,
            start_side: start.map(|_| side.into()),
            body: body.into(),
        }
    }

    #[test]
    fn validation_rules() {
        let d = DiffIndex::parse(DIFF);
        assert_eq!(validate(&c("pkg/retry.go", Some(40), "RIGHT", None, "x"), &d), None);
        assert_eq!(validate(&c("pkg/retry.go", Some(38), "RIGHT", None, "x"), &d), None, "context line is OK");
        assert!(validate(&c("pkg/retry.go", Some(60), "RIGHT", None, "x"), &d).is_some());
        assert_eq!(validate(&c("pkg/retry.go", Some(39), "LEFT", None, "x"), &d), None, "deleted line on LEFT");
        assert!(validate(&c("pkg/retry.go", Some(45), "LEFT", None, "x"), &d).is_some(), "LEFT 45 doesn't exist");
        assert_eq!(validate(&c("pkg/retry.go", Some(41), "RIGHT", Some(39), "x"), &d), None);
        assert!(validate(&c("pkg/retry.go", Some(39), "RIGHT", Some(41), "x"), &d).unwrap().contains("before"));
        assert!(
            validate(&c("pkg/retry.go", Some(93), "RIGHT", Some(40), "x"), &d).unwrap().contains("different hunks")
        );
        assert!(validate(&c("nope.go", Some(1), "RIGHT", None, "x"), &d).is_some());
        assert!(validate(&c("new.txt", Some(1), "RIGHT", None, "x"), &d).unwrap().contains("file-level"));
        assert!(validate(&c("gone.go", Some(1), "RIGHT", None, "x"), &d).unwrap().contains("LEFT"));
        assert_eq!(validate(&c("gone.go", Some(1), "LEFT", None, "x"), &d), None);
        let mut f = c("new.txt", None, "RIGHT", None, "x");
        f.subject_type = "file".into();
        assert_eq!(validate(&f, &d), None);

        let multi = "Try:\n```suggestion\na\nb\n```\n";
        assert!(validate(&c("pkg/retry.go", Some(40), "RIGHT", None, multi), &d).unwrap().contains("suggestion"));
        assert_eq!(validate(&c("pkg/retry.go", Some(41), "RIGHT", Some(40), multi), &d), None);
        let single = "```suggestion\na\n```";
        assert_eq!(validate(&c("pkg/retry.go", Some(40), "RIGHT", None, single), &d), None);
    }

    #[test]
    fn normalization() {
        let mut a = c("p", Some(5), "RIGHT", Some(5), "x");
        normalize(&mut a);
        assert_eq!((a.start_line, a.start_side.clone()), (None, None));
        let mut a = c("p", Some(5), "RIGHT", Some(3), "x");
        a.start_side = None;
        a.side = None;
        normalize(&mut a);
        assert_eq!(a.side.as_deref(), Some("RIGHT"));
        assert_eq!(a.start_side.as_deref(), Some("RIGHT"));
    }

    fn said(path: &str, line: u64, outdated: bool) -> SaidItem {
        SaidItem {
            source: Source::Bot,
            author: "ai-reviewer".into(),
            kind: "review comment",
            path: Some(path.into()),
            line: Some(line),
            outdated,
            commit: None,
            body: "prior".into(),
            created_at: chrono::Utc::now(),
            url: "u".into(),
        }
    }

    #[test]
    fn dup_flags() {
        let a = c("pkg/retry.go", Some(40), "RIGHT", None, "x");
        assert!(dup_of(&a, &[said("pkg/retry.go", 43, false)]).is_some_and(|d| !d.outdated));
        assert!(dup_of(&a, &[said("pkg/retry.go", 44, false)]).is_none());
        assert!(dup_of(&a, &[said("pkg/retry.go", 400, true)]).is_some_and(|d| d.outdated));
        assert!(dup_of(&a, &[said("other.go", 40, false)]).is_none());
    }

    #[test]
    fn fenced_block_extraction() {
        let md = "text\n```go\nfunc x() {}\n```\nmore\n~~~~\na\n~~~\nstill in\n~~~~\n";
        let b = fenced_blocks(md);
        assert_eq!(b.len(), 2);
        assert_eq!(split_fence(&b[0]), ("go", "func x() {}"));
        assert_eq!(split_fence(&b[1]).1, "a\n~~~\nstill in");
        assert_eq!(fenced_blocks("````\n```\ninner\n```\n````").len(), 1);
    }

    fn out(summary: &str, comments: Vec<AgentComment>) -> ReviewOutput {
        ReviewOutput { verdict: "COMMENT".into(), summary: summary.into(), comments }
    }

    #[test]
    fn polish_verification() {
        let o = out(
            "Nice.\n```go\nx := 1\n```\n",
            vec![
                c("a", Some(1), "RIGHT", None, "bad wording\n```suggestion\nfoo()\n```"),
                c("b", Some(2), "RIGHT", None, "ok"),
            ],
        );
        let good = out(
            "Nice work.\n```go\nx := 1\n```\n",
            vec![
                c("a", Some(1), "RIGHT", None, "Better wording.\n```suggestion\nfoo()\n```"),
                c("b", Some(2), "RIGHT", None, "OK."),
            ],
        );
        let r = verify_polish(&o, &good);
        assert!(r.summary.is_ok());
        assert!(r.comments.iter().all(std::result::Result::is_ok));

        let bad = out(
            "Nice.\n```go\nx := 2\n```\n",
            vec![
                c("a", Some(1), "RIGHT", None, "Better.\n```suggestion\nfoo( )\n```"),
                c("b", Some(3), "RIGHT", None, "OK."),
            ],
        );
        let r = verify_polish(&o, &bad);
        assert!(r.summary.is_err());
        assert!(r.comments[0].as_ref().unwrap_err().contains("code block"));
        assert!(r.comments[1].as_ref().unwrap_err().contains("moved"));

        let short = out("Nice.\n```go\nx := 1\n```\n", vec![]);
        let r = verify_polish(&o, &short);
        assert!(r.comments.iter().all(std::result::Result::is_err));
    }

    #[test]
    fn stream_parsing_and_fallback() {
        let lines = [
            r#"{"type":"system","subtype":"init","model":"m","slash_commands":["pr-reviewer","writing"]}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Looking"},{"type":"tool_use","name":"Read","input":{"file_path":"/wt/pkg/retry.go"}}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"num_turns":3,"total_cost_usd":0.31,"duration_ms":5,"result":"done","structured_output":{"verdict":"COMMENT","summary":"s","comments":[]}}"#,
        ];
        let evs: Vec<_> = lines.iter().flat_map(|l| parse_stream_line(l, Some("/wt"))).collect();
        assert!(matches!(&evs[0], StreamEvent::Init { slash_commands, .. } if slash_commands.len() == 2));
        assert_eq!(evs[2], StreamEvent::Tool { name: "Read".into(), summary: "Reading pkg/retry.go".into() });
        let StreamEvent::Result(r) = &evs[3] else { panic!() };
        assert_eq!(extract_review(r).unwrap().summary, "s");

        let fallback = RunResult {
            text: "Here:\n```json\n{\"verdict\":\"APPROVE\",\"summary\":\"y\",\"comments\":[]}\n```\n".into(),
            ..Default::default()
        };
        assert_eq!(extract_review(&fallback).unwrap().verdict, "APPROVE");
        assert!(extract_review(&RunResult { text: "nothing".into(), ..Default::default() }).is_err());
    }
}
