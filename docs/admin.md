# burrow admin

`burrow admin` is a web admin for a running server, opened in the browser on your own machine. Watch live traffic, clients, services, visitors, websites and certificates, and change them in forms that explain every field, instead of editing YAML by hand. Every change is checked as `burrow validate` does, and shown as a diff with what it affects before it applies. Your file keeps its comments, and earlier versions can be reverted.

It runs on your machine with a visitor config that has admin rights, so the server opens no new port.

![The Overview page of burrow admin](images/admin-overview.png)

Back to the [README](../README.md). Working on the UI itself: [admin-ui-dev.md](admin-ui-dev.md). All server keys: [configuration.md](configuration.md). For AI apps, the same admin access over MCP: [mcp.md](mcp.md).

## Setup

Give one visitor admin rights in the server's config:

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
- Use a dedicated visitor for admin, so one leaked secret never gives both admin and services. `validate` warns when an admin visitor also reaches services or has `reach`.

Then, on the server, print the config for that visitor, and copy it to your machine:

```sh
burrow visitor-config ops-laptop --server myserver.example.com -o ops-laptop.yaml
```

`--server` is the server's public name or IP address; leave it out when server.yaml sets `public_address`. A visitor with admin access and nothing else gets a config without forwards, and `visitor-config` says to open it with `burrow admin`. It needs no `burrow visit`.

## Running it

Start it on your machine with the visitor's config. It opens the browser:

```sh
burrow admin -c ops-laptop.yaml     # opens the browser on http://127.0.0.1:7839
burrow admin --no-open --print-url  # ./visitor.yaml, or the one installed visitor service
```

- The browser opens a private launch file that forwards to the UI with its token. It is written to a folder only you can use (`$XDG_RUNTIME_DIR/burrow`, else `burrow-<uid>` in the temp folder; `%TEMP%\burrow` on Windows), and it is deleted when `burrow admin` stops.
- Without `-c`, it uses `./visitor.yaml`, or else the installed visitor service; `--service NAME` picks one of several.
- `--no-open` does not open the browser, and `--print-url` prints the link with its token. `--url-file FILE` writes the link to a file only you can read. If the link file or the launch file cannot be written, `burrow admin` says why and exits at once.
- The UI listens on 127.0.0.1:7839, or on the port of the session it reuses, so an open tab reconnects. If that port is taken, it says so and uses a free one. `--port N` uses exactly N and fails when it is taken; `--port 0` always takes a free one.
- A visitor without admin rights gets an error in the terminal and no browser opens.
- It stops after 30 minutes without a browser tab; `--idle-exit` changes that, like `90s`, `30m` or `2h`.
- If the page says "Open the link from your terminal", it was opened without the token: use the link `burrow admin` opened or printed, or run it again with `--print-url`.

## Installed servers

An installed server can write its config only when it has an admin visitor, or was installed with `service install --admin-writable`. Its config folder, `/etc/burrow/<name>/`, then belongs to the service's account. Otherwise the folder belongs to root, and the UI is read-only and shows the command that makes it writable. Nothing moves either way. [service.md](service.md#changing-the-config) shows how to add admin access to a server installed without it.

The installed copy is then the config: the file you installed from was only its first version, and `service status` says so instead of suggesting a reinstall. A reinstall, for example to change `--run-as`, still copies the source over it. If the installed copy differs, it is first kept as a version in its history, so you can go back to it from Activity. A reinstall from the source file also brings back admin access you revoked in the UI, because the source still has it; revoke it in the source too.

## Locked fields, history and revert

What the pages do, and what stays a file edit:

- A service page says how the service is reached: a public port, a website and visitors, in any mix. Turning one off keeps its values until you add the change, and Review says what goes: the port that closes, the website and its settings, or the visitors that can no longer reach it.
- Settings has four tabs: Server (the listener, this server and security), Connections (WebSocket, ready connections to clients, heartbeat, TCP, UDP and limits), Web (the web proxy and, once it is on, the local CA) and Monitoring. The search and the All, Needs a restart and Changed filters above them look through every tab, and each tab says how many of its settings match, are changed or are locked.
- Some settings are shown but are changed only in server.yaml on the server: `listen` (it needs a restart), `insecure_plaintext`, the heartbeat interval, the connection setup limits and every file path. The server refuses changes to them.
- The web proxy's ports are changed in Settings, and New website turns the web proxy on when the server has none. Review names each port a change opens and the websites a removal stops. A port the tunnel shares stays a file edit, and a port another program holds is refused before anything is written.
- WebSocket can be turned on, moved or turned off from Settings. The server first checks that a new port is free, and Review names the clients connected over WebSocket now: they are cut off and need their own config changed. A session that itself comes over WebSocket cannot move or turn off that listener; do that in server.yaml. The `websocket.tls` files stay a file edit.
- Before each change from burrow admin, the previous file is kept beside it in `.server.yaml.history/` (the last 20, mode 0600). Activity lists the versions by the time they were applied, and you can go back to each after a review. When that folder cannot be written, the history is kept in memory only.
- Every change goes through a review page, with what changes in server.yaml and what it affects, before it is applied, and a hand edit made meanwhile shows as a conflict instead of being overwritten.

## Drop-in files

When the server reads a folder of drop-in files ([configuration.md](configuration.md#drop-in-files)), the admin shows which file each service, client and visitor is in, and writes every change to that file:

- **Edit** changes an item in its own file and keeps the file's comments.
- **Add** writes a new item to `server.yaml`, or, with **Save in**, to a file of its own (`server.d/<name>.yaml`) or to another drop-in file.
- **Split into its own file** and **Merge into server.yaml** in an item's ⋯ menu move it between files. When a drop-in file is left with nothing in it, Review offers to delete it.
- **Rename** with references updates them in every file.
- **Review** shows one diff per file ("server.yaml: no change").
- **Settings → Files** lists each file, how many items it holds and whether it is read-only; its links open the list of that file's items.
- A file that is **read-only on disk** shows a lock; its items can be looked at but not edited, renamed, moved or removed.
- A change is written to every file it touches or to none. If a file changed on disk since you opened your change, it is refused and names the file; review your change again.
- **History** keeps the whole set of files, and a revert puts back every writable file, creating or deleting drop-in files as needed.

`burrow mcp` follows the same rules, with the `place` and `drop_file` operations for moving items and deleting an empty file.

## Mixed versions

An admin older than v0.2.1 can still change items in `server.yaml`; the server refuses its changes to items in drop-in files, with a message saying so.

The burrow on your machine and the one on the server don't need the same version. A newer burrow admin works with an older server: it says the server is older in Needs attention, and leaves out what that server cannot do. An older burrow admin, for example v0.1.2, can open a newer server too. When the config uses settings it doesn't know, such as clients with their own target or weight, websites or named login services, its Needs attention lists them ("This burrow admin is older than the server and cannot edit some settings here: …"), and it shows them as text instead of a form. Editing stays allowed, because the server checks every change. To edit those settings in forms, update burrow on your machine.

## Security notes

The admin can change the server's config, so it is guarded closely:

- The UI listens on `127.0.0.1` only, and needs a random token that is never on the command line. The token is kept for up to 12 hours per visitor config, in a file only you can read, so an open tab reconnects when you run `burrow admin` again. Expired session files are deleted the next time `burrow admin` starts. For a remote machine, use `ssh -L`.
- Secrets are generated by the server and shown once; existing secrets never leave the server.
- Credential headers in `request_headers` and `response_headers`, like `Authorization`, `Cookie` or `X-Api-Key`, show by name only, also to read-only admins, in diffs and in the server's log ([Headers](web/headers-limits-pages.md)).
- burrow admin can keep or remove `${VAR}` and `file:` references, but never add one, a revert included. A reference reads the server's environment or a file, and a value that leaves the server, like an answer header or a redirect, would carry it out. Write new references in server.yaml with `burrow edit` or over ssh. `burrow validate`, status and the Overview show a reference as written, never its value.
- You cannot remove your own access: the server refuses a change that would lock out the session making it.
- burrow admin can lower admin access but never grant or widen it. These are refused, a revert included: giving a visitor `admin`, raising `read` to `write`, adding a secret to an admin visitor, widening its `admin_from`, and renaming an admin visitor. Make those changes in the file with `burrow edit` or over ssh. Removing admin, lowering the level, removing a secret, narrowing `admin_from` and deleting an admin visitor work from the UI.
- On every reload, the session checks its visitor again. It ends when the right, the secret or the source address is gone, or when its level changes; the next session opens with the new level.

## Next steps

- [burrow mcp](mcp.md): the same admin access for an AI app.
- [Monitoring](monitoring.md): `burrow status` and metrics, without a browser.
- [Running as a service](service.md#changing-the-config): make an installed server writable for the admin.
