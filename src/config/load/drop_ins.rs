use std::path::Path;

use burrow_admin_proto::codes;

use super::{Format, Input};
use crate::base::failure::{file_reason, path_arg};
use crate::config::diagnostics::Diagnostic;
use crate::config::docs::{self, drop_in_folder, open_to_others, path_of};
use crate::config::tree::{FileId, Span};

pub fn read(main: &Path, inputs: &mut Vec<Input>, out: &mut Vec<Diagnostic>) {
    let folder = drop_in_folder(main);
    let listed = match docs::list(main) {
        Ok(Some(l)) => l,
        Ok(None) => return,
        Err(e) => {
            out.push(
                Diagnostic::error(
                    codes::DROP_IN_UNREADABLE,
                    format!(
                        "cannot read the folder {}: {}",
                        folder.display(),
                        file_reason(&e)
                    ),
                )
                .help("give burrow's user read access to the folder, or remove it"),
            );
            return;
        }
    };
    for name in &listed.names {
        let Some(path) = path_of(main, name) else {
            continue;
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => inputs.push(Input {
                name: path.display().to_string(),
                text,
                format: Format::Yaml,
            }),
            Err(e) => out.push(
                Diagnostic::error(
                    codes::DROP_IN_UNREADABLE,
                    format!("cannot read {}: {}", path.display(), file_reason(&e)),
                )
                .help("give burrow's user read access to it, or rename it to end in .off"),
            ),
        }
    }
}

pub fn checks(main: &Path, inputs: &[Input], out: &mut Vec<Diagnostic>) {
    let folder = drop_in_folder(main);
    if !folder.is_dir() {
        return;
    }
    if open_to_others(&folder) {
        out.push(unsafe_one(&folder, "folder"));
    }
    let ignored = docs::list(main)
        .ok()
        .flatten()
        .map(|l| l.ignored)
        .unwrap_or_default();
    for name in &ignored {
        out.push(
            Diagnostic::warning(
                codes::DROP_IN_IGNORED,
                format!(
                    "{} is not read: only files ending in .yaml are",
                    folder
                        .join(name.rsplit('/').next().unwrap_or(name))
                        .display()
                ),
            )
            .help("rename it to end in .yaml to use it, or to end in .off to keep it switched off"),
        );
    }
    for (i, input) in inputs.iter().enumerate().skip(1) {
        let path = Path::new(&input.name);
        if !path.exists() {
            continue;
        }
        if open_to_others(path) {
            out.push(unsafe_one(path, "file"));
        }
        let file = FileId(u16::try_from(i).unwrap_or(u16::MAX));
        out.extend(super::permission_warning(path).map(|d| d.at(Span::default().in_file(file))));
    }
}

fn unsafe_one(path: &Path, what: &str) -> Diagnostic {
    Diagnostic::error(
        codes::DROP_IN_UNSAFE,
        format!(
            "the drop-in {what} {} can be changed by every user on this machine",
            path.display()
        ),
    )
    .help(format!(
        "burrow does not load config that anyone can change; remove that access with: chmod o-w {}",
        path_arg(path)
    ))
}
