pub const PREFIX: &str = "unix:";
pub const MAX_PATH: usize = 104;

pub fn path_of(target: &str) -> Option<&str> {
    target.strip_prefix(PREFIX)
}

pub fn check_path(path: &str) -> Result<(), String> {
    if !path.starts_with('/') {
        return Err(format!(
            "socket path \"{path}\" must be absolute, starting with \"/\""
        ));
    }
    if path.len() > MAX_PATH {
        return Err(format!(
            "socket path \"{path}\" is longer than {MAX_PATH} bytes, the limit of Unix sockets"
        ));
    }
    if path.contains('\0') {
        return Err(format!("socket path {path:?} may not contain a NUL byte"));
    }
    if path.split('/').any(|part| part == "..") {
        return Err(format!("socket path \"{path}\" may not contain \"..\""));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(path_of("unix:/run/x.sock"), Some("/run/x.sock"));
        assert_eq!(path_of("127.0.0.1:22"), None);
        assert!(check_path("/var/run/docker.sock").is_ok());
        assert!(check_path("run/x").is_err());
        assert!(check_path("/a/../b").unwrap_err().contains(".."));
        assert!(check_path("/a\0b").unwrap_err().contains("NUL"));
        assert!(check_path(&format!("/{}", "x".repeat(200))).is_err());
    }
}
