use std::path::{Path, PathBuf};

use burrow_config::{ServerConfig, ServiceWeb, SiteTls, TargetTls, WebSite};

const FILE_PREFIX: &str = "file:";

#[derive(Default)]
struct Files(Vec<(String, PathBuf)>);

impl Files {
    fn add(&mut self, field: String, path: &str) {
        let path = path.trim();
        if !path.is_empty() {
            self.0.push((field, PathBuf::from(path)));
        }
    }

    fn secret(&mut self, field: String, value: &str) {
        if let Some(path) = value.trim().strip_prefix(FILE_PREFIX) {
            self.add(field, path);
        }
    }

    fn site(&mut self, name: &str, site: &WebSite) {
        let at = |key: &str| format!("services.{name}.web.{key}");
        if let Some(SiteTls::Files(f)) = &site.tls {
            self.add(at("tls.cert"), &f.cert);
            self.add(at("tls.key"), &f.key);
        }
        if let Some(TargetTls::Ca(c)) = &site.target_tls {
            self.add(at("target_tls.ca"), &c.ca);
        }
        if let Some(p) = &site.error_pages {
            self.add(at("error_pages"), p);
        }
    }
}

pub(super) fn read(cfg: &ServerConfig) -> Vec<(String, PathBuf)> {
    let mut files = Files::default();
    for (name, svc) in &cfg.services {
        if let Some(ServiceWeb::Site(site)) = &svc.web {
            files.site(name, site);
        }
    }
    let Some(web) = &cfg.web else {
        return files.0;
    };
    for (i, c) in web.certificates.iter().enumerate() {
        files.add(format!("web.certificates[{i}].cert"), &c.cert);
        files.add(format!("web.certificates[{i}].key"), &c.key);
    }
    if let Some(p) = &web.error_pages {
        files.add("web.error_pages".into(), p);
    }
    let Some(le) = &web.letsencrypt else {
        return files.0;
    };
    if let Some(ca) = &le.ca {
        files.add("web.letsencrypt.ca".into(), ca);
    }
    for (i, e) in le.dns.iter().enumerate() {
        let at = |key: &str| format!("web.letsencrypt.dns[{i}].{key}");
        if let Some(c) = &e.command {
            files.add(at("command"), c);
        }
        if let Some(c) = &e.cloudflare {
            files.secret(at("cloudflare.token"), &c.token);
        }
        if let Some(r) = &e.rfc2136 {
            files.secret(at("rfc2136.secret"), &r.secret);
        }
        if let Some(a) = &e.acme_dns {
            files.secret(at("acme_dns.username"), &a.username);
            files.secret(at("acme_dns.password"), &a.password);
        }
    }
    files.0
}

pub(super) fn access_log(cfg: &ServerConfig) -> Option<PathBuf> {
    let file = cfg.web.as_ref()?.access_log.as_ref()?.file().trim();
    (!file.is_empty()).then(|| PathBuf::from(file))
}

pub(super) fn log_folder_refusal(log: &Path, service_logs: &Path) -> Option<String> {
    if log.is_relative() || !log.starts_with(service_logs) || log == service_logs {
        let name = log
            .file_name()
            .map_or_else(|| "access.log".into(), |n| n.to_string_lossy().into_owned());
        return Some(format!(
            "web.access_log ({}) must be in the service's log folder, which only the service can write: use {}",
            log.display(),
            service_logs.join(name).display()
        ));
    }
    None
}
