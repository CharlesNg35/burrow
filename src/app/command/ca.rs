use std::io::Write;
use std::path::{Path, PathBuf};

use crate::app::cli::CaExportArgs;
use crate::base::failure::Failure;

fn state_of(a: &CaExportArgs) -> Result<PathBuf, Failure> {
    if let Some(dir) = &a.state_dir {
        return Ok(dir.clone());
    }
    if let Some(config) = &a.config {
        return Ok(crate::install::state_dir_for(config));
    }
    let servers: Vec<_> = crate::install::all_manifests()
        .into_iter()
        .filter(|m| m.role == "server")
        .filter(|m| {
            a.name
                .as_deref()
                .is_none_or(|n| n == m.name || n == m.short())
        })
        .collect();
    match servers.as_slice() {
        [m] => Ok(m
            .web_state
            .clone()
            .unwrap_or_else(|| crate::install::service_state_dir(m.short()))),
        [] => Err(Failure::usage(match &a.name {
            Some(n) => format!("no server service named {n} is installed"),
            None => "no server service is installed here".into(),
        })
        .hint(
            "name the server's config with -c server.yaml, or its state folder with --state-dir",
        )),
        _ => Err(
            Failure::usage("several server services are installed").hint(format!(
                "name one: {}",
                servers
                    .iter()
                    .map(|m| m.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        ),
    }
}

const MAX_ROOT_FILE: u64 = 64 * 1024;

pub fn export(a: &CaExportArgs, out: &mut dyn Write) -> Result<u8, Failure> {
    let state = state_of(a)?;
    let path = state.join("ca").join("root.pem");
    let raw = read(&path, &state)?;
    let mut certs = certificates(&raw).ok_or_else(|| {
        Failure::new(format!(
            "{} holds no certificate, so nothing is exported",
            path.display()
        ))
    })?;
    let bytes = if a.der {
        certs.swap_remove(0)
    } else {
        let lf = pem::EncodeConfig::new().set_line_ending(pem::LineEnding::LF);
        certs
            .into_iter()
            .map(|c| pem::encode_config(&pem::Pem::new("CERTIFICATE", c), lf))
            .collect::<String>()
            .into_bytes()
    };
    match &a.out {
        Some(file) => {
            std::fs::write(file, &bytes)
                .map_err(|e| Failure::new(format!("cannot write {}: {e}", file.display())))?;
            crate::say_err!("wrote the local CA's certificate to {}", file.display());
        }
        None => out
            .write_all(&bytes)
            .map_err(|e| Failure::new(format!("cannot print the certificate: {e}")))?,
    }
    Ok(0)
}

fn read(path: &Path, state: &Path) -> Result<Vec<u8>, Failure> {
    use std::io::Read;
    let fail = |e: std::io::Error| match e.kind() {
        std::io::ErrorKind::NotFound => Failure::usage(format!(
            "there is no local CA in {}: burrow makes it when a site first uses tls: internal",
            state.display()
        )),
        std::io::ErrorKind::PermissionDenied => Failure::new(format!(
            "cannot read {}: {e}",
            path.display()
        ))
        .hint(
            "the state folder belongs to the service; run this with sudo, or as an administrator",
        ),
        _ => Failure::new(format!("cannot read {}: {e}", path.display())),
    };
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut opts, libc::O_NOFOLLOW);
    let file = opts.open(path).map_err(|e| {
        if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
            Failure::new(format!(
                "{} is a link, not burrow's own file, so it is not read",
                path.display()
            ))
        } else {
            fail(e)
        }
    })?;
    if !file.metadata().map_err(fail)?.is_file() {
        return Err(Failure::new(format!("{} is not a file", path.display())));
    }
    let mut bytes = Vec::new();
    file.take(MAX_ROOT_FILE)
        .read_to_end(&mut bytes)
        .map_err(fail)?;
    Ok(bytes)
}

fn certificates(raw: &[u8]) -> Option<Vec<Vec<u8>>> {
    use tokio_rustls::rustls::RootCertStore;
    use tokio_rustls::rustls::pki_types::CertificateDer;
    use tokio_rustls::rustls::pki_types::pem::PemObject;
    let mut found = Vec::new();
    for c in CertificateDer::pem_slice_iter(raw) {
        let c = c.ok()?;
        found.push(c.to_vec());
        RootCertStore::empty().add(c).ok()?;
    }
    (!found.is_empty()).then_some(found)
}

#[cfg(all(test, feature = "web"))]
mod tests {
    use super::*;

    fn args(state: &Path, der: bool, out: Option<PathBuf>) -> CaExportArgs {
        CaExportArgs {
            name: None,
            config: None,
            state_dir: Some(state.to_path_buf()),
            out,
            der,
        }
    }

    #[test]
    fn the_path_matches_the_web_proxy_s() {
        let state = Path::new("/var/lib/burrow/home");
        assert_eq!(
            state.join("ca").join("root.pem"),
            burrow_web::tls::internal::root_pem_path(state)
        );
    }

    #[test]
    fn the_root_certificate_is_printed_as_pem_or_der_and_never_the_key() {
        let state = tempfile::tempdir().unwrap();
        let missing = export(&args(state.path(), false, None), &mut Vec::new()).unwrap_err();
        assert!(
            missing.message.starts_with("there is no local CA in "),
            "{}",
            missing.message
        );
        let local = burrow_web::tls::internal::Internal::default();
        local.plan(
            Some(state.path().to_path_buf()),
            [("nas".to_string(), vec!["nas.lan".to_string()])].into(),
        );
        local.step(
            burrow_web::tls::now_secs(),
            &*burrow_web::tls::internal::default_writer(),
        );
        let mut pem = Vec::new();
        export(&args(state.path(), false, None), &mut pem).unwrap();
        let text = String::from_utf8(pem).unwrap();
        assert!(text.starts_with("-----BEGIN CERTIFICATE-----"), "{text}");
        assert!(!text.contains("PRIVATE KEY"));
        let file = state.path().join("ca.der");
        export(
            &args(state.path(), true, Some(file.clone())),
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(
            std::fs::read(&file).unwrap(),
            certificates(text.as_bytes()).unwrap()[0]
        );
        let root = state.path().join("ca").join("root.pem");
        let real = std::fs::read_to_string(&root).unwrap();
        std::fs::write(
            &root,
            format!("-----BEGIN PRIVATE KEY-----\nMIIB\n-----END PRIVATE KEY-----\n{real}"),
        )
        .unwrap();
        let mut only = Vec::new();
        export(&args(state.path(), false, None), &mut only).unwrap();
        assert_eq!(
            String::from_utf8(only).unwrap(),
            text,
            "only the parsed certificate is printed"
        );
    }

    #[test]
    fn a_root_file_that_is_no_certificate_or_a_link_is_never_printed() {
        let state = tempfile::tempdir().unwrap();
        let ca = state.path().join("ca");
        std::fs::create_dir_all(&ca).unwrap();
        let root = ca.join("root.pem");
        for junk in [
            "root:$6$secret:19000:0:99999:7:::\n",
            "-----BEGIN CERTIFICATE-----\nbm90IGEgY2VydA==\n-----END CERTIFICATE-----\n",
        ] {
            std::fs::write(&root, junk).unwrap();
            let mut out = Vec::new();
            let e = export(&args(state.path(), false, None), &mut out).unwrap_err();
            assert!(
                e.message
                    .ends_with("holds no certificate, so nothing is exported"),
                "{}",
                e.message
            );
            assert!(out.is_empty());
        }
        #[cfg(unix)]
        {
            let secret = state.path().join("shadow");
            std::fs::write(&secret, "root:$6$secret:19000:0:99999:7:::\n").unwrap();
            std::fs::remove_file(&root).unwrap();
            std::os::unix::fs::symlink(&secret, &root).unwrap();
            let mut out = Vec::new();
            let e = export(&args(state.path(), false, None), &mut out).unwrap_err();
            assert!(
                e.message
                    .ends_with("is a link, not burrow's own file, so it is not read"),
                "{}",
                e.message
            );
            assert!(out.is_empty());
        }
    }
}
