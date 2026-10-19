pub struct Asset {
    pub path: &'static str,
    pub mime: &'static str,
    pub body: &'static [u8],
    pub gzipped: bool,
    pub etag: &'static str,
    pub immutable: bool,
}

include!(concat!(env!("OUT_DIR"), "/admin_assets.rs"));

pub fn find(path: &str) -> Option<&'static Asset> {
    ASSETS.iter().find(|a| a.path == path)
}

pub fn index() -> &'static Asset {
    find("/index.html").expect("the admin UI has an index page")
}

pub fn placeholder() -> bool {
    cfg!(admin_ui_placeholder)
}

pub fn info() -> String {
    let mut out = format!(
        "admin UI {}\nmanifest {}\n",
        if placeholder() {
            "not built in (placeholder page)".to_string()
        } else {
            format!("version {UI_VERSION}")
        },
        UI_MANIFEST_HASH
    );
    let mut total = 0;
    for a in ASSETS {
        total += a.body.len();
        out.push_str(&format!(
            "{:40} {:>9} bytes{}\n",
            a.path,
            a.body.len(),
            if a.gzipped { " gzip" } else { "" }
        ));
    }
    out.push_str(&format!("total {total} bytes embedded\n"));
    out
}

pub fn body(a: &Asset, gzip_ok: bool) -> (std::borrow::Cow<'static, [u8]>, bool) {
    if !a.gzipped || gzip_ok {
        return (std::borrow::Cow::Borrowed(a.body), a.gzipped);
    }
    let mut out = Vec::new();
    let mut d = flate2::read::GzDecoder::new(a.body);
    match std::io::Read::read_to_end(&mut d, &mut out) {
        Ok(_) => (std::borrow::Cow::Owned(out), false),
        Err(_) => (std::borrow::Cow::Borrowed(a.body), true),
    }
}
