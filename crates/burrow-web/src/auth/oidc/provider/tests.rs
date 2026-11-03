use super::*;
use serde_json::json;

fn claims(v: serde_json::Value) -> Claims {
    serde_json::from_value(v).unwrap()
}

#[test]
fn the_claims_must_carry_the_nonce_a_verified_email_and_be_for_this_client() {
    let ok = claims(
        json!({"nonce": "n1", "email": " alice@gmail.com ", "email_verified": true, "aud": "c"}),
    );
    assert_eq!(
        check_claims(&ok, "n1", "c").unwrap(),
        Identity {
            email: "alice@gmail.com".into()
        }
    );
    let e = check_claims(&ok, "n2", "c").unwrap_err();
    assert!(e.contains("nonce"), "{e}");
    let unverified = claims(json!({"nonce": "n", "email": "a@b.c", "email_verified": false}));
    assert!(
        check_claims(&unverified, "n", "c")
            .unwrap_err()
            .contains("not verified")
    );
    let as_text = claims(json!({"nonce": "n", "email": "a@b.c", "email_verified": "true"}));
    assert!(
        check_claims(&as_text, "n", "c").is_ok(),
        "some providers send the flag as text"
    );
    let unsaid = claims(json!({"nonce": "n", "email": "a@b.c"}));
    assert!(
        check_claims(&unsaid, "n", "c").is_ok(),
        "a provider without the flag"
    );
    let no_email = claims(json!({"nonce": "n"}));
    assert!(
        check_claims(&no_email, "n", "c")
            .unwrap_err()
            .contains("email scope")
    );
    let other = claims(json!({"nonce": "n", "email": "a@b.c", "aud": ["c", "d"], "azp": "d"}));
    assert!(check_claims(&other, "n", "c").unwrap_err().contains("azp"));
    let mine = claims(json!({"nonce": "n", "email": "a@b.c", "aud": ["c", "d"], "azp": "c"}));
    assert!(check_claims(&mine, "n", "c").is_ok());
}

#[test]
fn only_signatures_with_a_public_key_are_taken() {
    assert!(asymmetric(Algorithm::RS256));
    assert!(asymmetric(Algorithm::ES256));
    assert!(asymmetric(Algorithm::EdDSA));
    assert!(!asymmetric(Algorithm::HS256));
}

#[test]
fn a_key_is_found_by_its_id_and_must_fit_the_algorithm() {
    let keys: JwkSet = serde_json::from_value(json!({"keys": [
        {"kty": "EC", "crv": "P-256", "kid": "a", "alg": "ES256",
         "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
         "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0"},
        {"kty": "OKP", "crv": "X25519", "kid": "odd", "x": "AAAA"}
    ]}))
    .unwrap();
    assert!(key_for(&keys, Some("a"), Algorithm::ES256).is_some());
    assert!(key_for(&keys, Some("a"), Algorithm::RS256).is_none());
    assert!(key_for(&keys, Some("b"), Algorithm::ES256).is_none());
    assert!(key_for(&keys, None, Algorithm::ES256).is_some());
}
