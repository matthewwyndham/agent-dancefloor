# dancefloor

A terminal dashboard for your live Claude Code sessions. It is `lazydocker` for agents.

`dancefloor` finds every Claude Code session running on this machine. It shows where each
session works, what it runs, how full its context window is, and which subagents it spawned.

```
 dancefloor  5 sessions · 1 busy · sort status · every 2s
┌ Sessions ───────────────────────────────┐┌───────────────────────────────────────────────────────────────────┐
│● api-server         api-ser… █████░  75%││ 1 Detail  │  2 Agents  │  3 Prompt  │  4 Usage  │  5 Activity     │
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
│                                         ││ context  445k / 1.0M                                              │
│                                         ││██████████████████████████████  44%                                │
└─────────────────────────────────────────┘└───────────────────────────────────────────────────────────────────┘
 j/k move tab pane 1-5 jump s sort r refresh ? help q quit
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
dancefloor --context-limit 1m     # pin the context window for every session
dancefloor --context-default 1m   # the window to assume when a session proves nothing
```

A token count is written `1m`, `200k` or `750000`. Both flags also take
`--flag=value`.

### Keys

| Key         | Action                              |
| ----------- | ----------------------------------- |
| `j` `k` `↑` `↓` | Move between sessions, or scroll the focused pane |
| `enter`     | Focus the pane, then open the tool call under the cursor |
| `esc`       | Back to the session list            |
| `y`         | Copy an open tool call              |
| `tab`       | Next pane, `shift-tab` for previous |
| `1` to `5`  | Jump to Detail, Agents, Prompt, Usage, Activity |
| `s`         | Cycle the sort order                |
| `r`         | Refresh now                         |
| `?`         | Help                                |
| `q`         | Quit                                |

`enter` moves the keys from the session list into the pane; the coloured border says which
half has them. `esc` gives them back.

The sort order cycles through status, context, uptime, and directory. Status sorts busy
sessions first.

## Config

`~/.config/dancefloor/config.json` sets the default context window, so you do not pass
`--context-default` on every run. `$XDG_CONFIG_HOME` is honoured when it is set.

```json
{
  "default_context_limit": "1m"
}
```

The value takes the same shorthand as the flag, or a plain number. `--context-default`
outranks the file. A missing or half-written file is not an error.

## The panes

**Detail** shows the session name, status, and how long it held that status. It also shows the
directory, the git branch, the worktree, the pull request, the model, the permission mode, the
uptime, and the process cost.

**Agents** lists the subagents the session spawned. Each entry names the agent type, the prompt
or skill it runs, and how long its transcript has been idle.

**Prompt** shows the last prompt the user submitted, with the session title above it.

**Usage** breaks the newest request into input, cache read, cache write, and output tokens. It
also totals recent activity over the part of the transcript that was read.

**Activity** answers what the session is doing now. It leads with the recap Claude writes when
you walk away, names the skill or MCP tool that drives the turn, and gives the length of the
turn that just ended. Under that come the files the session edited and every tool call in the
part of the transcript that was read, newest first. A recap is a `/config` toggle, so most
sessions show the tool stream alone.

Each tool row carries both the summary someone wrote for the call and the command itself, cut
to one row. Press `enter` to focus the pane and scroll the list, `enter` again to open the call
under the cursor, and `y` to put its full command on the clipboard.

## Where the data comes from

`dancefloor` reads files that Claude Code already writes. It never talks to the API, and it
never writes to your Claude Code state.

| Source | What it gives |
| ------ | ------------- |
| `~/.claude/sessions/<pid>.json` | The live session registry: pid, session id, directory, name, and busy or idle status |
| `~/.claude/projects/<dir>/<session>.jsonl` | Token usage, model, title, branch, permission mode, worktree, pull request, last prompt, and the activity stream |
| `~/.claude/projects/<dir>/<session>/subagents/` | One `meta.json` per spawned subagent |
| `ps` | CPU and resident memory per session process |
| `wl-copy`, `xclip`, `xsel`, or `pbcopy` | Whichever is installed, to copy a tool call |

A registry file outlives the process that wrote it. Every entry is confirmed against `ps`
before `dancefloor` reports it, so a crashed session disappears on the next refresh.

Two sessions often run in the same directory, so the process id identifies a session and the
directory does not.

## Known limits

**The context limit is worked out, not read.** Assistant messages record the base model id.
They record `claude-opus-5` even when the session runs the `[1m]` long-context variant, so the
window is never stated where the usage is. `dancefloor` takes the strongest signal it has, in
this order:

| Signal      | Where it comes from                                              |
| ----------- | ---------------------------------------------------------------- |
| Override    | `--context-limit`                                                |
| Observed    | usage already past 200k, so the long window is a fact            |
| Recorded    | a `cost-state` line that billed this model at `[1m]`             |
| Configured  | the model that Claude Code settings name for the session's directory |
| Declared    | `--context-default`, or the config file                          |
| Assumed     | nothing said otherwise, so 200k                                  |

A live session often reaches none of the first four. Claude Code writes the `cost-state` line at
shutdown, so a session that is still running has none, unless it was resumed from one that
ended. Settings only answer when they name a `model`, and many machines choose the model with
`/model` instead. So a fresh 1M session reads as 200k until its usage passes 200k. Set
`default_context_limit` to stop that. A `~` after the limit means the number is the 200k
fallback and nothing declared otherwise.

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
