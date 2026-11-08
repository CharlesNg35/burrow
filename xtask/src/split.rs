use std::path::{Path, PathBuf};

use crate::ui::{blake2s, hashed};

const PLACEHOLDER: &str = "______________________";
const MAIN_JS: &str = "app.js";

fn link_name(key: &str) -> String {
    format!("./__wasm_split.{key}.js")
}

pub struct Parts {
    pub main: PathBuf,
    pub chunks: Vec<PathBuf>,
    pub loader: PathBuf,
}

pub fn split(raw: &Path, work: &Path) -> Result<Parts, String> {
    let input = std::fs::read(raw).map_err(|e| format!("{}: {e}", raw.display()))?;
    let main = work.join("app_split.wasm");
    let link = link_name(PLACEHOLDER);
    let main_module = format!("./{MAIN_JS}");
    let mut opts = wasm_split_cli_support::Options::new(&input);
    opts.output_dir = work;
    opts.main_out_path = &main;
    opts.main_module = &main_module;
    opts.link_name = &link;
    let out = wasm_split_cli_support::transform(opts).map_err(|e| format!("wasm-split: {e:#}"))?;
    let mut chunks = out.split_modules;
    chunks.sort();
    Ok(Parts {
        main,
        chunks,
        loader: work.join(link.trim_start_matches("./")),
    })
}

pub struct Out {
    pub name: String,
    pub bytes: Vec<u8>,
}

pub struct Linked {
    pub js: Out,
    pub wasm: Vec<u8>,
    pub loader: Option<Out>,
    pub chunks: Vec<Out>,
}

fn replace_bytes(bytes: &mut [u8], from: &[u8], to: &[u8]) {
    debug_assert_eq!(from.len(), to.len());
    let mut i = 0;
    while i + from.len() <= bytes.len() {
        if &bytes[i..i + from.len()] == from {
            bytes[i..i + from.len()].copy_from_slice(to);
            i += from.len();
        } else {
            i += 1;
        }
    }
}

fn rewrite(text: &str, from: &str, to: &str, what: &str) -> Result<String, String> {
    if !text.contains(from) {
        return Err(format!("the wasm-split loader does not refer to {what}"));
    }
    Ok(text.replace(from, to))
}

fn read(p: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))
}

fn text(p: &Path) -> Result<String, String> {
    String::from_utf8(read(p)?).map_err(|e| format!("{}: {e}", p.display()))
}

pub fn link(parts: &Parts, js: &Path, wasm: &Path, release: bool) -> Result<Linked, String> {
    let mut chunks = Vec::new();
    for c in &parts.chunks {
        let name = c.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        chunks.push(Out {
            name: name.to_string(),
            bytes: read(c)?,
        });
    }
    let loader = parts
        .loader
        .exists()
        .then(|| text(&parts.loader))
        .transpose()?;
    rename(text(js)?, read(wasm)?, loader, chunks, release)
}

fn rename(
    mut js: String,
    mut wasm: Vec<u8>,
    loader: Option<String>,
    mut chunks: Vec<Out>,
    release: bool,
) -> Result<Linked, String> {
    let placeholder = link_name(PLACEHOLDER);
    let Some(mut loader) = loader.filter(|_| js.contains(&placeholder)) else {
        if !chunks.is_empty() {
            return Err("wasm-split wrote lazy chunks, but app.js does not load them".into());
        }
        let name = hashed(MAIN_JS, js.as_bytes(), release);
        return Ok(Linked {
            js: Out {
                name,
                bytes: js.into_bytes(),
            },
            wasm,
            loader: None,
            chunks,
        });
    };
    let named = if release {
        let mut all = Vec::new();
        for part in [js.as_bytes(), loader.as_bytes()]
            .into_iter()
            .chain(chunks.iter().map(|c| c.bytes.as_slice()))
        {
            all.extend_from_slice(&(part.len() as u64).to_le_bytes());
            all.extend_from_slice(part);
        }
        link_name(&blake2s(&all)[..PLACEHOLDER.len()])
    } else {
        placeholder.clone()
    };
    js = js.replace(&placeholder, &named);
    replace_bytes(&mut wasm, placeholder.as_bytes(), named.as_bytes());
    for c in &mut chunks {
        replace_bytes(&mut c.bytes, placeholder.as_bytes(), named.as_bytes());
        let name = hashed(&c.name, &c.bytes, release);
        let from = format!("\"./{}\"", c.name);
        loader = rewrite(&loader, &from, &format!("\"./{name}\""), &c.name)?;
        c.name = name;
    }
    let js_name = hashed(MAIN_JS, js.as_bytes(), release);
    let from = format!("\"./{MAIN_JS}\"");
    loader = rewrite(&loader, &from, &format!("\"./{js_name}\""), MAIN_JS)?;
    Ok(Linked {
        js: Out {
            name: js_name,
            bytes: js.into_bytes(),
        },
        wasm,
        loader: Some(Out {
            name: named.trim_start_matches("./").to_string(),
            bytes: loader.into_bytes(),
        }),
        chunks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINK: &str = "./__wasm_split.______________________.js";

    fn js() -> String {
        format!("import * as s from '{LINK}';\nimports['{LINK}'] = s;\n")
    }

    fn loader() -> String {
        "import { initSync } from \"./app.js\";\nfetch(new URL(\"./split_editor.wasm\", import.meta.url));\n".into()
    }

    fn chunk() -> Vec<Out> {
        let mut bytes = b"\0asm".to_vec();
        bytes.extend_from_slice(LINK.as_bytes());
        vec![Out {
            name: "split_editor.wasm".into(),
            bytes,
        }]
    }

    fn wasm() -> Vec<u8> {
        [b"\0asm".as_slice(), LINK.as_bytes(), b"tail"].concat()
    }

    #[test]
    fn a_release_build_names_every_part_by_its_content_and_rewrites_the_references() {
        let l = rename(js(), wasm(), Some(loader()), chunk(), true).expect("linked");
        let named = l.loader.as_ref().expect("a loader");
        assert!(named.name.starts_with("__wasm_split."));
        assert_eq!(
            named.name.len(),
            LINK.len() - 2,
            "the wasm keeps its length"
        );
        let js = String::from_utf8(l.js.bytes.clone()).expect("text");
        assert!(!js.contains(PLACEHOLDER));
        assert_eq!(js.matches(&format!("./{}", named.name)).count(), 2);
        assert_eq!(l.wasm.len(), wasm().len());
        assert!(
            l.wasm
                .windows(named.name.len())
                .any(|w| w == named.name.as_bytes())
        );
        let part = &l.chunks[0];
        assert!(part.name.starts_with("split_editor-") && part.name.ends_with(".wasm"));
        assert!(!String::from_utf8_lossy(&part.bytes).contains(PLACEHOLDER));
        let text = String::from_utf8(named.bytes.clone()).expect("text");
        assert!(text.contains(&format!("from \"./{}\"", l.js.name)));
        assert!(text.contains(&format!("\"./{}\"", part.name)));
        assert!(l.js.name.starts_with("app-"));
        let again = rename(js_with(" "), wasm(), Some(loader()), chunk(), true).expect("linked");
        assert_ne!(
            again.loader.expect("a loader").name,
            named.name,
            "a change in any part renames the loader"
        );
    }

    fn js_with(extra: &str) -> String {
        format!("{}{extra}", js())
    }

    #[test]
    fn a_dev_build_keeps_the_names() {
        let l = rename(js(), wasm(), Some(loader()), chunk(), false).expect("linked");
        assert_eq!(l.js.name, "app.js");
        assert_eq!(
            l.loader.expect("a loader").name,
            LINK.trim_start_matches("./")
        );
        assert_eq!(l.chunks[0].name, "split_editor.wasm");
        assert_eq!(l.wasm, wasm());
    }

    #[test]
    fn without_a_lazy_function_there_is_no_loader() {
        let l = rename("plain".into(), wasm(), Some(loader()), Vec::new(), true).expect("linked");
        assert!(l.loader.is_none());
        assert!(l.chunks.is_empty());
        assert!(rename("plain".into(), wasm(), Some(loader()), chunk(), true).is_err());
    }

    #[test]
    fn a_loader_that_misses_a_reference_is_refused() {
        let other = "import { initSync } from \"./app.js\";\n".to_string();
        let e = rename(js(), wasm(), Some(other), chunk(), true).err();
        assert_eq!(
            e.as_deref(),
            Some("the wasm-split loader does not refer to split_editor.wasm")
        );
    }

    #[test]
    fn bytes_are_replaced_in_place() {
        let mut b = b"xxabcxxabc".to_vec();
        replace_bytes(&mut b, b"abc", b"def");
        assert_eq!(b, b"xxdefxxdef");
    }
}
