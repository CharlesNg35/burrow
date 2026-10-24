use std::path::Path;

use instant_acme::{Account, AccountCredentials, Key};
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};

use super::http::Client;
use super::storage::{account_dir, write_private};

pub struct Opened {
    pub account: Account,
    pub created: bool,
}

pub async fn open(
    client: &Client,
    state: &Path,
    directory: &str,
    email: &str,
) -> Result<Opened, String> {
    let dir = account_dir(state, directory);
    let (json, key_path) = (dir.join("account.json"), dir.join("account.key"));
    let read = {
        let (json, key_path) = (json.clone(), key_path.clone());
        tokio::task::spawn_blocking(move || {
            (std::fs::read(json).ok(), std::fs::read(key_path).ok())
        })
        .await
        .map_err(|e| e.to_string())?
    };
    if let (Some(saved), _) = &read
        && let Ok(creds) = serde_json::from_slice::<AccountCredentials>(saved)
    {
        let account = Account::builder_with_http(Box::new(client.clone()))
            .from_credentials(creds)
            .await
            .map_err(|e| super::describe(&e))?;
        return Ok(Opened {
            account,
            created: false,
        });
    }
    let pkcs8 = match read
        .1
        .and_then(|pem| PrivatePkcs8KeyDer::from_pem_slice(&pem).ok())
    {
        Some(k) => k,
        None => {
            let (_, fresh) = Key::generate_pkcs8().map_err(|e| super::describe(&e))?;
            let pem = pem_of(&fresh)?;
            let path = key_path.clone();
            tokio::task::spawn_blocking(move || write_private(&path, pem.as_bytes()))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| format!("cannot save the account key in {}: {e}", dir.display()))?;
            fresh
        }
    };
    let key = Key::from_pkcs8_der(pkcs8.clone_key()).map_err(|e| super::describe(&e))?;
    let (account, creds) = Account::builder_with_http(Box::new(client.clone()))
        .create_from_key((key, PrivateKeyDer::Pkcs8(pkcs8)), directory.to_string())
        .await
        .map_err(|e| super::describe(&e))?;
    let contact = format!("mailto:{email}");
    if let Err(e) = account.update_contacts(&[contact.as_str()]).await {
        tracing::warn!(
            "ACME account {}: the contact was not saved: {}",
            account.id(),
            super::describe(&e)
        );
    }
    let saved = serde_json::to_vec(&creds).map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || write_private(&json, &saved))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("cannot save the ACME account: {e}"))?;
    Ok(Opened {
        account,
        created: true,
    })
}

fn pem_of(key: &PrivatePkcs8KeyDer<'_>) -> Result<String, String> {
    rcgen::KeyPair::try_from(key.secret_pkcs8_der())
        .map(|k| k.serialize_pem())
        .map_err(|e| format!("cannot write the account key: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_account_key_is_written_as_pkcs8_pem_that_reads_back() {
        let (_, key) = Key::generate_pkcs8().unwrap();
        let pem = pem_of(&key).unwrap();
        let back = PrivatePkcs8KeyDer::from_pem_slice(pem.as_bytes()).unwrap();
        assert_eq!(back.secret_pkcs8_der(), key.secret_pkcs8_der());
        assert!(Key::from_pkcs8_der(back).is_ok());
    }
}
