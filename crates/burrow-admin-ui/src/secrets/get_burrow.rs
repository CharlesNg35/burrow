use std::collections::BTreeMap;

use burrow_admin_proto::plan::{Download, Downloads};
use burrow_ui_kit::{CodeBlock, Disclosure, ExternalLink, Select};
use leptos::prelude::*;

pub const SYSTEMS: [&str; 3] = ["linux", "macos", "windows"];

pub fn os_label(os: &str) -> &'static str {
    match os {
        "windows" => "Windows",
        "macos" => "macOS",
        _ => "Linux",
    }
}

pub fn covers_every_system(d: &Downloads) -> bool {
    SYSTEMS.iter().all(|o| !chips(d, o).is_empty())
}

pub fn chips<'a>(d: &'a Downloads, os: &str) -> Vec<&'a Download> {
    d.files.iter().filter(|f| f.os == os).collect()
}

pub fn chosen<'a>(d: &'a Downloads, os: &str, picked: Option<&str>) -> Option<&'a Download> {
    let all = chips(d, os);
    picked
        .and_then(|a| all.iter().find(|f| f.arch == a))
        .or_else(|| all.iter().find(|f| f.preferred))
        .or(all.first())
        .copied()
}

pub fn fetch_lines(f: &Download) -> String {
    if f.os == "windows" {
        format!(
            "$ Invoke-WebRequest {} -OutFile burrow.zip\n$ Expand-Archive burrow.zip -DestinationPath .\n$ cd {}",
            f.url, f.folder
        )
    } else {
        format!("$ curl -fsSL {} | tar -xzf -\n$ cd {}", f.url, f.folder)
    }
}

pub fn without_curl(f: &Download) -> Option<String> {
    (f.os == "linux").then(|| format!("wget -qO- {} | tar -xzf -", f.url))
}

#[component]
pub fn GetBurrow(d: Downloads, #[prop(into)] os: Signal<String>) -> impl IntoView {
    let open = RwSignal::new(false);
    let picked: RwSignal<BTreeMap<String, String>> = RwSignal::new(BTreeMap::new());
    let d = StoredValue::new(d);
    let file = Memo::new(move |_| {
        let o = os.get();
        picked.with(|p| d.with_value(|d| chosen(d, &o, p.get(&o).map(String::as_str)).cloned()))
    });
    let version = d.with_value(|d| d.version.clone());
    let page = d.with_value(|d| d.page.clone());
    let detail = {
        let version = version.clone();
        Signal::derive(move || {
            if open.get() {
                format!("v{version}, the version this server runs")
            } else {
                let arch = file.with(|f| f.as_ref().map(|f| f.arch.clone()).unwrap_or_default());
                format!("{} · {arch} · v{version}", os_label(&os.get()))
            }
        })
    };
    let aside = ViewFn::from(move || {
        let page = page.clone();
        view! {
            <Show when=move || open.get()>
                <span aria-hidden="true">"·"</span>
                <ExternalLink href=page.clone() testid="release-page">"Release page"</ExternalLink>
            </Show>
        }
    });
    let lines =
        Signal::derive(move || file.with(|f| f.as_ref().map(fetch_lines).unwrap_or_default()));
    let options = Signal::derive(move || {
        let o = os.get();
        d.with_value(|d| {
            chips(d, &o)
                .into_iter()
                .map(|f| (f.arch.clone(), f.label.clone()))
                .collect::<Vec<_>>()
        })
    });
    let arch = Signal::derive(move || {
        file.with(|f| f.as_ref().map(|f| f.arch.clone()).unwrap_or_default())
    });
    let choose = Callback::new(move |a: String| {
        let o = os.get_untracked();
        picked.update(|p| {
            p.insert(o, a);
        });
    });
    let several = Signal::derive(move || options.with(|o| o.len() > 1));
    let wget = move || {
        file.with(|f| f.as_ref().and_then(without_curl)).map(|line| view! {
            <p class="mt-2 text-xs text-text-muted" data-testid="get-burrow-wget">"No curl? "<code class="font-mono">{line}</code></p>
        })
    };
    view! {
        <div class="mb-3" data-testid="get-burrow">
            <Disclosure id="get-burrow-toggle" summary="Get burrow".to_string() detail=detail aside=aside open=open flush=true testid="get-burrow-toggle">
                <div class="mt-2">
                    <Show when=move || several.get()>
                        <div class="mb-2 flex items-center gap-2 text-sm">
                            <label class="font-semibold" for="get-burrow-chip">"Chip"</label>
                            <div class="w-60 max-w-full">
                                <Select id="get-burrow-chip" options=options value=arch on_change=choose small=true/>
                            </div>
                        </div>
                    </Show>
                    <CodeBlock text=lines testid="get-burrow-commands"/>
                    {wget}
                </div>
            </Disclosure>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(os: &str, arch: &str, preferred: bool) -> Download {
        let folder = format!("burrow-1.0.0-{os}-{arch}");
        let ext = if os == "windows" { "zip" } else { "tar.gz" };
        Download {
            os: os.into(),
            arch: arch.into(),
            label: arch.into(),
            url: format!("https://x/download/v1.0.0/{folder}.{ext}"),
            folder,
            preferred,
        }
    }

    #[test]
    fn the_preferred_chip_is_taken_until_another_is_picked() {
        let d = Downloads {
            version: "1.0.0".into(),
            page: "https://x/tag/v1.0.0".into(),
            files: vec![
                file("macos", "amd64", false),
                file("macos", "arm64", true),
                file("windows", "amd64", true),
            ],
        };
        assert_eq!(
            chosen(&d, "macos", None).map(|f| f.arch.as_str()),
            Some("arm64")
        );
        assert_eq!(
            chosen(&d, "macos", Some("amd64")).map(|f| f.arch.as_str()),
            Some("amd64")
        );
        assert_eq!(
            chosen(&d, "macos", Some("riscv64")).map(|f| f.arch.as_str()),
            Some("arm64")
        );
        assert!(chosen(&d, "linux", None).is_none());
        let win = chosen(&d, "windows", None).expect("windows");
        assert_eq!(
            fetch_lines(win),
            "$ Invoke-WebRequest https://x/download/v1.0.0/burrow-1.0.0-windows-amd64.zip -OutFile burrow.zip\n$ Expand-Archive burrow.zip -DestinationPath .\n$ cd burrow-1.0.0-windows-amd64"
        );
        assert!(without_curl(win).is_none());
        let linux = file("linux", "arm64", false);
        assert_eq!(
            fetch_lines(&linux),
            "$ curl -fsSL https://x/download/v1.0.0/burrow-1.0.0-linux-arm64.tar.gz | tar -xzf -\n$ cd burrow-1.0.0-linux-arm64"
        );
        assert_eq!(
            without_curl(&linux).as_deref(),
            Some(
                "wget -qO- https://x/download/v1.0.0/burrow-1.0.0-linux-arm64.tar.gz | tar -xzf -"
            )
        );
    }
}
