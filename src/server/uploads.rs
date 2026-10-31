use std::path::{Path, PathBuf};

use burrow_admin_proto::msg::Uploaded;

pub const MAX_KEPT: usize = 64;

pub struct Request<'a> {
    pub kind: burrow_syntax::upload::Kind,
    pub pem: &'a [u8],
    pub hosts: &'a [String],
    pub cas: &'a [u8],
}

pub fn prune_unused(web: Option<&crate::config::settings::WebSettings>) -> usize {
    let Some(web) = web else {
        return 0;
    };
    let Some(state) = web
        .state_dir
        .as_deref()
        .filter(|s| crate::install::prunes_uploads_in(s))
    else {
        return 0;
    };
    let keep = referenced(web.uploads.clone(), web.history.as_deref());
    let removed = remove_unreferenced(state, &keep);
    if removed > 0 {
        tracing::info!("removed {removed} uploaded files the config no longer uses");
    }
    removed
}

pub fn referenced(mut config_ids: Vec<String>, history: Option<&Path>) -> Vec<String> {
    config_ids.extend(
        history
            .map(crate::config::save::history_upload_ids)
            .unwrap_or_default(),
    );
    config_ids
}

pub fn drafts(state: &Path, referenced: &[String]) -> usize {
    let used = |name: &str| {
        name.strip_suffix(".pem")
            .is_some_and(|id| referenced.iter().any(|r| r == id))
    };
    std::fs::read_dir(folder(state)).map_or(0, |d| {
        d.flatten()
            .filter(|e| !used(&e.file_name().to_string_lossy()))
            .count()
    })
}

#[cfg(feature = "web")]
fn remove_unreferenced(state: &Path, keep: &[String]) -> usize {
    burrow_web::tls::uploads::prune(state, keep, burrow_syntax::duration::now_secs())
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
        burrow_syntax::duration::now_secs(),
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

    fn old_upload(state: &Path, id: &str) {
        let dir = folder(state);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join(format!("{id}.pem"));
        std::fs::write(&f, "x").unwrap();
        let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&f)
            .unwrap()
            .set_modified(hour_ago)
            .unwrap();
    }

    #[test]
    fn an_apply_removes_old_uploads_no_config_uses_so_new_ones_fit_again() {
        let state = tempfile::tempdir().unwrap();
        let dir = folder(state.path());
        let ids: Vec<String> = (0..MAX_KEPT).map(|i| format!("pair-{i:016x}")).collect();
        for id in &ids {
            old_upload(state.path(), id);
        }
        std::fs::write(dir.join("ca-00000000000000ff.pem"), "fresh draft").unwrap();
        assert!(
            drafts(state.path(), &[]) > MAX_KEPT,
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
        assert!(drafts(state.path(), &[]) < MAX_KEPT, "the next upload fits");
    }

    #[test]
    fn an_upload_only_a_kept_admin_version_names_survives_the_prune_so_a_revert_finds_it() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let config = dir.path().join("server.yaml");
        let secret = crate::wire::secret::Secret::generate().encode();
        std::fs::write(
            &config,
            format!(
                "listen: 7835\nclients:\n  home: {{ secret: {secret} }}\nweb:\n  https: 443\nservices:\n  nas: {{ client: home, target: 443, web: {{ hosts: [nas.example.com], tls: passthrough }} }}\n"
            ),
        )
        .unwrap();
        let history = crate::config::save::history_dir_of(&config);
        std::fs::create_dir_all(&history).unwrap();
        std::fs::write(
            history.join("1000-00ab.yaml"),
            "web:\n  certificates:\n    - hosts: [a.example.com]\n      upload: \"pair-00000000000000aa\"\n  letsencrypt: {ca_upload: ca-00000000000000ca}\n",
        )
        .unwrap();
        for id in [
            "pair-00000000000000aa",
            "ca-00000000000000ca",
            "pair-00000000000000bb",
        ] {
            old_upload(&state, id);
        }
        let env = crate::config::env::ProcessEnv;
        let outcome =
            crate::config::load_file(&config, Some(crate::config::Role::Server), &env).unwrap();
        let Some(crate::config::Loaded::Server(settings, _)) = outcome.loaded else {
            panic!("{:?}", outcome.report);
        };
        let mut web = settings.web.unwrap();
        web.state_dir = Some(state.clone());
        assert_eq!(prune_unused(Some(&web)), 1);
        assert!(file(&state, "pair-00000000000000aa").unwrap().exists());
        assert!(file(&state, "ca-00000000000000ca").unwrap().exists());
        assert!(!file(&state, "pair-00000000000000bb").unwrap().exists());
    }
}
