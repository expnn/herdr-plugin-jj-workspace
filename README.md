# jj workspaces

A [Herdr](https://herdr.dev) plugin to create and remove [Jujutsu](https://jj-vcs.github.io/jj/) (`jj`) workspaces with one keypress. New tabs open with Codex on the left and an interactive terminal on the right.

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
   command = "nathanflurry.jj-workspace.new-tab"
   description = "new jj workspace"

   [[keys.command]]
   key = "prefix+shift+a"
   type = "plugin_action"
   command = "nathanflurry.jj-workspace.new"
   description = "new jj workspace"

   [[keys.command]]
   key = "prefix+d"
   type = "plugin_action"
   command = "nathanflurry.jj-workspace.remove"
   description = "remove jj workspace"
   ```

## Quickstart

- `prefix+a` or `prefix+shift+a` — choose a Herdr workspace, name the jj workspace, and open it as a new tab in the selected workspace
- `prefix+d` — remove the current jj checkout and close its tab

The source selector starts on the current workspace with fuzzy search focused.
Type to filter by workspace label or path, use `↑`/`↓` (including
`Ctrl+↑`/`Ctrl+↓`) to navigate the matches, and press `Tab` to edit the name.

Candidates are **jj workspaces only**: a Herdr workspace is listed when its
path — the workspace root, or the active pane's directory normalized up to
the nearest `.jj` — is inside a jj repository. New tabs are always created at
the workspace root, so launching from a pane in a subdirectory still checks
out at the root. When no workspace is a jj repository, the wizard opens with
an empty state instead of a list.

The plugin creates an empty sparse checkout from the resolved base revision
(`trunk()` by default), materializes only Codex's startup instructions, and
opens its tab. The right terminal then runs `scripts/setup-workspace.sh` (a
plain POSIX shell script shipped with the plugin — `cat` it any time to see
exactly what it does): it materializes the full checkout, creates the
bookmark, runs `jj git fetch`, and rebases the new working-copy commit onto
the base revision while Codex starts on the left.

The setup script runs under `/bin/sh`, so it works no matter which interactive
shell your Herdr panes use (fish, bash, zsh, and other POSIX shells).

## Configuration

Optional plugin settings live in `config.toml` in the plugin config directory:

```sh
herdr plugin config-dir nathanflurry.jj-workspace
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
command = "codex"                         # command typed into the left pane to start the agent
bootstrap_paths = ["AGENTS.md", "AGENTS.override.md", ".codex", ".agents"]
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
  Defaults to `codex`. The value is injected into the pane's **interactive
  shell** via `herdr pane run`, so shell aliases, functions, and `$VAR`
  expansion all work — set it to whatever your shell resolves (for example
  `"opencode"` or `"codex --full-auto"`). It is a single string; array form is
  not supported.
- `agent.bootstrap_paths` — files materialized in each new sparse checkout via
  `jj sparse set --clear --add ...`. Paths are repo-relative (no `~`
  expansion) and shared across all agent types. Nonexistent paths are silently
  skipped by jj; they persist in the sparse pattern list, so if the repository
  later gains a file matching one, it is materialized automatically.

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

The wizard shows the resolved value in the **base** field (tab to edit) and
lets you override it per run with any jj revset expression. Typed values are
validated against the source repository before the workspace is created
(`jj log -r <expr> --no-graph --limit 0`), and an invalid expression keeps the
wizard open with jj's own error message. The same resolved revision is used
as the right-pane rebase destination, so `workspace add -r` and the setup
script's `rebase -d` always agree.

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
`chmod +x "$(herdr plugin config-dir nathanflurry.jj-workspace)/../scripts/setup-workspace.sh"`
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
`~/.local/state/herdr/plugins/nathanflurry.jj-workspace/error.log`
(`$HERDR_PLUGIN_STATE_DIR/error.log`; `$XDG_STATE_HOME` overrides the
`~/.local/state` prefix). `cat` that file to copy and inspect the full
message.

## License

MIT — see [LICENSE](LICENSE).
