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

pub fn export(a: &CaExportArgs, out: &mut dyn Write) -> Result<u8, Failure> {
    let state = state_of(a)?;
    let path = state.join("ca").join("root.pem");
    let pem = read(&path, &state)?;
    let bytes = if a.der {
        to_der(&pem)
            .ok_or_else(|| Failure::new(format!("{} is not a certificate", path.display())))?
    } else {
        pem.into_bytes()
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

fn read(path: &Path, state: &Path) -> Result<String, Failure> {
    std::fs::read_to_string(path).map_err(|e| match e.kind() {
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
    })
}

fn to_der(pem: &str) -> Option<Vec<u8>> {
    use tokio_rustls::rustls::pki_types::CertificateDer;
    use tokio_rustls::rustls::pki_types::pem::PemObject;
    CertificateDer::from_pem_slice(pem.as_bytes())
        .ok()
        .map(|c| c.to_vec())
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
        assert_eq!(std::fs::read(&file).unwrap(), to_der(&text).unwrap());
    }
}
