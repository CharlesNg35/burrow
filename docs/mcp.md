# burrow mcp

`burrow mcp` lets an AI app such as Claude Code, Claude Desktop or Cursor read a running server and, if you allow it, change its config. It speaks the [Model Context Protocol](https://modelcontextprotocol.io/) (MCP). Like [burrow admin](admin.md), it runs on your machine with a visitor config and uses the same encrypted admin channel. The server opens no new port, and the visitor's `admin: read` or `admin: write` is the only permission.

Ask your AI app questions like "which clients are offline?" or "why does this certificate fail?", or have it prepare a config change that you review before it applies. The AI sees what `burrow admin` shows, never a secret.

Back to the [README](../README.md).

## Setup

Setup takes two steps: an admin visitor on the server, and one entry in your AI app's settings. First give a visitor admin rights and copy its config to your machine, as [admin.md](admin.md#setup) shows. Then let `burrow mcp` print the config for your app, with the full paths of this burrow and of the visitor config filled in:

```sh
# a terminal on your machine
burrow mcp -c ops-laptop.yaml --print-config          # the standard config, for any app
burrow mcp -c ops-laptop.yaml --print-config claude   # or codex, or cursor: that app's own form
burrow mcp -c ops-laptop.yaml --check                 # start as an app would, and report
```

`--print-config` without an app name prints one standard config that works in any app: the JSON on stdout, so `> file` keeps only the JSON, and the same as fields on stderr. With `--allow-write`, the printed arguments include it too. The app then starts `burrow mcp` itself and talks to it over stdin and stdout, with no port and no token.

The standard config, as `--print-config` prints it for burrow in `/home/me/bin`:

```json
{
  "mcpServers": {
    "burrow": {
      "command": "/home/me/bin/burrow",
      "args": ["mcp", "-c", "/home/me/ops-laptop.yaml"]
    }
  }
}
```

The same for an app with a form, like Cherry Studio, where each argument goes on its own line:

```text
Command:
  /home/me/bin/burrow
Arguments, one per line:
  mcp
  -c
  /home/me/ops-laptop.yaml
```

An app with a form passes each line of its Arguments field as one argument. Typed on one line, `mcp -c /home/me/ops-laptop.yaml` reaches burrow as a single argument, which is no command: Cherry Studio then showed only "MCP error -32000: Connection closed". burrow now answers the app with an error that says to put each argument on its own line, and the form view above shows the lines.

The config names the burrow you ran by its full path, so the app finds it even when burrow is not on its `PATH`, and the visitor config by its full path, so the app's working folder does not matter:

- If the `burrow` your shell finds on `PATH` is this same burrow, for example a link such as `/opt/homebrew/bin/burrow`, the config names that path, so it keeps working after an upgrade.
- Otherwise it names the file itself; if you move that file, print the config again.
- If neither path can be found, the config says plain `burrow`, and a note on stderr says so.

Where the config goes, per app. `burrow mcp` works the same on Linux, macOS and Windows; on Windows the paths are Windows paths, like `C:\\Users\\me\\bin\\burrow.exe` in JSON.

- **Claude Desktop:** in `claude_desktop_config.json`, which Settings, Developer, Edit Config opens (`~/Library/Application Support/Claude/claude_desktop_config.json` on macOS, `%APPDATA%\Claude\claude_desktop_config.json` on Windows). Restart Claude Desktop after changing it.
- **Cursor:** in `~/.cursor/mcp.json` (`%USERPROFILE%\.cursor\mcp.json` on Windows); `--print-config cursor` prints the same JSON, with the file named on stderr.
- **Cherry Studio:** Settings, MCP Servers, Add server: type STDIO, the Command, and the Arguments one per line; or import the JSON.

`--print-config claude`, `codex` and `cursor` print the same command and arguments in that app's own form, from the same values as the standard config. Claude Code reads the standard JSON as is from a project's `.mcp.json`; for your user, `--print-config claude` gives one command:

```sh
# Claude Code: run once in a terminal
claude mcp add burrow -- /home/me/bin/burrow mcp -c /home/me/ops-laptop.yaml
```

Codex takes the same command and arguments in its config file, as `--print-config codex` prints it:

```toml
# ~/.codex/config.toml
[mcp_servers.burrow]
command = "/home/me/bin/burrow"
args = ["mcp", "-c", "/home/me/ops-laptop.yaml"]
```

Without `-c`, `burrow mcp` uses `./visitor.yaml`, or else the installed visitor service; `--service NAME` picks one of several, as for `burrow admin`. An app starts it in its own working folder, so give `-c` with the full path, as the standard config does.

### Check the setup

`burrow mcp --check` does the same start an app does, then reports and exits; exit code 0 means an app will get these tools:

```text
  config:  /home/me/ops-laptop.yaml (visitor "ops")
  server:  reached tunnel.example.com
  admin:   write
  writes:  off; changes can be checked, not applied; add --allow-write to apply them
  tools:   5 (check_change, describe_settings, get_config, get_status, list_versions)
ok: an AI app that starts burrow mcp with this config gets these tools (check-only mode)
```

If something is wrong, it ends with exit code 1 and the same error an app would show: no config, the server not reached, or the visitor without admin rights.

### Starting, stopping and the server link

- `burrow mcp` answers the app's handshake at once and connects to the server in the background. A tool call waits for that link for up to 30 seconds.
- If the link fails or drops, for example while the laptop sleeps or changes network, each tool returns an error that says where it failed and what to do, like "burrow mcp on this computer could not connect to tunnel.example.com: could not look up tunnel.example.com (DNS: ...)" with the hint to check this computer's network. The next call connects again, with a fresh DNS lookup; after failures it waits 1, 2, 4, then at most 8 seconds between tries.
- `get_status` shows the link under `mcp_link`: its state (`up`, `down` or `connecting`), the server, the visitor, the session's mode under `session` (`read-only`, `check-only` or `write`) and how often it reconnected.
- It stops within a second when the app closes its stdin, on Ctrl-C and on SIGTERM, also while a call is still waiting for the server.
- Started by hand in a terminal, it says on stderr that an app is meant to start it, and shows the full commands that print the setup and check it, with `-c` and the config's full path, so they work from any folder:

  ```text
  burrow mcp speaks MCP on stdin and stdout and is meant to be started by your AI app; stop it with Ctrl-C

  Setup for your AI app:
    Any app:   /home/me/bin/burrow mcp -c /home/me/ops-laptop.yaml --print-config
    One app:   /home/me/bin/burrow mcp -c /home/me/ops-laptop.yaml --print-config claude|codex|cursor
    Check it:  /home/me/bin/burrow mcp -c /home/me/ops-laptop.yaml --check
  ```

  It waits for an app; stop it with Ctrl-C. Nothing but MCP ever goes to stdout; warnings, like the one about a config file that others can read, go to stderr.

### Errors and the log file

Desktop apps hide what a program writes to stderr, so `burrow mcp` also tells the app:

- If it cannot start, for example because the visitor config is missing or has an error, it answers the app's handshake with an MCP error that says why, what to do and where the log is, instead of only exiting. The app shows that message, rather than a bare "Connection closed".
- It keeps a small log, `burrow-mcp.log`, next to the visitor config, or in burrow's private state folder when that folder is not writable or the config was not found. It holds the start, link failures and reconnects, and startup errors, never a secret, up to 256 KiB, then one older file, `burrow-mcp.log.old`.
- If burrow is started without any app talking to it and stdin closes, it says so plainly: no AI app spoke to it.

## What the AI can do

burrow gives the AI app these tools. Some are listed only when writing is allowed:

| Tool | What it does | Listed when |
|---|---|---|
| `get_status` | the live status: clients with their burrow version and system, the latest release, visitors, services, traffic | always |
| `get_config` | the config and its version; secrets only as fingerprints or `${ENV}` names. `show: with_defaults` adds every default in effect, as burrow admin's Config file tab does | always |
| `describe_settings` | what each key means: type, default, help with examples, essential or advanced, locked | always |
| `list_versions` | the kept versions, who changed what, which can be reverted | always |
| `validate_config` | checks a config or a fragment, like a `web:` section: the server checks it with its own validation, environment variables and files, as `burrow validate` does there (`checked_by: server`); writes nothing | always |
| `check_change` | validates a change and returns its diff, problems, impact and a plan id; writes nothing | `admin: write` |
| `revert_to_version` | the plan to go back to a kept version; writes nothing | `admin: write` and `--allow-write` |
| `apply_change` | applies a checked plan | `admin: write` and `--allow-write` |
| `update_clients` | asks clients to update burrow to a newer release, one at a time; without `confirm` it only says which clients it would ask | `admin: write` and `--allow-write` |
| `update_server` | updates the server itself to the latest release through its root updater; without `confirm` it only says from which version to which, or why it cannot | `admin: write` and `--allow-write`, and a server installed with `update.remote: true` |

In every mode, the instructions tell the AI to check a config example with `validate_config` (or read the exact keys with `describe_settings`) before it shows you one, and never to present an unchecked example as correct. `validate_config` checks a fragment as part of a larger config, so a missing `listen`, clients or services is not reported; give `"scope": "whole"` to check a complete file. The server checks it, with the rules of the burrow it runs and its own `${VAR}` values and files, and the result says `checked_by: server`; it takes up to 64 KiB and 10 seconds, also for `admin: read`. With a server older than v0.2.3, or when the server cannot be reached, burrow mcp checks it on your computer instead: the result says `checked_by: local`, and its note names both versions or why the server was not reached, since `${VAR}` values and files are then not checked. Secrets, the values of `${VAR}`s and long values are never echoed back.

Each session has one of three modes, and the instructions burrow mcp gives the AI say which, so the AI does not promise what it cannot do:

| Mode | When | What the AI can do |
|---|---|---|
| read-only | the visitor has `admin: read` | read; it tells you that changes need `admin: write` and `--allow-write` |
| check-only | `admin: write`, without `--allow-write` | read and check changes; `check_change` says the plan cannot be applied here, and the AI tells you to restart burrow mcp with `--allow-write` |
| write | `admin: write` and `--allow-write` | read, check and apply changes, and update clients, each after your OK |

`burrow mcp` is check-only or read-only unless you start it with `--allow-write`, and the visitor also needs `admin: write`. To allow writing in Claude Code:

```sh
# Claude Code
claude mcp add burrow -- /home/me/bin/burrow mcp -c /home/me/ops-laptop.yaml --allow-write
```

Updating clients also takes two steps: `update_clients` without `confirm` returns the latest release and which clients it would ask, with the reason for each client it skips; called again with `confirm: ["update"]`, it asks them, one at a time. Progress and results show in `get_status`, under each client's `update`. See [updates.md](updates.md).

Updating the server works the same way: `update_server` without `confirm` returns the server's version, the latest release and, when it cannot update, why (started by hand, a container, or a service installed without `update.remote: true`); called again with `confirm: ["update"]`, the server's root updater installs the release and the server restarts. Clients, visitors and the `burrow mcp` session pause for a few seconds and reconnect. The result shows in `get_status` under `server_update.last`, "rolled back" with the reason if the new release was not healthy within a minute. It is the same entry point as burrow admin's **Update** button ([updates.md](updates.md#update-the-server-from-burrow-admin)).

A change always takes two steps: `check_change` returns the diff and a plan id, and `apply_change` applies exactly that plan. Impacts that need your OK, like a restart or dropped connections, must be named in `apply_change`, so the AI has to ask you first. If the config changed in between, the server refuses and the AI checks again. A plan is kept for 30 minutes, with at most 16 plans at a time.

## Over HTTP

For an app that connects to a URL instead, run `burrow mcp --http` yourself:

```sh
# a terminal on your machine
burrow mcp -c ops-laptop.yaml --http   # listens on http://127.0.0.1:7838/mcp
```

It prints the URL and the header to send, and how to print a ready setup:

```text
burrow mcp: MCP over HTTP
  URL:    http://127.0.0.1:7838/mcp
  Header: Authorization: Bearer <token>

Setup for your AI app:
  Any app:   /home/me/bin/burrow mcp -c /home/me/ops-laptop.yaml --http --print-config
  One app:   /home/me/bin/burrow mcp -c /home/me/ops-laptop.yaml --http --print-config claude|codex|cursor
```

- It listens on `127.0.0.1` only, on 7838, on every system; `--port N` uses another port and `--port 0` a free one.
- Every request needs that header. The token is made once per visitor config, so the app's setup keeps working after a restart. It is kept in a file only you can read (mode 0600, or an access list for you alone on Windows), in burrow's private state folder, next to the [burrow admin](admin.md#security-notes) sessions. `--token-file FILE` keeps it in FILE instead.
- `--no-print-token` prints only the URL line: use it in a service, or when the output goes to a log or a shared screen. `--print-token` prints just the token and exits.
- `--new-token` makes a new token. The old one stops working at once, also in a `burrow mcp` that is running. The token is never a command-line argument.
- Requests from a web page (an `Origin` other than `http://127.0.0.1:<port>` or `http://localhost:<port>`) get 403, so a website cannot reach it through your browser.

`--http --print-config` prints the standard config for an app that connects to a URL, as JSON and as fields (URL and header); `--http --print-config claude`, `codex` or `cursor` print it in that app's form. Cursor, in `~/.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "burrow": {
      "url": "http://127.0.0.1:7838/mcp",
      "headers": { "Authorization": "Bearer <token>" }
    }
  }
}
```

Claude Code takes the same URL and header in one command:

```sh
# Claude Code: run once in a terminal
claude mcp add --transport http burrow http://127.0.0.1:7838/mcp --header "Authorization: Bearer <token>"
```

Codex reads the token from an environment variable, so it stays out of its file: `--print-config codex` also prints the `export BURROW_MCP_TOKEN=...` line for the shell that starts Codex.

```toml
# ~/.codex/config.toml
[mcp_servers.burrow]
url = "http://127.0.0.1:7838/mcp"
bearer_token_env_var = "BURROW_MCP_TOKEN"
```

For an AI app on another machine, forward the port with `ssh -L 7838:127.0.0.1:7838 you@laptop` rather than opening it.

## Safety

An AI app that can change your server must not be able to do harm by mistake or by a trick in the data it reads. These rules hold:

- **No writes by default.** Writes need `--allow-write` on your machine and `admin: write` on the server; without them the session is check-only or read-only, and the AI is told so.
- **Client and server updates need your OK too.** `update_clients` asks nobody and `update_server` changes nothing until called again with `confirm`, and the clients and the server's root updater fetch the release from the official releases themselves; the AI cannot name a download or a version, and cannot turn server updates on.
- **Only checked changes are applied.** `apply_change` takes a plan id from `check_change`; the server refuses another change or a config that changed since, and impacts that need confirming must be named.
- **Secrets never reach the AI.** Results show secrets as fingerprints or `${ENV}` names, and the AI cannot generate secrets or ask for connect snippets.
- **The server's rules from [admin.md](admin.md#security-notes) apply unchanged:** it refuses removing your own visitor or the last one with write access, a change that ends your own session must be confirmed by name, and the last 20 versions are kept and can be reverted.
- **Results are data, not instructions.** Names, host names, log lines and other text in results can be written by outsiders. The tool descriptions tell the AI so, and nothing applies without a diff you can see.
- **Your app asks you** before each tool call unless you turned that off; keep it on for `apply_change`.
- Activity in burrow admin shows changes made through `burrow mcp` under the visitor's name.

## Next steps

- [burrow admin](admin.md): the same access, in your browser.
- [Security](security.md#burrow-mcp): how the admin channel is protected.
