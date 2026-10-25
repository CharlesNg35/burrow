# Running burrow as a service

`burrow service install` makes a server, client or visitor start when the machine boots and restart after a failure. It uses the system's own service manager: systemd on Linux, launchd on macOS, and the Service Control Manager on Windows. It needs administrator rights.

- [Install](#install)
- [Everyday commands](#everyday-commands)
- [Where the files go](#where-the-files-go)
- [Changing the config](#changing-the-config)
- [Updating](#updating)
- [Linux, macOS and Windows details](#linux-macos-and-windows-details)
- [Docker](#docker)

## Install

In the folder that holds the config:

```sh
sudo ./burrow service install                    # finds server.yaml, client.yaml or visitor.yaml here
sudo ./burrow service install -c home.yaml       # or name the config file
```

On Windows, open a terminal with "Run as administrator" and run `.\burrow.exe service install`.

The install copies burrow and the config to system folders (see [below](#where-the-files-go)), so you can delete the download folder afterwards. On Linux and macOS, `burrow` then works from any folder, as in the commands below.

<details>
<summary><b>What <code>install</code> does, step by step</b></summary>

- **Checks first.** It checks the config, and that every port it listens on is free. If another program holds a port, it names it (`tcp port 7840 (service "web") is already in use by nginx (pid 812)`), or the burrow service that holds it, and changes nothing.
- **Copies the config** into the service's own folder, readable only by the service. `burrow service status` tells you when the file you installed from has changed since.
- **Copies the binary** to `/usr/local/bin/burrow` (`%ProgramFiles%\burrow\burrow.exe` on Windows), creating the folder if needed. So the service never depends on the folder you downloaded burrow to. If other users can change `/usr/local/bin`, the service gets its own copy in a folder only root can change. Installing again from the same file copies nothing new.
- **Runs the service under its own account,** without login or home folder: `burrow` on Linux, `_burrow` on macOS, the virtual account `NT SERVICE\burrow-<name>` on Windows. `--run-as USER` picks another existing account. On Linux, a port below 1024 gets only the one permission it needs (`CAP_NET_BIND_SERVICE`). On macOS, the service account may open a port below 1024 only on all addresses (a bare port like `https: 443`, or `0.0.0.0`); another address, like `127.0.0.1:443`, needs `--run-as root`.
- **Keeps Let's Encrypt's account and certificates in a private folder** when the config has `web.letsencrypt`: `/var/lib/burrow/<name>` on Linux, `/Library/Application Support/burrow/<name>` on macOS, `%ProgramData%\burrow\services\<name>` on Windows. Only the service account can read it, and `burrow status` names it. `burrow service uninstall` keeps it (and says how to delete it), so a reinstall needs no new certificates; `--purge` deletes it.
- **Writes logs to a file,** rotated at 10 MB with 5 old files kept, so a log never fills the disk.
- **Checks the files the config uses.** Every certificate, key, CA, error-pages folder, DNS command and `file:` secret must be an absolute path the service account can read, since only the config is copied. A website access log (`web.access_log`) must be a file of its own directly in the service's log folder, the only place besides its state folder the service may write: like `/var/log/burrow/<name>/access.log`, or `%ProgramData%\burrow\logs\<name>-access.log` on Windows, where the folder is shared. The install names the path to use.
- **Saves environment variables.** Variables used as `${VAR}` in the config are saved for the service, readable by it only. `sudo` drops environment variables, so keep them with `sudo --preserve-env=HOME_SECRET ./burrow service install`; the install says so if one is missing.

</details>

**The service name** comes from the config file name (`server.yaml` gives `burrow-server`, `home.yaml` gives `burrow-home`), or from `--name`. You can install several servers, clients and visitors side by side, each with its own config file name or `--name`.

| install option | meaning |
| --- | --- |
| `-c FILE` | the config to install; the one in the current folder when left out |
| `--name NAME` | service name, instead of `burrow-<config file name>` |
| `--run-as USER` | run as this existing account instead of a dedicated one |
| `--no-start` | install without starting |
| `--print` | show what would be installed, and change nothing |
| `--admin-writable` | let a server change its own config, for `burrow admin`; on by default when a visitor has admin access |

## Everyday commands

```sh
burrow service list                    # every burrow service on this machine
burrow service status                  # state, version, account, files, the last log lines
sudo burrow service logs -f            # follow the log (-n 200 for the last 200 lines)
sudo burrow service restart            # also: start, stop
sudo burrow service reload             # apply the config now (it is also applied on save)
sudo burrow service uninstall          # remove it; --purge also deletes its config folder and saved variables
sudo burrow status -f                  # live clients and traffic of a server, see monitoring.md
```

With one burrow service installed, the commands find it by themselves. With several, add its name: `burrow service status burrow-home`.

**A burrow unit burrow has no record of**, such as one installed by an older burrow or written by hand, is listed as `not managed by this burrow`, and burrow leaves it alone. `burrow service status <name>` gives the exact commands to take it over: stop it, then install from its config. `install` refuses to replace such a unit while it runs:

```sh
sudo systemctl stop burrow-client
sudo burrow service install -c /etc/burrow/client.yaml
```

## Where the files go

Each service gets its own config folder, named after the service without `burrow-` (`<name>` below). `<role>` is `server`, `client` or `visitor`, and `<ext>` is the config's extension. For example, `server.yaml` installs as `/etc/burrow/server/server.yaml`.

| what | Linux and macOS | Windows |
| --- | --- | --- |
| config | `/etc/burrow/<name>/<role>.<ext>` | `%ProgramData%\burrow\<name>\<role>.<ext>` |
| saved environment variables | `/etc/burrow/<name>.env` | `%ProgramData%\burrow\<name>.env` |
| binary | `/usr/local/bin/burrow` | `%ProgramFiles%\burrow\burrow.exe` |
| log | `/var/log/burrow/<name>/<name>.log` | `%ProgramData%\burrow\logs\<name>.log` |
| systemd unit (Linux) | `/etc/systemd/system/burrow-<name>.service` | |
| launchd job (macOS) | `/Library/LaunchDaemons/com.github.charlesng35.burrow-<name>.plist` | |

**Who may change the config folder:**

- Normally the folder belongs to root (mode 0755), and the service can only read its config.
- A server with a visitor that has admin access, or installed with `--admin-writable`, owns its folder instead (mode 0700), so `burrow admin` can save changes. On Linux, systemd lets the service write to that folder only. `burrow service status` then shows `config: /etc/burrow/server/server.yaml (the service may change it, for burrow admin)`.
- Switching between the two moves nothing: only the folder's owner changes, and the config stays where it is.

`services`, `jobs`, `logs`, `bin` and `instances` are burrow's own folders, so they cannot be service names.

## Changing the config

The installed copy is the config the service runs. Edit it, and the service applies it on save:

```sh
sudo burrow edit                       # pick an installed service's config, edit, check and apply it
sudo nano /etc/burrow/server/server.yaml   # or edit the file directly
```

`burrow edit` refuses a change that the service could not run after a restart, such as a port below 1024 it was not installed for.

**Adding admin access later.** A server installed without an admin visitor cannot write its config, so `burrow admin` is read-only. Its hint gives the exact command: `install the service again so it may change its config; nothing moves: sudo burrow service install -c /etc/burrow/server/server.yaml --admin-writable`. Add the admin visitor to the installed config, then install again from that same file:

```sh
sudo burrow edit burrow-server         # add a visitor with admin: write
sudo burrow service install --admin-writable -c /etc/burrow/server/server.yaml
```

This reinstalls the existing service `burrow-server`: burrow recognizes its installed config, so no `--name` is needed. The folder then belongs to the service, and `burrow admin` can save changes.

<details>
<summary><b>Installing again</b></summary>

- You can install again from the installed copy, as above, or from the original file. An installed config always leads to its own service: `-c /etc/burrow/home/client.yaml` reinstalls `burrow-home`.
- Installing from another file under the same service name is refused; rename the file or pass `--name` to add a second service.
- A reinstall from the original file copies it over the installed one. When the installed copy has changes made with `burrow admin`, it is first kept as a version in the admin history, so you can revert to it.
- A reinstall from the original file also brings back admin access you removed in `burrow admin`, because the original still has it. Remove it there too.

</details>

<details>
<summary><b>Running the same tunnel twice</b></summary>

The same tunnel cannot run twice: a second copy of a server config, or a second client with the same name for the same server, is refused while the first one runs. A service is never kept from starting by a copy started by hand: it starts with a warning, and the server then refuses the duplicate client or the duplicate ports.

</details>

## Updating

```sh
sudo burrow update            # install the latest release and restart the services that use it
burrow update --check         # only tell whether a newer release exists
sudo burrow update --to 0.1.0 # install this release (also an older one)
```

`burrow update` downloads the release for your system, checks it against the release's `SHA256SUMS`, and runs it to confirm its version. It then replaces the installed binary and restarts every service that uses it. If a service does not come up again, the old binary is put back. Without `sudo`, it only replaces the binary you ran.

<details>
<summary><b>More about updating</b></summary>

- The same version is left alone unless you add `--force`.
- `burrow update --check` also points out installed services that still run an older version.
- Downloads go through `HTTPS_PROXY` (or `ALL_PROXY`) when set, and `NO_PROXY` is respected.
- `BURROW_UPDATE_URL` points to a mirror with the same layout as the GitHub releases. It must use `https://`, unless it is on the same machine.
- `sudo ./burrow service update` does the same with a binary you downloaded yourself (`--force` also allows an older one). A binary installed by a package manager is upgraded there instead, and in a container you pull the new image.

</details>

## Linux, macOS and Windows details

<details>
<summary><b>Linux (systemd)</b></summary>

The unit is sandboxed: a read-only system, no home folders, no new privileges, and no capabilities unless a port below 1024 needs one. A config error stops it with exit code 3, instead of restarting it in a loop. `sudo burrow service install --print` shows the unit without installing anything, if you prefer to manage it yourself.

</details>

<details>
<summary><b>macOS (launchd)</b></summary>

launchd restarts the job after a failure, at most once every 10 seconds.

The macOS builds are signed with burrow's own certificate, so the macOS firewall, when it is on, asks once whether burrow may accept incoming connections and keeps that answer across `burrow update`. A file downloaded with a browser is marked as quarantined and macOS refuses to open it at first; allow it under System Settings, Privacy & Security, or remove the mark with `xattr -d com.apple.quarantine burrow`. Downloading with `curl` or updating with `burrow update` sets no such mark.

</details>

<details>
<summary><b>Windows</b></summary>

Run the commands from a terminal opened with "Run as administrator". The service starts automatically after the network, and Windows restarts it 5 seconds after a failure (30 seconds from the third failure in a day). Set variables used by the config in the same terminal before installing, for example `$env:HOME_SECRET = "..."`. `reload` restarts the service on Windows; every role also applies its config on save.

</details>

## Docker

The image `ghcr.io/charlesng35/burrow` is small (distroless and static), runs as a non-root user, and reads its config from `/etc/burrow`. Without a command, it runs `server -c /etc/burrow/server.yaml`.

```sh
docker run --rm --user "$(id -u):$(id -g)" -v "$PWD/burrow:/etc/burrow:ro" \
  ghcr.io/charlesng35/burrow validate -c /etc/burrow/server.yaml
docker run -d --name burrow -p 7835:7835 -p 7840:7840 --user "$(id -u):$(id -g)" \
  -v "$PWD/burrow:/etc/burrow:ro" ghcr.io/charlesng35/burrow
```

- Run it with `--user` matching the owner of your config file (mode `0600`).
- Mount the folder that holds the config, not the file itself. An editor that saves by writing a new file would leave a file mount on the old one, and the change would never apply.
- Publish each service's port with `-p`, and the clients' port 7835.

<details>
<summary><b>Docker Compose: a server and a client</b></summary>

```yaml
# compose.yaml
services:
  burrow-server:
    image: ghcr.io/charlesng35/burrow:latest
    command: ["server", "-c", "/etc/burrow/server.yaml"]
    user: "${UID:-1000}:${GID:-1000}"
    volumes:
      - ./server:/etc/burrow:ro
    ports:
      - "7835:7835"
      - "7840:7840"
    restart: unless-stopped

  burrow-client:
    image: ghcr.io/charlesng35/burrow:latest
    command: ["client", "-c", "/etc/burrow/client.yaml"]
    user: "${UID:-1000}:${GID:-1000}"
    environment:
      HOME_SECRET: ${HOME_SECRET:-}
    volumes:
      - ./client:/etc/burrow:ro
    network_mode: host
    restart: unless-stopped
```

The client uses `network_mode: host` so that `127.0.0.1` in its targets means the host machine, not the container.

</details>
