# Updating clients from the server

From 0.2.1 on, every client tells the server which burrow it runs: the release, the system (like `linux/arm64`), how it runs (a service, a container or started by hand) and whether it allows remote updates. burrow admin, `burrow status` and `burrow mcp` show this. A client that allows it can then be updated from burrow admin or `burrow mcp`. The server only asks it to update; the client finds the latest release itself, fetches it from the official releases, checks it, and goes back to its old version if the new one does not connect.

Back to the [README](../README.md). Updating the server or a client by hand: [service.md](service.md#update-burrow).

- [Allow it on the client](#allow-it-on-the-client)
- [Update from burrow admin](#update-from-burrow-admin)
- [What the client does](#what-the-client-does)
- [Clients older than 0.2.1](#clients-older-than-021)
- [What the server cannot do](#what-the-server-cannot-do)

## Allow it on the client

Remote updates are off until the client's own config turns them on:

```yaml
# client.yaml
server: myserver.example.com:7835
name: home
secret: ${HOME_SECRET}
allow: [127.0.0.1/32:3000]
update:
  remote: true # let the server's admin ask this client to update to a newer release
```

- `burrow init client` asks about it (the answer is no unless you say yes), and `--remote-updates` turns it on without asking.
- `burrow validate` says when a config allows it.
- On the client's machine, `burrow status` shows the running client, how it runs and whether remote updates are possible there.

How the client runs decides how it updates:

| runs as | what happens | needs |
| --- | --- | --- |
| a service from `burrow service install` | a small updater that runs as root (Administrator on Windows) replaces the binary and restarts the service through systemd, launchd or the Windows service manager | install the service again after you turn `update.remote` on, so the updater is added: `sudo burrow service install -c client.yaml` |
| started by hand (`burrow client`, also under your own supervisor and on FreeBSD) | the client replaces its own binary and starts it again in the same terminal (on Windows as a new process in the same window) | the account that runs it may write to the binary's folder |
| a container | nothing: update the image, for example `docker pull ghcr.io/charlesng35/burrow:latest` | |

A client that allows remote updates but cannot do them says why, for example "the service needs installing again to allow it". burrow admin shows that reason.

## Update from burrow admin

- The Overview's Clients tab lists each client's version and system, marks the clients that have a newer release ("0.2.2 available") and shows **Update all to 0.2.2 (N)** for the clients that can take it.
- A client's page has a **Burrow on this client** card: version, system, how it runs, remote updates, the newer release, and an **Update to 0.2.2** button. A disabled button says why on hover.
- Both ask for confirmation first. **Update all** updates one client at a time. The next client starts only after the one before it has connected again with the new version, and it stops at the first client that fails, says so and names the reason.
- The result shows on the client's card ("0.2.1 → 0.2.2: updated, asked by ops (203.0.113.7)", or "rolled back" with the reason) and in the server's log.
- burrow admin needs `admin: write`; read-only admins see versions but no buttons.

The latest release comes from the official releases' `SHA256SUMS`. The server looks it up only while an admin looks at the status, and remembers it for 6 hours (15 minutes after a failed check). Clients are always updated to the latest release: there is no choice of version. A server that cannot reach GitHub shows versions but no "available" marks and offers no update. With `burrow mcp --allow-write`, the `update_clients` tool does the same as the buttons ([mcp.md](mcp.md#what-the-ai-can-do)).

## What the client does

When the server asks a client to update:

1. The client refuses unless remote updates are on in its own config, or while another update runs. A client started by hand does the steps below itself. A service only starts its updater, which runs as root (SYSTEM on Windows) and takes nothing from the client: no version, no file, no folder. The updater checks the opt-in again in the installed config, which root owns.
2. It finds the latest release, X, in the official GitHub releases, or in a mirror set in the client machine's own `BURROW_UPDATE_URL`, as for `burrow update`. It goes on only when X is newer than what is installed (updates never go back), and takes a pre-release only when it already runs one.
3. It downloads X for its own system, checks it against the release's `SHA256SUMS`, runs it to confirm it reports X, and checks that X accepts the config. If any check fails, nothing changes, and the server shows the reason.
4. It keeps the old binary beside the new one, as `burrow.previous`, installs X and restarts.
5. If X does not connect to the server and report itself within 60 seconds, the old binary is put back and started again. The server shows "rolled back" with the reason. For a service, only a report from the service's own process counts. If X keeps crashing before it connects, the old one is put back too: a service's updater sees the restarts, and a client started by hand goes back on its third start.

60 seconds covers starting the service, one failed connection attempt (10 s by default), the next tries and the handshake, with room to spare. A server that is down for longer during the update also leads to a roll back; ask again once it is up.

On Linux the updater is a pair of systemd units, `burrow-<name>-update.path` and `burrow-<name>-update.service`, started when the client creates the file `update-now` in its state folder. On macOS it is a launchd job, `com.github.charlesng35.burrow-<name>-update`, started by a file in its queue folder. The updater removes these files without opening them. On Windows it is a service, `burrow-<name>-update`, that only the client's own service account may start. The updater writes its result to `/var/lib/burrow/updates/<name>` (`/Library/Application Support/burrow/updates/<name>` on macOS, `%ProgramData%\burrow\updates\<name>` on Windows), which the client may read but not change. `burrow service uninstall` removes all of it, and installing without `update.remote` removes it too.

A client started by hand will not replace a program that installed services use. Update those with `sudo burrow update`.

## Clients older than 0.2.1

Older clients send no version, so burrow admin shows them as "older than 0.2.1". Update each of them by hand once (`sudo burrow update` on that machine, see [service.md](service.md#update-burrow)). Then add `update: { remote: true }` if you want later updates from the server. Older clients and servers keep working with 0.2.1 in both directions.

## What the server cannot do

Remote updates are built so that someone who controls the server, or an admin session, cannot run their own code on a client:

- The client decides. Without `update.remote: true` in the client's own config, it refuses every request. A service's updater reads the installed config itself (owned by root), not what the client process says.
- The server only asks. The client, or its updater, picks the latest official release itself and checks `SHA256SUMS` from the same place, so the server cannot point it at another file or version.
- A service's updater takes no input from the client's account. The account can start it, and nothing else: the trigger is removed unread, and the updater writes only into a folder the account cannot change. Even a compromised client process therefore cannot make root run anything but the newest official release.
- Updates never go back to an older release. That would let someone bring back a fixed bug.
- On Windows, the client's service account may only start its updater, not stop, change or delete it.

What is left is the trust you already give the release on GitHub, the same as for `burrow update`. See [security.md](security.md#client-updates) and [D75](decisions.md#d75-the-servers-admin-may-ask-a-client-to-update-and-the-client-fetches-checks-and-rolls-back-itself).
