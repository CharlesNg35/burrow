//! Copies a server's drop-in folder with its config at `burrow service install`, and checks the files an earlier install left there.
//! Start at `copy`; `unix` and `other` place each file through the opened folder.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use super::Backend;
use super::system;

#[cfg(unix)]
mod given;
#[cfg(not(unix))]
mod other;
mod snapshot;
#[cfg(unix)]
mod unix;

use crate::base::failure::Failure;
use crate::base::output::say;
use crate::config::docs::{self, Doc, Docs, drop_in_folder};
#[cfg(unix)]
pub(super) use given::{give_back, to_root};
#[cfg(not(unix))]
use other as place;
pub(super) use snapshot::Snapshot;
#[cfg(unix)]
use unix as place;

fn file_part(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

fn listed(main: &Path) -> Result<Vec<String>, Failure> {
    docs::list(main)
        .map(|l| l.map(|l| l.names).unwrap_or_default())
        .map_err(|e| {
            Failure::new(format!(
                "cannot read {}: {e}",
                drop_in_folder(main).display()
            ))
        })
}

fn is_link_or_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & REPARSE_POINT != 0 {
            return true;
        }
    }
    !meta.file_type().is_dir()
}

fn refuse_link(to: &Path) -> Result<(), Failure> {
    if is_link_or_file(to) {
        return Err(Failure::new(format!(
            "{} is a link or a file, not a folder, so burrow will not install drop-in files there",
            to.display()
        ))
        .hint("remove it, and run the install again"));
    }
    Ok(())
}

const COPIED: &str = ".burrow-copied";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Put {
    Config,
    Locked,
    Note,
}

fn copied_before(to: &Path) -> Vec<String> {
    docs::read_regular(&to.join(COPIED))
        .map(|t| t.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

fn leftovers(source: &Path, config: &Path) -> Result<Vec<String>, Failure> {
    let to = drop_in_folder(config);
    refuse_link(&to)?;
    let wanted: Vec<String> = listed(source)?
        .iter()
        .map(|n| file_part(n).to_string())
        .collect();
    let before = copied_before(&to);
    Ok(listed(config)?
        .iter()
        .map(|n| file_part(n).to_string())
        .filter(|f| !wanted.contains(f) && !before.contains(f))
        .collect())
}

pub(super) fn check_leftovers(
    source: &Path,
    config: &Path,
    env: &BTreeMap<String, String>,
) -> Result<(), Failure> {
    if crate::base::fs::canonical_or_same(source) == crate::base::fs::canonical_or_same(config) {
        return Ok(());
    }
    let kept = leftovers(source, config)?;
    if kept.is_empty() {
        return Ok(());
    }
    let to = drop_in_folder(config);
    let main = crate::config::load::Input {
        name: source.display().to_string(),
        text: std::fs::read_to_string(source).unwrap_or_default(),
        format: crate::config::Format::from_path(source).unwrap_or(crate::config::Format::Yaml),
    };
    let mut found = Vec::new();
    let mut inputs = crate::config::load::read_set(source, main, &mut found);
    for file in &kept {
        let path = to.join(file);
        if let Ok(text) = docs::read_regular(&path) {
            inputs.push(crate::config::load::Input {
                name: path.display().to_string(),
                text,
                format: crate::config::Format::Yaml,
            });
        }
    }
    let lookup = |k: &str| env.get(k).cloned();
    let dir = docs::folder_of(source);
    let state = crate::install::state_dir_for(config);
    let outcome = crate::config::load::finish(
        crate::config::load::load_set(
            source,
            (&dir, &state),
            inputs,
            Some(crate::config::Role::Server),
            &lookup,
        ),
        found,
    );
    let Some(first) = outcome.report.diagnostics.iter().find(|d| d.is_error()) else {
        return Ok(());
    };
    let shown: Vec<String> = kept
        .iter()
        .map(|f| to.join(f).display().to_string())
        .collect();
    Err(Failure::new(format!(
        "{} {} not from the source and would stay, but with it the installed config is invalid: {}",
        shown.join(", "),
        if shown.len() == 1 { "is" } else { "are" },
        first.message
    ))
    .hint("burrow never deletes a drop-in file it did not copy; remove or fix it by hand, then install again"))
}

pub(super) fn copy(
    backend: &dyn Backend,
    (source, config): (&Path, &Path),
    account: &str,
    writable: bool,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let (from, to) = (drop_in_folder(source), drop_in_folder(config));
    refuse_link(&to)?;
    let names = listed(source)?;
    let wanted: Vec<&str> = names.iter().map(|n| file_part(n)).collect();
    let installed = listed(config)?;
    if names.is_empty() && installed.is_empty() {
        return Ok(());
    }
    let kept = Snapshot::take(config);
    let folder = place::Folder::open(backend, (config, &to), account, writable)?;
    let copied = fill(&folder, (&from, &to), (&wanted, &installed), out);
    if copied.is_err() {
        let _ = kept.restore(&folder, config);
    }
    copied
}

fn fill(
    folder: &place::Folder,
    (from, to): (&Path, &Path),
    (wanted, installed): (&[&str], &[String]),
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let before = copied_before(to);
    for old in installed.iter().map(|n| file_part(n)) {
        if wanted.contains(&old) {
            continue;
        }
        let gone = to.join(old);
        if before.iter().any(|b| b == old) {
            folder.remove(old)?;
            say(
                out,
                format!(
                    "removed {}, which an earlier install copied and {} no longer has",
                    gone.display(),
                    from.display()
                ),
            )?;
        } else {
            say(
                out,
                format!(
                    "left {} in place: burrow did not copy it, and never deletes a drop-in file it did not copy",
                    gone.display()
                ),
            )?;
        }
    }
    for file in wanted {
        let src = from.join(file);
        let text = std::fs::read(&src)
            .map_err(|e| Failure::new(format!("cannot read {}: {e}", src.display())))?;
        let put = if docs::read_only(&src).is_some() {
            Put::Locked
        } else {
            Put::Config
        };
        folder.put(file, &text, put)?;
    }
    folder.put(COPIED, wanted.join("\n").as_bytes(), Put::Note)?;
    if wanted.is_empty() {
        return Ok(());
    }
    say(
        out,
        format!(
            "copied {} -> {} ({} {})",
            from.display(),
            to.display(),
            wanted.len(),
            crate::config::diagnostics::plural(wanted.len(), "file")
        ),
    )
}

pub(super) fn installed(config: &Path, main: String) -> Docs {
    let mut docs = Docs::one(docs::main_name(config), main);
    if is_link_or_file(&drop_in_folder(config)) {
        return docs;
    }
    for name in listed(config).unwrap_or_default() {
        if let Some(text) = docs::path_of(config, &name)
            .and_then(|p| system::read_plain_file(&p))
            .and_then(|b| String::from_utf8(b).ok())
        {
            docs.list.push(Doc { name, text });
        }
    }
    docs
}

pub(super) fn same_set(source: &Path, installed: &Docs) -> bool {
    let Ok(src) = docs::read(source) else {
        return false;
    };
    src.list.len() == installed.list.len()
        && src
            .list
            .iter()
            .zip(&installed.list)
            .enumerate()
            .all(|(i, (a, b))| {
                a.text == b.text && (i == 0 || file_part(&a.name) == file_part(&b.name))
            })
}

#[cfg(test)]
mod tests;
