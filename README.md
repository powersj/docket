# Reviewer

A macOS app for keeping track of the GitHub pull requests you care about and
writing Claude-assisted reviews that you edit and post yourself.

Reviewer shows what needs you right now, waits for your CI agentic reviewer to
finish, runs Claude Code on the PR in a read-only sandbox, polishes the
wording, and gives you a draft to edit. Nothing is posted until you confirm it.

## Requirements

- macOS
- [Rust](https://rustup.rs) (to build)
- [GitHub CLI](https://cli.github.com) (`gh`), logged in as the account you
  review with
- [Claude Code](https://claude.com/claude-code) 2.1.308 or later, logged in
- A review skill and a writing skill for Claude Code (by default `pr-reviewer`
  and `writing`), installed under `~/.claude/skills/<name>/SKILL.md` or as a
  plugin

## Build and run

```sh
make build
./target/release/reviewer
```

| Command      | What it does                     |
|--------------|----------------------------------|
| `make build` | Builds `target/release/reviewer` |
| `make test`  | Runs the unit tests              |
| `make clean` | Removes build output             |

## Set up

On first launch, Reviewer shows a welcome screen with an example config. Click
**Create example and reveal in Finder**, edit the file, then click **reload**.

The config lives at `~/Library/Application Support/reviewer/config.toml`. The
settings you'll usually change:

```toml
[github]
login = "your-login"              # must match the account gh is logged in as

[agentic]
enabled_repos = ["your-org/*"]    # repos where the CI agentic reviewer runs

[claude]
review_skill = "pr-reviewer"
writing_skill = "writing"

[[watch]]
repo = "your-org/your-repo"
authors = []                      # empty means all authors
```

Changes to the config are picked up on the next refresh (⌘R). If the file has
an error, Reviewer keeps using the last good config and shows a banner with
the line number.

The footer shows whether `gh` and `claude` are ready. Click either item to see
what's wrong and how to fix it.

## Using Reviewer

The sidebar groups pull requests into:

- **Needs you**: new review requests (★), drafts ready to post, PRs that need a
  re-review, and anything that failed.
- **In progress**: PRs waiting on the CI agentic reviewer or being reviewed by
  Claude.
- **One group per repo** for everything else. Bot PRs collapse into their own
  subgroup.

A review goes through these steps:

1. **Start review** (`r`). If the repo uses the CI agentic reviewer, Reviewer
   adds the `agentic-review` label and waits for the bot to comment. You can
   **Continue without it** at any time.
2. Claude reviews the PR, then a second pass polishes the wording. The status
   card shows progress; **Cancel** (⌘.) stops it.
3. Edit the draft on the **Review** tab. Each inline comment shows its diff
   context. Click a line to move a comment, or uncheck a comment to leave it
   out. Comments that repeat something already said are flagged.
4. **Post** (⌘↩) opens a confirmation sheet. The verdict always starts as
   **Comment**. **Post as pending** lets you finish the review on GitHub.

Right-click a PR to ignore or snooze it, open it on GitHub, or add its repo or
author to your config.

### Keyboard shortcuts

Press `?` in the app for the full list.

| Key         | Action                                |
|-------------|---------------------------------------|
| `j` / `k`   | Next / previous PR                    |
| `r`         | Start review / re-run                 |
| `i`         | Ignore (⌘Z to undo)                   |
| `s`         | Snooze                                |
| `o`         | Open in GitHub                        |
| `[` / `]`   | Previous / next comment               |
| `space`     | Include or exclude the focused comment |
| `e`         | Edit the focused comment or summary   |
| ⌘1–⌘3       | Overview / Prior review / Review tabs |
| ⌘↩          | Post                                  |
| ⌘R          | Refresh                               |
| ⌘⇧P         | Switch between Write and Preview      |
| ⌘⇧S         | Insert a suggestion block             |

## Safety

Claude never runs code from a pull request. Each review runs:

- in an empty sandbox directory, with the PR checkout added read-only;
- with only your user Claude settings, so a PR's `.claude/` settings, hooks,
  skills and `.mcp.json` are ignored;
- with only read tools (`Read`, `Grep`, `Glob`, and `git diff/log/show/blame`);
- without GitHub or SSH credentials in its environment;
- with turn, budget and time limits from the config.

PR text is marked as untrusted data in the prompt. `claude.extra_args` is
passed last and can weaken these protections.

## Files

| What               | Where                                        |
|--------------------|----------------------------------------------|
| Config             | `~/Library/Application Support/reviewer/config.toml` |
| Database           | `~/Library/Application Support/reviewer/reviewer.db` |
| PR checkouts       | `~/Library/Caches/reviewer/worktrees` (configurable) |
| App and run logs   | `~/Library/Logs/reviewer/`                   |

## Development

The design is in [DESIGN.md](DESIGN.md).

`make test` runs the unit tests. Three more tests are opt-in:

```sh
cargo test -- --ignored sandbox   # calls the real Claude CLI; costs a few cents
SNAPSHOT_DIR=/tmp/snap cargo test -- --ignored ui_snapshot   # renders the UI to HTML
```
