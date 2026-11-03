pub const SYSTEM: &str = "*S-1-5-18";
pub const ADMINS: &str = "*S-1-5-32-544";
pub const RESULTS_RIGHTS: &str = "(OI)(CI)RX";
pub const OWN_SELF_RIGHTS: &str = "(RX,WD,AD)";
pub const OWN_CHILD_RIGHTS: &str = "(OI)(CI)(IO)M";

pub fn folder_acl(account: &str, rights: &[&str]) -> Vec<String> {
    let mut args = vec![
        "/L".to_string(),
        "/inheritance:r".to_string(),
        "/grant:r".to_string(),
        format!("{SYSTEM}:(OI)(CI)F"),
        format!("{ADMINS}:(OI)(CI)F"),
    ];
    args.extend(rights.iter().map(|r| format!("{account}:{r}")));
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_client_account_cannot_replace_its_own_folder_and_icacls_never_follows_a_link() {
        let own = folder_acl(
            r"NT SERVICE\burrow-nas",
            &[OWN_SELF_RIGHTS, OWN_CHILD_RIGHTS],
        );
        assert_eq!(
            own[0], "/L",
            "icacls works on a link itself, never its target"
        );
        assert!(own.contains(&r"NT SERVICE\burrow-nas:(RX,WD,AD)".to_string()));
        assert!(own.contains(&r"NT SERVICE\burrow-nas:(OI)(CI)(IO)M".to_string()));
        for bad in ["DE", "WDAC", "WO", "M", "F"] {
            assert!(
                !OWN_SELF_RIGHTS
                    .trim_matches(['(', ')'])
                    .split(',')
                    .any(|r| r == bad),
                "{bad} on the folder itself would let the account replace it"
            );
        }
        assert!(
            OWN_CHILD_RIGHTS.contains("(IO)"),
            "Modify only on what is inside"
        );
        let results = folder_acl("x", &[RESULTS_RIGHTS]);
        assert_eq!(results.last().unwrap(), "x:(OI)(CI)RX");
        assert!(!RESULTS_RIGHTS.contains(['M', 'W', 'F', 'D']));
    }
}
