//! SQLite persistence (design section 11).

pub mod migrations;
pub mod schema;

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::github::model::{PrSummary, Time};
use crate::state::{PrRecord, Stage};
pub use schema::*;

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

fn ts(t: &Time) -> String {
    t.to_rfc3339()
}

fn ts_opt(t: Option<&Time>) -> Option<String> {
    t.map(ts)
}

fn parse_ts(s: Option<String>) -> Option<Time> {
    s.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok()).map(|t| t.with_timezone(&chrono::Utc))
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        Self::init(Connection::open(path)?)
    }

    #[cfg(test)]
    pub fn memory() -> Result<Store> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Store> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrations::migrate(&mut conn)?;
        Ok(Store { conn: Arc::new(Mutex::new(conn)) })
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T> {
        let c = self.conn.lock().unwrap();
        Ok(f(&c)?)
    }

    // --- prs -------------------------------------------------------------

    pub fn load_prs(&self) -> Result<Vec<(PrRecord, Option<PrSummary>)>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, stage, stage_detail, stage_before_error, stage_updated_at, snoozed_until, ignored_at_sha,
                        agentic_label_applied_at, agentic_done_sha, agentic_done_at, reviewed_sha, reviewed_pending,
                        closed, raw_json FROM prs",
            )?;
            let rows = st.query_map([], |r| {
                let rec = PrRecord {
                    id: r.get(0)?,
                    stage: Stage::from_db(&r.get::<_, String>(1)?),
                    stage_detail: r.get(2)?,
                    stage_before: r.get::<_, Option<String>>(3)?.map(|s| Stage::from_db(&s)),
                    stage_updated_at: parse_ts(r.get(4)?).unwrap_or_else(chrono::Utc::now),
                    snoozed_until: parse_ts(r.get(5)?),
                    ignored_at_sha: r.get(6)?,
                    agentic_label_applied_at: parse_ts(r.get(7)?),
                    agentic_done_sha: r.get(8)?,
                    agentic_done_at: parse_ts(r.get(9)?),
                    reviewed_sha: r.get(10)?,
                    reviewed_pending: r.get(11)?,
                    closed: r.get(12)?,
                };
                let summary = r.get::<_, Option<String>>(13)?.and_then(|j| serde_json::from_str(&j).ok());
                Ok((rec, summary))
            })?;
            rows.collect()
        })
    }

    pub fn save_pr(&self, rec: &PrRecord, s: Option<&PrSummary>) -> Result<()> {
        let raw = s.map(|s| serde_json::to_string(s).unwrap_or_default());
        let (repo, number) =
            rec.id.rsplit_once('#').map(|(r, n)| (r.to_string(), n.parse::<i64>().unwrap_or(0))).unwrap_or_default();
        self.with(|c| {
            c.execute(
                "INSERT INTO prs(id, repo, number, title, author, url, is_draft, head_sha, base_sha, is_requested,
                    stage, stage_detail, stage_before_error, stage_updated_at, snoozed_until, ignored_at_sha,
                    agentic_label_applied_at, agentic_done_sha, agentic_done_at, reviewed_sha, reviewed_pending,
                    last_seen_at, updated_at, closed, raw_json)
                 VALUES (?1,?2,?3,COALESCE(?4,''),COALESCE(?5,''),COALESCE(?6,''),COALESCE(?7,0),COALESCE(?8,''),COALESCE(?9,''),COALESCE(?10,0),?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25)
                 ON CONFLICT(id) DO UPDATE SET
                    title=COALESCE(?4, title), author=COALESCE(?5, author), url=COALESCE(?6, url),
                    is_draft=COALESCE(?7, is_draft), head_sha=COALESCE(?8, head_sha), base_sha=COALESCE(?9, base_sha),
                    is_requested=COALESCE(?10, is_requested), stage=?11, stage_detail=?12, stage_before_error=?13,
                    stage_updated_at=?14, snoozed_until=?15, ignored_at_sha=?16, agentic_label_applied_at=?17,
                    agentic_done_sha=?18, agentic_done_at=?19, reviewed_sha=?20, reviewed_pending=?21,
                    last_seen_at=COALESCE(?22, last_seen_at), updated_at=COALESCE(?23, updated_at), closed=?24,
                    raw_json=COALESCE(?25, raw_json)",
                params![
                    rec.id,
                    repo,
                    number,
                    s.map(|s| s.title.clone()),
                    s.map(|s| s.author.clone()),
                    s.map(|s| s.url.clone()),
                    s.map(|s| s.is_draft),
                    s.map(|s| s.head_sha.clone()),
                    s.map(|s| s.base_sha.clone()),
                    s.map(|s| s.requested),
                    rec.stage.as_db(),
                    rec.stage_detail,
                    rec.stage_before.map(super::state::Stage::as_db),
                    ts(&rec.stage_updated_at),
                    ts_opt(rec.snoozed_until.as_ref()),
                    rec.ignored_at_sha,
                    ts_opt(rec.agentic_label_applied_at.as_ref()),
                    rec.agentic_done_sha,
                    ts_opt(rec.agentic_done_at.as_ref()),
                    rec.reviewed_sha,
                    rec.reviewed_pending,
                    s.map(|_| ts(&chrono::Utc::now())),
                    s.map(|s| ts(&s.updated_at)),
                    rec.closed,
                    raw,
                ],
            )
            .map(|_| ())
        })
    }

    // --- drafts ----------------------------------------------------------

    fn draft_from_row(r: &Row) -> rusqlite::Result<Draft> {
        Ok(Draft {
            id: r.get(0)?,
            pr_id: r.get(1)?,
            run_id: r.get(2)?,
            head_sha: r.get(3)?,
            merge_base: r.get(4)?,
            verdict_suggested: r.get(5)?,
            summary_raw: r.get(6)?,
            summary_polished: r.get(7)?,
            summary_edited: r.get(8)?,
            summary_warning: r.get(9)?,
            comments: serde_json::from_str(&r.get::<_, String>(10)?).unwrap_or_default(),
            edited: r.get(11)?,
            superseded: r.get(12)?,
            updated_at: parse_ts(r.get(13)?).unwrap_or_else(chrono::Utc::now),
        })
    }

    const DRAFT_COLS: &str =
        "id, pr_id, run_id, head_sha, merge_base, verdict_suggested, summary_raw, summary_polished,
        summary_edited, summary_warning, comments_json, edited, superseded, updated_at";

    pub fn current_draft(&self, pr_id: &str) -> Result<Option<Draft>> {
        self.with(|c| {
            c.query_row(
                &format!(
                    "SELECT {} FROM drafts WHERE pr_id=?1 AND superseded=0 ORDER BY id DESC LIMIT 1",
                    Self::DRAFT_COLS
                ),
                [pr_id],
                Self::draft_from_row,
            )
            .optional()
        })
    }

    #[cfg(test)]
    pub fn draft(&self, id: i64) -> Result<Option<Draft>> {
        self.with(|c| {
            c.query_row(&format!("SELECT {} FROM drafts WHERE id=?1", Self::DRAFT_COLS), [id], Self::draft_from_row)
                .optional()
        })
    }

    /// Insert a new current draft; older ones for the PR become history.
    pub fn insert_draft(&self, d: &Draft) -> Result<i64> {
        self.with(|c| {
            c.execute("UPDATE drafts SET superseded=1 WHERE pr_id=?1", [&d.pr_id])?;
            c.execute(
                "INSERT INTO drafts(pr_id, run_id, head_sha, merge_base, verdict_suggested, summary_raw, summary_polished,
                    summary_edited, summary_warning, comments_json, edited, superseded, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,0,?12)",
                params![
                    d.pr_id,
                    d.run_id,
                    d.head_sha,
                    d.merge_base,
                    d.verdict_suggested,
                    d.summary_raw,
                    d.summary_polished,
                    d.summary_edited,
                    d.summary_warning,
                    serde_json::to_string(&d.comments).unwrap_or_default(),
                    d.edited,
                    ts(&d.updated_at),
                ],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn update_draft(&self, d: &Draft) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE drafts SET summary_polished=?2, summary_edited=?3, summary_warning=?4, comments_json=?5,
                    edited=?6, updated_at=?7, verdict_suggested=?8 WHERE id=?1",
                params![
                    d.id,
                    d.summary_polished,
                    d.summary_edited,
                    d.summary_warning,
                    serde_json::to_string(&d.comments).unwrap_or_default(),
                    d.edited,
                    ts(&d.updated_at),
                    d.verdict_suggested,
                ],
            )
            .map(|_| ())
        })
    }

    // --- agent runs ------------------------------------------------------

    pub fn insert_run(&self, pr_id: &str, kind: &str, head: &str, mb: Option<&str>) -> Result<i64> {
        self.with(|c| {
            c.execute(
                "INSERT INTO agent_runs(pr_id, kind, head_sha, merge_base, status, started_at) VALUES (?1,?2,?3,?4,'running',?5)",
                params![pr_id, kind, head, mb, ts(&chrono::Utc::now())],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn set_run_log(&self, id: i64, log: &str, mb: Option<&str>) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE agent_runs SET log_path=?2, merge_base=COALESCE(?3, merge_base) WHERE id=?1",
                params![id, log, mb],
            )
            .map(|_| ())
        })
    }

    pub fn finish_run(
        &self,
        id: i64,
        status: &str,
        cost: Option<f64>,
        turns: Option<u32>,
        error: Option<&str>,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE agent_runs SET status=?2, ended_at=?3, cost_usd=?4, turns=?5, error=?6 WHERE id=?1",
                params![id, status, ts(&chrono::Utc::now()), cost, turns, error],
            )
            .map(|_| ())
        })
    }

    /// Runs left 'running' by a previous process become 'interrupted'.
    pub fn interrupt_stale_runs(&self) -> Result<usize> {
        self.with(|c| c.execute("UPDATE agent_runs SET status='interrupted' WHERE status='running'", []))
    }

    pub fn last_run(&self, pr_id: &str) -> Result<Option<AgentRun>> {
        self.with(|c| {
            c.query_row(
                "SELECT id, pr_id, kind, head_sha, merge_base, status, started_at, ended_at, cost_usd, turns, log_path, error
                 FROM agent_runs WHERE pr_id=?1 ORDER BY id DESC LIMIT 1",
                [pr_id],
                |r| {
                    Ok(AgentRun {
                        id: r.get(0)?,
                        pr_id: r.get(1)?,
                        kind: r.get(2)?,
                        head_sha: r.get(3)?,
                        merge_base: r.get(4)?,
                        status: r.get(5)?,
                        started_at: parse_ts(r.get(6)?).unwrap_or_else(chrono::Utc::now),
                        ended_at: parse_ts(r.get(7)?),
                        cost_usd: r.get(8)?,
                        turns: r.get(9)?,
                        log_path: r.get(10)?,
                        error: r.get(11)?,
                    })
                },
            )
            .optional()
        })
    }

    // --- posted reviews --------------------------------------------------

    pub fn insert_posted(&self, p: &PostedRow) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO posted_reviews(review_id, pr_id, draft_id, head_sha, verdict, pending, url, posted_at, body)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![p.review_id.cast_signed(), p.pr_id, p.draft_id, p.head_sha, p.verdict, p.pending, p.url, ts(&p.posted_at), p.body],
            )
            .map(|_| ())
        })
    }

    pub fn posted_ids(&self, pr_id: &str) -> Result<std::collections::HashSet<u64>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT review_id FROM posted_reviews WHERE pr_id=?1")?;
            let ids = st.query_map([pr_id], |r| r.get::<_, i64>(0))?;
            ids.map(|r| r.map(i64::cast_unsigned)).collect()
        })
    }

    pub fn posted_for_draft(&self, draft_id: i64) -> Result<Option<PostedRow>> {
        self.with(|c| {
            c.query_row(
                "SELECT review_id, pr_id, draft_id, head_sha, verdict, pending, url, posted_at, body
                 FROM posted_reviews WHERE draft_id=?1 ORDER BY posted_at DESC LIMIT 1",
                [draft_id],
                |r| {
                    Ok(PostedRow {
                        review_id: r.get::<_, i64>(0)?.cast_unsigned(),
                        pr_id: r.get(1)?,
                        draft_id: r.get(2)?,
                        head_sha: r.get(3)?,
                        verdict: r.get(4)?,
                        pending: r.get(5)?,
                        url: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                        posted_at: parse_ts(r.get(7)?).unwrap_or_else(chrono::Utc::now),
                        body: r.get::<_, Option<String>>(8)?.unwrap_or_default(),
                    })
                },
            )
            .optional()
        })
    }

    // --- label events ----------------------------------------------------

    pub fn insert_label_event(&self, pr_id: &str, label: &str, head: &str, at: &Time, by_app: bool) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO label_events(pr_id, label, head_sha, applied_at, applied_by_app) VALUES (?1,?2,?3,?4,?5)",
                params![pr_id, label, head, ts(at), by_app],
            )
            .map(|_| ())
        })
    }

    // --- ui state --------------------------------------------------------

    pub fn ui_get(&self, key: &str) -> Option<String> {
        self.with(|c| c.query_row("SELECT value FROM ui_state WHERE key=?1", [key], |r| r.get(0)).optional())
            .ok()
            .flatten()
    }

    pub fn ui_set(&self, key: &str, value: &str) {
        let r = self.with(|c| {
            c.execute(
                "INSERT INTO ui_state(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=?2",
                params![key, value],
            )
        });
        if let Err(e) = r {
            tracing::warn!("ui_state write failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::schema::{AgentComment, DraftComment};

    fn draft(pr: &str) -> Draft {
        Draft {
            id: 0,
            pr_id: pr.into(),
            run_id: None,
            head_sha: "h".into(),
            merge_base: "m".into(),
            verdict_suggested: "COMMENT".into(),
            summary_raw: "raw".into(),
            summary_polished: Some("pol".into()),
            summary_edited: None,
            summary_warning: None,
            comments: vec![DraftComment {
                id: 1,
                anchor: AgentComment {
                    path: "a".into(),
                    subject_type: "line".into(),
                    line: Some(1),
                    side: Some("RIGHT".into()),
                    start_line: None,
                    start_side: None,
                    body: "b".into(),
                },
                body_raw: "b".into(),
                body_polished: None,
                body_edited: None,
                include: true,
                invalid: None,
                dup: None,
                polish_warning: None,
                may_be_outdated: false,
            }],
            edited: false,
            superseded: false,
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn migrations_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("db");
        Store::open(&p).unwrap();
        Store::open(&p).unwrap();
    }

    #[test]
    fn pr_roundtrip() {
        let s = Store::memory().unwrap();
        let mut rec = PrRecord::new("o/a#1".into(), chrono::Utc::now());
        rec.stage = Stage::Error(crate::state::Phase::ClaudeReviewing);
        rec.stage_detail = Some("boom".into());
        rec.snoozed_until = Some(chrono::Utc::now());
        s.save_pr(&rec, None).unwrap();
        rec.stage = Stage::Posted;
        s.save_pr(&rec, None).unwrap();
        let all = s.load_prs().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].0.stage, Stage::Posted);
        assert_eq!(all[0].0.stage_detail.as_deref(), Some("boom"));
        assert!(all[0].1.is_none());
    }

    #[test]
    fn drafts_supersede() {
        let s = Store::memory().unwrap();
        let id1 = s.insert_draft(&draft("p")).unwrap();
        let mut d2 = draft("p");
        d2.summary_raw = "second".into();
        let id2 = s.insert_draft(&d2).unwrap();
        let cur = s.current_draft("p").unwrap().unwrap();
        assert_eq!(cur.id, id2);
        assert_eq!(cur.summary(), "pol");
        assert!(s.draft(id1).unwrap().unwrap().superseded);
        let mut cur = cur;
        cur.summary_edited = Some("mine".into());
        cur.edited = true;
        s.update_draft(&cur).unwrap();
        let cur = s.current_draft("p").unwrap().unwrap();
        assert_eq!(cur.summary(), "mine");
        assert_eq!(cur.comments.len(), 1);
    }

    #[test]
    fn runs_and_posted() {
        let s = Store::memory().unwrap();
        let r = s.insert_run("p", "review", "h", None).unwrap();
        assert_eq!(s.interrupt_stale_runs().unwrap(), 1);
        s.finish_run(r, "ok", Some(0.3), Some(4), None).unwrap();
        assert_eq!(s.last_run("p").unwrap().unwrap().cost_usd, Some(0.3));
        s.insert_posted(&PostedRow {
            review_id: 7,
            pr_id: "p".into(),
            draft_id: Some(1),
            head_sha: "h".into(),
            verdict: Some("COMMENT".into()),
            pending: false,
            url: "u".into(),
            posted_at: chrono::Utc::now(),
            body: "b".into(),
        })
        .unwrap();
        assert!(s.posted_ids("p").unwrap().contains(&7));
        assert!(s.posted_for_draft(1).unwrap().is_some());
        s.ui_set("zoom", "1.1");
        s.ui_set("zoom", "1.2");
        assert_eq!(s.ui_get("zoom").as_deref(), Some("1.2"));
    }
}
