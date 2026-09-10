#!/bin/sh
# setup-workspace: prepare a freshly created jj workspace for coding.
#
# Usage:
#   setup-workspace.sh <jj-exe> <base-rev> <bookmark-name> \
#                      <workspace-id> <tab-id> <pane-id> [jj leading args...]
#
# Run from the new workspace directory (the pane's cwd). Steps:
#   1. launch the finish-tab watcher in the background: it waits for the
#      coding agent in the left pane, optionally auto-answers the codex trust
#      prompt (only when agent.auto_trust is enabled in config.toml, codex
#      only, within the startup window), and pulls focus to the new workspace.
#      Failure here does not affect the checkout.
#   2. materialize the full checkout:  jj sparse set --clear --add .
#   3. create the workspace bookmark:  jj bookmark create <name> -r @
#      (failure is a warning only — the workspace still exists)
#   4. fetch from the remote:          jj git fetch
#   5. rebase the working-copy commit: jj rebase -s @ -d <base-rev>
#
# The plugin executable is located relative to this script
# (<plugin_root>/target/release/jj-workspace — the same layout the plugin
# manifest's actions rely on), so no plugin path needs to be passed in.
# Any leading arguments after the six fixed ones are inserted before every
# jj subcommand (they carry the leading arguments of an argv-form
# `jj.command`, e.g. `["/path/jj", "--at-op", "@-"]`).

set -eu

if [ "$#" -lt 6 ]; then
    echo "usage: setup-workspace.sh <jj-exe> <base-rev> <bookmark-name> <workspace-id> <tab-id> <pane-id> [jj leading args...]" >&2
    exit 2
fi
JJ_EXE="$1"
BASE_REV="$2"
NAME="$3"
WS_ID="$4"
TAB_ID="$5"
PANE_ID="$6"
shift 6

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PLUGIN_EXE="$SCRIPT_DIR/../target/release/jj-workspace"

# Launch the agent watcher in the background; its failure must not block the
# checkout below, so this line stays before `set -e` takes effect and runs
# detached from this script's exit status.
nohup "$PLUGIN_EXE" finish-tab "$WS_ID" "$TAB_ID" "$PANE_ID" \
    >/dev/null 2>&1 </dev/null &

set -ex

"$JJ_EXE" "$@" sparse set --clear --add .
"$JJ_EXE" "$@" bookmark create "$NAME" -r @ ||
    printf '%s\n' "warning: could not create bookmark $NAME (workspace still created)" >&2
"$JJ_EXE" "$@" git fetch
"$JJ_EXE" "$@" rebase -s @ -d "$BASE_REV"

