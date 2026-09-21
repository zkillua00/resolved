// Shared with unit tests. This guard controls the build entry point, not trust.
pub fn validate_target(marker: &str, os: &str, arch: &str) -> Result<&'static str, &'static str> {
    if marker != "1" {
        return Err(
            "build resolved-updater only through scripts/bundle-macos.sh (RESOLVED_UPDATER_BUILD=1 required)",
        );
    }
    if os != "macos" {
        return Err("resolved-updater requires a macOS target");
    }
    match arch {
        "aarch64" => Ok("arm64"),
        "x86_64" => Ok("x64"),
        _ => Err("resolved-updater supports only aarch64 and x86_64"),
    }
}

pub fn validate_version(value: &str) -> Result<(), &'static str> {
    // Bounded, printable ASCII identity; accept stable and nightly display versions.
    // Reject whitespace/control characters, including cargo directive injection.
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b".+-_".contains(&c))
        || !value.as_bytes()[0].is_ascii_alphanumeric()
    {
        return Err(
            "version must be 1..128 ASCII letters/digits/dot/plus/hyphen/underscore, beginning with a letter or digit",
        );
    }
    Ok(())
}

pub fn validate_build_number(value: &str) -> Result<(), &'static str> {
    if value.is_empty()
        || !value.bytes().all(|c| c.is_ascii_digit())
        || !matches!(value.parse::<u64>(), Ok(1..))
    {
        return Err("build number must be a nonzero decimal u64");
    }
    Ok(())
}
