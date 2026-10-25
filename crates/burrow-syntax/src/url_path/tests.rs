use super::*;

fn ok(raw: &str) -> String {
    normalize_path(raw).unwrap_or_else(|e| panic!("{raw:?}: {e:?}"))
}

#[test]
fn escapes_of_unreserved_bytes_are_decoded_and_the_rest_upper_cased() {
    for (raw, want) in [
        ("/%61pi", "/api"),
        ("/%7e", "/~"),
        ("/%2f", "/%2F"),
        ("/a%3bb", "/a%3Bb"),
        ("/%2fx", "/%2Fx"),
        ("/%2Fx", "/%2Fx"),
        ("/file%252e%252e", "/file%252e%252e"),
        ("/a%c3%a9", "/a%C3%A9"),
    ] {
        assert_eq!(ok(raw), want, "{raw}");
    }
}

#[test]
fn repeated_slashes_are_merged_and_a_final_slash_is_kept() {
    assert_eq!(ok("//admin"), "/admin");
    assert_eq!(ok("/a//b///c"), "/a/b/c");
    assert_eq!(ok("/a/"), "/a/");
    assert_eq!(ok("/"), "/");
    assert_eq!(ok("//"), "/");
}

#[test]
fn bytes_above_ascii_and_quotes_are_percent_encoded() {
    assert_eq!(ok("/é"), "/%C3%A9");
    assert_eq!(ok("/é"), ok("/%C3%A9"));
    assert_eq!(ok("/a\"b"), "/a%22b");
    assert_eq!(ok("/a b"), "/a%20b");
    assert_eq!(ok("/{x}|[y]^"), "/{x}|[y]^");
}

#[test]
fn dot_segments_bad_escapes_nul_and_backslashes_are_refused() {
    use PathError::*;
    for (raw, want) in [
        ("/a/../b", DotSegment),
        ("/a/./b", DotSegment),
        ("/..", DotSegment),
        ("/.", DotSegment),
        ("/%2e%2e/x", DotSegment),
        ("/%2E/x", DotSegment),
        ("/.%2e/x", DotSegment),
        ("/public%2F..%2Fadmin", DotSegment),
        ("/a%5C..%5Cb", DotSegment),
        ("/a%5c..", DotSegment),
        ("/a\\b", Backslash),
        ("/%00", Nul),
        ("/a\0", Nul),
        ("/a%", BadEscape),
        ("/a%g1", BadEscape),
        ("/a%1", BadEscape),
        ("no-slash", NoSlash),
        ("", NoSlash),
    ] {
        assert_eq!(normalize_path(raw), Err(want), "{raw}");
    }
}

#[test]
fn data_that_only_looks_like_dots_or_slashes_passes_unchanged() {
    for raw in [
        "/api/v4/projects/group%2Fproject",
        "/a..b",
        "/...",
        "/a.",
        "/.well-known/x",
        "/.hidden",
        "/a%2F%2Fb",
    ] {
        assert_eq!(ok(raw), raw);
    }
}

#[test]
fn normalizing_twice_changes_nothing() {
    for raw in [
        "/%61pi",
        "//a//b/",
        "/é",
        "/a\"b",
        "/%2fx",
        "/api/v4/projects/group%2fproject",
        "/file%252e",
        "/a%20b",
    ] {
        let once = ok(raw);
        assert_eq!(ok(&once), once, "{raw}");
    }
}

#[test]
fn prefixes_match_on_segment_boundaries_of_the_normalized_path() {
    assert!(path_matches("/", "/anything"));
    assert!(path_matches("/api", "/api"));
    assert!(path_matches("/api", "/api/"));
    assert!(path_matches("/api", "/api/x"));
    assert!(!path_matches("/api", "/apix"));
    assert!(!path_matches("/api", "/api.."));
    assert!(!path_matches("/api", "/api..%2Fsecret"));
    assert!(!path_matches("/api", "/"));
    assert!(path_matches("/api", &ok("//api//x")));
    assert!(path_matches("/api", &ok("/%61pi/x")));
}

#[test]
fn configured_paths_are_stored_normalized_without_a_final_slash() {
    assert_eq!(route_path("/%61pi").unwrap(), "/api");
    assert_eq!(route_path("/api/").unwrap(), "/api");
    assert_eq!(route_path("//api//v2").unwrap(), "/api/v2");
    assert_eq!(route_path("/").unwrap(), "/");
    assert_eq!(route_path("/api").unwrap(), route_path("/%61pi").unwrap());
    for bad in [
        "api", "/api/*", "/a?b", "/a#b", "/a;b", "/a%2Fb", "/a%5Cb", "/../x", "/a%",
    ] {
        let e = route_path(bad).unwrap_err();
        assert_eq!(
            e,
            format!("path \"{bad}\" must start with / and hold no ?, # or *, like /api"),
        );
    }
}
