use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use burrow_admin_proto::frame::{self, AdminOpen};
use burrow_admin_proto::msg::Level;
use tracing::{debug, warn};

use super::core::ports::{Live, LiveSession, Store, Version, WhyNot, WriteError};
use super::core::{Core, Session, text};
use super::transport::{self, Access};
use super::{Hub, Opened};
use crate::config::Format;
use crate::config::model::AdminLevel;
use crate::config::settings::ServerSettings;
use crate::edit::save::{Refused, write_back};
use crate::handshake::Channel;
use crate::metrics::Status;
use crate::protocol::Status as Answer;
use crate::server::Shared;
use crate::throttle::Throttle;

const KEEP: usize = 20;

pub struct FileStore {
    path: PathBuf,
    format: Format,
    memory: Mutex<Vec<(Version, String)>>,
    problem: Mutex<Option<String>>,
}

pub fn history_dir_of(config: &Path) -> PathBuf {
    let name = config
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config".into());
    config
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
        .join(format!(".{name}.history"))
}

impl FileStore {
    pub fn new(path: &Path) -> FileStore {
        FileStore {
            path: path.to_path_buf(),
            format: Format::from_path(path).unwrap_or(Format::Yaml),
            memory: Mutex::new(Vec::new()),
            problem: Mutex::new(None),
        }
    }

    fn dir(&self) -> PathBuf {
        history_dir_of(&self.path)
    }

    fn folder(&self) -> PathBuf {
        self.path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    }

    fn files(&self) -> Vec<(Version, PathBuf)> {
        let Ok(entries) = std::fs::read_dir(self.dir()) else {
            return Vec::new();
        };
        let mut out: Vec<(u128, Version, PathBuf)> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let (at, rest) = name.split_once('-')?;
                let hex = rest.split('.').next()?;
                let nanos: u128 = at.parse().ok()?;
                (hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then(|| {
                    (
                        nanos,
                        Version {
                            rev: format!("b2s:{hex}"),
                            at: (nanos / 1_000_000_000) as u64,
                        },
                        e.path(),
                    )
                })
            })
            .collect();
        out.sort_by_key(|(n, _, _)| std::cmp::Reverse(*n));
        out.into_iter().map(|(_, v, p)| (v, p)).collect()
    }

    fn write_history(&self, rev: &str, body: &str) -> std::io::Result<()> {
        let dir = self.dir();
        if let Err(e) = std::fs::create_dir(&dir)
            && e.kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(e);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let meta = std::fs::symlink_metadata(&dir)?;
            if !meta.is_dir() {
                return Err(std::io::Error::other("the history path is not a folder"));
            }
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
            if let Ok(cfg) = std::fs::metadata(&self.path)
                && (cfg.uid(), cfg.gid()) != (meta.uid(), meta.gid())
                && rustix::process::geteuid().is_root()
            {
                std::os::unix::fs::chown(&dir, Some(cfg.uid()), Some(cfg.gid()))?;
            }
        }
        let ext = self
            .path
            .extension()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_else(|| "yaml".into());
        let hex = rev.trim_start_matches("b2s:");
        let files = self.files();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let newest = files
            .first()
            .and_then(|(_, p)| {
                p.file_name()?
                    .to_str()?
                    .split('-')
                    .next()?
                    .parse::<u128>()
                    .ok()
            })
            .unwrap_or(0);
        let file = dir.join(format!("{}-{hex}.{ext}", nanos.max(newest + 1)));
        if let Some(pos) = files.iter().position(|(v, _)| v.rev == rev) {
            if pos > 0 {
                std::fs::rename(&files[pos].1, &file)?;
            }
            return Ok(());
        }
        crate::instance::write_private(&file, body)?;
        #[cfg(unix)]
        if rustix::process::geteuid().is_root()
            && let Ok(cfg) = std::fs::metadata(&self.path)
        {
            use std::os::unix::fs::MetadataExt;
            let _ = std::os::unix::fs::chown(&file, Some(cfg.uid()), Some(cfg.gid()));
        }
        for (_, old) in self.files().into_iter().skip(KEEP) {
            let _ = std::fs::remove_file(old);
        }
        Ok(())
    }
}

impl Store for FileStore {
    fn path(&self) -> &Path {
        &self.path
    }

    fn format(&self) -> Format {
        self.format
    }

    fn read(&self) -> std::io::Result<String> {
        std::fs::read_to_string(&self.path)
    }

    fn write(&self, before: &str, after: &str) -> Result<(), WriteError> {
        match write_back(&self.path, before, after) {
            Ok(()) => Ok(()),
            Err(Refused::Changed) => Err(WriteError::Changed),
            Err(Refused::Io(e)) => Err(WriteError::Io(crate::failure::file_reason(&e))),
        }
    }

    fn writable(&self) -> Result<(), WhyNot> {
        let folder = self.folder();
        let probe = folder.join(format!(
            ".{}.burrow-probe-{}",
            self.path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            std::process::id()
        ));
        let made = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe);
        let file_ok = crate::edit::access::can_write(&self.path);
        match made {
            Ok(_) => {
                let _ = std::fs::remove_file(&probe);
                if file_ok {
                    return Ok(());
                }
                Err(WhyNot {
                    code: "file-not-writable".into(),
                    message: format!("the server cannot write {}", self.path.display()),
                    hint: format!(
                        "give the account the server runs as write access to {}",
                        self.path.display()
                    ),
                })
            }
            Err(e) => {
                let ro = e.raw_os_error() == Some(30);
                let installed = std::env::var_os("INVOCATION_ID").is_some();
                Err(WhyNot {
                    code: if ro {
                        "read-only-fs".into()
                    } else {
                        "folder-not-writable".into()
                    },
                    message: if ro && installed {
                        format!(
                            "the service sandbox keeps {} read-only for the server",
                            folder.display()
                        )
                    } else {
                        format!(
                            "the server cannot create files in {} ({})",
                            folder.display(),
                            crate::failure::file_reason(&e)
                        )
                    },
                    hint: if installed || ro {
                        "install the service again so it may change its config: sudo burrow service install --admin-writable -c SERVER_CONFIG (this is the default when a visitor has admin access)".into()
                    } else {
                        format!(
                            "give the account the server runs as write access to {}",
                            folder.display()
                        )
                    },
                })
            }
        }
    }

    fn keep(&self, rev: &str, body: &str) {
        if let Err(e) = self.write_history(rev, body) {
            *self.problem.lock().expect("problem lock") = Some(format!(
                "earlier versions are kept in memory only: cannot write {} ({})",
                self.dir().display(),
                crate::failure::file_reason(&e)
            ));
            let mut m = self.memory.lock().expect("memory lock");
            m.retain(|(v, _)| v.rev != rev);
            m.insert(
                0,
                (
                    Version {
                        rev: rev.to_string(),
                        at: text::now_unix(),
                    },
                    body.to_string(),
                ),
            );
            m.truncate(KEEP);
        }
    }

    fn versions(&self) -> Vec<Version> {
        let mut out: Vec<Version> = self.files().into_iter().map(|(v, _)| v).collect();
        for (v, _) in self.memory.lock().expect("memory lock").iter() {
            if !out.iter().any(|o| o.rev == v.rev) {
                out.push(v.clone());
            }
        }
        out.sort_by_key(|v| std::cmp::Reverse(v.at));
        out
    }

    fn version(&self, rev: &str) -> Option<String> {
        if let Some((_, t)) = self
            .memory
            .lock()
            .expect("memory lock")
            .iter()
            .find(|(v, _)| v.rev == rev)
        {
            return Some(t.clone());
        }
        let (_, path) = self.files().into_iter().find(|(v, _)| v.rev == rev)?;
        let body = std::fs::read_to_string(path).ok()?;
        (text::rev(&body) == rev).then_some(body)
    }

    fn history_dir(&self) -> Option<PathBuf> {
        Some(self.dir())
    }

    fn history_problem(&self) -> Option<String> {
        self.problem.lock().expect("problem lock").clone()
    }
}

pub struct ServerLive(pub Arc<Shared>);

impl Live for ServerLive {
    fn status(&self) -> Status {
        self.0.status()
    }

    fn settings(&self) -> Arc<ServerSettings> {
        self.0.current_settings()
    }

    fn sessions(&self) -> Vec<LiveSession> {
        self.0
            .client_sessions()
            .into_iter()
            .map(|(client, secret_fp, from, _)| LiveSession {
                client,
                secret_fp,
                from,
            })
            .collect()
    }

    fn allows(&self) -> BTreeMap<String, Vec<String>> {
        self.0
            .client_sessions()
            .into_iter()
            .filter_map(|(c, _, _, allow)| allow.map(|a| (c, a)))
            .collect()
    }

    fn free_port(&self, near: u16, count: u16, udp: bool) -> Option<u16> {
        let mut p = near.max(1024);
        while p.checked_add(count)?.checked_sub(1).is_some() {
            let all = (0..count).all(|i| {
                let addr = SocketAddr::from(([0, 0, 0, 0], p + i));
                if udp {
                    std::net::UdpSocket::bind(addr).is_ok()
                } else {
                    std::net::TcpListener::bind(addr).is_ok()
                }
            });
            if all {
                return Some(p);
            }
            p = p.checked_add(count)?;
        }
        None
    }
}

pub fn hub(
    shared: &Arc<Shared>,
    path: &Path,
    env: crate::run::Env,
) -> (Arc<Hub>, tokio::sync::mpsc::Receiver<crate::reload::Nudge>) {
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let core = Core::new(
        Box::new(FileStore::new(path)),
        Box::new(ServerLive(shared.clone())),
        Box::new(env),
    );
    (Hub::new(core, tx), rx)
}

struct SettingsAccess {
    shared: Arc<Shared>,
    visitor: String,
    secret: crate::secret::Secret,
    peer: SocketAddr,
    level: AdminLevel,
}

impl Access for SettingsAccess {
    fn revoked(&self) -> Option<String> {
        let settings = self.shared.current_settings();
        let Some(v) = settings.visitors.get(&self.visitor) else {
            return Some("visitor removed".into());
        };
        if !v.secrets.contains(&self.secret) {
            return Some("secret removed".into());
        }
        if v.admin.is_none() {
            return Some("admin removed".into());
        }
        if v.admin != Some(self.level) {
            return Some("admin level changed".into());
        }
        if !v.admits_admin(self.peer.ip()) {
            return Some("source not allowed".into());
        }
        None
    }
}

async fn answer(ch: &mut Channel, records: &[&[u8]]) {
    use tokio::io::AsyncWriteExt;
    for r in records {
        if ch.writer.send(&mut ch.stream, r).await.is_err() {
            return;
        }
    }
    let _ = ch.writer.send(&mut ch.stream, &[]).await;
    let _ = ch.stream.shutdown().await;
}

pub async fn serve(shared: Arc<Shared>, mut channel: Channel, first: &[u8], opened: Opened) {
    static REFUSED: Throttle = Throttle::new();
    let Opened {
        visitor,
        secret,
        peer,
        via_websocket,
    } = opened;
    let Ok(open) = AdminOpen::decode(first) else {
        answer(&mut channel, &[&[Answer::Invalid as u8]]).await;
        return;
    };
    let Some(hub) = shared.admin() else {
        answer(
            &mut channel,
            &[
                &[Answer::Invalid as u8],
                b"this server runs without a config file, so it has nothing to administer",
            ],
        )
        .await;
        return;
    };
    let settings = shared.current_settings();
    let level = settings
        .visitors
        .get(&visitor)
        .filter(|v| v.secrets.contains(&secret) && v.admits_admin(peer.ip()))
        .and_then(|v| v.admin);
    let Some(level) = level else {
        if let Some(more) = REFUSED.allow() {
            warn!(
                "visitor \"{visitor}\" from {} asked for admin access, which it does not have{more}",
                peer.ip()
            );
        }
        answer(&mut channel, &[&[Answer::Forbidden as u8]]).await;
        return;
    };
    let Some(seat) = hub.seat(&visitor) else {
        debug!("visitor \"{visitor}\" asked for an admin session over the limit");
        answer(&mut channel, &[&[Answer::Busy as u8]]).await;
        return;
    };
    let (lo, hi) = (
        burrow_admin_proto::PROTOCOL_MIN,
        burrow_admin_proto::PROTOCOL_MAX,
    );
    let version = open.max.min(hi);
    if version < open.min.max(lo) || open.min > open.max {
        let text = format!(
            "this server speaks admin protocol versions {lo} to {hi}, the caller {} to {}; update burrow on the side with the older version",
            open.min, open.max
        );
        answer(&mut channel, &[&[Answer::Invalid as u8], text.as_bytes()]).await;
        return;
    }
    if channel
        .writer
        .send(&mut channel.stream, &frame::accepted(version))
        .await
        .is_err()
    {
        return;
    }
    let client: String = open
        .client
        .chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect();
    debug!(
        "visitor \"{visitor}\" from {} opened an admin session ({client})",
        peer.ip()
    );
    let session = Session::new(
        visitor.clone(),
        match level {
            AdminLevel::Read => Level::Read,
            AdminLevel::Write => Level::Write,
        },
        peer,
        secret.clone(),
        via_websocket,
        client,
    );
    let access = Box::new(SettingsAccess {
        shared: shared.clone(),
        visitor,
        secret,
        peer,
        level,
    });
    transport::run(
        hub,
        seat,
        channel,
        session,
        access,
        shared.reloads(),
        shared.applies(),
        shared.shutdown_signal(),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_kept_again_counts_as_new_and_is_not_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.yaml");
        std::fs::write(&path, "listen: 1\n").unwrap();
        let store = FileStore::new(&path);
        let base = "listen: 1\n".to_string();
        let base_rev = text::rev(&base);
        for i in 0..(KEEP * 2) {
            store.keep(&base_rev, &base);
            let body = format!("listen: {}\n", 100 + i);
            store.keep(&text::rev(&body), &body);
        }
        assert_eq!(
            store.version(&base_rev).as_deref(),
            Some(base.as_str()),
            "the version an apply and revert cycle returns to stays in the history"
        );
        assert_eq!(store.versions().len(), KEEP);
        assert_eq!(store.versions()[1].rev, base_rev);
    }

    #[test]
    fn the_history_keeps_twenty_private_versions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.yaml");
        std::fs::write(&path, "listen: 1\n").unwrap();
        let store = FileStore::new(&path);
        assert!(store.writable().is_ok());
        let mut revs = Vec::new();
        for i in 0..25 {
            let body = format!("listen: {i}\n");
            let rev = text::rev(&body);
            store.keep(&rev, &body);
            store.keep(&rev, &body);
            revs.push((rev, body));
        }
        assert!(store.history_problem().is_none());
        let versions = store.versions();
        assert_eq!(versions.len(), KEEP);
        let (rev, body) = &revs[24];
        assert_eq!(store.version(rev).as_deref(), Some(body.as_str()));
        for (i, (rev, body)) in revs.iter().enumerate() {
            let kept = store.version(rev);
            if i < 25 - KEEP {
                assert!(
                    kept.is_none(),
                    "version {i} is among the oldest and deleted"
                );
            } else {
                assert_eq!(kept.as_deref(), Some(body.as_str()), "version {i} is kept");
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir_mode = std::fs::metadata(store.dir()).unwrap().permissions().mode();
            assert_eq!(dir_mode & 0o777, 0o700);
            for (_, file) in store.files() {
                let mode = std::fs::metadata(file).unwrap().permissions().mode();
                assert_eq!(mode & 0o777, 0o600);
            }
        }
    }

    #[test]
    fn a_changed_file_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.yaml");
        std::fs::write(&path, "listen: 1\n").unwrap();
        let store = FileStore::new(&path);
        assert_eq!(
            store.write("listen: 2\n", "listen: 3\n"),
            Err(WriteError::Changed)
        );
        assert_eq!(store.write("listen: 1\n", "listen: 3\n"), Ok(()));
        assert_eq!(store.read().unwrap(), "listen: 3\n");
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_the_server_cannot_write_is_reported_before_any_change() {
        use std::os::unix::fs::PermissionsExt;
        if rustix::process::geteuid().is_root() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("etc");
        std::fs::create_dir(&inner).unwrap();
        let path = inner.join("server.yaml");
        std::fs::write(&path, "listen: 1\n").unwrap();
        std::fs::set_permissions(&inner, std::fs::Permissions::from_mode(0o500)).unwrap();
        let store = FileStore::new(&path);
        let why = store.writable().err();
        store.keep(&text::rev("listen: 1\n"), "listen: 1\n");
        let problem = store.history_problem();
        let kept = store.version(&text::rev("listen: 1\n"));
        std::fs::set_permissions(&inner, std::fs::Permissions::from_mode(0o700)).unwrap();
        let why = why.expect("not writable");
        assert_eq!(why.code, "folder-not-writable");
        assert!(problem.unwrap().contains("in memory only"));
        assert_eq!(
            kept.as_deref(),
            Some("listen: 1\n"),
            "kept in memory instead"
        );
    }
}
