use super::super::fake::{Fake, Mode, soa};
use super::*;

const SECRET: &str = "c2VjcmV0LWtleS1mb3ItdGVzdHMtMzItYnl0ZXMhIQ==";
const FQDN: &str = "_acme-challenge.board.apps.example.com";

async fn primary() -> Fake {
    let f = Fake::start(true).await;
    f.add(soa("apps.example.com"));
    let key = base64::engine::general_purpose::STANDARD
        .decode(SECRET)
        .unwrap();
    f.key(TSigner::new(key, algorithm(), wire::name("acme").unwrap(), FUDGE).unwrap());
    f
}

fn provider(f: &Fake, key: &str, secret: &str) -> Rfc2136 {
    Rfc2136::new(&f.addr.to_string(), key, Secret::new(secret.into())).unwrap()
}

#[tokio::test]
async fn an_update_adds_and_removes_exactly_its_own_value() {
    let f = primary().await;
    let p = provider(&f, "acme", SECRET);
    let n = wire::name(FQDN).unwrap();
    p.present(&n, "wild").await.unwrap();
    p.present(&n, "bare").await.unwrap();
    p.present(&n, "bare").await.unwrap();
    assert_eq!(f.values(FQDN), vec!["wild", "bare"]);
    p.cleanup(&n, "wild").await.unwrap();
    assert_eq!(f.values(FQDN), vec!["bare"]);
    p.cleanup(&n, "wild").await.unwrap();
    assert_eq!(f.values(FQDN), vec!["bare"]);
}

#[tokio::test]
async fn a_wrong_key_name_or_secret_gets_its_own_text() {
    let f = primary().await;
    let n = wire::name(FQDN).unwrap();
    let err = provider(&f, "other", SECRET)
        .present(&n, "v")
        .await
        .unwrap_err();
    assert_eq!(
        err.text(),
        format!(
            "{} does not know the key other (BADKEY): check key_name and that the key is hmac-sha256",
            f.addr
        )
    );
    let wrong = "d3Jvbmctc2VjcmV0LWtleS1mb3ItdGVzdHMtMzItYg==";
    let err = provider(&f, "acme", wrong)
        .present(&n, "v")
        .await
        .unwrap_err();
    assert_eq!(
        err.text(),
        format!(
            "{} refused the signature of the key acme (BADSIG): check the secret",
            f.addr
        )
    );
    assert!(f.values(FQDN).is_empty());
}

#[tokio::test]
async fn an_unsigned_success_answer_is_not_trusted() {
    let f = primary().await;
    f.mode(Mode::Forge);
    let err = provider(&f, "acme", SECRET)
        .present(&wire::name(FQDN).unwrap(), "v")
        .await
        .unwrap_err();
    assert_eq!(
        err,
        Refusal::Failed(format!(
            "{} answered the signed update without a signature, so the answer was not trusted",
            f.addr
        ))
    );
}

#[tokio::test]
async fn a_clock_too_far_from_the_servers_gets_the_badtime_text() {
    let f = primary().await;
    f.mode(Mode::Skewed);
    let err = provider(&f, "acme", SECRET)
        .present(&wire::name(FQDN).unwrap(), "v")
        .await
        .unwrap_err();
    assert_eq!(
        err.text(),
        format!(
            "{} refused the update (BADTIME): its clock and this server's differ by more than 300 s",
            f.addr
        )
    );
    assert!(f.values(FQDN).is_empty());
}

#[tokio::test]
async fn a_name_outside_the_servers_zones_cannot_be_written() {
    let f = primary().await;
    let err = provider(&f, "acme", SECRET)
        .present(&wire::name("erp.acme.example.org").unwrap(), "v")
        .await
        .unwrap_err();
    assert_eq!(
        err,
        Refusal::NotWritable(format!(
            "{} is not the primary of a zone that holds erp.acme.example.org",
            f.addr
        ))
    );
}

#[test]
fn the_secret_must_be_base64_and_the_key_name_is_canonical() {
    let err = Rfc2136::new("127.0.0.1:53", "acme", Secret::new("not base64!".into()))
        .err()
        .unwrap();
    assert_eq!(err, "the rfc2136 secret is not base64");
    let p = Rfc2136::new("127.0.0.1:53", "Burrow-Key", Secret::new(SECRET.into())).unwrap();
    assert_eq!(p.key_name.to_ascii(), "burrow-key.");
}
