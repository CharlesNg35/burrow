use std::time::Instant;

use burrow_admin_proto::msg::{ErrorCode, HashPassword, HashedPassword};
use zeroize::Zeroizing;

use super::{ApiError, MAX_PENDING_SECRETS, Pending, Session};

pub const PREFIX: &str = "p";

pub fn hash_password(s: &Session, req: HashPassword) -> Result<HashedPassword, ApiError> {
    let password = Zeroizing::new(req.password);
    if !s.writes() {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "read access cannot set passwords",
        ));
    }
    if password.is_empty() {
        return Err(ApiError::new(ErrorCode::Invalid, "the password is empty"));
    }
    s.prune();
    if s.pending.lock().expect("pending lock").len() >= MAX_PENDING_SECRETS {
        return Err(ApiError::new(
            ErrorCode::Busy,
            format!(
                "at most {MAX_PENDING_SECRETS} unused secrets and passwords per session; apply or reload the page"
            ),
        ));
    }
    let hash = crate::server::passwords::hash(password.as_bytes())
        .map_err(|e| ApiError::new(ErrorCode::Invalid, e))?;
    let handle = {
        let mut n = s.next_handle.lock().expect("handle lock");
        *n += 1;
        format!("{PREFIX}{n}")
    };
    s.pending.lock().expect("pending lock").push(Pending {
        handle: handle.clone(),
        value: Zeroizing::new(hash),
        fp: String::new(),
        at: Instant::now(),
    });
    Ok(HashedPassword { handle })
}
