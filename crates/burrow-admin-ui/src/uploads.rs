use burrow_admin_proto::Path;
use burrow_admin_proto::msg::{self, Uploaded};
use burrow_admin_proto::serde_json::{self, Value, json};
use burrow_ui_kit::Button;
use leptos::html::Input;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use burrow_config::{ClientAuthFiles, Entry, ServiceEntry, SiteFiles, TargetCa, WebSite};

use crate::domain::FieldCtx;
use crate::edit::Edit;
use crate::store::store;

const ACCEPT: &str = ".pem,.crt,.cer,.key,.crl";

thread_local! {
    static FACTS: std::cell::RefCell<std::collections::HashMap<String, String>> =
        Default::default();
}

fn remember(u: &Uploaded) {
    FACTS.with(|f| f.borrow_mut().insert(u.id.clone(), facts(u)));
}

fn facts_of(id: &str) -> Option<String> {
    FACTS.with(|f| f.borrow().get(id).cloned())
}

fn path_key(upload_key: &str) -> &'static str {
    if upload_key == ClientAuthFiles::CRL_UPLOAD.key() {
        ClientAuthFiles::CRL.key()
    } else if upload_key == TargetCa::CA_UPLOAD.key() {
        TargetCa::CA.key()
    } else {
        SiteFiles::CERT.key()
    }
}

fn sibling(path: &Path, key: &str) -> Path {
    let mut p = path.clone();
    p.pop();
    p.push(key.to_string());
    p
}

fn hosts_near(e: Edit, path: &Path) -> Vec<String> {
    let mut p = path.clone();
    while p.pop().is_some() {
        let mut at = p.clone();
        at.push(WebSite::HOSTS.key().into());
        if let Some(v) = e.peek(&at) {
            return burrow_admin_proto::value::one_or_many(Some(&v))
                .iter()
                .filter_map(|h| h.as_str().map(str::to_string))
                .collect();
        }
    }
    Vec::new()
}

fn request_body(e: Edit, path: &Path, kind: &str, pem: String) -> Value {
    let service = (path.first().map(String::as_str) == Some(ServiceEntry::SECTION))
        .then(|| path.get(1).cloned())
        .flatten();
    let ca_upload = e
        .peek(&sibling(path, ClientAuthFiles::CA_UPLOAD.key()))
        .and_then(|v| v.as_str().map(str::to_string));
    serde_json::to_value(msg::Upload {
        kind: kind.to_string(),
        pem,
        hosts: hosts_near(e, path),
        ca_of: service,
        ca_upload,
    })
    .unwrap_or(Value::Null)
}

fn facts(u: &Uploaded) -> String {
    let mut parts = Vec::new();
    if !u.names.is_empty() {
        parts.push(u.names.join(", "));
    }
    if !u.issuer.is_empty() {
        parts.push(format!("issued by {}", u.issuer));
    }
    if let Some(t) = u.not_after {
        parts.push(format!("valid until {}", crate::format::day(t)));
    }
    parts.push(format!("SHA-256 {}", u.sha256));
    parts.join(" · ")
}

fn read_files(
    input: &web_sys::HtmlInputElement,
    done: impl FnOnce(Result<String, String>) + 'static,
) {
    let Some(list) = input.files() else {
        return;
    };
    let promises = js_sys::Array::new();
    for i in 0..list.length() {
        if let Some(f) = list.get(i) {
            promises.push(&f.text());
        }
    }
    if promises.length() == 0 {
        return;
    }
    let all = js_sys::Promise::all(&promises);
    leptos::task::spawn_local(async move {
        let got = wasm_bindgen_futures::JsFuture::from(all).await;
        done(match got {
            Ok(v) => Ok(js_sys::Array::from(&v)
                .iter()
                .filter_map(|t| t.as_string())
                .map(|t| if t.ends_with('\n') { t } else { t + "\n" })
                .collect()),
            Err(_) => Err("the browser could not read the file".into()),
        })
    });
}

pub fn upload_editor(fc: FieldCtx, kind: String) -> AnyView {
    let s = store();
    let e = s.edit;
    let path = fc.path.clone();
    let key = path.last().cloned().unwrap_or_default();
    let file_path = sibling(&path, path_key(&key));
    let in_file = Memo::new(move |_| {
        e.value(&file_path)
            .and_then(|v| v.as_str().map(str::to_string))
    });
    let p2 = path.clone();
    let id = Memo::new(move |_| e.value(&p2).and_then(|v| v.as_str().map(str::to_string)));
    let note = RwSignal::new(None::<(bool, String)>);
    let busy = RwSignal::new(false);
    let input = NodeRef::<Input>::new();
    let pick = Callback::new(move |_| {
        if let Some(i) = input.get() {
            i.click();
        }
    });
    let (p3, k2, fc2) = (path.clone(), kind.clone(), fc.clone());
    let on_files = move |ev: web_sys::Event| {
        let Some(el) = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
        else {
            return;
        };
        let (p, k, fc) = (p3.clone(), k2.clone(), fc2.clone());
        busy.set(true);
        read_files(&el.clone(), move |text| {
            el.set_value("");
            let pem = match text {
                Ok(t) => t,
                Err(m) => {
                    busy.set(false);
                    note.set(Some((false, m)));
                    return;
                }
            };
            crate::api::request(msg::req::UPLOAD, request_body(e, &p, &k, pem), move |r| {
                busy.set(false);
                match r.map(serde_json::from_value::<Uploaded>) {
                    Ok(Ok(u)) => {
                        fc.touched.set(true);
                        e.local_error(&fc.path, None);
                        e.set(&fc.path, Some(Value::String(u.id.clone())));
                        note.set((!u.warnings.is_empty()).then(|| (true, u.warnings.join("; "))));
                        remember(&u);
                    }
                    Ok(Err(_)) => note.set(Some((
                        false,
                        burrow_admin_logic::wording::PROTOCOL_ERROR.into(),
                    ))),
                    Err(err) => note.set(Some((false, err.message))),
                }
            });
        });
    };
    let can = s.can_write_now();
    let label = Signal::derive(move || {
        if id.get().is_some() {
            "Replace".to_string()
        } else {
            "Upload".to_string()
        }
    });
    let current = move || {
        match (in_file.get(), id.get()) {
        (Some(p), _) => view! {
            <div data-testid="upload-in-file">
                <span class="font-mono text-[13px]" data-locked="">{p}</span>
                <burrow_ui_kit::LockLine text=burrow_admin_logic::wording::NAMED_IN_FILE/>
            </div>
        }
        .into_any(),
        (None, Some(i)) => {
            let line = facts_of(&i).unwrap_or_else(|| i.clone());
            view! { <p class="text-[13px]" data-testid="upload-set">"Uploaded: "<span class="break-all font-mono">{line}</span></p> }.into_any()
        }
        (None, None) => view! { <p class="text-[13px] text-text-muted" data-testid="upload-none">{fc.node.none_label.clone().unwrap_or_default()}</p> }.into_any(),
    }
    };
    let blocked = Signal::derive(move || !can || busy.get() || in_file.get().is_some());
    view! {
        <div id=fc.id.clone() class="flex flex-col gap-1.5" data-testid="file-upload" data-kind=kind.clone()>
            {current}
            <div class="flex items-center gap-2">
                <Button label=label small=true icon="arrow-up" disabled=blocked busy=busy testid="upload-pick" on_click=pick/>
                <input
                    node_ref=input
                    type="file"
                    class="hidden"
                    accept=ACCEPT
                    multiple=kind == burrow_syntax::upload::Kind::Pair.prefix()
                    data-testid="upload-input"
                    on:change=on_files
                />
                {(kind == burrow_syntax::upload::Kind::Pair.prefix()).then(|| view! { <span class="text-xs text-text-muted">"the certificate chain and its key, in one file or two"</span> })}
            </div>
            {move || note.get().map(|(warn, m)| {
                let class = if warn { "text-xs text-warning" } else { "text-xs text-danger" };
                view! { <p class=class role="status" data-testid="upload-note">{m}</p> }
            })}
        </div>
    }
    .into_any()
}

pub fn download_local_ca() {
    crate::api::request(msg::req::LOCAL_CA, json!({}), |r| {
        if let Some(pem) = r
            .ok()
            .and_then(|v| v.get("pem").and_then(Value::as_str).map(str::to_string))
        {
            burrow_ui_kit::download("burrow-local-ca.crt", &pem, "application/x-x509-ca-cert");
        }
    });
}

pub fn object_of(
    node: &burrow_admin_proto::schema::Node,
    key: &str,
) -> Option<burrow_admin_proto::schema::ObjectC> {
    use burrow_admin_proto::schema::SemType;
    match &burrow_admin_logic::tree::inner(node).sem {
        SemType::Union(u) => u.branches.iter().find_map(|b| object_of(b, key)),
        SemType::Object(o) if o.fields.iter().any(|f| f.key == key) => Some(o.clone()),
        _ => None,
    }
}

pub fn certificate_source(fc: &FieldCtx, obj: &burrow_admin_proto::schema::ObjectC) -> AnyView {
    use burrow_admin_proto::schema::SemType;
    obj.fields
        .iter()
        .filter(|f| {
            matches!(
                burrow_admin_logic::tree::inner(&f.node).sem,
                SemType::Upload(_)
            )
        })
        .map(|f| {
            let mut path = fc.path.clone();
            path.push(f.key.clone());
            let mut spath = fc.spath.clone();
            spath.push(f.key.clone());
            let sub = FieldCtx {
                node: f.node.clone(),
                path,
                spath,
                id: format!("{}-{}", fc.id, f.key),
                msg_id: format!("{}-{}-msg", fc.id, f.key),
                ..fc.clone()
            };
            view! {
                <div class="flex flex-col gap-1">
                    <span class="text-[13px] font-medium">{f.node.label.clone()}</span>
                    {crate::domain::editor(sub)}
                </div>
            }
        })
        .collect_view()
        .into_any()
}
