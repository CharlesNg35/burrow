use std::path::{Path, PathBuf};

use burrow_admin_proto::msg::{ErrorCode, Upload, Uploaded};
use burrow_config::ServiceWeb;
use zeroize::Zeroizing;

use super::{ApiError, Core, Session};
use crate::server::uploads::{self as files, MAX_KEPT};

fn saved_config(core: &Core) -> Option<burrow_config::ServerConfig> {
    let text = core.read().ok()?;
    let tree = crate::config::tree::parse(&text).ok()?;
    serde_json::from_value(tree.to_json()).ok()
}

fn drafts(core: &Core, state: &Path) -> usize {
    let ids = saved_config(core)
        .map(|cfg| burrow_config::uploads::upload_ids(&cfg))
        .unwrap_or_default();
    let keep = files::referenced(ids, core.store.history_dir().as_deref());
    files::drafts(state, &keep)
}

fn saved_ca(core: &Core, service: &str) -> Option<PathBuf> {
    let cfg = saved_config(core)?;
    let Some(ServiceWeb::Site(site)) = cfg.services.get(service)?.web.as_ref() else {
        return None;
    };
    let c = site.client_auth.as_ref()?;
    let dir = core.store.path().parent().unwrap_or(Path::new("."));
    match (&c.ca, &c.ca_upload) {
        (Some(p), _) => Some(dir.join(p.trim())),
        (None, Some(id)) => files::file(&core.store.state_dir(), id.trim()),
        (None, None) => None,
    }
}

fn cas(core: &Core, req: &Upload) -> Result<Zeroizing<Vec<u8>>, ApiError> {
    let bad = |m: String| ApiError::new(ErrorCode::Invalid, m);
    let file = match (&req.ca_upload, &req.ca_of) {
        (Some(id), _) => files::file(&core.store.state_dir(), id.trim())
            .ok_or_else(|| bad(format!("\"{id}\" is not an upload burrow made")))?,
        (None, Some(service)) => saved_ca(core, service).ok_or_else(|| {
            bad(format!(
                "service \"{service}\" has no device CA yet; set the CA first"
            ))
        })?,
        (None, None) => return Ok(Zeroizing::new(Vec::new())),
    };
    std::fs::read(&file).map(Zeroizing::new).map_err(|e| {
        bad(format!(
            "cannot read the CA: {}",
            crate::base::failure::file_reason(&e)
        ))
    })
}

pub fn upload(core: &Core, s: &Session, req: &Upload) -> Result<Uploaded, ApiError> {
    if !s.writes() {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "read access cannot upload files",
        ));
    }
    let Some(kind) = burrow_syntax::upload::Kind::parse(&req.kind) else {
        return Err(ApiError::new(
            ErrorCode::BadRequest,
            format!("unknown upload kind \"{}\"", req.kind),
        ));
    };
    if let Some(why) = crate::install::missing_state_folder(core.store.path()) {
        return Err(ApiError::new(ErrorCode::Invalid, why));
    }
    let state = core.store.state_dir();
    if drafts(core, &state) >= MAX_KEPT {
        return Err(ApiError::new(
            ErrorCode::Busy,
            format!(
                "burrow keeps at most {MAX_KEPT} uploaded files that neither the config nor a version in Activity uses; apply your changes first: then burrow removes those that are older than {} minutes",
                burrow_syntax::upload::DRAFT_SECS / 60
            ),
        ));
    }
    let cas = if kind == burrow_syntax::upload::Kind::Crl {
        cas(core, req)?
    } else {
        Zeroizing::new(Vec::new())
    };
    let r = files::Request {
        kind,
        pem: req.pem.as_bytes(),
        hosts: &req.hosts,
        cas: &cas,
    };
    files::store(&state, &r).map_err(|e| ApiError::new(ErrorCode::Invalid, e))
}

pub fn local_ca(core: &Core) -> Result<serde_json::Value, ApiError> {
    if let Some(why) = crate::install::missing_state_folder(core.store.path()) {
        return Err(ApiError::new(ErrorCode::Invalid, why));
    }
    let path = files::root_pem(&core.store.state_dir());
    match std::fs::read_to_string(&path) {
        Ok(pem) => Ok(serde_json::json!({ "pem": pem })),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ApiError::new(
            ErrorCode::Invalid,
            "there is no local CA yet; burrow makes it for the first site with tls: internal",
        )),
        Err(e) => Err(ApiError::new(
            ErrorCode::Internal,
            format!(
                "cannot read {}: {}",
                path.display(),
                crate::base::failure::file_reason(&e)
            ),
        )),
    }
}
