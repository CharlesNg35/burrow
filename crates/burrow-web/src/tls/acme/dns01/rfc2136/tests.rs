use super::super::fake::{Fake, Mode, soa};
use super::*;

const SECRET: &str = "c2VjcmV0LWtleS1mb3ItdGVzdHMtMzItYnl0ZXMhIQ==";
const FQDN: &str = "_acme-challenge.board.apps.example.com";

use super::super::TsigAlgorithm as Alg;

async fn primary() -> Fake {
    primary_with(Alg::HmacSha256, SECRET).await
}

async fn primary_with(alg: Alg, secret: &str) -> Fake {
    let f = Fake::start(true).await;
    f.add(soa("apps.example.com"));
    let key = base64::engine::general_purpose::STANDARD
        .decode(secret)
        .unwrap();
    f.key(TSigner::new(key, algorithm(alg), wire::name("acme").unwrap(), FUDGE).unwrap());
    f
}

fn provider(f: &Fake, key: &str, secret: &str) -> Rfc2136 {
    provider_with(f, key, secret, Alg::HmacSha256)
}

fn provider_with(f: &Fake, key: &str, secret: &str, alg: Alg) -> Rfc2136 {
    Rfc2136::new(&f.addr.to_string(), key, Secret::new(secret.into()), alg).unwrap()
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
            "{} does not know the key other with hmac-sha256 (BADKEY): check key_name and algorithm",
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
    let err = Rfc2136::new(
        "127.0.0.1:53",
        "acme",
        Secret::new("not base64!".into()),
        Alg::default(),
    )
    .err()
    .unwrap();
    assert_eq!(err, "the rfc2136 secret is not base64");
    let p = Rfc2136::new(
        "127.0.0.1:53",
        "Burrow-Key",
        Secret::new(SECRET.into()),
        Alg::default(),
    )
    .unwrap();
    assert_eq!(p.key_name.to_ascii(), "burrow-key.");
}

const SECRET_384: &str = "r2F4xx/e095hTZXFDba7O5Lon/p4i5QLiZgIGTzz4Ldl7wd6BOXhVzwXCCz3tQJj";
const SECRET_512: &str =
    "JcVH5r9JhcHXNC/6Y7NX+s68xVzS2QyiRINH0qfiqib6ZLCa8/f6LYZGEIlD52BqBNNKmZokhJMa3miqkCVcnA==";

#[tokio::test]
async fn keys_of_each_algorithm_sign_and_check_their_updates() {
    let n = wire::name(FQDN).unwrap();
    for (alg, secret) in [
        (Alg::HmacSha256, SECRET),
        (Alg::HmacSha384, SECRET_384),
        (Alg::HmacSha512, SECRET_512),
    ] {
        let f = primary_with(alg, secret).await;
        let p = provider_with(&f, "acme", secret, alg);
        p.present(&n, "v").await.unwrap();
        assert_eq!(f.values(FQDN), vec!["v"], "{}", alg.name());
        p.cleanup(&n, "v").await.unwrap();
        assert!(f.values(FQDN).is_empty(), "{}", alg.name());
    }
    let f = primary_with(Alg::HmacSha512, SECRET_512).await;
    let err = provider_with(&f, "acme", SECRET_512, Alg::HmacSha256)
        .present(&n, "v")
        .await
        .unwrap_err();
    assert_eq!(
        err.text(),
        format!(
            "{} does not know the key acme with hmac-sha256 (BADKEY): check key_name and algorithm",
            f.addr
        )
    );
}

#[test]
fn algorithm_names_read_as_bind_writes_them() {
    assert_eq!(Alg::default(), Alg::HmacSha256);
    assert_eq!(Alg::parse("HMAC-SHA512"), Some(Alg::HmacSha512));
    assert_eq!(Alg::parse("hmac-sha384."), Some(Alg::HmacSha384));
    assert_eq!(Alg::parse("hmac-md5"), None);
    assert_eq!(Alg::parse("hmac-sha1"), None);
    assert_eq!(Alg::ALL.map(Alg::mac_bytes), [32, 48, 64]);
}
