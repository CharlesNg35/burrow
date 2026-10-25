use std::path::{Path, PathBuf};

use burrow_admin_proto::msg::Uploaded;

pub const MAX_KEPT: usize = 64;

pub struct Request<'a> {
    pub kind: burrow_syntax::upload::Kind,
    pub pem: &'a [u8],
    pub hosts: &'a [String],
    pub cas: &'a [u8],
}

pub fn kept(state: &Path) -> usize {
    std::fs::read_dir(folder(state)).map_or(0, |d| d.flatten().count())
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
pub fn store(state: &Path, req: &Request<'_>, now: u64) -> Result<Uploaded, String> {
    use burrow_web::tls::uploads;
    let up = uploads::Upload {
        kind: req.kind,
        pem: req.pem,
        hosts: req.hosts,
        cas: req.cas,
    };
    let stored = uploads::store(state, &up, now, &super::private_write)
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
pub fn file(_: &Path, _: &str) -> Option<PathBuf> {
    None
}

#[cfg(not(feature = "web"))]
pub fn root_pem(state: &Path) -> PathBuf {
    state.join("ca")
}

#[cfg(not(feature = "web"))]
pub fn store(_: &Path, _: &Request<'_>, _: u64) -> Result<Uploaded, String> {
    Err("this build of burrow has no web proxy".into())
}
