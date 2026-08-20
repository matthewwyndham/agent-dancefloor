# dancefloor

A terminal dashboard for your live Claude Code sessions. It is `lazydocker` for agents.

`dancefloor` finds every Claude Code session running on this machine. It shows where each
session works, what it runs, how full its context window is, and which subagents it spawned.

```
 dancefloor  5 sessions · 1 busy · sort status · every 2s
┌ Sessions ───────────────────────────────┐┌───────────────────────────────────────────────────────────────────┐
│● api-server         api-ser… █████░  75%││ 1 Detail  │  2 Agents  │  3 Prompt  │  4 Usage                    │
│○ checkout-flow      web-shop ███░░░  44%││checkout-flow  ○ idle for 2m18s                                    │
│○ flaky-e2e          web-sho… ████░░  53%││                                                                   │
│○ docs-sweep         docs-si… ░░░░░░   0%││title       Split the checkout reducer                             │
│○ perf-audit         web-shop ██░░░░  25%││cwd         /Users/you/code/web-shop                               │
│                                         ││branch      refactor/checkout-reducer                              │
│                                         ││pr          #128 acme/web-shop                                     │
│                                         ││pr url      https://github.com/acme/web-shop/pull/128              │
│                                         ││model       claude-opus-5                                          │
│                                         ││mode        normal · perms auto                                    │
│                                         ││                                                                   │
│                                         ││uptime      1d1h                                                   │
│                                         ││last write  2m18s ago                                              │
│                                         ││ context  445k / 1.0M~                                             │
│                                         ││██████████████████████████████  44%                                │
└─────────────────────────────────────────┘└───────────────────────────────────────────────────────────────────┘
 j/k move tab pane 1-4 jump s sort r refresh ? help q quit
```

## Install

```sh
cargo install --path .
```

The binary lands in `~/.cargo/bin/dancefloor`.

## Use

```sh
dancefloor                        # the dashboard
dancefloor --once                 # one plain-text table, then exit
dancefloor --interval 5           # refresh every 5 seconds instead of 2
dancefloor --context-limit 1000000  # pin the context window
```

### Keys

| Key         | Action                              |
| ----------- | ----------------------------------- |
| `j` `k`     | Move between sessions               |
| `tab`       | Next pane, `shift-tab` for previous |
| `1` `2` `3` `4` | Jump to Detail, Agents, Prompt, Usage |
| `s`         | Cycle the sort order                |
| `r`         | Refresh now                         |
| `?`         | Help                                |
| `q`         | Quit                                |

The sort order cycles through status, context, uptime, and directory. Status sorts busy
sessions first.

## The panes

**Detail** shows the session name, status, and how long it held that status. It also shows the
directory, the git branch, the worktree, the pull request, the model, the permission mode, the
uptime, and the process cost.

**Agents** lists the subagents the session spawned. Each entry names the agent type, the prompt
or skill it runs, and how long its transcript has been idle.

**Prompt** shows the last prompt the user submitted, with the session title above it.

**Usage** breaks the newest request into input, cache read, cache write, and output tokens. It
also totals recent activity over the part of the transcript that was read.

## Where the data comes from

`dancefloor` reads files that Claude Code already writes. It never talks to the API, and it
never writes to your Claude Code state.

| Source | What it gives |
| ------ | ------------- |
| `~/.claude/sessions/<pid>.json` | The live session registry: pid, session id, directory, name, and busy or idle status |
| `~/.claude/projects/<dir>/<session>.jsonl` | Token usage, model, title, branch, permission mode, worktree, pull request, and last prompt |
| `~/.claude/projects/<dir>/<session>/subagents/` | One `meta.json` per spawned subagent |
| `ps` | CPU and resident memory per session process |

A registry file outlives the process that wrote it. Every entry is confirmed against `ps`
before `dancefloor` reports it, so a crashed session disappears on the next refresh.

Two sessions often run in the same directory, so the process id identifies a session and the
directory does not.

## Known limits

**The context limit is inferred.** Claude Code records the base model id in the transcript. It
records `claude-opus-5` even when the session runs the `[1m]` long-context variant, so the real
window size is not on disk. `dancefloor` assumes 200k, and 1M once it sees usage above 200k. A
`~` after the limit means the number was inferred. Use `--context-limit` to pin it.

**Rate limits are not shown.** Claude Code sends the 5-hour and 7-day figures to the status line
hook on stdin. It does not write them to disk, so no external tool can read them.

**Recent activity covers the transcript tail.** A transcript grows without bound, so
`dancefloor` reads only the last megabyte. The Usage pane says so. The last prompt is the one
exception: it is searched for further back, because a long turn pushes it out of that window.

## Develop

```sh
cargo test          # unit tests plus the render suite
cargo run -- --once
```

The render suite drives the real draw path over a test backend at six terminal sizes, including
1x1. `ratatui` panics when a fixed-size region cannot fit, so a new panel with an unguarded
length constraint fails those tests instead of breaking the app on a small terminal.
