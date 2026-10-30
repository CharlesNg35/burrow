use std::path::{Path, PathBuf};

use burrow_admin_proto::msg::Uploaded;

pub const MAX_KEPT: usize = 64;

pub struct Request<'a> {
    pub kind: burrow_syntax::upload::Kind,
    pub pem: &'a [u8],
    pub hosts: &'a [String],
    pub cas: &'a [u8],
}

pub fn prune_unused(
    settings: &crate::config::settings::ServerSettings,
    cfg: &burrow_config::ServerConfig,
) -> usize {
    let Some(state) = settings.web.as_ref().and_then(|w| w.state_dir.clone()) else {
        return 0;
    };
    let removed = remove_unreferenced(&state, &burrow_config::uploads::upload_ids(cfg));
    if removed > 0 {
        tracing::info!("removed {removed} uploaded files the config no longer uses");
    }
    removed
}

pub fn kept(state: &Path) -> usize {
    std::fs::read_dir(folder(state)).map_or(0, |d| d.flatten().count())
}

#[cfg(feature = "web")]
fn remove_unreferenced(state: &Path, keep: &[String]) -> usize {
    burrow_web::tls::uploads::prune(state, keep, burrow_web::tls::now_secs())
}

#[cfg(feature = "web")]
pub fn folder(state: &Path) -> PathBuf {
    burrow_web::tls::uploads::folder(state)
}

#[cfg(feature = "web")]
pub fn file(state: &Path, id: &str) -> Option<PathBuf> {
    burrow_web::tls::uploads::path(state, id)
}

#[cfg(feature = "web")]
pub fn root_pem(state: &Path) -> PathBuf {
    burrow_web::tls::internal::root_pem_path(state)
}

#[cfg(feature = "web")]
pub fn store(state: &Path, req: &Request<'_>) -> Result<Uploaded, String> {
    use burrow_web::tls::uploads;
    let up = uploads::Upload {
        kind: req.kind,
        pem: req.pem,
        hosts: req.hosts,
        cas: req.cas,
    };
    let stored = uploads::store(
        state,
        &up,
        burrow_web::tls::now_secs(),
        &super::private_write,
    )
    .map_err(|e| format!("the file {e}"))?;
    Ok(Uploaded {
        id: stored.id,
        warnings: stored.warnings,
        names: stored.described.names,
        issuer: stored.described.issuer,
        not_after: stored.described.not_after,
        sha256: stored.described.sha256,
    })
}

#[cfg(not(feature = "web"))]
pub fn folder(state: &Path) -> PathBuf {
    state.join("files")
}

#[cfg(not(feature = "web"))]
fn remove_unreferenced(_: &Path, _: &[String]) -> usize {
    0
}

#[cfg(not(feature = "web"))]
pub fn file(_: &Path, _: &str) -> Option<PathBuf> {
    None
}

#[cfg(not(feature = "web"))]
pub fn root_pem(state: &Path) -> PathBuf {
    state.join("ca")
}

#[cfg(not(feature = "web"))]
pub fn store(_: &Path, _: &Request<'_>) -> Result<Uploaded, String> {
    Err("this build of burrow has no web proxy".into())
}

#[cfg(all(test, feature = "web"))]
mod tests {
    use super::*;

    #[test]
    fn an_apply_removes_old_uploads_no_config_uses_so_new_ones_fit_again() {
        let state = tempfile::tempdir().unwrap();
        let dir = folder(state.path());
        std::fs::create_dir_all(&dir).unwrap();
        let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        let ids: Vec<String> = (0..MAX_KEPT).map(|i| format!("pair-{i:016x}")).collect();
        for id in &ids {
            let f = dir.join(format!("{id}.pem"));
            std::fs::write(&f, "x").unwrap();
            std::fs::File::options()
                .write(true)
                .open(&f)
                .unwrap()
                .set_modified(hour_ago)
                .unwrap();
        }
        std::fs::write(dir.join("ca-00000000000000ff.pem"), "fresh draft").unwrap();
        assert!(
            kept(state.path()) > MAX_KEPT,
            "the admin would refuse new uploads"
        );
        let cfg: burrow_config::ServerConfig = serde_json::from_value(serde_json::json!({
            "web": {"https": 443, "certificates": [{"hosts": ["a.example.com"], "upload": ids[7]}]}
        }))
        .unwrap();
        assert_eq!(
            burrow_config::uploads::upload_ids(&cfg),
            vec![ids[7].clone()]
        );
        assert_eq!(
            remove_unreferenced(state.path(), &burrow_config::uploads::upload_ids(&cfg)),
            MAX_KEPT - 1
        );
        assert!(
            file(state.path(), &ids[7]).unwrap().exists(),
            "the file in use stays"
        );
        assert!(
            dir.join("ca-00000000000000ff.pem").exists(),
            "a draft younger than 30 min stays"
        );
        assert!(kept(state.path()) < MAX_KEPT, "the next upload fits");
    }
}
