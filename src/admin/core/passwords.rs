use burrow_admin_proto::msg::{ErrorCode, HashPassword, HashedPassword};
use zeroize::Zeroizing;

use super::{ApiError, Session};

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
    s.has_room()?;
    let hash = crate::server::passwords::hash(password.as_bytes())
        .map_err(|e| ApiError::new(ErrorCode::Invalid, e))?;
    let handle = s.hold(PREFIX, Zeroizing::new(hash), String::new())?;
    Ok(HashedPassword { handle })
}
