//! Unified diff parsing and the hunk index used for comment validation and
//! hunk context (design sections 8.2, 8.4, 10.5).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineKind {
    Add,
    Del,
    Ctx,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old: Option<u64>,
    pub new: Option<u64>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    #[serde(rename = "LEFT")]
    Left,
    #[serde(rename = "RIGHT")]
    Right,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Left => "LEFT",
            Side::Right => "RIGHT",
        }
    }
    pub fn parse(s: &str) -> Option<Side> {
        match s.to_ascii_uppercase().as_str() {
            "LEFT" => Some(Side::Left),
            "RIGHT" => Some(Side::Right),
            _ => None,
        }
    }
}

impl DiffLine {
    /// The line number on `side`, if this line exists there.
    pub fn on(&self, side: Side) -> Option<u64> {
        match side {
            Side::Left => self.old,
            Side::Right => self.new,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiffIndex {
    pub files: Vec<FileDiff>,
}

fn unquote(p: &str) -> String {
    let p = p.trim();
    if p.len() >= 2 && p.starts_with('"') && p.ends_with('"') {
        let inner = &p[1..p.len() - 1];
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('t') => out.push('\t'),
                    Some('n') => out.push('\n'),
                    Some(o) => out.push(o),
                    None => {}
                }
            } else {
                out.push(c);
            }
        }
        out
    } else {
        p.to_string()
    }
}

fn strip_prefix_path(p: &str) -> Option<String> {
    let p = unquote(p);
    if p == "/dev/null" {
        return None;
    }
    Some(p.strip_prefix("a/").or_else(|| p.strip_prefix("b/")).unwrap_or(&p).to_string())
}

fn parse_range(s: &str) -> Option<(u64, u64)> {
    let s = &s[1..];
    match s.split_once(',') {
        Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?)),
        None => Some((s.parse().ok()?, 1)),
    }
}

impl DiffIndex {
    pub fn parse(text: &str) -> DiffIndex {
        let mut files: Vec<FileDiff> = vec![];
        let mut cur: Option<FileDiff> = None;
        let (mut old_no, mut new_no) = (0u64, 0u64);
        let mut in_hunk = false;

        for raw in text.split('\n') {
            let line = raw.strip_suffix('\r').unwrap_or(raw);
            if let Some(rest) = line.strip_prefix("diff --git ") {
                if let Some(f) = cur.take() {
                    files.push(f);
                }
                in_hunk = false;
                // Best effort from the header; ---/+++ lines refine it.
                let path = rest.rsplit_once(" b/").map(|(_, b)| b.to_string()).unwrap_or_else(|| rest.to_string());
                cur = Some(FileDiff {
                    path: unquote(&path),
                    old_path: None,
                    status: FileStatus::Modified,
                    binary: false,
                    hunks: vec![],
                    additions: 0,
                    deletions: 0,
                });
                continue;
            }
            let Some(f) = cur.as_mut() else { continue };
            if in_hunk {
                let mut push = |kind, old, new, text: &str| {
                    if let Some(h) = f.hunks.last_mut() {
                        h.lines.push(DiffLine { kind, old, new, text: text.to_string() });
                    }
                };
                if let Some(t) = line.strip_prefix('+') {
                    push(LineKind::Add, None, Some(new_no), t);
                    new_no += 1;
                    f.additions += 1;
                    continue;
                } else if let Some(t) = line.strip_prefix('-') {
                    push(LineKind::Del, Some(old_no), None, t);
                    old_no += 1;
                    f.deletions += 1;
                    continue;
                } else if let Some(t) = line.strip_prefix(' ') {
                    push(LineKind::Ctx, Some(old_no), Some(new_no), t);
                    old_no += 1;
                    new_no += 1;
                    continue;
                } else if line.starts_with('\\') {
                    continue;
                } else if !line.starts_with("@@") {
                    in_hunk = false;
                }
            }
            if line.starts_with("@@") {
                let mut parts = line.split_whitespace().skip(1);
                let (Some(o), Some(n)) = (parts.next(), parts.next()) else { continue };
                let (Some((os, _)), Some((ns, _))) = (parse_range(o), parse_range(n)) else { continue };
                old_no = os;
                new_no = ns;
                in_hunk = true;
                f.hunks.push(Hunk { header: line.to_string(), lines: vec![] });
            } else if line.starts_with("new file mode") {
                f.status = FileStatus::Added;
            } else if line.starts_with("deleted file mode") {
                f.status = FileStatus::Deleted;
            } else if let Some(p) = line.strip_prefix("rename from ") {
                f.status = FileStatus::Renamed;
                f.old_path = Some(unquote(p));
            } else if let Some(p) = line.strip_prefix("rename to ") {
                f.path = unquote(p);
            } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
                f.binary = true;
            } else if let Some(p) = line.strip_prefix("--- ") {
                if let Some(p) = strip_prefix_path(p) {
                    if f.status != FileStatus::Renamed {
                        f.old_path = None;
                    }
                    if f.status == FileStatus::Deleted {
                        f.path = p;
                    }
                }
            } else if let Some(p) = line.strip_prefix("+++ ")
                && let Some(p) = strip_prefix_path(p)
            {
                f.path = p;
            }
        }
        if let Some(f) = cur.take() {
            files.push(f);
        }
        DiffIndex { files }
    }

    pub fn file(&self, path: &str) -> Option<&FileDiff> {
        self.files.iter().find(|f| f.path == path)
    }

    /// The hunk index containing `line` on `side`, if any.
    pub fn hunk_of(&self, path: &str, side: Side, line: u64) -> Option<usize> {
        let f = self.file(path)?;
        f.hunks.iter().position(|h| h.lines.iter().any(|l| l.on(side) == Some(line)))
    }

    /// Lines around an anchor: `above` lines before `start`, the anchored range,
    /// and `below` after `end`, all from the same hunk. Each item says whether
    /// it's part of the anchor.
    pub fn context(
        &self,
        path: &str,
        side: Side,
        start: u64,
        end: u64,
        above: usize,
        below: usize,
    ) -> Option<Vec<(DiffLine, bool)>> {
        let f = self.file(path)?;
        let hi = self.hunk_of(path, side, end)?;
        let lines = &f.hunks[hi].lines;
        let first = lines.iter().position(|l| l.on(side) == Some(start)).unwrap_or(0);
        let last = lines.iter().position(|l| l.on(side) == Some(end))?;
        let from = first.saturating_sub(above);
        let to = (last + below + 1).min(lines.len());
        Some(
            lines[from..to]
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    let idx = from + i;
                    let anchored = idx >= first && idx <= last && l.on(side).is_some();
                    (l.clone(), anchored)
                })
                .collect(),
        )
    }

    /// The text of the anchored lines on `side` (for pre-filling suggestions).
    pub fn text_of(&self, path: &str, side: Side, start: u64, end: u64) -> Option<String> {
        let f = self.file(path)?;
        let hi = self.hunk_of(path, side, end)?;
        let lines: Vec<&str> = f.hunks[hi]
            .lines
            .iter()
            .filter(|l| l.on(side).is_some_and(|n| n >= start && n <= end))
            .map(|l| l.text.as_str())
            .collect();
        Some(lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const SAMPLE: &str = "diff --git a/pkg/retry.go b/pkg/retry.go
index 111..222 100644
--- a/pkg/retry.go
+++ b/pkg/retry.go
@@ -38,6 +38,8 @@ func Retry() {
 	delay := base
 	for attempt := 0; ; attempt++ {
-		delay = delay * 2
+		delay *= 2
+		time.Sleep(delay)
+		log.Print(attempt)
 		if err == nil {
 			return
 		}
@@ -80,2 +82,2 @@ func other() {
-	a := 1
+	a := 2
 	return a
diff --git a/old.txt b/new.txt
similarity index 100%
rename from old.txt
rename to new.txt
diff --git a/img.png b/img.png
new file mode 100644
index 0000000..1234567
Binary files /dev/null and b/img.png differ
diff --git a/gone.go b/gone.go
deleted file mode 100644
index 333..000
--- a/gone.go
+++ /dev/null
@@ -1,2 +0,0 @@
-package gone
-var x = 1
diff --git a/new.go b/new.go
new file mode 100644
--- /dev/null
+++ b/new.go
@@ -0,0 +1,2 @@
+package x
+\\ end
\\ No newline at end of file
";

    #[test]
    fn parses_files() {
        let d = DiffIndex::parse(SAMPLE);
        let names: Vec<_> = d.files.iter().map(|f| (f.path.as_str(), f.status, f.binary)).collect();
        assert_eq!(
            names,
            vec![
                ("pkg/retry.go", FileStatus::Modified, false),
                ("new.txt", FileStatus::Renamed, false),
                ("img.png", FileStatus::Added, true),
                ("gone.go", FileStatus::Deleted, false),
                ("new.go", FileStatus::Added, false),
            ]
        );
        assert_eq!(d.files[1].old_path.as_deref(), Some("old.txt"));
        assert!(d.files[1].hunks.is_empty());
        let r = &d.files[0];
        assert_eq!((r.additions, r.deletions), (4, 2));
        assert_eq!(r.hunks.len(), 2);
        assert_eq!(d.files[4].hunks[0].lines.len(), 2);
    }

    #[test]
    fn line_numbers() {
        let d = DiffIndex::parse(SAMPLE);
        let h = &d.files[0].hunks[0];
        let added: Vec<_> = h.lines.iter().filter(|l| l.kind == LineKind::Add).map(|l| l.new.unwrap()).collect();
        assert_eq!(added, vec![40, 41, 42]);
        let del = h.lines.iter().find(|l| l.kind == LineKind::Del).unwrap();
        assert_eq!(del.old, Some(40));
        assert_eq!(d.hunk_of("pkg/retry.go", Side::Right, 42), Some(0));
        assert_eq!(d.hunk_of("pkg/retry.go", Side::Right, 43), Some(0)); // context "if err == nil"
        assert_eq!(d.hunk_of("pkg/retry.go", Side::Right, 60), None);
        assert_eq!(d.hunk_of("pkg/retry.go", Side::Left, 40), Some(0));
        assert_eq!(d.hunk_of("pkg/retry.go", Side::Right, 82), Some(1));
        assert_eq!(d.hunk_of("gone.go", Side::Left, 2), Some(0));
        assert_eq!(d.hunk_of("gone.go", Side::Right, 1), None);
    }

    #[test]
    fn context_window() {
        let d = DiffIndex::parse(SAMPLE);
        let ctx = d.context("pkg/retry.go", Side::Right, 41, 42, 3, 1).unwrap();
        let anchored: Vec<_> = ctx.iter().filter(|(_, a)| *a).map(|(l, _)| l.new.unwrap()).collect();
        assert_eq!(anchored, vec![41, 42]);
        // 3 above 41: line 40 (+), the deleted line, and 39 (ctx).
        assert_eq!(ctx.len(), 3 + 2 + 1);
        assert_eq!(
            d.text_of("pkg/retry.go", Side::Right, 41, 42).unwrap(),
            "\t\ttime.Sleep(delay)\n\t\tlog.Print(attempt)"
        );
    }
}
