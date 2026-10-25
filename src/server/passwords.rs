#[cfg(feature = "web")]
pub fn hash(password: &[u8]) -> Result<String, String> {
    Ok(burrow_web::auth::hash_password(password))
}

#[cfg(not(feature = "web"))]
pub fn hash(_: &[u8]) -> Result<String, String> {
    Err("this build of burrow has no web proxy".into())
}
