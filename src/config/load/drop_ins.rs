use std::path::Path;

use burrow_admin_proto::codes;

use super::{Format, Input};
use crate::base::failure::{file_reason, path_arg};
use crate::config::diagnostics::Diagnostic;
use crate::config::docs::{self, drop_in_folder, open_to_others, path_of};
use crate::config::tree::{FileId, Span};

pub fn add(main: &Path, inputs: &mut Vec<Input>, out: &mut Vec<Diagnostic>) {
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
    if open_to_others(&folder) {
        out.push(unsafe_one(&folder, "folder"));
    }
    for name in &listed.ignored {
        out.push(
            Diagnostic::warning(
                codes::DROP_IN_IGNORED,
                format!(
                    "{} is not read: only files ending in .yaml are",
                    path_of(main, name).display()
                ),
            )
            .help("rename it to end in .yaml to use it, or to end in .off to keep it switched off"),
        );
    }
    for name in &listed.names {
        let path = path_of(main, name);
        if open_to_others(&path) {
            out.push(unsafe_one(&path, "file"));
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let file = FileId(u16::try_from(inputs.len()).unwrap_or(u16::MAX));
                out.extend(
                    super::permission_warning(&path).map(|d| d.at(Span::default().in_file(file))),
                );
                inputs.push(Input {
                    name: path.display().to_string(),
                    text,
                    format: Format::Yaml,
                });
            }
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
