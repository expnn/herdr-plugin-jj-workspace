# jj workspaces

A [Herdr](https://herdr.dev) plugin to create and remove [Jujutsu](https://jj-vcs.github.io/jj/) (`jj`) workspaces with one keypress. New tabs open with your coding agent on the left and an interactive terminal on the right.

## Install

1. Install the plugin (Herdr builds it with `cargo` at install time):

   ```sh
   herdr plugin install expnn/herdr-plugin-jj-workspace
   ```

   For local development, `plugin link` does **not** build — build first:

   ```sh
   cargo build --release
   herdr plugin link .
   ```

2. Bind keys in your Herdr keybindings config (`prefix` is your leader, default
   `ctrl+b`). These are unbound in stock Herdr:

   ```toml
   [[keys.command]]
   key = "prefix+a"
   type = "plugin_action"
   command = "expnn.jj-workspace.new-tab"
   description = "new jj workspace"

   [[keys.command]]
   key = "prefix+shift+a"
   type = "plugin_action"
   command = "expnn.jj-workspace.new"
   description = "new jj workspace"

   [[keys.command]]
   key = "prefix+d"
   type = "plugin_action"
   command = "expnn.jj-workspace.remove"
   description = "remove jj workspace"
   ```

## Quickstart

- `prefix+a` or `prefix+shift+a` — name the jj workspace for the focused pane's repository and open it as a new tab in the same workspace
- `prefix+d` — remove the current jj checkout and close its tab

The wizard uses a single read-only source: the focused pane's directory,
resolved up to its jj workspace root — secondary workspaces are normalized to
the main repository root — and shown in the read-only **Source Workspace**
section. Press `Tab` to cycle the editable fields (new workspace name ↔ base
revision).

The source is always the repository of the pane you trigger the action from:
launching from a pane in a subdirectory still checks out at the root, and
creating a workspace for another repository means focusing a pane inside it
first, then triggering the action again. When the focused pane is not inside
a jj repository, the action shows an error toast and the wizard does not
open.

The plugin creates an empty sparse checkout from the resolved base revision
(`trunk()` by default), materializes the coding agent's startup files (a
cross-agent default covering 15 mainstream agents — see
`agent.bootstrap_paths` below), and opens its tab. The right terminal then
runs `scripts/setup-workspace.sh` (a plain POSIX shell script shipped with
the plugin — `cat` it any time to see exactly what it does): it materializes
the full checkout, creates the bookmark, runs `jj git fetch`, then re-resolves
the base revision in the main repository's working-copy context and rebases
the new working-copy commit onto the resolved commit (a warning skips the
rebase if the base can no longer be resolved) while the agent starts on the
left.

The setup script runs under `/bin/sh`, so it works no matter which interactive
shell your Herdr panes use (fish, bash, zsh, and other POSIX shells).

## Configuration

Optional plugin settings live in `config.toml` in the plugin config directory:

```sh
herdr plugin config-dir expnn.jj-workspace
```

The plugin reads **only** `config.toml` from that directory — process
environment variables and `.env` files are **not** configuration sources. A
missing file means all built-in defaults. Any syntax error, wrong type, unknown
key, or empty string value makes the plugin refuse to run: the wizard renders
the error in its UI, and headless actions print the error and exit non-zero.

Example:

```toml
[jj]
base_rev = "trunk()"                      # revision new workspaces are based on
workspace_root = "~/.herdr/workspaces"    # where new workspaces are checked out (~ expanded)
# command = "jj"                          # jj executable: bare name (PATH lookup), absolute path, or argv list

[agent]
command = "opencode"                      # command typed into the left pane to start the agent
extend_bootstrap_paths = ["docs/AGENTS.md"]  # your own startup files, appended to the default list
# bootstrap_paths = [...]                 # full takeover of the startup list (34-entry cross-agent default; see below)
auto_trust = false                        # auto-press Enter for the codex trust prompt during startup (see below)
# trust_window_secs = 10                  # window (seconds) in which auto_trust may answer the trust prompt
# startup_timeout_secs = 20               # wait budget (seconds) for the agent to be detected
# poll_interval_ms = 200                  # agent-status polling interval (ms)
```

Unknown keys are rejected (`deny_unknown_fields`), so a typo like `bas_rev`
fails loudly instead of being silently ignored.

Keys:

- `jj.base_rev` — revision new workspaces are based on. Defaults to `trunk()`.
- `jj.workspace_root` — directory where new jj workspaces are checked out. A
  leading `~` is expanded to your home directory. Defaults to
  `~/.herdr/workspaces`.
- `jj.command` — how the plugin invokes `jj`. A string is either a bare name
  (no `/`, looked up on the plugin process's `PATH`) or an absolute path
  (a leading `~` is expanded first). A list is a full argv: the first element
  is resolved by the same rules and the remaining elements are prepended to
  every `jj` invocation (for wrapper scripts). Relative paths containing `/`
  (like `./bin/jj`) are rejected. Defaults to `"jj"`. The value is resolved
  exactly once per plugin invocation and used everywhere — including the
  right-pane setup script, which bakes in the absolute path — so the pane
  shell's `PATH` never participates and all `jj` calls use the same binary.
  If resolution fails the plugin refuses to run, listing the searched `PATH`
  directories.
- `agent.command` — command typed into the left pane to start the coding agent.
  Defaults to `opencode`. The value is injected into the pane's **interactive
  shell** via `herdr pane run`, so shell aliases, functions, and `$VAR`
  expansion all work — set it to whatever your shell resolves (for example
  `"codex"` or `"codex --full-auto"`). It is a single string; array form is
  not supported.
- `agent.bootstrap_paths` — files materialized in each new sparse checkout via
  `jj sparse set --clear --add ...`. Paths are repo-relative (no `~`
  expansion) and shared across all agent types. Nonexistent paths are silently
  skipped by jj; they persist in the sparse pattern list, so if the repository
  later gains a file matching one, it is materialized automatically. The
  built-in default is a 34-entry cross-agent union — the startup files and
  directories (per official docs) of 15 mainstream coding agents: root
  instruction files (`AGENTS.md`, `AGENT.md`, `AGENTS.override.md`,
  `CLAUDE.md`, `CLAUDE.local.md`, `GEMINI.md`, `QWEN.md`, `CRUSH.md`), root
  tool-specific files (`.mcp.json`, `opencode.json`, `opencode.jsonc`,
  `.cursorrules`, `.windsurfrules`, `.goosehints`, `.augment-guidelines`,
  `.github/copilot-instructions.md`), and directories (`.agents`, `.claude`,
  `.codex`, `.cursor`, `.gemini`, `.qwen`, `.opencode`, `.windsurf`,
  `.devin`, `.clinerules`, `.cline`, `.kilo`, `.kilocode`, `.augment`,
  `.continue`, `.github/instructions`, `.crush`, `.goose`). It covers only
  startup-time synchronous reads: files agents load lazily during a session
  (nested `AGENTS.md`, glob-triggered rules) are already materialized by the
  right pane's full checkout seconds later. The evidence matrix and exclusion
  rationale live in
  design.md of the `agent-agnostic-bootstrap-paths` openspec change. 95% of
  users only need `agent.extend_bootstrap_paths` below — write this key by
  hand only when you need to **fully take over** the list (for example to
  exclude default entries); it stays the supported escape hatch.
- `agent.extend_bootstrap_paths` — your own startup files, appended to the
  resolved `bootstrap_paths` (your explicit value, or the 34-entry default)
  via the same `jj sparse set --add ...` mechanism. Paths are repo-relative,
  same rules as `bootstrap_paths`. Already-present entries are skipped with
  the base list's order preserved, so the effective list stays clean even if
  a future default gains one of your entries — the default keeps evolving and
  your additions ride along. Defaults to `[]` (no additions). To materialize
  **only** your own files, combine `bootstrap_paths = []` with this key: the
  explicit empty list clears the baseline, so the startup step materializes
  just what you listed (whitelist mode).

### Choosing the base revision

New workspaces are created on top of a base revision, resolved per source
repository in this order:

1. **Per repository** — `jj config set --repo herdr.base-rev 'dev@origin'`
   (run inside the repo; stored in jj's machine-local repo config, shared by
   all workspaces of that repository)
2. **Personal global** — `jj config set --user herdr.base-rev '...'` (applies
   to every repository, your own machines only)
3. **Plugin default** — `[jj] base_rev` in config.toml
4. Built-in default — `trunk()`

The wizard shows the resolved value in the **base** field (press `Tab` to
reach it) and lets you override it per run with any jj revset expression. Typed values are
validated against the source repository before the workspace is created
(`jj log -r <expr> --no-graph --limit 1`): an invalid expression, or one that
resolves to no commits, keeps the wizard open with an error message. The same
value drives `workspace add -r`; after `jj git fetch`, the setup script
re-resolves it in the main repository's working-copy context (via the
workspace's `.jj/repo` pointer) and rebases onto the resolved commit(s), so
both steps always evaluate the base the same way.

### Agent startup handling

After creating a workspace, the plugin watches the agent in the left pane via
Herdr's native agent detection (`herdr agent list`) — no screen scraping, and
every agent Herdr can detect works the same way:

- As soon as the agent process is detected, the new workspace is focused.
- If the agent reports `blocked` (waiting for your input) and stays blocked
  for more than a moment, you get a `"<agent> needs attention"` toast.

`agent.auto_trust` (default `false`) opts into pressing Enter automatically
for the Codex directory-trust prompt — but only for Codex, only within the
first `trust_window_secs` (default 10) seconds, and only while the blocked
status persists; the reply is confirmed by the status transitioning back to
non-blocked. This is a behavior change from earlier versions, which
auto-answered the Codex trust prompt by default: set `auto_trust = true` to
keep the old behavior. Only the Codex **trust** prompt is ever auto-answered;
any other prompt (permission requests, confirmations) always requires you.

### How jj workspaces relate

One repository, many working copies. A secondary workspace's `.jj/repo` is a
file pointer into the main workspace's store, so:

- **The commit layer is shared.** A commit or bookmark created in any
  workspace is instantly visible in every other one (change IDs are global).
- **Working-copy files are not.** Materialized files stay local to each
  workspace — that isolation is the point of workspaces, so the plugin does
  not (and will not) sync files between them.
- **Syncing back to your mainline is your manual jj workflow** — commit in
  the workspace, then rebase/push from any workspace. The plugin only sets
  the base revision (see [Choosing the base revision](#choosing-the-base-revision));
  it never auto-merges or auto-pushes.
- **Removing a workspace** runs `jj workspace forget` and deletes the
  checkout directory. The main workspace is never removed, and a workspace
  with uncommitted changes is refused: commits and bookmarks are safe in the
  shared store, but materialized uncommitted changes would be lost.

### Removing a workspace and opencode sessions

Before deleting anything, `remove` migrates opencode sessions bound to the
workspace (its root and subdirectories) back to the main repo, so their
conversation history stays visible and resumable there. The migration is
fail-closed: if opencode's database cannot be read, its schema changed in an
unexpected way, or the main repo's opencode project id cannot be resolved,
the plugin refuses to remove the workspace — nothing is forgotten, deleted or
closed, and the message says what to fix (typically: open opencode in the
main repo once, then retry). When opencode is not installed, or no session is
bound to the workspace, the step is silently skipped and removal proceeds as
usual.

### Migrating from `.env`

| `.env` key          | `config.toml` key      |
| ------------------- | ---------------------- |
| `JJ_BASE_REV`       | `[jj] base_rev`        |
| `JJ_WORKSPACE_ROOT` | `[jj] workspace_root`  |
| `JJ_START_COMMAND`  | `[agent] command`      |

The old `.env` file is no longer read; delete it and write `config.toml`
instead.

## Troubleshooting

**`jj.command: 'jj' was not found on PATH`** — the Herdr server's `PATH` can
be minimal, for example when `herdr --remote <host>` self-bootstraps a remote
server. The plugin process inherits that `PATH`, not your interactive shell's.
Fix: run `which jj` in your normal shell and write the absolute path into
`config.toml`:

```toml
[jj]
command = "/home/you/.local/bin/jj"
```

Resolution is unique: the plugin resolves `jj.command` once per invocation in
its own process and bakes the absolute path into every `jj` call (including
the right-pane setup script). The pane shell's `PATH` never participates, so a
missing `jj` is detected immediately with an actionable error instead of
failing halfway through workspace creation — even when your interactive shell
could find `jj` just fine.

**`permission denied: scripts/setup-workspace.sh`** — the shipped setup script
lost its executable bit (uncommon install). Re-apply it:
`chmod +x "$(herdr plugin config-dir expnn.jj-workspace)/../scripts/setup-workspace.sh"`
or run `herdr plugin install` again. The pane command also works if invoked as
`sh <script> …` as a fallback.

**`refusing to remove …: it has uncommitted changes`** — remove protects your
work: already-committed commits and bookmarks survive a workspace removal,
but materialized uncommitted changes would be deleted. Commit (`jj commit`)
or discard (`jj restore`) the changes in that workspace, then run remove
again.

**Where to find full error details** — toasts are short-lived and truncated
by Herdr (240 chars, single line, a few seconds). Every plugin error is also
appended with a UTC timestamp to
`~/.local/state/herdr/plugins/expnn.jj-workspace/error.log`
(`$HERDR_PLUGIN_STATE_DIR/error.log`; `$XDG_STATE_HOME` overrides the
`~/.local/state` prefix). `cat` that file to copy and inspect the full
message.

## License

MIT — see [LICENSE](LICENSE).
