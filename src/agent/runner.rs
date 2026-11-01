//! Spawning `claude`, streaming its events, and cancelling it.

use std::os::unix::process::CommandExt as _;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use super::output::{RunResult, StreamEvent, parse_stream_line};

#[derive(Debug, Clone)]
pub struct RunSpec {
    pub binary: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub prompt: String,
    pub timeout: Duration,
    pub log_path: PathBuf,
    /// Path prefix stripped from tool summaries (the worktree).
    pub strip: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("cancelled")]
    Cancelled,
    #[error("timed out after {0:?}")]
    Timeout(Duration),
    #[error("{0}")]
    Failed(String),
}

pub async fn run(
    spec: RunSpec,
    tx: UnboundedSender<StreamEvent>,
    cancel: CancellationToken,
) -> Result<RunResult, RunError> {
    let fail = |m: String| RunError::Failed(m);
    std::fs::create_dir_all(&spec.cwd).map_err(|e| fail(format!("creating sandbox dir: {e}")))?;
    if let Some(dir) = spec.log_path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let mut log = tokio::fs::File::create(&spec.log_path).await.map_err(|e| fail(format!("creating log: {e}")))?;

    let mut std_cmd = std::process::Command::new(&spec.binary);
    std_cmd.args(&spec.args).current_dir(&spec.cwd).process_group(0);
    for k in super::sandbox::scrub_env(std::env::vars()) {
        std_cmd.env_remove(k);
    }
    let mut cmd = tokio::process::Command::from(std_cmd);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| fail(format!("starting {}: {e}", spec.binary)))?;
    let pid = child.id().map(|p| p as i32);

    // The prompt goes on stdin, never argv.
    let mut stdin = child.stdin.take().unwrap();
    let prompt = spec.prompt.clone();
    tokio::spawn(async move {
        stdin.write_all(prompt.as_bytes()).await.ok();
        stdin.shutdown().await.ok();
    });

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            buf.push_str(&l);
            buf.push('\n');
        }
        buf
    });

    let mut lines = BufReader::new(stdout).lines();
    let mut result: Option<RunResult> = None;
    let deadline = tokio::time::sleep(spec.timeout);
    tokio::pin!(deadline);

    let outcome: Result<(), RunError> = loop {
        tokio::select! {
            _ = cancel.cancelled() => break Err(RunError::Cancelled),
            _ = &mut deadline => break Err(RunError::Timeout(spec.timeout)),
            line = lines.next_line() => match line {
                Ok(Some(l)) => {
                    log.write_all(l.as_bytes()).await.ok();
                    log.write_all(b"\n").await.ok();
                    for ev in parse_stream_line(&l, spec.strip.as_deref()) {
                        if let StreamEvent::Result(r) = &ev {
                            result = Some(r.clone());
                        }
                        tx.send(ev).ok();
                    }
                }
                Ok(None) => break Ok(()),
                Err(e) => break Err(fail(format!("reading agent output: {e}"))),
            },
        }
    };

    if outcome.is_err()
        && let Some(pid) = pid
    {
        terminate_group(pid, &mut child).await;
    }
    let status = child.wait().await.ok();
    let stderr_text = stderr_task.await.unwrap_or_default();
    if !stderr_text.is_empty() {
        log.write_all(b"--- stderr ---\n").await.ok();
        log.write_all(stderr_text.as_bytes()).await.ok();
    }
    log.flush().await.ok();
    outcome?;

    match result {
        Some(r) if r.is_error => Err(fail(format!(
            "agent ended with {}: {}",
            if r.subtype.is_empty() { "an error" } else { &r.subtype },
            first_line(&r.text).unwrap_or("no message")
        ))),
        Some(r) => Ok(r),
        None => Err(fail(format!(
            "agent exited ({}) without a result{}",
            status.map(|s| s.to_string()).unwrap_or_else(|| "unknown status".into()),
            first_line(&stderr_text).map(|l| format!(": {l}")).unwrap_or_default()
        ))),
    }
}

fn first_line(s: &str) -> Option<&str> {
    s.lines().map(str::trim).find(|l| !l.is_empty())
}

/// SIGTERM the process group, then SIGKILL after 5 s.
async fn terminate_group(pid: i32, child: &mut tokio::process::Child) {
    unsafe {
        libc::kill(-pid, libc::SIGTERM);
    }
    if tokio::time::timeout(Duration::from_secs(5), child.wait()).await.is_err() {
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_claude(dir: &std::path::Path, body: &str) -> String {
        let p = dir.join("claude");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::process::Command::new("chmod").arg("+x").arg(&p).status().unwrap();
        p.display().to_string()
    }

    fn spec(dir: &std::path::Path, binary: String, timeout: Duration) -> RunSpec {
        RunSpec {
            binary,
            args: vec!["-p".into()],
            cwd: dir.join("sandbox"),
            prompt: "hello prompt".into(),
            timeout,
            log_path: dir.join("logs/run.log"),
            strip: Some("/wt".into()),
        }
    }

    #[tokio::test]
    async fn streams_events_and_returns_result() {
        let dir = tempfile::tempdir().unwrap();
        // Echo stdin into a file so we can check the prompt went over stdin, and
        // record the env so we can check tokens were scrubbed.
        let bin = fake_claude(
            dir.path(),
            r#"cat > "$PWD/stdin.txt"
env > "$PWD/env.txt"
echo '{"type":"system","subtype":"init","model":"m","slash_commands":[]}'
echo '{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"/wt/a.go"}}]}}'
echo '{"type":"result","subtype":"success","is_error":false,"num_turns":2,"total_cost_usd":0.5,"result":"","structured_output":{"verdict":"COMMENT","summary":"ok","comments":[]}}'"#,
        );
        unsafe { std::env::set_var("SOME_SECRET_TOKEN", "x") };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let r = run(spec(dir.path(), bin, Duration::from_secs(10)), tx, CancellationToken::new()).await.unwrap();
        assert_eq!(r.cost_usd, 0.5);
        let mut evs = vec![];
        while let Ok(e) = rx.try_recv() {
            evs.push(e);
        }
        assert!(evs.iter().any(|e| matches!(e, StreamEvent::Tool { summary, .. } if summary == "Reading a.go")));
        let sb = dir.path().join("sandbox");
        assert_eq!(std::fs::read_to_string(sb.join("stdin.txt")).unwrap(), "hello prompt");
        assert!(!std::fs::read_to_string(sb.join("env.txt")).unwrap().contains("SOME_SECRET_TOKEN"));
        assert!(std::fs::read_to_string(dir.path().join("logs/run.log")).unwrap().contains("structured_output"));
    }

    #[tokio::test]
    async fn cancel_kills_the_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_claude(dir.path(), "sleep 30 & wait");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            c2.cancel();
        });
        let start = std::time::Instant::now();
        let r = run(spec(dir.path(), bin, Duration::from_secs(60)), tx, cancel).await;
        assert!(matches!(r, Err(RunError::Cancelled)));
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn timeout_and_missing_result() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_claude(dir.path(), "sleep 30");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let r = run(spec(dir.path(), bin, Duration::from_millis(200)), tx, CancellationToken::new()).await;
        assert!(matches!(r, Err(RunError::Timeout(_))));

        let bin = fake_claude(dir.path(), "echo oops >&2; exit 3");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let r = run(spec(dir.path(), bin, Duration::from_secs(5)), tx, CancellationToken::new()).await;
        let Err(RunError::Failed(m)) = r else { panic!() };
        assert!(m.contains("oops"), "{m}");
    }
}
