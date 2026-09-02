# jj workspaces

A [Herdr](https://herdr.dev) plugin to create and remove [Jujutsu](https://jj-vcs.github.io/jj/) (`jj`) workspaces with one keypress. New tabs open with Codex on the left and an interactive terminal on the right.

## Install

1. Install the plugin (Herdr builds it with `cargo` at install time):

   ```sh
   herdr plugin install NathanFlurry/herdr-plugin-jj-workspace
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

For a jj source, the plugin creates the checkout from local `trunk()` first and
opens its tab immediately. The right terminal then runs `jj git fetch` and
rebases the new working-copy commit onto the updated `trunk()`. If the selected
folder is not a jj workspace, the plugin opens the same folder and shows a
warning instead of creating a checkout.

## License

MIT — see [LICENSE](LICENSE).
