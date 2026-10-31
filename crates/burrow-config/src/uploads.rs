use burrow_syntax::upload::Kind;

use crate::model::{
    ClientAuthFiles, LetsEncrypt, ServerConfig, ServiceWeb, SiteFiles, SiteTls, TargetCa,
    TargetTls, WebSite,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place<'a> {
    Site { service: &'a str, key: &'static str },
    Certificate(usize),
    LetsEncrypt,
}

pub enum Files<'a> {
    Pair {
        cert: &'a mut Option<String>,
        key: &'a mut Option<String>,
    },
    One {
        path: &'a mut Option<String>,
        required: bool,
    },
}

pub struct UploadSlot<'a> {
    pub place: Place<'a>,
    pub kind: Kind,
    pub upload: Option<&'a str>,
    pub files: Files<'a>,
}

fn pair<'a>(place: Place<'a>, f: &'a mut SiteFiles) -> UploadSlot<'a> {
    UploadSlot {
        place,
        kind: Kind::Pair,
        upload: f.upload.as_deref(),
        files: Files::Pair {
            cert: &mut f.cert,
            key: &mut f.key,
        },
    }
}

fn one<'a>(
    place: Place<'a>,
    (path, upload): (&'a mut Option<String>, &'a Option<String>),
    kind: Kind,
    required: bool,
) -> UploadSlot<'a> {
    UploadSlot {
        place,
        kind,
        upload: upload.as_deref(),
        files: Files::One { path, required },
    }
}

fn site<'a>(service: &'a str, s: &'a mut WebSite, out: &mut Vec<UploadSlot<'a>>) {
    let at = |key: &'static str| Place::Site { service, key };
    if let Some(SiteTls::Files(f)) = s.tls.as_mut() {
        out.push(pair(at("tls"), f));
    }
    if let Some(TargetTls::Ca(TargetCa { ca, ca_upload })) = s.target_tls.as_mut() {
        out.push(one(at("target_tls"), (ca, ca_upload), Kind::Ca, true));
    }
    if let Some(ClientAuthFiles {
        ca,
        ca_upload,
        crl,
        crl_upload,
    }) = s.client_auth.as_mut()
    {
        out.push(one(at("client_auth"), (ca, ca_upload), Kind::Ca, true));
        out.push(one(at("client_auth"), (crl, crl_upload), Kind::Crl, false));
    }
}

pub fn upload_slots(cfg: &mut ServerConfig) -> Vec<UploadSlot<'_>> {
    let mut out = Vec::new();
    for (name, svc) in cfg.services.iter_mut() {
        if let Some(ServiceWeb::Site(s)) = svc.web.as_mut() {
            site(name, s, &mut out);
        }
    }
    let Some(web) = cfg.web.as_mut() else {
        return out;
    };
    for (i, c) in web.certificates.iter_mut().enumerate() {
        out.push(UploadSlot {
            place: Place::Certificate(i),
            kind: Kind::Pair,
            upload: c.upload.as_deref(),
            files: Files::Pair {
                cert: &mut c.cert,
                key: &mut c.key,
            },
        });
    }
    if let Some(LetsEncrypt { ca, ca_upload, .. }) = web.letsencrypt.as_mut() {
        out.push(one(Place::LetsEncrypt, (ca, ca_upload), Kind::Ca, false));
    }
    out
}

pub fn upload_ids(cfg: &ServerConfig) -> Vec<String> {
    let mut cfg = cfg.clone();
    upload_slots(&mut cfg)
        .into_iter()
        .filter_map(|s| s.upload.map(|id| id.trim().to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin_schema::AdminSchema;
    use burrow_admin_proto::schema::{Node, SemType};
    use burrow_admin_proto::serde_json::{self, json};

    fn schema_kinds(node: &Node, out: &mut Vec<String>) {
        match &node.sem {
            SemType::Upload(u) => out.push(u.kind.clone()),
            SemType::Object(o) => o.fields.iter().for_each(|f| schema_kinds(&f.node, out)),
            SemType::Map(m) => schema_kinds(&m.value, out),
            SemType::List(l) => schema_kinds(&l.item, out),
            SemType::Union(u) => u.branches.iter().for_each(|b| schema_kinds(b, out)),
            SemType::Optional(o) => schema_kinds(&o.inner, out),
            _ => {}
        }
    }

    #[test]
    fn every_upload_field_of_the_schema_is_one_upload_place() {
        let mut fields = Vec::new();
        schema_kinds(&ServerConfig::node(), &mut fields);
        fields.sort();
        let cfg: ServerConfig = serde_json::from_value(json!({
            "listen": "127.0.0.1:7835",
            "web": {
                "https": 443,
                "certificates": [{"hosts": ["a.example.com"], "upload": "pair-000000000000000a"}],
                "letsencrypt": {"email": "ops@example.com", "ca_upload": " ca-000000000000000b "}
            },
            "services": {"site": {"client": "home", "target": 8080, "web": {
                "hosts": ["b.example.com"],
                "tls": {"upload": "pair-000000000000000c"},
                "target_tls": {"ca_upload": "ca-000000000000000d"},
                "client_auth": {"ca_upload": "ca-000000000000000e", "crl_upload": "crl-000000000000000f"}
            }}}
        }))
        .unwrap();
        assert_eq!(
            upload_ids(&cfg),
            [
                "pair-000000000000000c",
                "ca-000000000000000d",
                "ca-000000000000000e",
                "crl-000000000000000f",
                "pair-000000000000000a",
                "ca-000000000000000b",
            ]
        );
        let mut c = cfg.clone();
        let mut kinds: Vec<String> = upload_slots(&mut c)
            .iter()
            .map(|s| s.kind.prefix().to_string())
            .collect();
        kinds.sort();
        assert_eq!(kinds, fields, "an upload field the slot list does not know");
    }
}
