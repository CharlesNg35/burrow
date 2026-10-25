# burrow admin

`burrow admin` opens a local web UI for a running server: a live dashboard, and editing of clients, visitors, services and server settings with validation, a diff preview and comments kept. It runs on your machine with a visitor config, and the server exposes no new port: give that visitor admin rights in the server config.

Back to the [README](../README.md). Working on the UI itself: [admin-ui-dev.md](admin-ui-dev.md). All server keys: [configuration.md](configuration.md).

## Setup

```yaml
# server.yaml
visitors:
  ops-laptop:
    secret: ${OPS_SECRET}
    admin: write # read | write
    admin_from: [203.0.113.0/24] # optional: admin only from these addresses
```

- `admin: read` shows everything and changes nothing; `admin: write` may also change the config.
- `admin_from` limits admin access to these source IPs or CIDRs. Without it, admin works from any address the visitor can connect from.
- Use a dedicated visitor for admin; `validate` warns when it also reaches services or has `reach`, since one leaked secret should not give both.

Then, on the server, print the config for that visitor, and copy it to your machine:

```sh
burrow visitor-config ops-laptop --server myserver.example.com -o ops-laptop.yaml
```

`--server` is the server's public name or IP address; you can leave it out when server.yaml sets `public_address`. A visitor with admin access and nothing else gets a config without forwards, and `visitor-config` says to open it with `burrow admin`. You do not need `burrow visit` for it.

## Running it

```sh
burrow admin -c ops-laptop.yaml     # opens the browser on http://127.0.0.1:7839
burrow admin --no-open --print-url  # ./visitor.yaml, or the one installed visitor service
```

- The browser opens a private launch file that burrow writes for you, which forwards to the UI with its token. The file is readable by you only, and is deleted when `burrow admin` stops.
- Without `-c`, it uses `./visitor.yaml`, or else the installed visitor service; `--service NAME` picks one of several.
- `--no-open` does not open the browser, and `--print-url` prints the link with its token. `--url-file FILE` writes the link to a file readable by you only. The UI listens on 127.0.0.1:7839, or on the port of the session it reuses (an open tab reconnects there); when that port is taken it says so and uses a free one. `--port N` uses exactly N and fails when it is taken, and `--port 0` always takes a free one. If the link file or the private launch page cannot be written, `burrow admin` says why and exits at once.
- A visitor without admin rights gets an error in the terminal and no browser opens.
- It stops after 30 minutes without a browser tab; `--idle-exit` changes that, like `90s`, `30m` or `2h`.
- If the page says "Open the link from your terminal", it was opened without the token: use the link `burrow admin` opened or printed, or run it again with `--print-url`.

## Installed servers

An installed server can write its config only when it has an admin visitor, or when it was installed with `service install --admin-writable`. Its config folder, `/etc/burrow/<name>/`, then belongs to the service's account. Otherwise the folder belongs to root, and the UI is read-only and shows the command that makes it writable. Nothing moves either way. See [service.md](service.md#changing-the-config), including how to add admin access to a server installed without it.

That installed copy is then the config: the file you installed from was only its first version, and `service status` says so instead of suggesting a reinstall. A reinstall, for example to change `--run-as`, still copies the source over it, but when the installed copy differs from the source it first keeps that copy as a version in its history, so Activity can revert to it. A reinstall from the source file also brings back admin access you revoked in the UI, because the source still has it; revoke it in the source too.

## Locked fields, history and revert

- Some settings are shown but cannot be changed from the UI: `listen`, `websocket`, `insecure_plaintext`, the heartbeat interval, the handshake limits, and every file path. The web proxy's ports can: Settings, Web proxy, or the New website flow, which turns the web proxy on when the server has none. Review names each port it opens and the websites a removal stops; a port the tunnel shares stays a file edit, and a port another program holds is refused before anything is written. Change them in server.yaml on the server; `listen` needs a restart, the others apply on save. The server refuses changes to them.
- Before each change from burrow admin, the previous file is kept beside it in `.server.yaml.history/` (the last 20, mode 0600). Activity lists the versions, and each can be reverted after a review. When that folder cannot be written, the history is kept in memory only.
- Every change goes through a review page with the diff and its impact before it is applied, and a hand edit made meanwhile shows as a conflict instead of being overwritten.

## Security notes

- The UI listens on `127.0.0.1` only and needs a random token that is never on the command line. The token is kept for up to 12 hours per visitor config, in a file only you can read, so an open tab reconnects when you run `burrow admin` again. Expired session files are deleted the next time `burrow admin` starts. For a remote machine, use `ssh -L`.
- Secrets are generated by the server and shown once; existing secrets never leave the server.
- burrow admin can keep or remove `${VAR}` and `file:` references, but never add one, a revert included: a reference reads the server's environment or a file, and a value that leaves the server, like an answer header or a redirect, would carry it out. Write new references in server.yaml with `burrow edit` or over ssh. `burrow validate`, status and the Overview show a reference as written, never its value.
- You cannot remove your own access: the server refuses a change that would lock out the session making it.
- burrow admin can lower admin access but never grant or widen it. Giving a visitor `admin`, raising `read` to `write`, adding a secret to an admin visitor, widening its `admin_from` and renaming an admin visitor are refused, a revert included; make those changes in the file with `burrow edit` or over ssh. Removing admin, lowering the level, removing a secret, narrowing `admin_from` and deleting an admin visitor work from the UI.
- On every reload, the session checks its visitor again, and it ends when the right, the secret or the source address is gone, or when its level changes; the next session opens with the new level.
