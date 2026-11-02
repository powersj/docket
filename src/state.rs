//! PR lifecycle state machine (design section 7), agentic cutoff/done detection
//! (section 6) and PR list sectioning (section 10.2). Everything here is pure.

use std::collections::HashSet;

use chrono::Duration;
use serde::{Deserialize, Serialize};

use crate::config::{Config, same_login};
use crate::github::model::{PrDetail, PrState, PrSummary, Review, Time};

/// A stage in which an agent run or wait was in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Phase {
    AwaitingAgentic,
    ClaudeReviewing,
    Polishing,
}

impl Phase {
    pub fn stage(self) -> Stage {
        match self {
            Phase::AwaitingAgentic => Stage::AwaitingAgentic,
            Phase::ClaudeReviewing => Stage::ClaudeReviewing,
            Phase::Polishing => Stage::Polishing,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Phase::AwaitingAgentic => "AwaitingAgentic",
            Phase::ClaudeReviewing => "ClaudeReviewing",
            Phase::Polishing => "Polishing",
        }
    }
    fn parse(s: &str) -> Option<Phase> {
        Some(match s {
            "AwaitingAgentic" => Phase::AwaitingAgentic,
            "ClaudeReviewing" => Phase::ClaudeReviewing,
            "Polishing" => Phase::Polishing,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    New,
    Triage,
    Ignored,
    Snoozed,
    Ready,
    AwaitingAgentic,
    ClaudeReviewing,
    Polishing,
    DraftReady,
    Posted,
    ReReviewNeeded,
    Error(Phase),
    Interrupted(Phase),
}

impl Stage {
    pub fn as_db(self) -> String {
        match self {
            Stage::Error(p) => format!("Error:{}", p.name()),
            Stage::Interrupted(p) => format!("Interrupted:{}", p.name()),
            s => format!("{s:?}"),
        }
    }

    pub fn from_db(s: &str) -> Stage {
        if let Some(p) = s.strip_prefix("Error:").and_then(Phase::parse) {
            return Stage::Error(p);
        }
        if let Some(p) = s.strip_prefix("Interrupted:").and_then(Phase::parse) {
            return Stage::Interrupted(p);
        }
        match s {
            "Triage" => Stage::Triage,
            "Ignored" => Stage::Ignored,
            "Snoozed" => Stage::Snoozed,
            "Ready" => Stage::Ready,
            "AwaitingAgentic" => Stage::AwaitingAgentic,
            "ClaudeReviewing" => Stage::ClaudeReviewing,
            "Polishing" => Stage::Polishing,
            "DraftReady" => Stage::DraftReady,
            "Posted" => Stage::Posted,
            "ReReviewNeeded" => Stage::ReReviewNeeded,
            _ => Stage::New,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Stage::New => "New",
            Stage::Triage => "Triage",
            Stage::Ignored => "Ignored",
            Stage::Snoozed => "Snoozed",
            Stage::Ready => "Ready",
            Stage::AwaitingAgentic => "Waiting on bot",
            Stage::ClaudeReviewing => "Reviewing",
            Stage::Polishing => "Polishing",
            Stage::DraftReady => "Draft ready",
            Stage::Posted => "Posted",
            Stage::ReReviewNeeded => "Re-review",
            Stage::Error(_) => "Error",
            Stage::Interrupted(_) => "Interrupted",
        }
    }

    /// An agent process (or agentic wait) belongs to this stage.
    pub fn is_active(self) -> bool {
        matches!(self, Stage::AwaitingAgentic | Stage::ClaudeReviewing | Stage::Polishing)
    }

    pub fn is_hidden(self) -> bool {
        matches!(self, Stage::Ignored | Stage::Snoozed)
    }
}

/// Everything about a PR that we persist (the `prs` table minus GitHub data).
#[derive(Debug, Clone, PartialEq)]
pub struct PrRecord {
    pub id: String,
    pub stage: Stage,
    /// Error message, or "timeout" while the agentic wait has timed out.
    pub stage_detail: Option<String>,
    /// Stage to restore on Reopen.
    pub stage_before: Option<Stage>,
    pub stage_updated_at: Time,
    pub snoozed_until: Option<Time>,
    pub ignored_at_sha: Option<String>,
    pub agentic_label_applied_at: Option<Time>,
    pub agentic_done_sha: Option<String>,
    pub agentic_done_at: Option<Time>,
    pub reviewed_sha: Option<String>,
    pub reviewed_pending: bool,
    pub closed: bool,
}

impl PrRecord {
    pub fn new(id: String, now: Time) -> Self {
        Self {
            id,
            stage: Stage::New,
            stage_detail: None,
            stage_before: None,
            stage_updated_at: now,
            snoozed_until: None,
            ignored_at_sha: None,
            agentic_label_applied_at: None,
            agentic_done_sha: None,
            agentic_done_at: None,
            reviewed_sha: None,
            reviewed_pending: false,
            closed: false,
        }
    }

    fn set(&mut self, stage: Stage, now: Time) {
        if self.stage != stage {
            self.stage = stage;
            self.stage_updated_at = now;
        }
        if !matches!(stage, Stage::Error(_) | Stage::AwaitingAgentic) {
            self.stage_detail = None;
        }
    }

    pub fn agentic_timed_out(&self) -> bool {
        self.stage == Stage::AwaitingAgentic && self.stage_detail.as_deref() == Some("timeout")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The PR was selected in the list.
    Selected,
    /// Start review / Re-review / Re-run. `wait_for_agentic` is false when the
    /// repo isn't enabled or the bot already reviewed the head.
    StartReview {
        wait_for_agentic: bool,
    },
    Ignore {
        head: String,
    },
    Unignore,
    Snooze {
        until: Time,
    },
    /// Periodic check against the latest list data.
    Refreshed {
        head: String,
        requested: bool,
        auto_unignore: bool,
    },
    AgenticDone,
    AgenticTimedOut,
    /// Keep waiting after a timeout, or the label was re-applied.
    AgenticRewait,
    /// Continue without it / label unavailable.
    AgenticSkip,
    /// An agent slot freed up for a Ready PR.
    RunStarted,
    ReviewFinished,
    PolishFinished,
    RunFailed {
        phase: Phase,
        message: String,
    },
    Cancel,
    Retry,
    Discard {
        has_draft: bool,
    },
    Posted {
        head: String,
        pending: bool,
    },
    ReviewedOutside {
        head: String,
    },
    Closed,
    Reopened,
    /// App startup: anything that was live is now interrupted.
    AppStarted,
}

/// Apply an event. Returns whether anything changed. Events that don't apply
/// in the current stage are ignored.
pub fn apply(rec: &mut PrRecord, ev: Event, now: Time) -> bool {
    let before = rec.clone();
    use Stage::{
        AwaitingAgentic, ClaudeReviewing, DraftReady, Error, Ignored, Interrupted, New, Polishing, Posted,
        ReReviewNeeded, Ready, Snoozed, Triage,
    };
    match ev {
        Event::Selected => {
            if rec.stage == New {
                rec.set(Triage, now);
            }
        }
        Event::StartReview { wait_for_agentic } => {
            if !rec.stage.is_active() && !rec.closed {
                if wait_for_agentic {
                    rec.set(AwaitingAgentic, now);
                    rec.stage_detail = None;
                } else {
                    rec.set(Ready, now);
                }
                rec.snoozed_until = None;
                rec.ignored_at_sha = None;
            }
        }
        Event::Ignore { head } => {
            if !rec.stage.is_active() {
                rec.stage_before = Some(rec.stage);
                rec.set(Ignored, now);
                rec.ignored_at_sha = Some(head);
            }
        }
        Event::Unignore => {
            if rec.stage == Ignored {
                rec.ignored_at_sha = None;
                let back = rec.stage_before.take().filter(|s| !s.is_active()).unwrap_or(Triage);
                rec.set(back, now);
            }
        }
        Event::Snooze { until } => {
            if !rec.stage.is_active() {
                rec.stage_before = Some(rec.stage);
                rec.snoozed_until = Some(until);
                rec.set(Snoozed, now);
            }
        }
        Event::Refreshed { head, requested, auto_unignore } => match rec.stage {
            Snoozed if rec.snoozed_until.is_none_or(|u| u <= now) => {
                rec.snoozed_until = None;
                rec.stage_before = None;
                rec.set(Triage, now);
            }
            Ignored if auto_unignore && rec.ignored_at_sha.as_deref().is_some_and(|s| s != head) => {
                rec.ignored_at_sha = None;
                rec.stage_before = None;
                rec.set(Triage, now);
            }
            Posted => {
                let new_commits = rec.reviewed_sha.as_deref().is_some_and(|s| s != head);
                let re_requested = requested && !rec.reviewed_pending;
                if new_commits || re_requested {
                    rec.set(ReReviewNeeded, now);
                }
            }
            _ => {}
        },
        Event::AgenticDone | Event::AgenticSkip => {
            if rec.stage == AwaitingAgentic {
                if matches!(ev, Event::AgenticDone) {
                    rec.agentic_done_at = Some(now);
                }
                rec.set(Ready, now);
            }
        }
        Event::AgenticTimedOut => {
            if rec.stage == AwaitingAgentic {
                rec.stage_detail = Some("timeout".into());
            }
        }
        Event::AgenticRewait => {
            if rec.stage == AwaitingAgentic {
                rec.stage_detail = None;
                rec.stage_updated_at = now;
            }
        }
        Event::RunStarted => {
            if rec.stage == Ready {
                rec.set(ClaudeReviewing, now);
            }
        }
        Event::ReviewFinished => {
            if rec.stage == ClaudeReviewing {
                rec.set(Polishing, now);
            }
        }
        Event::PolishFinished => {
            if rec.stage == Polishing {
                rec.set(DraftReady, now);
            }
        }
        Event::RunFailed { phase, message } => {
            if rec.stage == phase.stage() {
                rec.set(Error(phase), now);
                rec.stage_detail = Some(message);
            }
        }
        Event::Cancel => {
            if rec.stage.is_active() {
                rec.set(Ready, now);
            }
        }
        Event::Retry => match rec.stage {
            // A failed review restarts from Ready so it takes a queue slot; a
            // failed polish resumes polishing the draft it already has.
            Error(p) | Interrupted(p) => rec.set(
                match p {
                    Phase::AwaitingAgentic => AwaitingAgentic,
                    Phase::ClaudeReviewing => Ready,
                    Phase::Polishing => Polishing,
                },
                now,
            ),
            _ => {}
        },
        Event::Discard { has_draft } => {
            if matches!(rec.stage, Interrupted(_) | Error(_)) {
                rec.set(if has_draft { DraftReady } else { Triage }, now);
            }
        }
        Event::Posted { head, pending } => {
            rec.reviewed_sha = Some(head);
            rec.reviewed_pending = pending;
            rec.set(Posted, now);
        }
        Event::ReviewedOutside { head } => {
            if matches!(rec.stage, New | Triage | ReReviewNeeded | DraftReady | Ready) {
                rec.reviewed_sha = Some(head);
                rec.reviewed_pending = false;
                rec.set(Posted, now);
            }
        }
        Event::Closed => {
            if !rec.closed {
                rec.closed = true;
                rec.stage_before = Some(match rec.stage {
                    // A live run is cancelled; come back to Ready.
                    s if s.is_active() => Ready,
                    s => s,
                });
                if rec.stage.is_active() {
                    rec.set(Ready, now);
                }
            }
        }
        Event::Reopened => {
            if rec.closed {
                rec.closed = false;
                if let Some(s) = rec.stage_before.take() {
                    rec.set(s, now);
                }
            }
        }
        Event::AppStarted => {
            if let Some(p) = match rec.stage {
                ClaudeReviewing => Some(Phase::ClaudeReviewing),
                Polishing => Some(Phase::Polishing),
                _ => None,
            } {
                rec.set(Interrupted(p), now);
            }
        }
    }
    *rec != before
}

// ---------------------------------------------------------------------------
// CI agentic reviewer (section 6)

/// The later of the head push time and the label time for this round.
pub fn agentic_cutoff(detail: &PrDetail, label_applied_at: Option<Time>) -> Option<Time> {
    let push = detail.head_first_check_suite_at.or(detail.head_committed_at);
    match (push, label_applied_at) {
        (Some(p), Some(l)) => Some(p.max(l)),
        (p, l) => p.or(l),
    }
}

pub fn agentic_done(detail: &PrDetail, cfg: &Config, cutoff: Option<Time>) -> bool {
    let after = |t: Time| cutoff.is_none_or(|c| t > c);
    let head = &detail.summary.head_sha;
    detail.comments.iter().any(|c| cfg.is_bot_login(&c.author) && after(c.created_at))
        || detail.reviews.iter().any(|r| {
            cfg.is_bot_login(&r.author)
                && (after(r.submitted_at.unwrap_or(r.created_at)) || r.commit_oid.as_deref() == Some(head))
        })
        || detail.threads.iter().flat_map(|t| &t.comments).any(|c| cfg.is_bot_login(&c.author) && after(c.created_at))
}

pub fn agentic_wait_elapsed(rec: &PrRecord, now: Time) -> Duration {
    now - rec.stage_updated_at
}

// ---------------------------------------------------------------------------
// Already said (section 6)

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    Bot,
    Me,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaidItem {
    pub source: Source,
    pub author: String,
    pub kind: &'static str,
    pub path: Option<String>,
    pub line: Option<u64>,
    pub outdated: bool,
    pub commit: Option<String>,
    pub body: String,
    pub created_at: Time,
    pub url: String,
}

/// Everything the CI agentic reviewer and I already said, newest first.
pub fn already_said(detail: &PrDetail, cfg: &Config) -> Vec<SaidItem> {
    let src = |author: &str| {
        if cfg.is_bot_login(author) {
            Some(Source::Bot)
        } else if same_login(author, &cfg.github.login) {
            Some(Source::Me)
        } else {
            None
        }
    };
    let mut out = vec![];
    for c in &detail.comments {
        if let Some(source) = src(&c.author) {
            out.push(SaidItem {
                source,
                author: c.author.clone(),
                kind: "comment",
                path: None,
                line: None,
                outdated: false,
                commit: None,
                body: c.body.clone(),
                created_at: c.created_at,
                url: c.url.clone(),
            });
        }
    }
    for r in &detail.reviews {
        if r.state == "PENDING" || r.body.trim().is_empty() {
            continue;
        }
        if let Some(source) = src(&r.author) {
            out.push(SaidItem {
                source,
                author: r.author.clone(),
                kind: "review",
                path: None,
                line: None,
                outdated: r.commit_oid.as_deref() != Some(&detail.summary.head_sha),
                commit: r.commit_oid.clone(),
                body: r.body.clone(),
                created_at: r.submitted_at.unwrap_or(r.created_at),
                url: r.url.clone(),
            });
        }
    }
    for t in &detail.threads {
        for c in &t.comments {
            if let Some(source) = src(&c.author) {
                out.push(SaidItem {
                    source,
                    author: c.author.clone(),
                    kind: "review comment",
                    path: Some(t.path.clone()),
                    line: t.line.or(t.original_line),
                    outdated: t.is_outdated,
                    commit: c.commit_oid.clone(),
                    body: c.body.clone(),
                    created_at: c.created_at,
                    url: c.url.clone(),
                });
            }
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.created_at));
    out
}

// ---------------------------------------------------------------------------
// My review status (section 6)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reviewed {
    OnHead,
    OnOlder,
    No,
}

pub fn my_review(detail: Option<&PrDetail>, rec: &PrRecord, head: &str, login: &str) -> Reviewed {
    let mine: Vec<&Review> = detail
        .map(|d| d.reviews.iter().filter(|r| r.state != "PENDING" && same_login(&r.author, login)).collect())
        .unwrap_or_default();
    if mine.iter().any(|r| r.commit_oid.as_deref() == Some(head)) || rec.reviewed_sha.as_deref() == Some(head) {
        Reviewed::OnHead
    } else if !mine.is_empty() || rec.reviewed_sha.is_some() {
        Reviewed::OnOlder
    } else {
        Reviewed::No
    }
}

pub fn my_status_text(requested: bool, r: Reviewed) -> String {
    let rev = match r {
        Reviewed::OnHead => "reviewed on head",
        Reviewed::OnOlder => "reviewed on older commit",
        Reviewed::No => "not reviewed",
    };
    if requested { format!("★ requested · {rev}") } else { rev.to_string() }
}

/// A review by me on the head that the app didn't post.
pub fn reviewed_outside(detail: &PrDetail, login: &str, posted_ids: &HashSet<u64>) -> bool {
    detail.reviews.iter().any(|r| {
        r.state != "PENDING"
            && same_login(&r.author, login)
            && r.commit_oid.as_deref() == Some(&detail.summary.head_sha)
            && !posted_ids.contains(&r.id)
    })
}

/// My pending review on GitHub, which blocks posting (section 9).
pub fn my_pending_review<'a>(detail: &'a PrDetail, login: &str) -> Option<&'a Review> {
    detail.reviews.iter().find(|r| r.state == "PENDING" && same_login(&r.author, login))
}

// ---------------------------------------------------------------------------
// List sections (section 10.2)

/// What the list needs about one PR.
#[derive(Debug, Clone)]
pub struct Row<'a> {
    pub summary: &'a PrSummary,
    pub rec: &'a PrRecord,
    pub queued: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub key: String,
    pub title: String,
    pub ids: Vec<String>,
    pub bot_ids: Vec<String>,
    pub starred: usize,
    pub cross_repo: bool,
}

pub fn needs_you(r: &Row) -> bool {
    let s = r.rec.stage;
    (r.summary.requested && matches!(s, Stage::New | Stage::Triage))
        || matches!(s, Stage::DraftReady | Stage::ReReviewNeeded | Stage::Error(_) | Stage::Interrupted(_))
        || r.rec.agentic_timed_out()
}

pub fn in_progress(r: &Row) -> bool {
    !r.rec.agentic_timed_out() && (r.rec.stage.is_active() || (r.rec.stage == Stage::Ready && r.queued))
}

pub fn matches_filter(p: &PrSummary, filter: &str) -> bool {
    let f = filter.trim().to_lowercase();
    if f.is_empty() {
        return true;
    }
    let f = f.trim_start_matches('#');
    p.title.to_lowercase().contains(f)
        || p.repo.to_lowercase().contains(f)
        || p.author.to_lowercase().contains(f.trim_start_matches('@'))
        || p.number.to_string() == f
}

pub fn sections(rows: &[Row], cfg: &Config, show_hidden: bool, filter: &str) -> Vec<Section> {
    let mut visible: Vec<&Row> = rows
        .iter()
        .filter(|r| r.summary.state == PrState::Open || r.rec.stage.is_active())
        .filter(|r| show_hidden || !r.rec.stage.is_hidden())
        .filter(|r| {
            !(cfg.filters.hide_drafts && r.summary.is_draft && matches!(r.rec.stage, Stage::New | Stage::Triage))
        })
        .filter(|r| matches_filter(r.summary, filter))
        .collect();
    visible.sort_by(|a, b| {
        b.summary.requested.cmp(&a.summary.requested).then(b.summary.updated_at.cmp(&a.summary.updated_at))
    });

    let mut out = vec![];
    let need: Vec<&Row> = visible.iter().copied().filter(|r| needs_you(r)).collect();
    let prog: Vec<&Row> = visible.iter().copied().filter(|r| !needs_you(r) && in_progress(r)).collect();
    let rest: Vec<&Row> = visible.iter().copied().filter(|r| !needs_you(r) && !in_progress(r)).collect();
    let mk = |key: &str, title: &str, rows: &[&Row], cross: bool| Section {
        key: key.into(),
        title: title.into(),
        ids: rows.iter().map(|r| r.summary.id()).collect(),
        bot_ids: vec![],
        starred: rows.iter().filter(|r| r.summary.requested).count(),
        cross_repo: cross,
    };
    out.push(mk("needs", "Needs you", &need, true));
    out.push(mk("progress", "In progress", &prog, true));

    let mut repos: Vec<&str> = rest.iter().map(|r| r.summary.repo.as_str()).collect();
    repos.sort_unstable();
    repos.dedup();
    for repo in repos {
        let in_repo: Vec<&Row> = rest.iter().copied().filter(|r| r.summary.repo == repo).collect();
        let (bots, humans): (Vec<&Row>, Vec<&Row>) = in_repo.iter().partition(|r| cfg.is_bot_author(&r.summary.author));
        let mut s = mk(&format!("repo:{repo}"), repo, &humans, false);
        s.bot_ids = bots.iter().map(|r| r.summary.id()).collect();
        s.starred += bots.iter().filter(|r| r.summary.requested).count();
        out.push(s);
    }
    out
}

/// Text for the row's next-action chip.
pub fn next_action(rec: &PrRecord, queued_pos: Option<usize>, now: Time) -> String {
    let mins = |t: Time| {
        let m = (now - t).num_minutes().max(0);
        if m < 60 { format!("{m}m") } else { format!("{}h", m / 60) }
    };
    match rec.stage {
        Stage::New | Stage::Triage => "Triage".into(),
        Stage::AwaitingAgentic if rec.agentic_timed_out() => "Bot timed out".into(),
        Stage::AwaitingAgentic => format!("Waiting on bot {}", mins(rec.stage_updated_at)),
        Stage::Ready => match queued_pos {
            Some(n) => format!("Queued ({})", ordinal(n)),
            None => "Ready".into(),
        },
        Stage::ClaudeReviewing => format!("Reviewing {}", mins(rec.stage_updated_at)),
        Stage::Polishing => format!("Polishing {}", mins(rec.stage_updated_at)),
        s => s.label().into(),
    }
}

pub fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (1, x) if x != 11 => "st",
        (2, x) if x != 12 => "nd",
        (3, x) if x != 13 => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

pub fn age(t: Time, now: Time) -> String {
    let d = now - t;
    if d.num_minutes() < 1 {
        "now".into()
    } else if d.num_hours() < 1 {
        format!("{}m", d.num_minutes())
    } else if d.num_days() < 1 {
        format!("{}h", d.num_hours())
    } else if d.num_days() < 60 {
        format!("{}d", d.num_days())
    } else {
        format!("{}mo", d.num_days() / 30)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::model::IssueComment;
    use crate::github::query;
    use chrono::TimeZone;

    fn t(h: u32, m: u32) -> Time {
        chrono::Utc.with_ymd_and_hms(2026, 10, 30, h, m, 0).unwrap()
    }

    fn rec() -> PrRecord {
        PrRecord::new("o/a#1".into(), t(0, 0))
    }

    fn cfg() -> Config {
        Config::parse("[github]\nlogin=\"me\"\n", std::path::Path::new("c")).unwrap()
    }

    #[test]
    fn stage_db_roundtrip() {
        for s in [
            Stage::New,
            Stage::Triage,
            Stage::DraftReady,
            Stage::Error(Phase::Polishing),
            Stage::Interrupted(Phase::ClaudeReviewing),
            Stage::Error(Phase::AwaitingAgentic),
        ] {
            assert_eq!(Stage::from_db(&s.as_db()), s);
        }
    }

    #[test]
    fn happy_path() {
        let mut r = rec();
        assert!(apply(&mut r, Event::Selected, t(1, 0)));
        assert_eq!(r.stage, Stage::Triage);
        apply(&mut r, Event::StartReview { wait_for_agentic: true }, t(1, 1));
        assert_eq!(r.stage, Stage::AwaitingAgentic);
        apply(&mut r, Event::AgenticDone, t(1, 5));
        assert_eq!(r.stage, Stage::Ready);
        apply(&mut r, Event::RunStarted, t(1, 5));
        apply(&mut r, Event::ReviewFinished, t(1, 9));
        assert_eq!(r.stage, Stage::Polishing);
        apply(&mut r, Event::PolishFinished, t(1, 10));
        assert_eq!(r.stage, Stage::DraftReady);
        apply(&mut r, Event::Posted { head: "h1".into(), pending: false }, t(1, 20));
        assert_eq!(r.stage, Stage::Posted);
        apply(&mut r, Event::Refreshed { head: "h1".into(), requested: false, auto_unignore: true }, t(2, 0));
        assert_eq!(r.stage, Stage::Posted);
        apply(&mut r, Event::Refreshed { head: "h2".into(), requested: false, auto_unignore: true }, t(3, 0));
        assert_eq!(r.stage, Stage::ReReviewNeeded);
    }

    #[test]
    fn re_request_needs_rereview_unless_pending() {
        let mut r = rec();
        apply(&mut r, Event::Posted { head: "h1".into(), pending: true }, t(1, 0));
        apply(&mut r, Event::Refreshed { head: "h1".into(), requested: true, auto_unignore: true }, t(1, 1));
        assert_eq!(r.stage, Stage::Posted);
        apply(&mut r, Event::Posted { head: "h1".into(), pending: false }, t(1, 2));
        apply(&mut r, Event::Refreshed { head: "h1".into(), requested: true, auto_unignore: true }, t(1, 3));
        assert_eq!(r.stage, Stage::ReReviewNeeded);
    }

    #[test]
    fn skip_agentic_goes_straight_to_ready() {
        let mut r = rec();
        apply(&mut r, Event::StartReview { wait_for_agentic: false }, t(1, 0));
        assert_eq!(r.stage, Stage::Ready);
    }

    #[test]
    fn timeout_and_rewait() {
        let mut r = rec();
        apply(&mut r, Event::StartReview { wait_for_agentic: true }, t(1, 0));
        apply(&mut r, Event::AgenticTimedOut, t(1, 30));
        assert!(r.agentic_timed_out());
        apply(&mut r, Event::AgenticRewait, t(1, 31));
        assert!(!r.agentic_timed_out());
        assert_eq!(r.stage_updated_at, t(1, 31));
        apply(&mut r, Event::AgenticSkip, t(1, 32));
        assert_eq!(r.stage, Stage::Ready);
        assert!(r.agentic_done_at.is_none());
    }

    #[test]
    fn cancel_error_retry_interrupt() {
        let mut r = rec();
        apply(&mut r, Event::StartReview { wait_for_agentic: false }, t(1, 0));
        apply(&mut r, Event::RunStarted, t(1, 0));
        apply(&mut r, Event::Cancel, t(1, 1));
        assert_eq!(r.stage, Stage::Ready);
        apply(&mut r, Event::RunStarted, t(1, 2));
        apply(&mut r, Event::RunFailed { phase: Phase::ClaudeReviewing, message: "boom".into() }, t(1, 3));
        assert_eq!(r.stage, Stage::Error(Phase::ClaudeReviewing));
        assert_eq!(r.stage_detail.as_deref(), Some("boom"));
        apply(&mut r, Event::Retry, t(1, 4));
        assert_eq!(r.stage, Stage::Ready);
        apply(&mut r, Event::RunStarted, t(1, 4));
        apply(&mut r, Event::ReviewFinished, t(1, 5));
        apply(&mut r, Event::AppStarted, t(2, 0));
        assert_eq!(r.stage, Stage::Interrupted(Phase::Polishing));
        apply(&mut r, Event::Retry, t(2, 1));
        assert_eq!(r.stage, Stage::Polishing);
        apply(&mut r, Event::AppStarted, t(2, 2));
        apply(&mut r, Event::Discard { has_draft: true }, t(2, 3));
        assert_eq!(r.stage, Stage::DraftReady);
    }

    #[test]
    fn failure_in_wrong_stage_ignored() {
        let mut r = rec();
        apply(&mut r, Event::Selected, t(1, 0));
        assert!(!apply(&mut r, Event::RunFailed { phase: Phase::Polishing, message: "x".into() }, t(1, 1)));
        assert!(!apply(&mut r, Event::ReviewFinished, t(1, 1)));
    }

    #[test]
    fn ignore_snooze() {
        let mut r = rec();
        apply(&mut r, Event::Selected, t(1, 0));
        apply(&mut r, Event::Ignore { head: "h1".into() }, t(1, 0));
        assert_eq!(r.stage, Stage::Ignored);
        apply(&mut r, Event::Refreshed { head: "h1".into(), requested: false, auto_unignore: true }, t(1, 1));
        assert_eq!(r.stage, Stage::Ignored);
        apply(&mut r, Event::Refreshed { head: "h2".into(), requested: false, auto_unignore: false }, t(1, 1));
        assert_eq!(r.stage, Stage::Ignored);
        apply(&mut r, Event::Refreshed { head: "h2".into(), requested: false, auto_unignore: true }, t(1, 2));
        assert_eq!(r.stage, Stage::Triage);

        apply(&mut r, Event::Ignore { head: "h2".into() }, t(1, 3));
        apply(&mut r, Event::Unignore, t(1, 4));
        assert_eq!(r.stage, Stage::Triage);

        apply(&mut r, Event::Snooze { until: t(5, 0) }, t(1, 5));
        assert_eq!(r.stage, Stage::Snoozed);
        apply(&mut r, Event::Refreshed { head: "h2".into(), requested: false, auto_unignore: true }, t(4, 59));
        assert_eq!(r.stage, Stage::Snoozed);
        apply(&mut r, Event::Refreshed { head: "h2".into(), requested: false, auto_unignore: true }, t(5, 0));
        assert_eq!(r.stage, Stage::Triage);
    }

    #[test]
    fn close_and_reopen() {
        let mut r = rec();
        apply(&mut r, Event::StartReview { wait_for_agentic: false }, t(1, 0));
        apply(&mut r, Event::RunStarted, t(1, 0));
        apply(&mut r, Event::Closed, t(1, 1));
        assert!(r.closed);
        assert_eq!(r.stage, Stage::Ready);
        apply(&mut r, Event::Reopened, t(1, 2));
        assert!(!r.closed);
        assert_eq!(r.stage, Stage::Ready);

        let mut r = rec();
        apply(&mut r, Event::Posted { head: "h".into(), pending: false }, t(1, 0));
        apply(&mut r, Event::Closed, t(1, 1));
        apply(&mut r, Event::Reopened, t(1, 2));
        assert_eq!(r.stage, Stage::Posted);
    }

    #[test]
    fn active_stages_refuse_ignore() {
        let mut r = rec();
        apply(&mut r, Event::StartReview { wait_for_agentic: true }, t(1, 0));
        assert!(!apply(&mut r, Event::Ignore { head: "h".into() }, t(1, 1)));
        assert!(!apply(&mut r, Event::StartReview { wait_for_agentic: false }, t(1, 1)));
    }

    fn detail() -> PrDetail {
        query::parse_detail(&query::tests::detail_json(), false).unwrap()
    }

    #[test]
    fn cutoff_uses_latest_of_push_and_label() {
        let d = detail();
        // First check suite is 08:00, committedDate is the day before.
        assert_eq!(agentic_cutoff(&d, None), Some(t(8, 0)));
        assert_eq!(agentic_cutoff(&d, Some(t(9, 0))), Some(t(9, 0)));
        assert_eq!(agentic_cutoff(&d, Some(t(7, 0))), Some(t(8, 0)));
        let mut d2 = d.clone();
        d2.head_first_check_suite_at = None;
        assert_eq!(agentic_cutoff(&d2, None), d.head_committed_at);
    }

    #[test]
    fn done_detection() {
        let c = cfg();
        let mut d = detail();
        // Fixture bot comment at 10:00 on head1.
        assert!(agentic_done(&d, &c, Some(t(9, 0))));
        assert!(!agentic_done(&d, &c, Some(t(11, 0))));
        // A bot review on the head commit counts even if older than cutoff.
        d.reviews.push(Review {
            id: 1,
            author: "AI-Reviewer[bot]".into(),
            state: "COMMENTED".into(),
            body: "x".into(),
            commit_oid: Some("head1".into()),
            submitted_at: Some(t(1, 0)),
            created_at: t(1, 0),
            url: String::new(),
        });
        assert!(agentic_done(&d, &c, Some(t(11, 0))));
        // Bot issue comments count too.
        let mut d = detail();
        d.threads.clear();
        d.comments.push(IssueComment {
            author: "ai-reviewer".into(),
            body: "b".into(),
            created_at: t(12, 0),
            url: String::new(),
        });
        assert!(agentic_done(&d, &c, Some(t(11, 0))));
    }

    #[test]
    fn already_said_groups_and_orders() {
        let c = cfg();
        let mut d = detail();
        d.reviews.push(Review {
            id: 2,
            author: "me".into(),
            state: "COMMENTED".into(),
            body: "my earlier review".into(),
            commit_oid: Some("old".into()),
            submitted_at: Some(t(12, 0)),
            created_at: t(12, 0),
            url: String::new(),
        });
        let said = already_said(&d, &c);
        // bob's approval and carol's comment aren't "already said"; my PENDING review is skipped.
        assert_eq!(said.len(), 2);
        assert_eq!(said[0].source, Source::Me);
        assert!(said[0].outdated);
        assert_eq!(said[1].source, Source::Bot);
        assert_eq!(said[1].path.as_deref(), Some("pkg/retry.go"));
    }

    #[test]
    fn review_status() {
        let d = detail();
        let r = rec();
        assert_eq!(my_review(Some(&d), &r, "head1", "me"), Reviewed::No);
        assert!(my_pending_review(&d, "me").is_some());
        let mut r2 = r.clone();
        r2.reviewed_sha = Some("old".into());
        assert_eq!(my_review(Some(&d), &r2, "head1", "me"), Reviewed::OnOlder);
        assert_eq!(my_status_text(true, Reviewed::OnOlder), "★ requested · reviewed on older commit");
        let mut d2 = d.clone();
        d2.reviews.push(Review {
            id: 77,
            author: "me".into(),
            state: "COMMENTED".into(),
            body: String::new(),
            commit_oid: Some("head1".into()),
            submitted_at: None,
            created_at: t(1, 0),
            url: String::new(),
        });
        assert!(reviewed_outside(&d2, "me", &HashSet::new()));
        assert!(!reviewed_outside(&d2, "me", &HashSet::from([77])));
    }

    fn summary(repo: &str, n: u64, author: &str, requested: bool, updated_h: u32) -> PrSummary {
        let mut p = query::parse_pr(&query::tests::pr_json(repo, n, author, "h"), requested).unwrap();
        p.updated_at = t(updated_h, 0);
        p
    }

    #[test]
    fn sectioning() {
        let c = Config::parse(
            "[github]\nlogin=\"me\"\n[filters]\nbot_authors=[\"dependabot\"]\n",
            std::path::Path::new("c"),
        )
        .unwrap();
        let prs = [
            summary("o/a", 1, "alice", true, 1),
            summary("o/a", 2, "bob", false, 5),
            summary("o/a", 3, "dependabot[bot]", false, 2),
            summary("o/b", 4, "carol", false, 3),
            summary("o/b", 5, "dave", false, 4),
            summary("o/b", 6, "erin", false, 6),
        ];
        let mut recs: Vec<PrRecord> = prs.iter().map(|p| PrRecord::new(p.id(), t(0, 0))).collect();
        recs[3].stage = Stage::DraftReady;
        recs[4].stage = Stage::ClaudeReviewing;
        recs[5].stage = Stage::Ignored;
        let rows: Vec<Row> = prs.iter().zip(&recs).map(|(s, r)| Row { summary: s, rec: r, queued: false }).collect();
        let secs = sections(&rows, &c, false, "");
        assert_eq!(secs[0].ids, vec!["o/a#1", "o/b#4"]);
        assert_eq!(secs[1].ids, vec!["o/b#5"]);
        assert_eq!(secs[2].key, "repo:o/a");
        assert_eq!(secs[2].ids, vec!["o/a#2"]);
        assert_eq!(secs[2].bot_ids, vec!["o/a#3"]);
        assert_eq!(secs.len(), 3, "o/b has nothing left once #6 is hidden");
        let secs = sections(&rows, &c, true, "");
        assert_eq!(secs[3].ids, vec!["o/b#6"]);
        let secs = sections(&rows, &c, false, "#4");
        assert_eq!(secs[0].ids, vec!["o/b#4"]);
        assert_eq!(secs.len(), 2);
    }

    #[test]
    fn chips_and_ages() {
        let mut r = rec();
        assert_eq!(next_action(&r, None, t(1, 0)), "Triage");
        r.stage = Stage::AwaitingAgentic;
        r.stage_updated_at = t(1, 0);
        assert_eq!(next_action(&r, None, t(1, 12)), "Waiting on bot 12m");
        r.stage = Stage::Ready;
        assert_eq!(next_action(&r, Some(2), t(1, 12)), "Queued (2nd)");
        assert_eq!(ordinal(11), "11th");
        assert_eq!(ordinal(21), "21st");
        assert_eq!(age(t(1, 0), t(3, 0)), "2h");
    }
}
