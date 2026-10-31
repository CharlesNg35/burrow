# burrow mcp

`burrow mcp` lets an AI app such as Claude Code, Claude Desktop or Cursor read a running server and, if you allow it, change its config. It speaks the [Model Context Protocol](https://modelcontextprotocol.io/) (MCP). Like [burrow admin](admin.md), it runs on your machine with a visitor config and uses the same encrypted admin channel. The server opens no new port, and the visitor's `admin: read` or `admin: write` is the only permission.

Ask your AI app questions like "which clients are offline?" or "why does this certificate fail?", or have it prepare a config change that you review before it applies. The AI sees what `burrow admin` shows, never a secret.

Back to the [README](../README.md).

## Setup

Setup takes two steps: an admin visitor on the server, and one entry in your AI app's settings. First give a visitor admin rights and copy its config to your machine, as [admin.md](admin.md#setup) shows. Then let `burrow mcp` print the setup for your app, with the full paths of this burrow and of the visitor config filled in:

```sh
# a terminal on your machine
burrow mcp -c ops-laptop.yaml --print-config claude   # or codex, or cursor
```

The app then starts `burrow mcp` itself and talks to it over stdin and stdout, with no port and no token.

The setup names the burrow you ran by its full path, so the app finds it even when burrow is not on its `PATH`:

- If the `burrow` your shell finds on `PATH` is this same burrow, for example a link such as `/opt/homebrew/bin/burrow`, the setup names that path, so it keeps working after an upgrade.
- Otherwise it names the file itself; if you move that file, print the setup again.
- If neither path can be found, the setup says plain `burrow`, and a note on stderr says so.

`burrow mcp` works the same on Linux, macOS and Windows. The setups below are for burrow in `/home/me/bin`. On Windows, Codex's file is `%USERPROFILE%\.codex\config.toml` and Cursor's is `%USERPROFILE%\.cursor\mcp.json`, and the paths are Windows paths, like `C:\\Users\\me\\bin\\burrow.exe` in JSON.

Claude Code, with one command:

```sh
# Claude Code: run once in a terminal
claude mcp add burrow -- /home/me/bin/burrow mcp -c /home/me/ops-laptop.yaml
```

Codex, in its config file:

```toml
# ~/.codex/config.toml
[mcp_servers.burrow]
command = "/home/me/bin/burrow"
args = ["mcp", "-c", "/home/me/ops-laptop.yaml"]
```

Cursor, in `~/.cursor/mcp.json`:

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

Claude Desktop takes the same entry as Cursor, in `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "burrow": {
      "command": "/Users/me/bin/burrow",
      "args": ["mcp", "-c", "/Users/me/ops-laptop.yaml"]
    }
  }
}
```

The file is `~/Library/Application Support/Claude/claude_desktop_config.json` on macOS and `%APPDATA%\Claude\claude_desktop_config.json` on Windows; Settings, Developer, Edit Config opens it. Restart Claude Desktop after changing it.

- Without `-c`, `burrow mcp` uses `./visitor.yaml`, or else the installed visitor service; `--service NAME` picks one of several, as for `burrow admin`.
- A visitor without admin rights gets an error, and the app shows the server as failed.
- Started by hand in a terminal, `burrow mcp` says on stderr that the app is meant to start it, and how to print the setup. Nothing but MCP ever goes to stdout.

## What the AI can do

burrow gives the AI app these tools. Some are listed only when writing is allowed:

| Tool | What it does | Listed when |
|---|---|---|
| `get_status` | the live status: clients, visitors, services, traffic | always |
| `get_config` | the config and its version; secrets only as fingerprints or `${ENV}` names | always |
| `describe_settings` | what each key means: type, default, help, essential or advanced, locked | always |
| `list_versions` | the kept versions, who changed what, which can be reverted | always |
| `check_change` | validates a change and returns its diff, problems, impact and a plan id; writes nothing | `admin: write` |
| `revert_to_version` | the plan to go back to a kept version; writes nothing | `admin: write` and `--allow-write` |
| `apply_change` | applies a checked plan | `admin: write` and `--allow-write` |

`burrow mcp` is read-only unless you start it with `--allow-write`, and the visitor also needs `admin: write`. To allow writing in Claude Code:

```sh
# Claude Code
claude mcp add burrow -- /home/me/bin/burrow mcp -c /home/me/ops-laptop.yaml --allow-write
```

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

Ready-made setup for your AI app:
  Claude Code:  /home/me/bin/burrow mcp -c ops-laptop.yaml --http --print-config claude
  Codex:        /home/me/bin/burrow mcp -c ops-laptop.yaml --http --print-config codex
  Cursor:       /home/me/bin/burrow mcp -c ops-laptop.yaml --http --print-config cursor
```

- It listens on `127.0.0.1` only, on 7838, on every system; `--port N` uses another port and `--port 0` a free one.
- Every request needs that header. The token is made once per visitor config, so the app's setup keeps working after a restart. It is kept in a file only you can read (mode 0600, or an access list for you alone on Windows), in burrow's private state folder, next to the [burrow admin](admin.md#security-notes) sessions. `--token-file FILE` keeps it in FILE instead.
- `--no-print-token` prints only the URL line: use it in a service, or when the output goes to a log or a shared screen. `--print-token` prints just the token and exits.
- `--new-token` makes a new token. The old one stops working at once, also in a `burrow mcp` that is running. The token is never a command-line argument.
- Requests from a web page (an `Origin` other than `http://127.0.0.1:<port>` or `http://localhost:<port>`) get 403, so a website cannot reach it through your browser.

The HTTP setups for each app:

```sh
# Claude Code: run once in a terminal
claude mcp add --transport http burrow http://127.0.0.1:7838/mcp --header "Authorization: Bearer <token>"
```

```toml
# ~/.codex/config.toml
[mcp_servers.burrow]
url = "http://127.0.0.1:7838/mcp"
bearer_token_env_var = "BURROW_MCP_TOKEN"
```

Codex reads the token from `BURROW_MCP_TOKEN`, so it stays out of the file. `--print-config codex` also prints the `export BURROW_MCP_TOKEN=...` line for the shell that starts Codex.

Cursor, in `~/.cursor/mcp.json`:

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

For an AI app on another machine, forward the port with `ssh -L 7838:127.0.0.1:7838 you@laptop` rather than opening it.

## Safety

An AI app that can change your server must not be able to do harm by mistake or by a trick in the data it reads. These rules hold:

- **Read-only by default.** Writes need `--allow-write` on your machine and `admin: write` on the server.
- **Only checked changes are applied.** `apply_change` takes a plan id from `check_change`; the server refuses another change or a config that changed since, and impacts that need confirming must be named.
- **Secrets never reach the AI.** Results show secrets as fingerprints or `${ENV}` names, and the AI cannot generate secrets or ask for connect snippets.
- **The server's rules from [admin.md](admin.md#security-notes) apply unchanged:** admin access can be lowered but never granted or widened, and the last 20 versions are kept and can be reverted.
- **Results are data, not instructions.** Names, host names, log lines and other text in results can be written by outsiders. The tool descriptions tell the AI so, and nothing applies without a diff you can see.
- **Your app asks you** before each tool call unless you turned that off; keep it on for `apply_change`.
- Activity in burrow admin shows changes made through `burrow mcp` under the visitor's name.

## Next steps

- [burrow admin](admin.md): the same access, in your browser.
- [Security](security.md#burrow-mcp): how the admin channel is protected.
