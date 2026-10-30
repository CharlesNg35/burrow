use std::collections::BTreeMap;
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use burrow_admin_proto::frame::{self, AdminOpen};
use tracing::{debug, warn};

use super::Hub;
use super::core::ports::{Live, LiveSession, Store, Version, WhyNot, WriteError};
use super::core::{Core, Session, text};
use super::transport::{self, Access};
use crate::config::Format;
use crate::config::save::{Refused, write_back};
use crate::config::settings::ServerSettings;
use crate::metrics::Status;
use crate::policy::throttle::Throttle;
use crate::server::Shared;
use crate::server::admin_hook::{AdminHook, Opened, answer};
use crate::wire::protocol::Status as Answer;
use crate::wire::transport::Channel;
use burrow_admin_proto::msg::AdminLevel;

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
                let (nanos, v) = parse_kept(&e.file_name().to_string_lossy())?;
                Some((nanos, v, e.path()))
            })
            .collect();
        out.sort_by_key(|(n, _, _)| std::cmp::Reverse(*n));
        out.into_iter().map(|(_, v, p)| (v, p)).collect()
    }

    fn kept_name(&self, rev: &str, newest: u128) -> String {
        let ext = self
            .path
            .extension()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_else(|| "yaml".into());
        let hex = text::rev_hex(rev).unwrap_or(rev);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        format!("{}-{hex}.{ext}", nanos.max(newest + 1))
    }

    #[cfg(unix)]
    fn write_history(&self, rev: &str, body: &str) -> std::io::Result<()> {
        use std::os::unix::fs::MetadataExt;
        let owner = std::fs::symlink_metadata(&self.path)
            .ok()
            .filter(|_| rustix::process::geteuid().is_root())
            .map(|cfg| (cfg.uid(), cfg.gid()));
        self.write_history_for(rev, body, owner)
    }

    #[cfg(unix)]
    fn write_history_for(
        &self,
        rev: &str,
        body: &str,
        owner: Option<(u32, u32)>,
    ) -> std::io::Result<()> {
        use rustix::fs::{AtFlags, Mode, OFlags};
        let dir = self.dir();
        if let Err(e) = std::fs::create_dir(&dir)
            && e.kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(e);
        }
        let fd = rustix::fs::open(
            &dir,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| std::io::Error::other("the history path is not a folder"))?;
        rustix::fs::fchmod(&fd, Mode::from_bits_truncate(0o700))?;
        if let Some(owner) = owner {
            hand_over(&fd, owner)?;
        }
        let list = |fd: &rustix::fd::OwnedFd| -> std::io::Result<Vec<(u128, Version, String)>> {
            let mut out = Vec::new();
            for entry in rustix::fs::Dir::read_from(fd)? {
                let name = entry?.file_name().to_string_lossy().into_owned();
                if let Some((nanos, v)) = parse_kept(&name) {
                    out.push((nanos, v, name));
                }
            }
            out.sort_by_key(|(n, _, _)| std::cmp::Reverse(*n));
            Ok(out)
        };
        let files = list(&fd)?;
        let name = self.kept_name(rev, files.first().map_or(0, |f| f.0));
        if let Some(pos) = files.iter().position(|(_, v, _)| v.rev == rev) {
            if pos > 0 {
                rustix::fs::renameat(&fd, files[pos].2.as_str(), &fd, name.as_str())?;
            }
            return Ok(());
        }
        let draft = format!(".{name}.burrow-new-{}", std::process::id());
        let _ = rustix::fs::unlinkat(&fd, draft.as_str(), AtFlags::empty());
        let written = (|| -> std::io::Result<()> {
            use std::io::Write;
            let mut file = std::fs::File::from(rustix::fs::openat(
                &fd,
                draft.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_bits_truncate(0o600),
            )?);
            file.write_all(body.as_bytes())?;
            file.sync_all()?;
            if let Some(owner) = owner {
                let _ = hand_over(&file, owner);
            }
            rustix::fs::renameat(&fd, draft.as_str(), &fd, name.as_str())?;
            Ok(())
        })();
        if written.is_err() {
            let _ = rustix::fs::unlinkat(&fd, draft.as_str(), AtFlags::empty());
        }
        written?;
        for (_, _, old) in list(&fd)?.into_iter().skip(KEEP) {
            let _ = rustix::fs::unlinkat(&fd, old.as_str(), AtFlags::empty());
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn write_history(&self, rev: &str, body: &str) -> std::io::Result<()> {
        let dir = self.dir();
        if let Err(e) = std::fs::create_dir(&dir)
            && e.kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(e);
        }
        let files = self.files();
        let newest = files
            .first()
            .and_then(|(_, p)| parse_kept(&p.file_name()?.to_string_lossy()))
            .map_or(0, |(n, _)| n);
        let file = dir.join(self.kept_name(rev, newest));
        if let Some(pos) = files.iter().position(|(v, _)| v.rev == rev) {
            if pos > 0 {
                std::fs::rename(&files[pos].1, &file)?;
            }
            return Ok(());
        }
        crate::base::fs::write_private(&file, body)?;
        for (_, old) in self.files().into_iter().skip(KEEP) {
            let _ = std::fs::remove_file(old);
        }
        Ok(())
    }
}

fn parse_kept(name: &str) -> Option<(u128, Version)> {
    let (at, rest) = name.split_once('-')?;
    let hex = rest.split('.').next()?;
    let nanos: u128 = at.parse().ok()?;
    let rev = text::rev_from_hex(hex)?;
    Some((
        nanos,
        Version {
            rev,
            at: (nanos / 1_000_000_000) as u64,
        },
    ))
}

impl Store for FileStore {
    fn path(&self) -> &Path {
        &self.path
    }

    fn state_dir(&self) -> PathBuf {
        crate::install::state_dir_for(&self.path)
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
            Err(Refused::Io(e)) => Err(WriteError::Io(crate::base::failure::file_reason(&e))),
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
        let file_ok = crate::base::access::can_write(&self.path);
        match made {
            Ok(_) => {
                let _ = std::fs::remove_file(&probe);
                if file_ok {
                    return Ok(());
                }
                Err(WhyNot {
                    code: burrow_admin_proto::codes::FILE_NOT_WRITABLE.into(),
                    message: format!("the server cannot write {}", self.path.display()),
                    hint: format!(
                        "give the account the server runs as write access to {}",
                        self.path.display()
                    ),
                })
            }
            Err(e) => Err(folder_why_not(&self.path, &folder, &e)),
        }
    }

    fn keep(&self, rev: &str, body: &str) {
        if let Err(e) = self.write_history(rev, body) {
            *self.problem.lock().expect("problem lock") = Some(format!(
                "earlier versions are kept in memory only: cannot write {} ({})",
                self.dir().display(),
                crate::base::failure::file_reason(&e)
            ));
            let mut m = self.memory.lock().expect("memory lock");
            m.retain(|(v, _)| v.rev != rev);
            m.insert(
                0,
                (
                    Version {
                        rev: rev.to_string(),
                        at: burrow_syntax::duration::now_secs(),
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

fn probe_bind(addr: SocketAddr) -> std::io::Result<()> {
    crate::wire::transport::tcp::bind_tcp(addr).map(drop)
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
            .map(|s| LiveSession {
                client: s.name,
                secret_fp: s.secret_fp,
                from: s.from,
                over_websocket: s.over_websocket,
            })
            .collect()
    }

    fn allows(&self) -> BTreeMap<String, Vec<String>> {
        self.0
            .client_sessions()
            .into_iter()
            .filter_map(|s| s.allow.map(|a| (s.name, a)))
            .collect()
    }

    fn bind_probe(&self, addr: SocketAddr) -> std::io::Result<()> {
        probe_bind(addr)
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

#[cfg(unix)]
fn hand_over(fd: impl std::os::fd::AsFd, (uid, gid): (u32, u32)) -> std::io::Result<()> {
    let now = rustix::fs::fstat(&fd)?;
    if (now.st_uid, now.st_gid) == (uid, gid) {
        return Ok(());
    }
    #[cfg(test)]
    tests::CHOWNS.with(|c| c.set(c.get() + 1));
    std::os::unix::fs::fchown(fd, Some(uid), Some(gid))
}

pub fn hub(
    shared: &Arc<Shared>,
    path: &Path,
    env: crate::config::env::Env,
) -> (
    Arc<Hub>,
    tokio::sync::mpsc::Receiver<crate::app::reload::Nudge>,
) {
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
    secret: crate::wire::secret::Secret,
    peer: SocketAddr,
    level: AdminLevel,
}

impl Access for SettingsAccess {
    fn revoked(&self) -> Option<String> {
        let settings = self.shared.current_settings();
        match settings.admin_access(&self.visitor, &self.secret, self.peer.ip()) {
            Err(why) => Some(why.into()),
            Ok(level) if level != self.level => Some("admin level changed".into()),
            Ok(_) => None,
        }
    }
}

impl AdminHook for Hub {
    fn serve(
        self: Arc<Self>,
        shared: Arc<Shared>,
        channel: Channel,
        open: AdminOpen,
        opened: Opened,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(serve(self, shared, channel, open, opened))
    }
}

async fn serve(
    hub: Arc<Hub>,
    shared: Arc<Shared>,
    mut channel: Channel,
    open: AdminOpen,
    opened: Opened,
) {
    static REFUSED: Throttle = Throttle::new();
    let Opened {
        visitor,
        secret,
        peer,
        via_websocket,
    } = opened;
    let settings = shared.current_settings();
    let Ok(level) = settings.admin_access(&visitor, &secret, peer.ip()) else {
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
        level,
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

fn folder_why_not(path: &Path, folder: &Path, e: &std::io::Error) -> WhyNot {
    let ro = e.kind() == std::io::ErrorKind::ReadOnlyFilesystem;
    let installed =
        std::env::var_os("INVOCATION_ID").is_some() || crate::install::in_service_folder(path);
    WhyNot {
        code: if ro {
            burrow_admin_proto::codes::READ_ONLY_FS.into()
        } else {
            burrow_admin_proto::codes::FOLDER_NOT_WRITABLE.into()
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
                crate::base::failure::file_reason(e)
            )
        },
        hint: if installed || ro {
            crate::install::writable_hint(path)
        } else {
            format!(
                "give the account the server runs as write access to {}",
                folder.display()
            )
        },
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn the_web_port_probe_never_keeps_the_port() {
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = held.local_addr().unwrap();
        let busy = super::probe_bind(addr).unwrap_err();
        assert_eq!(busy.kind(), std::io::ErrorKind::AddrInUse);
        drop(held);
        super::probe_bind(addr).unwrap();
        std::net::TcpListener::bind(addr).expect("free again right after the probe");
    }

    use super::*;

    #[cfg(unix)]
    thread_local! {
        pub(super) static CHOWNS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }

    #[cfg(unix)]
    #[test]
    fn root_hands_the_history_over_only_when_its_owner_differs() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.yaml");
        std::fs::write(&path, "listen: 1\n").unwrap();
        let mine = std::fs::metadata(&path)
            .map(|m| (m.uid(), m.gid()))
            .unwrap();
        let store = FileStore::new(&path);
        let body = "listen: 2\n";
        CHOWNS.with(|c| c.set(0));
        store
            .write_history_for(&text::rev(body), body, Some(mine))
            .unwrap();
        assert_eq!(CHOWNS.with(|c| c.get()), 0, "nothing to hand over");
        assert_eq!(store.versions().len(), 1);
        let theirs = (mine.0.wrapping_add(1), mine.1);
        let _ = store.write_history_for(&text::rev("listen: 3\n"), "listen: 3\n", Some(theirs));
        assert!(
            CHOWNS.with(|c| c.get()) > 0,
            "a folder of another owner is handed over"
        );
    }

    #[tokio::test]
    async fn a_refusal_to_a_websocket_peer_that_never_closes_gives_up() {
        let (ours, _held) = crate::wire::transport::testing::held_websocket().await;
        let mut ch = crate::wire::transport::testing::sealed(ours);
        tokio::time::timeout(
            crate::forward::ANSWER_TIMEOUT + std::time::Duration::from_secs(3),
            answer(&mut ch, &[&[Answer::Forbidden as u8]]),
        )
        .await
        .expect("the refusal gives up on a peer that holds the socket");
    }

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
        let names: Vec<String> = std::fs::read_dir(store.dir())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), KEEP, "no drafts are left behind: {names:?}");
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

    #[test]
    fn a_read_only_filesystem_is_told_apart_from_a_folder_without_rights() {
        let (path, folder) = (Path::new("/srv/server.yaml"), Path::new("/srv"));
        let why = |kind: std::io::ErrorKind| folder_why_not(path, folder, &kind.into()).code;
        assert_eq!(
            why(std::io::ErrorKind::ReadOnlyFilesystem),
            burrow_admin_proto::codes::READ_ONLY_FS
        );
        assert_eq!(
            why(std::io::ErrorKind::PermissionDenied),
            burrow_admin_proto::codes::FOLDER_NOT_WRITABLE
        );
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
        assert_eq!(why.code, burrow_admin_proto::codes::FOLDER_NOT_WRITABLE);
        assert!(problem.unwrap().contains("in memory only"));
        assert_eq!(
            kept.as_deref(),
            Some("listen: 1\n"),
            "kept in memory instead"
        );
    }
}
