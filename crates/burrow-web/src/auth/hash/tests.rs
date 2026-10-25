use bcrypt::Version;

use super::*;

fn bcrypt_as(version: Version, password: &str) -> String {
    bcrypt::hash_with_result(password, 4)
        .unwrap()
        .format_for_version(version)
}

fn argon2_with(alg: Algorithm, m: u32, t: u32, p: u32) -> String {
    let params = Params::new(m, t, p, None).unwrap();
    Argon2::new(alg, argon2::Version::V0x13, params)
        .hash_password(b"pw")
        .unwrap()
        .to_string()
}

#[test]
fn burrow_passwd_writes_owasp_argon2id_that_verifies() {
    let h = hash_password(b"correct horse");
    assert!(h.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"), "{h}");
    let s = stored(&h).unwrap();
    assert!(verify(b"correct horse", &s));
    assert!(!verify(b"correct hors", &s));
    assert_ne!(h, hash_password(b"correct horse"));
}

#[test]
fn bcrypt_2a_2b_and_2y_verify_and_2x_is_refused() {
    for v in [Version::TwoA, Version::TwoB, Version::TwoY] {
        let h = bcrypt_as(v, "s3cret:x");
        let s = stored(&h).unwrap();
        assert!(verify(b"s3cret:x", &s), "{h}");
        assert!(!verify(b"s3cret", &s), "{h}");
    }
    let h = bcrypt_as(Version::TwoX, "pw");
    assert_eq!(check_hash(&h), Err(HashProblem::Bcrypt2x));
}

#[test]
fn bcrypt_cost_must_lie_between_4_and_14() {
    let h = bcrypt_as(Version::TwoB, "pw");
    let costly = format!("{}15{}", &h[..4], &h[6..]);
    assert_eq!(check_hash(&costly), Err(HashProblem::BcryptCost(15)));
    let cheap = format!("{}03{}", &h[..4], &h[6..]);
    assert_eq!(check_hash(&cheap), Err(HashProblem::BcryptCost(3)));
    assert_eq!(check_hash(&h[..59]), Err(HashProblem::NotSupported));
}

#[test]
fn argon2i_argon2d_and_costly_argon2id_are_refused() {
    assert_eq!(
        check_hash(&argon2_with(Algorithm::Argon2i, 64, 1, 1)),
        Err(HashProblem::Argon2Variant("argon2i"))
    );
    assert_eq!(
        check_hash(&argon2_with(Algorithm::Argon2d, 64, 1, 1)),
        Err(HashProblem::Argon2Variant("argon2d"))
    );
    let fine = argon2_with(Algorithm::Argon2id, 64, 10, 4);
    assert_eq!(check_hash(&fine), Ok(()));
    let big = fine.replacen("m=64,", "m=4194304,", 1);
    let e = check_hash(&big).unwrap_err();
    assert_eq!(
        e,
        HashProblem::TooCostly {
            what: "memory",
            asked: 4_194_304,
            most: 65536
        }
    );
    assert_eq!(
        e.to_string(),
        "asks for more work than burrow allows (argon2 memory 4194304 KiB, at most 65536 KiB)"
    );
    let slow = fine.replacen("t=10,", "t=11,", 1);
    assert!(matches!(
        check_hash(&slow),
        Err(HashProblem::TooCostly { what: "t", .. })
    ));
    let wide = fine.replacen("p=4$", "p=5$", 1);
    assert!(matches!(
        check_hash(&wide),
        Err(HashProblem::TooCostly { what: "p", .. })
    ));
}

#[test]
fn other_strings_are_not_a_hash() {
    for bad in [
        "",
        "hunter2",
        "$1$abc$def",
        "$argon2id$v=19$m=19456,t=2,p=1$",
        "$scrypt$ln=16,r=8,p=1$aM15713r3Xsvxbi31lqr1Q$nFNh2CVHVjNldFVKDHDlm4CbdRSCdEBsjjJxD+iCs5E",
    ] {
        assert_eq!(check_hash(bad), Err(HashProblem::NotSupported), "{bad}");
    }
}

#[test]
fn the_dummy_is_a_default_argon2id_hash() {
    let Stored::Argon2(h) = dummy() else {
        panic!("the dummy is argon2id");
    };
    assert_eq!(h.algorithm.as_str(), "argon2id");
    assert_eq!(Params::try_from(h.as_ref()).unwrap().m_cost(), 19456);
    assert!(!verify(b"", dummy()));
}
