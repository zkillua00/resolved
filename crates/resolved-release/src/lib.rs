//! Offline release-feed parsing and artifact validation shared by the app and updater.
//!
//! Browser downloads need only an allowed release link. In-app downloads also
//! require a versioned ZIP, bounded size, and SHA-256 digest. These checks do not
//! establish publisher authenticity or make an archive eligible for installation.

use std::collections::HashMap;

use semver::Version;
use serde::{Deserialize, Serialize};
use url::Url;

pub const FEED_URL: &str = "https://apiworkbench.dev/downloads.json";
pub const MAX_FEED_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_ARCHIVE_BYTES: u64 = 1024 * 1024 * 1024;
const RELEASE_PATH: &str = "/zkillua00/resolved/releases/download/";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseDownload {
    pub name: String,
    pub url: String,
    pub size: Option<u64>,
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseCheck {
    pub version: String,
    pub newer: bool,
    pub downloads: Vec<ReleaseDownload>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateArtifact {
    pub version: String,
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub arch: String,
}

#[derive(Deserialize)]
struct Downloads {
    version: String,
    resolved: HashMap<String, HashMap<String, Vec<ReleaseDownload>>>,
}

fn stable_version(value: &str) -> Result<Version, String> {
    if value.len() > 128 {
        return Err("The download feed version exceeds the length limit.".to_owned());
    }
    Version::parse(value)
        .ok()
        .filter(|version| version.pre.is_empty())
        .ok_or_else(|| "The download feed contains an invalid stable version.".to_owned())
}

fn mapped_arch(arch: &str) -> &str {
    match arch {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    }
}

/// Parse a bounded feed and select browser downloads for the requested platform.
pub fn check_feed(
    bytes: &[u8],
    installed: &str,
    os: &str,
    arch: &str,
) -> Result<ReleaseCheck, String> {
    if bytes.len() > MAX_FEED_BYTES {
        return Err("The download feed exceeds the size limit.".to_owned());
    }
    let feed: Downloads = serde_json::from_slice(bytes).map_err(|error| {
        // Do not echo untrusted feed values into UI error messages.
        format!(
            "Could not read the download feed (line {}, column {}).",
            error.line(),
            error.column()
        )
    })?;
    let version = stable_version(&feed.version)?;
    let installed = Version::parse(installed)
        .or_else(|error| {
            // Historical nightlies use MAJOR.MINOR.PATCH.<12hex>, not SemVer.
            let Some((base, hash)) = installed.rsplit_once('.') else {
                return Err(error);
            };
            if hash.len() == 12 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                Version::parse(base).and_then(|version| {
                    if version.pre.is_empty() && version.build.is_empty() {
                        Ok(version)
                    } else {
                        Err(error)
                    }
                })
            } else {
                Err(error)
            }
        })
        .map_err(|_| "The installed build has an invalid version.".to_owned())?;
    let downloads = feed
        .resolved
        .get(os)
        .and_then(|platform| platform.get(mapped_arch(arch)))
        .cloned()
        .unwrap_or_default();
    for download in &downloads {
        validate_download_url(&download.url)?;
    }
    Ok(ReleaseCheck {
        version: version.to_string(),
        newer: version.cmp_precedence(&installed).is_gt(),
        downloads,
    })
}

impl ReleaseCheck {
    /// Select an integrity-verifiable macOS ZIP, retaining legacy browser fallback.
    pub fn macos_update(&self, arch: &str) -> Result<Option<UpdateArtifact>, String> {
        if !self.newer {
            return Ok(None);
        }
        stable_version(&self.version)?;
        let arch = mapped_arch(arch);
        if !matches!(arch, "arm64" | "x64") {
            return Err("Unsupported macOS update architecture.".to_owned());
        }
        let name = format!("Resolved-{}-macos-{arch}.zip", self.version);
        let mut matches = self
            .downloads
            .iter()
            .filter(|download| download.name == name);
        let download = matches
            .next()
            .ok_or_else(|| "No matching macOS update ZIP is listed.".to_owned())?;
        if matches.next().is_some() {
            return Err("The macOS update ZIP is ambiguous.".to_owned());
        }
        validate_download_url(&download.url)?;
        let url = Url::parse(&download.url).map_err(|error| error.to_string())?;
        let path = url.path().strip_prefix(RELEASE_PATH).unwrap();
        let (tag, asset) = path.split_once('/').unwrap();
        if asset != name || (tag != self.version && tag != format!("v{}", self.version)) {
            return Err("The macOS update URL does not match its version and filename.".to_owned());
        }
        let size = download
            .size
            .filter(|size| *size > 0 && *size <= MAX_ARCHIVE_BYTES)
            .ok_or_else(|| "The macOS update ZIP has a missing or invalid size.".to_owned())?;
        let sha256 = download
            .sha256
            .as_deref()
            .filter(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or_else(|| "The macOS update ZIP has a missing or invalid SHA-256.".to_owned())?;
        Ok(Some(UpdateArtifact {
            version: self.version.clone(),
            name,
            url: download.url.clone(),
            size,
            sha256: sha256.to_ascii_lowercase(),
            arch: arch.to_owned(),
        }))
    }
}

// Validate raw syntax before URL parsing, which normalizes dot segments and backslashes.
fn https_url(value: &str) -> Result<Url, String> {
    let invalid = || "Invalid HTTPS release URL.".to_owned();
    if !value.starts_with("https://")
        || value
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return Err(invalid());
    }
    let url = Url::parse(value).map_err(|_| invalid())?;
    let authority = value[8..].split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty()
        || authority.contains(['@', '%'])
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(443)
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    Ok(url)
}

/// Accept only unambiguous GitHub release assets in the official repository.
pub fn validate_download_url(value: &str) -> Result<(), String> {
    let url = https_url(value)?;
    let invalid = || "The download feed contains an invalid GitHub release link.".to_owned();
    // Do not let URL normalization hide traversal or encoded path separators.
    let raw_path = value[8..]
        .find('/')
        .map(|offset| &value[8 + offset..])
        .unwrap_or("");
    if url.host_str() != Some("github.com")
        || url.query().is_some()
        || raw_path.contains('%')
        || raw_path != url.path()
    {
        return Err(invalid());
    }
    let suffix = url.path().strip_prefix(RELEASE_PATH).ok_or_else(invalid)?;
    let segments: Vec<_> = suffix.split('/').collect();
    if segments.len() != 2
        || segments
            .iter()
            .any(|part| part.is_empty() || matches!(*part, "." | ".."))
    {
        return Err(invalid());
    }
    Ok(())
}

/// Validate an archive redirect destination. CDN signed query strings are allowed.
pub fn validate_asset_redirect(value: &str) -> Result<(), String> {
    let url = https_url(value)?;
    match url.host_str() {
        Some("github.com") => validate_download_url(value),
        Some("release-assets.githubusercontent.com" | "objects.githubusercontent.com") => Ok(()),
        _ => Err("The archive redirect has an untrusted host.".to_owned()),
    }
}
