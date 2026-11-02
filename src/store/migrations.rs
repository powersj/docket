//! Versioned schema migrations, tracked with `PRAGMA user_version`.

use rusqlite::Connection;

pub const MIGRATIONS: &[&str] = &[
    // 1: initial schema (design section 11).
    r"
    CREATE TABLE prs(
        id TEXT PRIMARY KEY,
        repo TEXT NOT NULL,
        number INTEGER NOT NULL,
        title TEXT NOT NULL DEFAULT '',
        author TEXT NOT NULL DEFAULT '',
        url TEXT NOT NULL DEFAULT '',
        is_draft INTEGER NOT NULL DEFAULT 0,
        head_sha TEXT NOT NULL DEFAULT '',
        base_sha TEXT NOT NULL DEFAULT '',
        is_requested INTEGER NOT NULL DEFAULT 0,
        stage TEXT NOT NULL,
        stage_detail TEXT,
        stage_before_error TEXT,
        stage_updated_at TEXT NOT NULL,
        snoozed_until TEXT,
        ignored_at_sha TEXT,
        agentic_label_applied_at TEXT,
        agentic_done_sha TEXT,
        agentic_done_at TEXT,
        reviewed_sha TEXT,
        reviewed_pending INTEGER NOT NULL DEFAULT 0,
        last_seen_at TEXT,
        updated_at TEXT,
        closed INTEGER NOT NULL DEFAULT 0,
        raw_json TEXT
    );
    CREATE TABLE drafts(
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        pr_id TEXT NOT NULL,
        run_id INTEGER,
        head_sha TEXT NOT NULL,
        merge_base TEXT NOT NULL DEFAULT '',
        verdict_suggested TEXT NOT NULL DEFAULT 'COMMENT',
        summary_raw TEXT NOT NULL DEFAULT '',
        summary_polished TEXT,
        summary_edited TEXT,
        summary_warning TEXT,
        comments_json TEXT NOT NULL DEFAULT '[]',
        edited INTEGER NOT NULL DEFAULT 0,
        superseded INTEGER NOT NULL DEFAULT 0,
        updated_at TEXT NOT NULL
    );
    CREATE INDEX drafts_pr ON drafts(pr_id, superseded);
    CREATE TABLE agent_runs(
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        pr_id TEXT NOT NULL,
        kind TEXT NOT NULL,
        head_sha TEXT NOT NULL,
        merge_base TEXT,
        status TEXT NOT NULL,
        started_at TEXT NOT NULL,
        ended_at TEXT,
        cost_usd REAL,
        turns INTEGER,
        log_path TEXT,
        error TEXT
    );
    CREATE TABLE posted_reviews(
        review_id INTEGER PRIMARY KEY,
        pr_id TEXT NOT NULL,
        draft_id INTEGER,
        head_sha TEXT NOT NULL,
        verdict TEXT,
        pending INTEGER NOT NULL DEFAULT 0,
        url TEXT,
        posted_at TEXT NOT NULL,
        body TEXT
    );
    CREATE TABLE label_events(
        pr_id TEXT NOT NULL,
        label TEXT NOT NULL,
        head_sha TEXT NOT NULL,
        applied_at TEXT NOT NULL,
        applied_by_app INTEGER NOT NULL
    );
    CREATE TABLE ui_state(key TEXT PRIMARY KEY, value TEXT NOT NULL);
    ",
];

pub fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    // A negative version means the file was not written by this app; refuse it
    // rather than skip every migration.
    let applied = usize::try_from(version).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, version))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(applied) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i64::try_from(i + 1).unwrap_or(i64::MAX))?;
        tx.commit()?;
    }
    Ok(())
}
