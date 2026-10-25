use std::path::{Path, PathBuf};

use burrow_admin_proto::msg::{ErrorCode, Upload, Uploaded};
use burrow_config::ServiceWeb;
use zeroize::Zeroizing;

use super::{ApiError, Core, Session};
use crate::server::uploads::{self as files, MAX_KEPT};

fn saved_ca(core: &Core, service: &str) -> Option<PathBuf> {
    let text = core.read().ok()?;
    let tree = crate::config::tree::parse(&text).ok()?;
    let cfg: burrow_config::ServerConfig = serde_json::from_value(tree.to_json()).ok()?;
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
    if !burrow_syntax::upload::KINDS.contains(&req.kind.as_str()) {
        return Err(ApiError::new(
            ErrorCode::BadRequest,
            format!("unknown upload kind \"{}\"", req.kind),
        ));
    }
    let state = core.store.state_dir();
    if files::kept(&state) >= MAX_KEPT {
        return Err(ApiError::new(
            ErrorCode::Busy,
            format!(
                "burrow keeps at most {MAX_KEPT} uploaded files; apply your changes first, which removes the unused ones"
            ),
        ));
    }
    let needs_cas = req.kind == burrow_syntax::upload::KINDS[2];
    let cas = if needs_cas {
        cas(core, req)?
    } else {
        Zeroizing::new(Vec::new())
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let r = files::Request {
        kind: &req.kind,
        pem: req.pem.as_bytes(),
        hosts: &req.hosts,
        cas: &cas,
    };
    files::store(&state, &r, now).map_err(|e| ApiError::new(ErrorCode::Invalid, e))
}

pub fn local_ca(core: &Core) -> Result<serde_json::Value, ApiError> {
    match std::fs::read_to_string(files::root_pem(&core.store.state_dir())) {
        Ok(pem) => Ok(serde_json::json!({ "pem": pem })),
        Err(_) => Err(ApiError::new(
            ErrorCode::Invalid,
            "there is no local CA yet; burrow makes it for the first site with tls: internal",
        )),
    }
}
