use resolved_release::*;
use serde_json::{Value, json};

fn download(name: &str) -> Value {
    json!({
        "name": name,
        "url": format!("https://github.com/zkillua00/resolved/releases/download/v0.12.2/{name}"),
        "size": 20750748,
        "sha256": "AB".repeat(32)
    })
}

// Mirrors the public feed's platform map and unrelated MCP/additional-file entries.
fn fixture(version: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "version": version,
        "resolved": {
            "macos": {
                "arm64": [download("Resolved-0.12.2-macos-arm64.zip")],
                "x64": [download("Resolved-0.12.2-macos-x64.zip")]
            },
            "windows": {"x64": [download("Resolved.cer"), download("Resolved.msix")]},
            "linux": {"arm64": [
                download("Resolved.rpm"), download("Resolved.tar.xz"), download("Resolved.deb")
            ]}
        },
        "resolved-mcp": {"some": "unrelated schema"},
        "additional_files": [{"anything": true}]
    }))
    .unwrap()
}

fn release() -> ReleaseCheck {
    check_feed(&fixture("0.12.2"), "0.12.1", "macos", "aarch64").unwrap()
}

#[test]
fn bounds_version_and_does_not_echo_malformed_values() {
    assert!(
        check_feed(
            &fixture(&format!("1.0.0+{}", "x".repeat(128))),
            "0.12.1",
            "macos",
            "arm64"
        )
        .is_err()
    );
    let bytes = br#"{"version":"1.0.0","resolved":"untrusted-content"}"#;
    let error = check_feed(bytes, "0.12.1", "macos", "arm64").unwrap_err();
    assert!(error.contains("line"));
    assert!(!error.contains("untrusted-content"));
}

#[test]
fn compares_versions_numerically_including_nightly_builds() {
    for (latest, installed, newer) in [
        ("0.12.2", "0.9.9", true),
        ("0.12.2", "0.12.2", false),
        ("0.12.2", "0.13.0", false),
        ("0.12.2", "0.12.2.abcdef123456", false),
        ("0.12.3", "0.12.2.ABCDEF123456", true),
        ("0.12.2", "0.12.2-rc.1", true),
        ("0.12.2+build.2", "0.12.2+build.1", false),
    ] {
        assert_eq!(
            check_feed(&fixture(latest), installed, "macos", "aarch64")
                .unwrap()
                .newer,
            newer,
            "{latest} vs {installed}"
        );
    }
}

#[test]
fn selects_only_desktop_downloads_for_the_current_platform() {
    for (os, arch, expected) in [
        ("macos", "aarch64", vec!["Resolved-0.12.2-macos-arm64.zip"]),
        ("macos", "arm64", vec!["Resolved-0.12.2-macos-arm64.zip"]),
        ("macos", "x86_64", vec!["Resolved-0.12.2-macos-x64.zip"]),
        ("macos", "x64", vec!["Resolved-0.12.2-macos-x64.zip"]),
        ("windows", "x86_64", vec!["Resolved.cer", "Resolved.msix"]),
        ("windows", "aarch64", vec![]),
        ("plan9", "aarch64", vec![]),
        (
            "linux",
            "aarch64",
            vec!["Resolved.rpm", "Resolved.tar.xz", "Resolved.deb"],
        ),
    ] {
        let release = check_feed(&fixture("0.12.2"), "0.12.1", os, arch).unwrap();
        assert_eq!(
            release
                .downloads
                .iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn legacy_missing_hashes_and_names_remain_browser_downloads() {
    let mut feed: Value = serde_json::from_slice(&fixture("0.12.2")).unwrap();
    let entry = &mut feed["resolved"]["macos"]["arm64"][0];
    *entry = json!({
        "name": "Resolved-macos-arm64.zip",
        "url": "https://github.com/zkillua00/resolved/releases/download/v0.12.2/Resolved-macos-arm64.zip"
    });
    let release = check_feed(
        &serde_json::to_vec(&feed).unwrap(),
        "0.12.1",
        "macos",
        "arm64",
    )
    .unwrap();
    assert_eq!(release.downloads.len(), 1);
    assert_eq!(release.downloads[0].size, None);
    assert_eq!(release.downloads[0].sha256, None);
    assert!(release.macos_update("arm64").is_err());
}

#[test]
fn selects_normalized_artifact_and_roundtrips_wire_models() {
    for arch in ["arm64", "aarch64", "x64", "x86_64"] {
        let release = check_feed(&fixture("0.12.2"), "0.12.1", "macos", arch).unwrap();
        let release: ReleaseCheck =
            serde_json::from_slice(&serde_json::to_vec(&release).unwrap()).unwrap();
        let artifact = release.macos_update(arch).unwrap().unwrap();
        assert_eq!(artifact.sha256, "ab".repeat(32));
        assert!(matches!(artifact.arch.as_str(), "arm64" | "x64"));
        assert_eq!(
            serde_json::from_slice::<UpdateArtifact>(&serde_json::to_vec(&artifact).unwrap())
                .unwrap(),
            artifact
        );
    }
    let mut release = release();
    release.newer = false;
    release.downloads.clear();
    assert_eq!(release.macos_update("unsupported").unwrap(), None);
}

#[test]
fn rejects_missing_or_invalid_hashes_and_sizes_only_for_ota() {
    for hash in [
        None,
        Some("".into()),
        Some("a".repeat(63)),
        Some("a".repeat(65)),
        Some("g".repeat(64)),
    ] {
        let mut release = release();
        release.downloads[0].sha256 = hash;
        assert!(release.macos_update("arm64").is_err());
    }
    for size in [None, Some(0), Some(MAX_ARCHIVE_BYTES + 1), Some(u64::MAX)] {
        let mut release = release();
        release.downloads[0].size = size;
        assert!(release.macos_update("arm64").is_err());
    }
    for size in [1, MAX_ARCHIVE_BYTES] {
        let mut release = release();
        release.downloads[0].size = Some(size);
        assert_eq!(release.macos_update("arm64").unwrap().unwrap().size, size);
    }
    let mut feed: Value = serde_json::from_slice(&fixture("0.12.2")).unwrap();
    feed["resolved"]["macos"]["arm64"][0]["sha256"] = json!("archive-checksum");
    let release = check_feed(
        &serde_json::to_vec(&feed).unwrap(),
        "0.12.1",
        "macos",
        "arm64",
    )
    .unwrap();
    assert!(release.macos_update("arm64").is_err());
}

#[test]
fn rejects_ambiguous_wrong_architecture_and_mismatched_artifacts() {
    let mut duplicate = release();
    duplicate.downloads.push(duplicate.downloads[0].clone());
    assert!(duplicate.macos_update("arm64").is_err());
    assert!(release().macos_update("x64").is_err());
    assert!(release().macos_update("sparc").is_err());
    let mut missing = release();
    missing.downloads[0].name = "Resolved-0.12.1-macos-arm64.zip".into();
    assert!(missing.macos_update("arm64").is_err());
    for path in [
        "v0.12.2/Resolved-0.12.2-macos-x64.zip",
        "v0.12.1/Resolved-0.12.2-macos-arm64.zip",
        "v0.12.2/Other.zip",
    ] {
        let mut release = release();
        release.downloads[0].url =
            format!("https://github.com/zkillua00/resolved/releases/download/{path}");
        assert!(release.macos_update("arm64").is_err());
    }
}

#[test]
fn rejects_invalid_versions_and_malformed_or_oversized_feeds() {
    for version in [
        "not-a-version",
        "v0.12.2",
        "0.12",
        "0.12.2-rc.1",
        "0.12.2.abcdef123456",
    ] {
        assert!(check_feed(&fixture(version), "0.12.1", "macos", "arm64").is_err());
    }
    for installed in [
        "0.12.2.invalid",
        "0.12.2.abcdef12345g",
        "0.12.2.abcdef12345",
        "v0.12.1",
    ] {
        assert!(check_feed(&fixture("0.12.2"), installed, "macos", "arm64").is_err());
    }
    for bytes in [
        b"".as_slice(),
        b"{",
        b"{}",
        br#"{"version":"0.12.2"}"#,
        b"\xff",
    ] {
        assert!(check_feed(bytes, "0.12.1", "macos", "arm64").is_err());
    }
    let mut padded = fixture("0.12.2");
    padded.resize(MAX_FEED_BYTES, b' ');
    assert!(check_feed(&padded, "0.12.1", "macos", "arm64").is_ok());
    padded.push(b' ');
    assert!(check_feed(&padded, "0.12.1", "macos", "arm64").is_err());
}

#[test]
fn rejects_malicious_release_links_including_normalization_tricks() {
    let good = "https://github.com/zkillua00/resolved/releases/download/v0.12.2/app.zip";
    assert!(validate_download_url(good).is_ok());
    assert!(validate_download_url(&good.replace("github.com", "github.com:443")).is_ok());
    let bad = [
        "file:///tmp/download".into(),
        good.replace("https:", "http:"),
        good.replace("github.com", "github.com.evil.example"),
        good.replace("github.com", "github.com:444"),
        good.replace("github.com", "user:pass@github.com"),
        good.replace("github.com", "%67ithub.com"),
        good.replace("github.com", "@github.com"),
        good.replace("zkillua00/resolved", "someone/else"),
        good.replace("v0.12.2/app.zip", "../v0.12.2/app.zip"),
        good.replace("v0.12.2/app.zip", "./v0.12.2/app.zip"),
        good.replace("app.zip", "%61pp.zip"),
        good.replace("app.zip", "%2e%2e/app.zip"),
        good.replace("app.zip", "%252e%252e%252fapp.zip"),
        good.replace("app.zip", "sub%2fapp.zip"),
        good.replace("app.zip", "sub\\app.zip"),
        good.replace("app.zip", ""),
        good.replace("app.zip", "nested/app.zip"),
        format!("{good}?download=1"),
        format!("{good}#fragment"),
        format!(" {good}"),
        format!("{good}\n"),
    ];
    for url in bad {
        assert!(validate_download_url(&url).is_err(), "{url}");
        let mut feed: Value = serde_json::from_slice(&fixture("0.12.2")).unwrap();
        feed["resolved"]["macos"]["arm64"][0]["url"] = json!(url);
        assert!(
            check_feed(
                &serde_json::to_vec(&feed).unwrap(),
                "0.12.1",
                "macos",
                "arm64"
            )
            .is_err()
        );
    }
}

#[test]
fn redirect_allowlist_permits_signed_cdn_queries_but_no_arbitrary_hosts() {
    assert!(validate_asset_redirect(&release().downloads[0].url).is_ok());
    for host in [
        "release-assets.githubusercontent.com",
        "objects.githubusercontent.com",
    ] {
        assert!(
            validate_asset_redirect(&format!("https://{host}/assets/123?sig=a%2Fb&expires=42"))
                .is_ok()
        );
        for value in [
            format!("http://{host}/asset"),
            format!("https://{host}:444/asset"),
            format!("https://user@{host}/asset"),
            format!("https://{host}/asset#fragment"),
            format!("https:///{host}/asset"),
            format!("https://{host}.evil.test/asset"),
        ] {
            assert!(validate_asset_redirect(&value).is_err(), "{value}");
        }
    }
    for value in [
        "https://github.com/someone/else/releases/download/v1/app.zip",
        "https://github.com/zkillua00/resolved/releases/download/v1/app.zip?sig=1",
        "https://apiworkbench.dev/file.zip",
        "https://evil.test/file.zip",
        "file:///tmp/file.zip",
    ] {
        assert!(validate_asset_redirect(value).is_err(), "{value}");
    }
}
