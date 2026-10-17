use std::io::Write;
use std::path::PathBuf;

use serde_json::json;

use super::{InitOptions, Login, Prompter, ask_login, resolve_output, write_new};
use crate::config::Role;
use crate::config::emit::render;
use crate::failure::Failure;

pub fn init_visitor(
    opts: &InitOptions,
    prompt: &mut dyn Prompter,
    out: &mut dyn Write,
) -> Result<PathBuf, Failure> {
    let (path, format) = resolve_output(Role::Visitor, opts)?;
    let Login {
        server,
        name,
        secret_text,
    } = ask_login(Role::Visitor, &path, opts, prompt)?;
    let value = json!({
        "server": server,
        "name": name,
        "secret": secret_text,
        "forwards": {},
    });
    write_new(&path, &render(&value, format), opts.force)?;
    writeln!(
        out,
        "wrote {}; add the services to forward, for example:\n\n  forwards:\n    db: {{ service: db, local: 5432 }}\n\nor print a ready config on the server with: {} visitor-config {name} -o {}",
        path.display(),
        crate::failure::command(),
        path.file_name().map_or_else(|| path.display().to_string(), |f| f.to_string_lossy().into_owned())
    )
    .map_err(|e| Failure::new(format!("cannot write output: {e}")))?;
    Ok(path)
}
