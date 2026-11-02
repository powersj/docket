//! Renders the main UI with sample data to static HTML, for eyeballing the
//! design without driving the window:
//! `SNAPSHOT_DIR=/tmp/x cargo test -- --ignored ui_snapshot`
//! then `qlmanage -t -s 1600 -o /tmp/x /tmp/x/*.html`.

use std::collections::BTreeMap;
use std::sync::Arc;

use dioxus::prelude::*;

use super::detail::Detail;
use super::pr_list::{ListClock, Sidebar};
use super::status_card::SecondClock;
use crate::agent::output::build_draft_comments;
use crate::agent::schema::{AgentComment, ReviewOutput};
use crate::config::{Config, ConfigState};
use crate::github::model::*;
use crate::github::query;
use crate::repo::DiffIndex;
use crate::service::{self as svc, Ctx, PrEntry, RunView};
use crate::state::{Phase, PrRecord, Stage};
use crate::store::Draft;

const CONFIG: &str = r#"
[github]
login = "me"
[agentic]
enabled_repos = ["chainguard-dev/*"]
[[watch]]
repo = "chainguard-dev/foo"
[[watch]]
repo = "chainguard-dev/bar"
"#;

const DIFF: &str = "diff --git a/pkg/retry.go b/pkg/retry.go
--- a/pkg/retry.go
+++ b/pkg/retry.go
@@ -36,9 +36,11 @@ func Retry(ctx context.Context, fn func() error) error {
 	delay := base
 	for attempt := 0; ; attempt++ {
 		err := fn()
-		delay = delay * 2
+		delay *= 2
+		time.Sleep(delay)
 		if err == nil {
 			return nil
 		}
+		log.Printf(\"retry %d: %v\", attempt, err)
 	}
 }
";

fn t(h: i64) -> Time {
    chrono::Utc::now() - chrono::Duration::hours(h)
}

fn pr(repo: &str, n: u64, title: &str, author: &str, requested: bool, ci: CiState, updated_h: i64) -> PrSummary {
    let mut p = query::parse_pr(&query::tests::pr_json(repo, n, author, &format!("head{n}")), requested).unwrap();
    p.title = title.into();
    p.updated_at = t(updated_h);
    p.ci =
        CiCounts { state: ci, passed: 12, failed: if ci == CiState::Failure { 2 } else { 0 }, pending: 0, total: 14 };
    p.body = "Backoff never reset after a successful attempt, so long-lived workers slept for minutes.\n\nThis resets `delay` on success and adds a log line.".into();
    p
}

fn seed(mut ctx: Ctx, selected: &str) {
    let cfg = Config::parse(CONFIG, std::path::Path::new("config.toml")).unwrap();
    ctx.cfg.set(ConfigState { current: Some(cfg.clone()), error: None, missing: false });
    let rows = [
        (
            pr("chainguard-dev/foo", 1234, "Fix CVE remediation retry loop", "alice", true, CiState::Success, 30),
            Stage::DraftReady,
        ),
        (pr("chainguard-dev/bar", 88, "Bump go to 1.25", "bob", false, CiState::Failure, 5), Stage::ReReviewNeeded),
        (
            pr("chainguard-dev/foo", 1240, "Add SBOM check to release workflow", "carol", false, CiState::Pending, 20),
            Stage::ClaudeReviewing,
        ),
        (
            pr("chainguard-dev/foo", 1238, "Retry flaky registry pulls", "erin", false, CiState::Success, 3),
            Stage::AwaitingAgentic,
        ),
        (
            pr(
                "chainguard-dev/foo",
                1241,
                "Refactor config loader into its own package",
                "dave",
                false,
                CiState::Success,
                2,
            ),
            Stage::Triage,
        ),
        (
            pr(
                "chainguard-dev/foo",
                1250,
                "build(deps): bump golang.org/x/net",
                "dependabot[bot]",
                false,
                CiState::Success,
                1,
            ),
            Stage::New,
        ),
        (
            pr(
                "chainguard-dev/foo",
                1251,
                "build(deps): bump k8s.io/client-go",
                "dependabot[bot]",
                false,
                CiState::Success,
                1,
            ),
            Stage::New,
        ),
        (pr("chainguard-dev/bar", 90, "Docs: clarify retry semantics", "frank", false, CiState::None, 50), Stage::New),
        (
            pr("chainguard-dev/bar", 91, "Add --dry-run to the sync command", "gina", true, CiState::Success, 70),
            Stage::Triage,
        ),
    ];
    let mut map = BTreeMap::new();
    for (s, stage) in rows {
        let mut rec = PrRecord::new(s.id(), t(0) - chrono::Duration::minutes(12));
        rec.stage = stage;
        let mut detail = query::parse_detail(&query::tests::detail_json(), s.requested).unwrap();
        detail.summary = s.clone();
        map.insert(s.id(), PrEntry { summary: s, rec, detail: Some(detail), detail_checked_at: Some(t(0)) });
    }
    ctx.prs.set(map);
    ctx.selected.set(Some(selected.into()));
    ctx.list_focused.set(true);
    ctx.refresh.set(svc::RefreshState { running: false, last_ok: Some(t(0)), last_error: None, loaded_once: true });
    ctx.preflight.set(crate::preflight::Report {
        gh: crate::preflight::Check { ok: true, summary: "gh: me".into(), problem: None, fix: None },
        sso: None,
        claude: crate::preflight::Check { ok: true, summary: "claude: ok".into(), problem: None, fix: None },
        skills: crate::preflight::Check { ok: true, summary: "skills: ok".into(), problem: None, fix: None },
        checked_at: Some(t(0)),
    });

    let diff = DiffIndex::parse(DIFF);
    let c = |line: u64, start: Option<u64>, body: &str| AgentComment {
        path: "pkg/retry.go".into(),
        subject_type: "line".into(),
        line: Some(line),
        side: Some("RIGHT".into()),
        start_line: start,
        start_side: start.map(|_| "RIGHT".into()),
        body: body.into(),
    };
    let out = ReviewOutput {
        verdict: "REQUEST_CHANGES".into(),
        summary: "Nice fix — resetting the backoff is the right call. Two notes:\n\n1. The sleep now happens **before** the success check, so a successful call still waits once.\n2. The new log line runs on every retry; consider `slog` at debug level.".into(),
        comments: vec![
            c(40, Some(39), "Backoff never resets after a success, and this sleeps even when `err == nil`.\n\n```suggestion\n\t\tif err == nil {\n\t\t\treturn nil\n\t\t}\n```"),
            c(44, None, "This logs on every attempt. At high retry counts that's noisy — debug level?"),
            c(90, None, "Unrelated: `a` shadows the outer variable."),
        ],
    };
    let said = crate::state::already_said(ctx.prs.read().get(selected).unwrap().detail.as_ref().unwrap(), &cfg);
    let mut comments = build_draft_comments(&out, &diff, &said);
    comments[0].body_polished = Some("Backoff never resets after a success, and this now sleeps even when `err == nil`.\n\n```suggestion\n\t\tif err == nil {\n\t\t\treturn nil\n\t\t}\n```".into());
    let draft = Draft {
        id: 1,
        pr_id: selected.into(),
        run_id: Some(1),
        head_sha: "head1234".into(),
        merge_base: "mb".into(),
        verdict_suggested: out.verdict.clone(),
        summary_raw: out.summary.clone(),
        summary_polished: None,
        summary_edited: None,
        summary_warning: None,
        comments,
        edited: false,
        superseded: false,
        updated_at: t(0),
    };
    ctx.drafts.write().insert(selected.into(), draft);
    ctx.diffs.write().insert(selected.into(), Arc::new(diff));
    ctx.focus_card.set(Some(1));
    ctx.runs.write().insert(
        "chainguard-dev/foo#1240".into(),
        RunView {
            active: true,
            phase: Phase::ClaudeReviewing,
            started: chrono::Utc::now() - chrono::Duration::seconds(134),
            line: "Reading pkg/sbom/check.go…".into(),
            log: vec![],
            cost: Some(0.31),
            turns: None,
            log_path: None,
        },
    );
}

#[derive(Props, Clone, PartialEq)]
struct RootProps {
    selected: String,
}

#[allow(non_snake_case)]
#[expect(clippy::needless_pass_by_value, reason = "Dioxus components take their props by value")]
fn Root(p: RootProps) -> Element {
    let ctx = use_hook(|| {
        let c = Ctx::new();
        seed(c, &p.selected);
        c
    });
    use_context_provider(|| ctx);
    let now = use_signal(chrono::Utc::now);
    use_context_provider(|| ListClock(now));
    use_context_provider(|| SecondClock(now));
    rsx! {
        div { class: "app",
            Sidebar {}
            Detail {}
            footer { class: "footer",
                span { class: "foot-item", span { class: "dot" } "gh: me" }
                span { class: "foot-item", span { class: "dot" } "claude: ok" }
                span { class: "faint", "·" }
                span { "refreshed 2m ago · 1 run active" }
            }
        }
    }
}

#[test]
#[ignore = "renders the UI to SNAPSHOT_DIR for a manual visual check"]
fn ui_snapshot() {
    let dir = std::env::var("SNAPSHOT_DIR").unwrap_or_else(|_| "target/snapshots".into());
    std::fs::create_dir_all(&dir).unwrap();
    svc::init_svc(crate::store::Store::memory().unwrap());
    for (name, sel) in [
        ("review", "chainguard-dev/foo#1234"),
        ("running", "chainguard-dev/foo#1240"),
        ("waiting", "chainguard-dev/foo#1238"),
    ] {
        let mut dom = VirtualDom::new_with_props(Root, RootProps { selected: sel.into() });
        dom.rebuild_in_place();
        let html = dioxus_ssr::render(&dom);
        let page = format!(
            "<!doctype html><html><head><meta charset=\"utf-8\">{}<style>html,body,#main{{height:900px;width:1440px;overflow:hidden}}</style></head><body><div id=\"main\">{html}</div></body></html>",
            super::styles::head()
        );
        std::fs::write(format!("{dir}/{name}.html"), page).unwrap();
    }
}
