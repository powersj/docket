//! Orchestration: refresh/polling, the review pipeline, agentic waits and
//! posting. UI state lives in Dioxus signals (`Ctx`); long-lived services
//! (GitHub client, store, repos, agent queue) live in `Svc`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use dioxus::prelude::*;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::agent::{self, output::StreamEvent, runner::RunError, schema::ReviewOutput};
use crate::config::{Config, ConfigState, paths};
use crate::github::{
    self, Compare, GitHub, HttpGitHub,
    model::{PrDetail, PrState, PrSummary, Time},
    mutate::{ReviewCommentPayload, ReviewPayload, WriteError},
};
use crate::preflight::{self, Report};
use crate::repo::{DiffIndex, Repos};
use crate::state::{self, Event, Phase, PrRecord, Stage};
use crate::store::{Draft, PostedRow, Store};

// ---------------------------------------------------------------------------
// UI-facing state

#[derive(Debug, Clone, PartialEq)]
pub struct PrEntry {
    pub summary: PrSummary,
    pub rec: PrRecord,
    pub detail: Option<PrDetail>,
    pub detail_checked_at: Option<Time>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LogLine {
    pub at: Time,
    pub kind: &'static str,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunView {
    pub active: bool,
    pub phase: Phase,
    pub started: Time,
    pub line: String,
    pub log: Vec<LogLine>,
    pub cost: Option<f64>,
    pub turns: Option<u32>,
    pub log_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ToastAction {
    OpenUrl(String),
    UndoIgnore(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub text: String,
    pub error: bool,
    pub action: Option<(String, ToastAction)>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RefreshState {
    pub running: bool,
    pub last_ok: Option<Time>,
    pub last_error: Option<(Time, String)>,
    pub loaded_once: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConfirmState {
    pub pr_id: String,
    pub draft_id: i64,
    pub verdict: String,
    pub included: usize,
    pub excluded: usize,
    pub excluded_reason: Option<String>,
    pub summary_paras: usize,
    pub head_moved: Option<Compare>,
    pub pending_review: Option<String>,
    pub own_pr: bool,
    pub posting: bool,
    pub error: Option<String>,
    pub unprocessable: bool,
    pub checking: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Sheet {
    ReplaceDraft(String),
    Shortcuts,
    Check(&'static str),
    Snooze(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct UiPrefs {
    pub compact: bool,
    pub show_hidden: bool,
    pub split_preview: bool,
    pub collapsed: HashSet<String>,
    pub expanded_bots: HashSet<String>,
    pub zoom: f64,
    pub sidebar_width: f64,
    /// Tab picked by hand, and the stage it was picked in; a stage change
    /// switches back to the stage's default tab.
    pub tab: HashMap<String, (Stage, usize)>,
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            compact: false,
            show_hidden: false,
            split_preview: false,
            collapsed: HashSet::new(),
            expanded_bots: HashSet::new(),
            zoom: 1.0,
            sidebar_width: 340.0,
            tab: HashMap::new(),
        }
    }
}

#[derive(Clone, Copy)]
pub struct Ctx {
    pub cfg: Signal<ConfigState>,
    pub prs: Signal<BTreeMap<String, PrEntry>>,
    pub selected: Signal<Option<String>>,
    pub preflight: Signal<Report>,
    pub runs: Signal<HashMap<String, RunView>>,
    pub queue: Signal<Vec<String>>,
    pub toasts: Signal<Vec<Toast>>,
    pub refresh: Signal<RefreshState>,
    pub drafts: Signal<HashMap<String, Draft>>,
    pub diffs: Signal<HashMap<String, Arc<DiffIndex>>>,
    pub head_moved: Signal<HashMap<String, Compare>>,
    pub ui: Signal<UiPrefs>,
    pub confirm: Signal<Option<ConfirmState>>,
    pub sheet: Signal<Option<Sheet>>,
    pub filter: Signal<String>,
    pub posted_drafts: Signal<HashMap<i64, PostedRow>>,
    pub popup: Signal<Option<Popup>>,
    /// Focused comment card id (0 = the summary).
    pub focus_card: Signal<Option<u64>>,
    /// Editors showing Preview instead of Write (0 = the summary).
    pub preview_on: Signal<HashSet<u64>>,
    pub list_focused: Signal<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PopupKind {
    Row,
    More,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Popup {
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub kind: PopupKind,
}

impl Ctx {
    pub fn new() -> Ctx {
        Ctx {
            cfg: Signal::new(ConfigState::default()),
            prs: Signal::new(BTreeMap::new()),
            selected: Signal::new(None),
            preflight: Signal::new(Report::default()),
            runs: Signal::new(HashMap::new()),
            queue: Signal::new(vec![]),
            toasts: Signal::new(vec![]),
            refresh: Signal::new(RefreshState::default()),
            drafts: Signal::new(HashMap::new()),
            diffs: Signal::new(HashMap::new()),
            head_moved: Signal::new(HashMap::new()),
            ui: Signal::new(UiPrefs::default()),
            confirm: Signal::new(None),
            sheet: Signal::new(None),
            filter: Signal::new(String::new()),
            posted_drafts: Signal::new(HashMap::new()),
            popup: Signal::new(None),
            focus_card: Signal::new(None),
            preview_on: Signal::new(HashSet::new()),
            list_focused: Signal::new(false),
        }
    }

    pub fn config(&self) -> Option<Config> {
        self.cfg.read().current.clone()
    }

    pub fn entry(&self, id: &str) -> Option<PrEntry> {
        self.prs.read().get(id).cloned()
    }

    pub fn toast(mut self, text: impl Into<String>, error: bool, action: Option<(String, ToastAction)>) {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.toasts.write().push(Toast { id, text: text.into(), error, action });
        let mut toasts = self.toasts;
        spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(if error { 12 } else { 6 })).await;
            toasts.write().retain(|t| t.id != id);
        });
    }
}

// ---------------------------------------------------------------------------
// Services

pub struct Svc {
    pub store: Store,
    gh: RwLock<Option<(String, Arc<dyn GitHub>)>>,
    repos: RwLock<Option<(std::path::PathBuf, Arc<Repos>)>>,
    sem: Mutex<(usize, Arc<Semaphore>)>,
    cancels: Mutex<HashMap<String, CancellationToken>>,
}

static SVC: OnceLock<Svc> = OnceLock::new();

pub fn init_svc(store: Store) {
    SVC.get_or_init(|| Svc {
        store,
        gh: RwLock::new(None),
        repos: RwLock::new(None),
        sem: Mutex::new((0, Arc::new(Semaphore::new(1)))),
        cancels: Mutex::new(HashMap::new()),
    });
}

pub fn svc() -> &'static Svc {
    SVC.get().expect("services initialised")
}

impl Svc {
    pub fn gh(&self, cfg: &Config) -> Arc<dyn GitHub> {
        if let Some((login, gh)) = self.gh.read().unwrap().as_ref()
            && *login == cfg.github.login
        {
            return gh.clone();
        }
        let gh: Arc<dyn GitHub> = Arc::new(HttpGitHub::new(&cfg.github.login));
        *self.gh.write().unwrap() = Some((cfg.github.login.clone(), gh.clone()));
        gh
    }

    pub fn repos(&self, cfg: &Config) -> Arc<Repos> {
        let root = cfg.worktree_root();
        if let Some((r, repos)) = self.repos.read().unwrap().as_ref()
            && *r == root
        {
            return repos.clone();
        }
        let repos = Arc::new(Repos::new(root.clone()));
        *self.repos.write().unwrap() = Some((root, repos.clone()));
        repos
    }

    fn semaphore(&self, n: usize) -> Arc<Semaphore> {
        let mut s = self.sem.lock().unwrap();
        if s.0 != n {
            *s = (n, Arc::new(Semaphore::new(n)));
        }
        s.1.clone()
    }

    fn slash_cache(&self) -> Option<Vec<String>> {
        self.store.ui_get("claude_slash_commands").and_then(|s| serde_json::from_str(&s).ok())
    }
}

// ---------------------------------------------------------------------------
// Record changes

/// Apply an event to a PR, persist, and update the signal.
pub fn dispatch(mut ctx: Ctx, id: &str, ev: Event) -> bool {
    let now = chrono::Utc::now();
    let mut prs = ctx.prs.write();
    let Some(e) = prs.get_mut(id) else { return false };
    let changed = state::apply(&mut e.rec, ev, now);
    if changed && let Err(err) = svc().store.save_pr(&e.rec, Some(&e.summary)) {
        tracing::error!("saving {id}: {err}");
    }
    changed
}

fn update_rec(mut ctx: Ctx, id: &str, f: impl FnOnce(&mut PrRecord)) {
    let mut prs = ctx.prs.write();
    if let Some(e) = prs.get_mut(id) {
        f(&mut e.rec);
        if let Err(err) = svc().store.save_pr(&e.rec, Some(&e.summary)) {
            tracing::error!("saving {id}: {err}");
        }
    }
}

// ---------------------------------------------------------------------------
// Startup and refresh

pub fn reload_config(mut ctx: Ctx) {
    let mut st = ctx.cfg.read().clone();
    st.reload(&paths::config_file());
    if *ctx.cfg.read() != st {
        ctx.cfg.set(st);
    }
}

pub fn load_ui_prefs(mut ctx: Ctx) {
    let s = &svc().store;
    let mut p = UiPrefs::default();
    let get = |k: &str| s.ui_get(k);
    p.compact = get("compact").as_deref() == Some("1");
    p.split_preview = get("split_preview").as_deref() == Some("1");
    p.zoom = get("zoom").and_then(|z| z.parse().ok()).unwrap_or(1.0);
    p.sidebar_width = get("sidebar_width").and_then(|z| z.parse().ok()).unwrap_or(340.0);
    p.collapsed = get("collapsed").and_then(|c| serde_json::from_str(&c).ok()).unwrap_or_default();
    ctx.ui.set(p);
}

pub fn save_ui_prefs(ctx: Ctx) {
    let s = &svc().store;
    let p = ctx.ui.read();
    s.ui_set("compact", if p.compact { "1" } else { "0" });
    s.ui_set("split_preview", if p.split_preview { "1" } else { "0" });
    s.ui_set("zoom", &p.zoom.to_string());
    s.ui_set("sidebar_width", &p.sidebar_width.to_string());
    s.ui_set("collapsed", &serde_json::to_string(&p.collapsed).unwrap_or_default());
}

pub fn startup(mut ctx: Ctx) {
    let store = &svc().store;
    if let Err(e) = store.interrupt_stale_runs() {
        tracing::warn!("marking stale runs: {e}");
    }
    let now = chrono::Utc::now();
    let mut map = BTreeMap::new();
    for (mut rec, summary) in store.load_prs().unwrap_or_default() {
        let Some(summary) = summary else { continue };
        if state::apply(&mut rec, Event::AppStarted, now) {
            store.save_pr(&rec, Some(&summary)).ok();
        }
        // An agentic wait survives restarts; Ready means "was queued".
        map.insert(rec.id.clone(), PrEntry { summary, rec, detail: None, detail_checked_at: None });
    }
    ctx.prs.set(map);
    prune_logs(ctx);
    // Worktrees for closed or ignored PRs are removed on startup.
    if let Some(cfg) = ctx.config() {
        let keep: HashSet<String> = ctx
            .prs
            .read()
            .values()
            .filter(|e| !e.rec.closed && e.rec.stage != Stage::Ignored)
            .map(|e| e.rec.id.clone())
            .collect();
        let repos = svc().repos(&cfg);
        spawn(async move { repos.gc(&keep).await });
    }
}

fn prune_logs(ctx: Ctx) {
    let Some(cfg) = ctx.config() else { return };
    let Ok(entries) = std::fs::read_dir(paths::log_dir()) else { return };
    let cutoff = std::time::SystemTime::now() - cfg.log_retention;
    for e in entries.flatten() {
        if e.metadata().and_then(|m| m.modified()).is_ok_and(|m| m < cutoff) {
            std::fs::remove_file(e.path()).ok();
        }
    }
}

pub async fn run_preflight(mut ctx: Ctx, force: bool) -> Report {
    if !force && ctx.preflight.read().fresh() {
        return ctx.preflight.read().clone();
    }
    let Some(cfg) = ctx.config() else { return Report::default() };
    let gh = svc().gh(&cfg);
    let ((gh_check, sso), claude) = tokio::join!(preflight::check_gh(&cfg, gh.as_ref()), preflight::check_claude(&cfg));
    let skills = preflight::check_skills(&cfg, svc().slash_cache().as_deref());
    let r = Report { gh: gh_check, sso, claude, skills, checked_at: Some(chrono::Utc::now()) };
    ctx.preflight.set(r.clone());
    r
}

pub async fn refresh(mut ctx: Ctx, force_preflight: bool) {
    if ctx.refresh.read().running {
        return;
    }
    reload_config(ctx);
    let Some(cfg) = ctx.config() else { return };
    ctx.refresh.write().running = true;
    let report = run_preflight(ctx, force_preflight).await;
    let result = if report.gh.ok { refresh_list(ctx, &cfg).await } else { Err(anyhow::anyhow!("GitHub check failed")) };
    let now = chrono::Utc::now();
    {
        let mut r = ctx.refresh.write();
        r.running = false;
        r.loaded_once = true;
        match result {
            Ok(()) => {
                r.last_ok = Some(now);
                r.last_error = None;
            }
            Err(e) => r.last_error = Some((now, e.to_string())),
        }
    }
    refresh_details(ctx, &cfg).await;
}

async fn refresh_list(mut ctx: Ctx, cfg: &Config) -> anyhow::Result<()> {
    let gh = svc().gh(cfg);
    if let Some(rl) = gh.rate_limit()
        && rl.remaining < 50
        && rl.reset_at.is_some_and(|r| r > chrono::Utc::now())
    {
        anyhow::bail!("GitHub rate limit low ({} left); waiting until reset", rl.remaining);
    }
    let list = gh.list(&github::searches_for(cfg)).await?;
    // Watched sources are filtered by author in-app; requested PRs always show.
    let list: Vec<PrSummary> = list.into_iter().filter(|p| p.requested || cfg.watches(&p.repo, &p.author)).collect();
    let seen: HashSet<String> = list.iter().map(super::github::model::PrSummary::id).collect();
    let now = chrono::Utc::now();
    let store = &svc().store;

    let mut gone: Vec<String> = vec![];
    {
        let mut prs = ctx.prs.write();
        for p in list {
            let id = p.id();
            let e = prs.entry(id.clone()).or_insert_with(|| PrEntry {
                summary: p.clone(),
                rec: PrRecord::new(id.clone(), now),
                detail: None,
                detail_checked_at: None,
            });
            e.summary = p;
            if e.rec.closed {
                state::apply(&mut e.rec, Event::Reopened, now);
            }
            state::apply(
                &mut e.rec,
                Event::Refreshed {
                    head: e.summary.head_sha.clone(),
                    requested: e.summary.requested,
                    auto_unignore: cfg.auto_unignore_on_new_commits,
                },
                now,
            );
            store.save_pr(&e.rec, Some(&e.summary)).ok();
        }
        for (id, e) in prs.iter() {
            if !seen.contains(id) && !e.rec.closed {
                gone.push(id.clone());
            }
        }
    }
    // Confirm closes for PRs we were working on; others are just marked closed.
    for id in gone {
        let Some(e) = ctx.entry(&id) else { continue };
        let interesting = !matches!(e.rec.stage, Stage::New | Stage::Triage | Stage::Ignored | Stage::Snoozed)
            || ctx.selected.read().as_deref() == Some(&id);
        let mut state_now = PrState::Closed;
        if interesting && let Ok(d) = gh.detail(&e.summary.repo, e.summary.number, e.summary.requested).await {
            state_now = d.summary.state;
            if let Some(x) = ctx.prs.write().get_mut(&id) {
                x.summary = d.summary.clone();
                x.detail = Some(d);
            }
        }
        if state_now != PrState::Open {
            close_pr(ctx, &id, state_now);
        }
    }
    Ok(())
}

fn close_pr(mut ctx: Ctx, id: &str, st: PrState) {
    if let Some(t) = svc().cancels.lock().unwrap().remove(id) {
        t.cancel();
    }
    ctx.queue.write().retain(|q| q != id);
    if let Some(e) = ctx.prs.write().get_mut(id) {
        e.summary.state = st;
    }
    dispatch(ctx, id, Event::Closed);
    if let (Some(cfg), Some(e)) = (ctx.config(), ctx.entry(id)) {
        let repos = svc().repos(&cfg);
        spawn(async move {
            repos.remove_worktree(&e.summary.repo, e.summary.number).await.ok();
        });
    }
}

/// Detail query for the selected PR and PRs in active stages, when they changed.
async fn refresh_details(ctx: Ctx, _cfg: &Config) {
    let sel = ctx.selected.read().clone();
    let ids: Vec<String> = ctx
        .prs
        .read()
        .iter()
        .filter(|(id, e)| {
            !e.rec.closed
                && (Some(id.as_str()) == sel.as_deref()
                    || e.rec.stage.is_active()
                    || matches!(e.rec.stage, Stage::DraftReady | Stage::Posted | Stage::ReReviewNeeded))
        })
        .filter(|(_, e)| {
            e.detail.as_ref().is_none_or(|d| {
                d.summary.updated_at != e.summary.updated_at || d.summary.head_sha != e.summary.head_sha
            })
        })
        .map(|(id, _)| id.clone())
        .collect();
    for id in ids {
        fetch_detail(ctx, &id).await;
    }
}

pub async fn fetch_detail(mut ctx: Ctx, id: &str) -> Option<PrDetail> {
    let cfg = ctx.config()?;
    let e = ctx.entry(id)?;
    let gh = svc().gh(&cfg);
    let d = match gh.detail(&e.summary.repo, e.summary.number, e.summary.requested).await {
        Ok(d) => d,
        Err(err) => {
            tracing::warn!("detail {id}: {err:#}");
            return None;
        }
    };
    let now = chrono::Utc::now();
    {
        let mut prs = ctx.prs.write();
        let x = prs.get_mut(id)?;
        let mut s = d.summary.clone();
        s.requested = x.summary.requested;
        x.summary = s;
        x.detail = Some(d.clone());
        x.detail_checked_at = Some(now);
    }
    if d.summary.state != PrState::Open && !e.rec.closed {
        close_pr(ctx, id, d.summary.state);
    }
    // A review by me on head that the app didn't post.
    let posted = svc().store.posted_ids(id).unwrap_or_default();
    if state::reviewed_outside(&d, &cfg.github.login, &posted) && !e.rec.stage.is_active() {
        dispatch(ctx, id, Event::ReviewedOutside { head: d.summary.head_sha.clone() });
    }
    check_head_moved(ctx, id).await;
    Some(d)
}

/// If the head moved past the draft's commit, fetch what changed.
async fn check_head_moved(mut ctx: Ctx, id: &str) {
    let (Some(cfg), Some(e)) = (ctx.config(), ctx.entry(id)) else { return };
    let Some(draft) = ctx.drafts.read().get(id).cloned() else { return };
    if !matches!(e.rec.stage, Stage::ClaudeReviewing | Stage::Polishing | Stage::DraftReady)
        || draft.head_sha == e.summary.head_sha
    {
        ctx.head_moved.write().remove(id);
        return;
    }
    let gh = svc().gh(&cfg);
    if let Ok(cmp) = gh.compare(&e.summary.repo, &draft.head_sha, &e.summary.head_sha).await {
        let files: HashSet<&str> = cmp.files.iter().map(String::as_str).collect();
        let mut d = draft.clone();
        let mut changed = false;
        for c in &mut d.comments {
            let m = files.contains(c.anchor.path.as_str());
            if c.may_be_outdated != m {
                c.may_be_outdated = m;
                changed = true;
            }
        }
        if changed {
            svc().store.update_draft(&d).ok();
            ctx.drafts.write().insert(id.to_string(), d);
        }
        ctx.head_moved.write().insert(id.to_string(), cmp);
    }
}

// ---------------------------------------------------------------------------
// Selection, drafts and diffs

pub fn select(mut ctx: Ctx, id: &str) {
    if ctx.selected.read().as_deref() == Some(id) {
        return;
    }
    ctx.selected.set(Some(id.to_string()));
    dispatch(ctx, id, Event::Selected);
    load_draft(ctx, id);
    let id = id.to_string();
    spawn(async move {
        fetch_detail(ctx, &id).await;
    });
}

pub fn load_draft(mut ctx: Ctx, id: &str) {
    if ctx.drafts.read().contains_key(id) {
        return;
    }
    let Ok(Some(d)) = svc().store.current_draft(id) else { return };
    if let Ok(Some(p)) = svc().store.posted_for_draft(d.id) {
        ctx.posted_drafts.write().insert(d.id, p);
    }
    ctx.drafts.write().insert(id.to_string(), d.clone());
    // The hunk index for comment context comes from the same merge-base diff.
    if !ctx.diffs.read().contains_key(id) && !d.merge_base.is_empty() {
        let (Some(cfg), Some(e)) = (ctx.config(), ctx.entry(id)) else { return };
        let repos = svc().repos(&cfg);
        let id = id.to_string();
        spawn(async move {
            if let Ok(diff) = repos.diff(&e.summary.repo, &d.merge_base, &d.head_sha).await {
                ctx.diffs.write().insert(id, Arc::new(diff));
            }
        });
    }
}

pub fn save_draft(mut ctx: Ctx, d: Draft) {
    let mut d = d;
    d.updated_at = chrono::Utc::now();
    if let Err(e) = svc().store.update_draft(&d) {
        ctx.toast(format!("Couldn't save draft: {e}"), true, None);
    }
    ctx.drafts.write().insert(d.pr_id.clone(), d);
}

// ---------------------------------------------------------------------------
// Triage actions

pub fn ignore(ctx: Ctx, id: &str) {
    let Some(e) = ctx.entry(id) else { return };
    if dispatch(ctx, id, Event::Ignore { head: e.summary.head_sha.clone() }) {
        ctx.toast(
            format!("Ignored #{}", e.summary.number),
            false,
            Some(("Undo".into(), ToastAction::UndoIgnore(id.to_string()))),
        );
        if let Some(cfg) = ctx.config() {
            let repos = svc().repos(&cfg);
            spawn(async move {
                repos.remove_worktree(&e.summary.repo, e.summary.number).await.ok();
            });
        }
    }
}

pub fn unignore(ctx: Ctx, id: &str) {
    dispatch(ctx, id, Event::Unignore);
}

pub fn snooze(ctx: Ctx, id: &str, until: Time) {
    if dispatch(ctx, id, Event::Snooze { until }) {
        let local = until.with_timezone(&chrono::Local);
        ctx.toast(format!("Snoozed until {}", local.format("%a %-I:%M %p")), false, None);
    }
}

pub fn snooze_preset(preset: &str) -> Time {
    use chrono::{Datelike, Duration, Local, NaiveTime, TimeZone};
    let now = Local::now();
    let nine = NaiveTime::from_hms_opt(9, 0, 0).unwrap();
    let at = |days: i64| {
        let d = (now + Duration::days(days)).date_naive().and_time(nine);
        Local.from_local_datetime(&d).single().unwrap_or(now + Duration::days(days))
    };
    let t = match preset {
        "monday" => {
            let wd = i64::from(now.weekday().num_days_from_monday());
            at(if wd == 0 { 7 } else { 7 - wd })
        }
        "week" => at(7),
        _ => at(1),
    };
    t.with_timezone(&chrono::Utc)
}

pub fn open_url(url: &str) {
    if url.starts_with("https://") || url.starts_with("http://") {
        std::process::Command::new("open").arg(url).spawn().ok();
    }
}

pub fn reveal(path: &std::path::Path) {
    std::process::Command::new("open").arg("-R").arg(path).spawn().ok();
}

// ---------------------------------------------------------------------------
// Review pipeline

/// Start review / Re-review / Re-run. The caller has already confirmed
/// replacing an edited draft.
pub async fn start_review(ctx: Ctx, id: String) {
    let Some(cfg) = ctx.config() else { return };
    let report = run_preflight(ctx, false).await;
    if !report.can_run_agent() {
        ctx.toast("Can't start: fix the Claude check in the footer first.", true, None);
        return;
    }
    let Some(e) = ctx.entry(&id) else { return };
    if e.rec.stage.is_active() || e.rec.closed {
        return;
    }
    let repo = e.summary.repo.clone();
    let needs_agentic = cfg.agentic_enabled_for(&repo);
    if !needs_agentic {
        update_rec(ctx, &id, |r| r.agentic_label_applied_at = None);
        dispatch(ctx, &id, Event::StartReview { wait_for_agentic: false });
        enqueue(ctx, id, false);
        return;
    }
    let Some(d) = fetch_detail(ctx, &id).await else {
        ctx.toast("Couldn't load the PR from GitHub.", true, None);
        return;
    };
    // Label time only counts if it was applied for this head.
    let label_at =
        e.rec.agentic_label_applied_at.filter(|_| e.rec.agentic_done_sha.as_deref() != Some(&d.summary.head_sha));
    let cutoff = state::agentic_cutoff(&d, label_at);
    if state::agentic_done(&d, &cfg, cutoff) {
        mark_agentic_done(ctx, &id, &d.summary.head_sha);
        dispatch(ctx, &id, Event::StartReview { wait_for_agentic: false });
        enqueue(ctx, id, false);
        return;
    }
    dispatch(ctx, &id, Event::StartReview { wait_for_agentic: true });
    if !d.summary.labels.iter().any(|l| l.eq_ignore_ascii_case(&cfg.agentic.label)) {
        if !report.can_write() {
            update_rec(ctx, &id, |r| {
                r.stage_detail = Some("unavailable:GitHub check failed; can't add the label".into());
            });
            return;
        }
        apply_label(ctx, &id, &cfg, &d.summary.head_sha, false).await;
    } else {
        update_rec(ctx, &id, |r| r.agentic_label_applied_at = None);
    }
}

async fn apply_label(ctx: Ctx, id: &str, cfg: &Config, head: &str, readd: bool) {
    let Some(e) = ctx.entry(id) else { return };
    let gh = svc().gh(cfg);
    if readd {
        gh.remove_label(&e.summary.repo, e.summary.number, &cfg.agentic.label).await.ok();
    }
    match gh.add_label(&e.summary.repo, e.summary.number, &cfg.agentic.label).await {
        Ok(()) => {
            let now = chrono::Utc::now();
            svc().store.insert_label_event(id, &cfg.agentic.label, head, &now, true).ok();
            update_rec(ctx, id, |r| r.agentic_label_applied_at = Some(now));
        }
        // 403/404: the label isn't available in this repo; not an error.
        Err(WriteError::Forbidden { status, message }) => {
            update_rec(ctx, id, |r| r.stage_detail = Some(format!("unavailable:{status} {message}")));
        }
        Err(err) => {
            dispatch(
                ctx,
                id,
                Event::RunFailed { phase: Phase::AwaitingAgentic, message: format!("adding label: {err}") },
            );
        }
    }
}

fn mark_agentic_done(ctx: Ctx, id: &str, head: &str) {
    let h = head.to_string();
    update_rec(ctx, id, move |r| {
        r.agentic_done_sha = Some(h);
        r.agentic_done_at = Some(chrono::Utc::now());
    });
}

pub fn continue_without(ctx: Ctx, id: &str) {
    if dispatch(ctx, id, Event::AgenticSkip) {
        enqueue(ctx, id.to_string(), false);
    }
}

pub fn keep_waiting(ctx: Ctx, id: &str) {
    dispatch(ctx, id, Event::AgenticRewait);
}

pub async fn rerun_agentic(ctx: Ctx, id: String) {
    let Some(cfg) = ctx.config() else { return };
    if !run_preflight(ctx, true).await.can_write() {
        ctx.toast("Can't change labels: fix the GitHub check first.", true, None);
        return;
    }
    let head = ctx.entry(&id).map(|e| e.summary.head_sha).unwrap_or_default();
    dispatch(ctx, &id, Event::AgenticRewait);
    apply_label(ctx, &id, &cfg, &head, true).await;
}

/// Poll PRs waiting on the agentic reviewer.
pub async fn agentic_tick(ctx: Ctx) {
    let Some(cfg) = ctx.config() else { return };
    let now = chrono::Utc::now();
    let waiting: Vec<PrEntry> = ctx
        .prs
        .read()
        .values()
        .filter(|e| e.rec.stage == Stage::AwaitingAgentic && e.rec.stage_detail.is_none() && !e.rec.closed)
        .filter(|e| {
            e.detail_checked_at.is_none_or(|t| (now - t).to_std().unwrap_or_default() >= cfg.agentic.wait_poll_interval)
        })
        .cloned()
        .collect();
    for e in waiting {
        check_agentic(ctx, &e.rec.id, &cfg).await;
    }
}

pub async fn check_agentic(ctx: Ctx, id: &str, cfg: &Config) {
    let Some(d) = fetch_detail(ctx, id).await else { return };
    let Some(e) = ctx.entry(id) else { return };
    if e.rec.stage != Stage::AwaitingAgentic {
        return;
    }
    // The cutoff follows the current head, so a push while waiting moves it.
    let cutoff = state::agentic_cutoff(&d, e.rec.agentic_label_applied_at);
    if state::agentic_done(&d, cfg, cutoff) {
        mark_agentic_done(ctx, id, &d.summary.head_sha);
        dispatch(ctx, id, Event::AgenticDone);
        enqueue(ctx, id.to_string(), false);
    } else if state::agentic_wait_elapsed(&e.rec, chrono::Utc::now()).to_std().unwrap_or_default()
        >= cfg.agentic.wait_timeout
    {
        dispatch(ctx, id, Event::AgenticTimedOut);
    }
}

fn enqueue(mut ctx: Ctx, id: String, polish_only: bool) {
    let Some(cfg) = ctx.config() else { return };
    if ctx.queue.read().contains(&id) {
        return;
    }
    ctx.queue.write().push(id.clone());
    let sem = svc().semaphore(cfg.max_concurrent_reviews);
    let token = CancellationToken::new();
    svc().cancels.lock().unwrap().insert(id.clone(), token.clone());
    spawn(async move {
        let permit = tokio::select! {
            p = sem.acquire_owned() => p.ok(),
            () = token.cancelled() => None,
        };
        ctx.queue.write().retain(|q| q != &id);
        let Some(_permit) = permit else { return };
        // Config is captured once per run.
        let Some(cfg) = ctx.config() else { return };
        let r = if polish_only {
            polish_existing(ctx, &id, &cfg, token).await
        } else {
            review_and_polish(ctx, &id, &cfg, token).await
        };
        svc().cancels.lock().unwrap().remove(&id);
        if let Some(rv) = ctx.runs.write().get_mut(&id) {
            rv.active = false;
        }
        if let Err(err) = r {
            let phase = match ctx.entry(&id).map(|e| e.rec.stage) {
                Some(Stage::Polishing) => Phase::Polishing,
                _ => Phase::ClaudeReviewing,
            };
            match err {
                RunError::Cancelled => {
                    dispatch(ctx, &id, Event::Cancel);
                }
                other => {
                    dispatch(ctx, &id, Event::RunFailed { phase, message: other.to_string() });
                    ctx.toast(format!("Run failed for {id}: {other}"), true, None);
                }
            }
        }
    });
}

pub fn queue_position(ctx: Ctx, id: &str) -> Option<usize> {
    ctx.queue.read().iter().position(|q| q == id).map(|i| i + 1)
}

pub fn cancel(ctx: Ctx, id: &str) {
    if let Some(t) = svc().cancels.lock().unwrap().remove(id) {
        t.cancel();
    }
    // Waiting on the bot has no process to kill.
    if ctx.entry(id).is_some_and(|e| e.rec.stage == Stage::AwaitingAgentic)
        || ctx.queue.read().contains(&id.to_string())
    {
        dispatch(ctx, id, Event::Cancel);
    }
}

pub fn retry(ctx: Ctx, id: &str) {
    let Some(e) = ctx.entry(id) else { return };
    let was = e.rec.stage;
    if !dispatch(ctx, id, Event::Retry) {
        return;
    }
    match was {
        Stage::Error(Phase::ClaudeReviewing) | Stage::Interrupted(Phase::ClaudeReviewing) => {
            enqueue(ctx, id.to_string(), false);
        }
        Stage::Error(Phase::Polishing) | Stage::Interrupted(Phase::Polishing) => enqueue(ctx, id.to_string(), true),
        _ => {}
    }
}

pub fn discard(ctx: Ctx, id: &str) {
    let has_draft = ctx.drafts.read().contains_key(id) || svc().store.current_draft(id).ok().flatten().is_some();
    dispatch(ctx, id, Event::Discard { has_draft });
}

fn start_run_view(mut ctx: Ctx, id: &str, phase: Phase, line: &str) {
    ctx.runs.write().insert(
        id.to_string(),
        RunView {
            active: true,
            phase,
            started: chrono::Utc::now(),
            line: line.into(),
            log: vec![],
            cost: None,
            turns: None,
            log_path: None,
        },
    );
}

fn run_line(mut ctx: Ctx, id: &str, kind: &'static str, text: String, set_line: bool) {
    if let Some(rv) = ctx.runs.write().get_mut(id) {
        if set_line {
            rv.line.clone_from(&text);
        }
        rv.log.push(LogLine { at: chrono::Utc::now(), kind, text });
        if rv.log.len() > 500 {
            rv.log.remove(0);
        }
    }
}

/// Run one agent process, streaming its events into the run view.
async fn run_agent(
    ctx: Ctx,
    id: &str,
    spec: agent::runner::RunSpec,
    token: CancellationToken,
) -> Result<agent::output::RunResult, RunError> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = tokio::spawn(agent::runner::run(spec, tx, token));
    while let Some(ev) = rx.recv().await {
        match ev {
            StreamEvent::Init { slash_commands, model } => {
                svc()
                    .store
                    .ui_set("claude_slash_commands", &serde_json::to_string(&slash_commands).unwrap_or_default());
                run_line(ctx, id, "init", format!("Started ({model})"), false);
            }
            StreamEvent::Text(t) => {
                let first = t.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().to_string();
                run_line(ctx, id, "text", t, false);
                if !first.is_empty() {
                    let mut ctx = ctx;
                    if let Some(rv) = ctx.runs.write().get_mut(id) {
                        rv.line = first.chars().take(120).collect();
                    }
                }
            }
            StreamEvent::Tool { summary, .. } => run_line(ctx, id, "tool", format!("{summary}…"), true),
            StreamEvent::Result(r) => {
                let mut ctx = ctx;
                if let Some(rv) = ctx.runs.write().get_mut(id) {
                    rv.cost = Some(rv.cost.unwrap_or(0.0) + r.cost_usd);
                    rv.turns = Some(r.turns);
                }
            }
            StreamEvent::Other => {}
        }
    }
    handle.await.map_err(|e| RunError::Failed(format!("agent task: {e}")))?
}

async fn review_and_polish(mut ctx: Ctx, id: &str, cfg: &Config, token: CancellationToken) -> Result<(), RunError> {
    let fail = |m: String| RunError::Failed(m);
    dispatch(ctx, id, Event::RunStarted);
    let Some(e) = ctx.entry(id) else { return Ok(()) };
    if e.rec.stage != Stage::ClaudeReviewing {
        return Ok(());
    }
    start_run_view(ctx, id, Phase::ClaudeReviewing, "Preparing checkout…");
    let detail = match e.detail.clone() {
        Some(d) if d.summary.head_sha == e.summary.head_sha => d,
        _ => fetch_detail(ctx, id).await.ok_or_else(|| fail("couldn't load PR details".into()))?,
    };
    let pr = detail.summary.clone();
    let token_gh = github::gh_token(&cfg.github.login).await.ok();
    let repos = svc().repos(cfg);
    let co = tokio::select! {
        r = repos.prepare(&pr.repo, pr.number, &pr.head_sha, &pr.base_sha, token_gh.as_deref()) => r.map_err(|e| fail(format!("checkout: {e:#}")))?,
        () = token.cancelled() => return Err(RunError::Cancelled),
    };
    ctx.diffs.write().insert(id.to_string(), Arc::new(co.diff.clone()));
    let said = state::already_said(&detail, cfg);

    let store = &svc().store;
    let run_id = store.insert_run(id, "review", &co.head, Some(&co.merge_base)).map_err(|e| fail(e.to_string()))?;
    let spec = agent::review_spec(&cfg.claude, &pr, &co, &said, run_id);
    store.set_run_log(run_id, &spec.log_path.display().to_string(), Some(&co.merge_base)).ok();
    if let Some(rv) = ctx.runs.write().get_mut(id) {
        rv.log_path = Some(spec.log_path.display().to_string());
        rv.line = "Starting Claude…".into();
    }
    let res = run_agent(ctx, id, spec, token.clone()).await;
    agent::cleanup_sandbox(run_id);
    let res = match res {
        Ok(r) => {
            store.finish_run(run_id, "ok", Some(r.cost_usd), Some(r.turns), None).ok();
            r
        }
        Err(err) => {
            store
                .finish_run(
                    run_id,
                    if matches!(err, RunError::Cancelled) { "cancelled" } else { "error" },
                    None,
                    None,
                    Some(&err.to_string()),
                )
                .ok();
            return Err(err);
        }
    };
    let out = agent::output::extract_review(&res).map_err(|e| fail(e.to_string()))?;
    let comments = agent::output::build_draft_comments(&out, &co.diff, &said);
    let draft = Draft {
        id: 0,
        pr_id: id.to_string(),
        run_id: Some(run_id),
        head_sha: co.head.clone(),
        merge_base: co.merge_base.clone(),
        verdict_suggested: out.verdict.clone(),
        summary_raw: out.summary.clone(),
        summary_polished: None,
        summary_edited: None,
        summary_warning: None,
        comments,
        edited: false,
        superseded: false,
        updated_at: chrono::Utc::now(),
    };
    let draft_id = store.insert_draft(&draft).map_err(|e| fail(e.to_string()))?;
    ctx.drafts.write().insert(id.to_string(), Draft { id: draft_id, ..draft });
    ctx.head_moved.write().remove(id);
    dispatch(ctx, id, Event::ReviewFinished);
    polish_existing(ctx, id, cfg, token).await
}

async fn polish_existing(mut ctx: Ctx, id: &str, cfg: &Config, token: CancellationToken) -> Result<(), RunError> {
    let fail = |m: String| RunError::Failed(m);
    let draft = match ctx.drafts.read().get(id).cloned() {
        Some(d) => d,
        None => svc().store.current_draft(id).ok().flatten().ok_or_else(|| fail("no draft to polish".into()))?,
    };
    let keep_cost = ctx.runs.read().get(id).and_then(|r| r.cost);
    start_run_view(ctx, id, Phase::Polishing, "Polishing wording…");
    if let Some(rv) = ctx.runs.write().get_mut(id) {
        rv.cost = keep_cost;
    }
    // Polish what the agent wrote; edits made meanwhile are kept as edits.
    let orig = ReviewOutput {
        verdict: draft.verdict_suggested.clone(),
        summary: draft.summary_raw.clone(),
        comments: draft
            .comments
            .iter()
            .map(|c| agent::schema::AgentComment { body: c.body_raw.clone(), ..c.anchor.clone() })
            .collect(),
    };
    let store = &svc().store;
    let run_id = store.insert_run(id, "polish", &draft.head_sha, None).map_err(|e| fail(e.to_string()))?;
    let spec = agent::polish_spec(&cfg.claude, &orig, run_id);
    store.set_run_log(run_id, &spec.log_path.display().to_string(), None).ok();
    let res = run_agent(ctx, id, spec, token).await;
    agent::cleanup_sandbox(run_id);
    let res = match res {
        Ok(r) => {
            store.finish_run(run_id, "ok", Some(r.cost_usd), Some(r.turns), None).ok();
            r
        }
        Err(err) => {
            store
                .finish_run(
                    run_id,
                    if matches!(err, RunError::Cancelled) { "cancelled" } else { "error" },
                    None,
                    None,
                    Some(&err.to_string()),
                )
                .ok();
            return Err(err);
        }
    };
    let polished = agent::output::extract_review(&res).map_err(|e| fail(format!("polish: {e}")))?;
    let check = agent::output::verify_polish(&orig, &polished);
    let mut d = ctx.drafts.read().get(id).cloned().unwrap_or(draft);
    match check.summary {
        Ok(s) => {
            d.summary_polished = Some(s);
            d.summary_warning = None;
        }
        Err(w) => d.summary_warning = Some(w),
    }
    for (c, r) in d.comments.iter_mut().zip(check.comments) {
        match r {
            Ok(b) => {
                c.body_polished = Some(b);
                c.polish_warning = None;
            }
            Err(w) => c.polish_warning = Some(w),
        }
    }
    store.update_draft(&d).map_err(|e| fail(e.to_string()))?;
    ctx.drafts.write().insert(id.to_string(), d);
    dispatch(ctx, id, Event::PolishFinished);
    Ok(())
}

// ---------------------------------------------------------------------------
// Posting (section 9)

pub async fn open_confirm(mut ctx: Ctx, id: String) {
    let Some(cfg) = ctx.config() else { return };
    let Some(draft) = ctx.drafts.read().get(&id).cloned() else { return };
    let Some(e) = ctx.entry(&id) else { return };
    let (included, excluded, reason) = counts(&draft);
    ctx.confirm.set(Some(ConfirmState {
        pr_id: id.clone(),
        draft_id: draft.id,
        verdict: "COMMENT".into(),
        included,
        excluded,
        excluded_reason: reason,
        summary_paras: paragraphs(draft.summary()),
        head_moved: None,
        pending_review: None,
        own_pr: crate::config::same_login(&e.summary.author, &cfg.github.login),
        posting: false,
        error: None,
        unprocessable: false,
        checking: true,
    }));
    // Pre-checks: preflight (never cached before writes), head, pending review.
    let report = run_preflight(ctx, true).await;
    let detail = fetch_detail(ctx, &id).await;
    let moved = ctx.head_moved.read().get(&id).cloned();
    let mut c = ctx.confirm.write();
    let Some(c) = c.as_mut().filter(|c| c.pr_id == id) else { return };
    c.checking = false;
    if !report.can_write() {
        c.error = Some(
            report
                .gh
                .problem
                .clone()
                .or(report.sso.as_ref().map(|o| format!("Token not SSO-authorized for {o}")))
                .unwrap_or_else(|| "GitHub check failed".into()),
        );
    }
    match detail {
        Some(d) => {
            c.pending_review = state::my_pending_review(&d, &cfg.github.login).map(|r| r.url.clone());
            if d.summary.head_sha != draft.head_sha {
                c.head_moved = Some(moved.unwrap_or_default());
            }
        }
        None => c.error = Some("Couldn't reach GitHub to check the PR.".into()),
    }
}

fn paragraphs(s: &str) -> usize {
    s.split("\n\n").filter(|p| !p.trim().is_empty()).count()
}

fn counts(d: &Draft) -> (usize, usize, Option<String>) {
    let included = d.comments.iter().filter(|c| c.include && c.invalid.is_none()).count();
    let excluded = d.comments.len() - included;
    let reason = d.comments.iter().find(|c| c.invalid.is_some()).map(|_| "⚠ invalid line".to_string());
    (included, excluded, reason)
}

pub fn payload(d: &Draft, verdict: Option<&str>, only_valid: bool, diff: Option<&DiffIndex>) -> ReviewPayload {
    let comments = d
        .comments
        .iter()
        .filter(|c| c.include && c.invalid.is_none())
        .filter(|c| {
            !only_valid
                || (!c.may_be_outdated && diff.is_none_or(|di| agent::output::validate(&c.anchor, di).is_none()))
        })
        .map(|c| {
            let a = &c.anchor;
            let file = a.subject_type == "file";
            ReviewCommentPayload {
                path: a.path.clone(),
                body: c.body().to_string(),
                line: if file { None } else { a.line },
                side: if file { None } else { a.side.clone() },
                start_line: if file { None } else { a.start_line },
                start_side: if file { None } else { a.start_side.clone() },
                subject_type: if file { "file".into() } else { "line".into() },
            }
        })
        .collect();
    ReviewPayload {
        commit_id: d.head_sha.clone(),
        body: d.summary().to_string(),
        event: verdict.map(String::from),
        comments,
    }
}

pub async fn post(mut ctx: Ctx, pending: bool, only_valid: bool) {
    let Some(c) = ctx.confirm.read().clone() else { return };
    let Some(cfg) = ctx.config() else { return };
    let Some(e) = ctx.entry(&c.pr_id) else { return };
    let Some(draft) = ctx.drafts.read().get(&c.pr_id).cloned() else { return };
    if c.pending_review.is_some() || c.checking {
        return;
    }
    let own = crate::config::same_login(&e.summary.author, &cfg.github.login);
    let verdict = if pending {
        None
    } else if own && c.verdict != "COMMENT" {
        Some("COMMENT".to_string())
    } else {
        Some(c.verdict.clone())
    };
    if let Some(x) = ctx.confirm.write().as_mut() {
        x.posting = true;
        x.error = None;
    }
    let diff = ctx.diffs.read().get(&c.pr_id).cloned();
    let body = payload(&draft, verdict.as_deref(), only_valid, diff.as_deref());
    let gh = svc().gh(&cfg);
    let res = github::post_review_once(gh.as_ref(), &e.summary.repo, e.summary.number, &cfg.github.login, &body).await;
    match res {
        Ok(r) => {
            let row = PostedRow {
                review_id: r.id,
                pr_id: c.pr_id.clone(),
                draft_id: Some(draft.id),
                head_sha: draft.head_sha.clone(),
                verdict: verdict.clone(),
                pending,
                url: r.html_url.clone(),
                posted_at: chrono::Utc::now(),
                body: body.body.clone(),
            };
            svc().store.insert_posted(&row).ok();
            ctx.posted_drafts.write().insert(draft.id, row);
            dispatch(ctx, &c.pr_id, Event::Posted { head: draft.head_sha.clone(), pending });
            ctx.confirm.set(None);
            let what = if pending { "Pending review saved" } else { "Review posted" };
            ctx.toast(
                format!("{what} on #{}", e.summary.number),
                false,
                Some((
                    "Open on GitHub".into(),
                    ToastAction::OpenUrl(if r.html_url.is_empty() { e.summary.url.clone() } else { r.html_url }),
                )),
            );
            let id = c.pr_id.clone();
            spawn(async move {
                fetch_detail(ctx, &id).await;
            });
        }
        Err(err) => {
            if let Some(x) = ctx.confirm.write().as_mut() {
                x.posting = false;
                x.unprocessable = matches!(err, WriteError::Unprocessable(_));
                x.error = Some(err.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::schema::{AgentComment, DraftComment};

    fn dc(id: u64, include: bool, invalid: bool, file: bool) -> DraftComment {
        DraftComment {
            id,
            anchor: AgentComment {
                path: "a.go".into(),
                subject_type: if file { "file".into() } else { "line".into() },
                line: Some(3),
                side: Some("RIGHT".into()),
                start_line: None,
                start_side: None,
                body: "raw".into(),
            },
            body_raw: "raw".into(),
            body_polished: Some("polished".into()),
            body_edited: None,
            include,
            invalid: invalid.then(|| "bad".into()),
            dup: None,
            polish_warning: None,
            may_be_outdated: false,
        }
    }

    fn draft() -> Draft {
        Draft {
            id: 1,
            pr_id: "o/a#1".into(),
            run_id: None,
            head_sha: "h".into(),
            merge_base: "m".into(),
            verdict_suggested: "APPROVE".into(),
            summary_raw: "s".into(),
            summary_polished: Some("S".into()),
            summary_edited: None,
            summary_warning: None,
            comments: vec![
                dc(1, true, false, false),
                dc(2, false, false, false),
                dc(3, true, true, false),
                dc(4, true, false, true),
            ],
            edited: false,
            superseded: false,
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn payload_includes_only_valid_included_comments() {
        let d = draft();
        let p = payload(&d, Some("COMMENT"), false, None);
        assert_eq!(p.commit_id, "h");
        assert_eq!(p.body, "S");
        assert_eq!(p.comments.len(), 2);
        assert_eq!(p.comments[0].body, "polished");
        assert_eq!(p.comments[1].subject_type, "file");
        assert!(p.comments[1].line.is_none());
        assert!(payload(&d, None, false, None).event.is_none());
        assert_eq!(counts(&d), (2, 2, Some("⚠ invalid line".into())));
    }
}
