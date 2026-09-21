//! Native publisher verification. A tree digest is only transaction identity:
//! every trust decision also requires Apple's signature and assessment tools.
use crate::download::{Failure, Result};
use resolved_release::UpdateArtifact;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    ffi::{CString, OsStr, OsString},
    fs::{self, File, Metadata, OpenOptions},
    io::Read,
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::{AsRawFd, FromRawFd, IntoRawFd},
    },
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Child,
};

const APP_ID: &str = "dev.apitester.desktop";
const HELPER_ID: &str = "dev.apitester.desktop.updater";
const PLIST_LIMIT: u64 = 1024 * 1024;
const OUTPUT_LIMIT: usize = 2 * 1024 * 1024;
const COMMAND_LIMIT: Duration = Duration::from_secs(60);
const REAP_LIMIT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
pub struct BundleIdentity {
    pub version: String,
    pub build: String,
    pub team_id: String,
    pub fingerprint: String,
}

#[derive(Clone, Debug)]
pub struct Host {
    pub path: PathBuf,
    pub identity: BundleIdentity,
}

fn invalid() -> Failure {
    Failure::new("verification", "Application verification failed.")
}

fn policy_unavailable() -> Failure {
    Failure::new(
        "verification",
        "Required macOS notarization policy could not be established.",
    )
}

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.start_kill();
        let deadline = Instant::now() + REAP_LIMIT;
        loop {
            match self.0.try_wait() {
                Ok(Some(_)) | Err(_) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Ok(None) => break,
            }
        }
    }
}

struct Output {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

async fn bounded_read(mut reader: impl tokio::io::AsyncRead + Unpin) -> Result<Vec<u8>> {
    let mut result = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let count = reader.read(&mut buffer).await.map_err(|_| invalid())?;
        if count == 0 {
            return Ok(result);
        }
        if result.len() + count > OUTPUT_LIMIT {
            return Err(invalid());
        }
        result.extend_from_slice(&buffer[..count]);
    }
}

async fn command(program: &'static str, args: &[OsString]) -> Result<Output> {
    command_input(program, args, None).await
}

async fn command_input(
    program: &'static str,
    args: &[OsString],
    input: Option<&[u8]>,
) -> Result<Output> {
    // No shell, inherited HOME, DYLD variables, PATH lookup, or user locale.
    if !matches!(
        program,
        "/usr/bin/codesign" | "/usr/sbin/spctl" | "/usr/bin/plutil" | "/usr/bin/sw_vers"
    ) {
        return Err(invalid());
    }
    let mut child = OwnedChild(
        tokio::process::Command::new(program)
            .args(args)
            .env_clear()
            .env("LANG", "C")
            .env("LC_ALL", "C")
            .current_dir("/")
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| invalid())?,
    );
    let stdout = child.0.stdout.take().ok_or_else(invalid)?;
    let stderr = child.0.stderr.take().ok_or_else(invalid)?;
    let stdin = child.0.stdin.take();
    tokio::time::timeout(COMMAND_LIMIT, async {
        let (stdout, stderr, status, ()) = tokio::try_join!(
            bounded_read(stdout),
            bounded_read(stderr),
            async { child.0.wait().await.map_err(|_| invalid()) },
            async {
                if let Some(input) = input {
                    let mut stdin = stdin.ok_or_else(invalid)?;
                    stdin.write_all(input).await.map_err(|_| invalid())?;
                    stdin.shutdown().await.map_err(|_| invalid())?;
                }
                Ok(())
            }
        )?;
        Ok(Output {
            success: status.success(),
            stdout,
            stderr,
        })
    })
    .await
    .map_err(|_| invalid())?
}

fn args(values: &[&str], path: &Path) -> Vec<OsString> {
    values
        .iter()
        .map(OsString::from)
        .chain([OsString::from("--"), path.as_os_str().to_owned()])
        .collect()
}

fn valid_team(team: &str) -> bool {
    team.len() == 10 && team.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn requirement(identifier: &str, team: Option<&str>) -> Result<String> {
    if !matches!(identifier, APP_ID | HELPER_ID) || team.is_some_and(|team| !valid_team(team)) {
        return Err(invalid());
    }
    // The intermediate and leaf OIDs distinguish Developer ID Application
    // from Apple Development, Mac App Store, and Developer ID Installer.
    let mut result = format!(
        "=anchor apple generic and identifier \"{identifier}\" \
         and certificate 1[field.1.2.840.113635.100.6.2.6] exists \
         and certificate leaf[field.1.2.840.113635.100.6.1.13] exists"
    );
    if let Some(team) = team {
        result.push_str(&format!(" and certificate leaf[subject.OU] = \"{team}\""));
    }
    Ok(result)
}

async fn signature(path: &Path, identifier: &str, team: Option<&str>) -> Result<()> {
    let requirement = requirement(identifier, team)?;
    let output = command(
        "/usr/bin/codesign",
        &args(
            &[
                "--verify",
                "--deep",
                "--strict",
                "--all-architectures",
                "--test-requirement",
                &requirement,
            ],
            path,
        ),
    )
    .await?;
    if !output.success {
        return Err(invalid());
    }
    Ok(())
}

fn parse_team(metadata: &[u8]) -> Result<String> {
    let text = std::str::from_utf8(metadata).map_err(|_| invalid())?;
    let mut teams = text
        .lines()
        .filter_map(|line| line.strip_prefix("TeamIdentifier="));
    let team = teams.next().ok_or_else(invalid)?;
    if !valid_team(team) || teams.next().is_some() {
        return Err(invalid());
    }
    Ok(team.to_owned())
}

async fn signing_team(path: &Path) -> Result<String> {
    let output = command(
        "/usr/bin/codesign",
        &args(&["--display", "--verbose=4"], path),
    )
    .await?;
    if !output.success || !output.stdout.is_empty() {
        return Err(invalid());
    }
    parse_team(&output.stderr)
}

fn valid_cdhash(hash: &str) -> bool {
    hash.len() == 40 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn parse_cdhash(metadata: &[u8]) -> Result<String> {
    let text = std::str::from_utf8(metadata).map_err(|_| invalid())?;
    let mut hashes = text.lines().filter_map(|line| line.strip_prefix("CDHash="));
    let hash = hashes.next().ok_or_else(invalid)?;
    if !valid_cdhash(hash) || hashes.next().is_some() {
        return Err(invalid());
    }
    Ok(hash.to_ascii_lowercase())
}

async fn cdhash(target: &Path) -> Result<String> {
    let output = command(
        "/usr/bin/codesign",
        &args(&["--display", "--verbose=4"], target),
    )
    .await?;
    if !output.success || !output.stdout.is_empty() {
        return Err(invalid());
    }
    parse_cdhash(&output.stderr)
}

fn process_target(pid: u32) -> Result<PathBuf> {
    if pid == 0 || pid > i32::MAX as u32 {
        return Err(invalid());
    }
    Ok(PathBuf::from(format!("+{pid}")))
}

fn running_requirement(team: &str, hash: &str) -> Result<String> {
    if !valid_cdhash(hash) {
        return Err(invalid());
    }
    // Bind the final dynamic verification to the exact CodeDirectory as well
    // as its publisher. An exec between the display and verify operations must
    // not turn a metadata comparison into acceptance of a different binary.
    Ok(format!(
        "{} and cdhash H\"{}\"",
        requirement(APP_ID, Some(team))?,
        hash.to_ascii_lowercase()
    ))
}

async fn running_signature(target: &Path, requirement: &str) -> Result<()> {
    let output = command(
        "/usr/bin/codesign",
        &args(&["--verify", "--test-requirement", requirement], target),
    )
    .await?;
    if !output.success {
        return Err(invalid());
    }
    Ok(())
}

/// The caller must provide a freshly verified host and retain its own process
/// lifetime observation (kqueue), so PID reuse cannot masquerade as that host.
pub async fn validate_running_host(pid: u32, host: &Host) -> Result<()> {
    let target = process_target(pid)?;
    if unsafe { libc::kill(pid as i32, 0) } != 0
        || fingerprint(&host.path)? != host.identity.fingerprint
    {
        return Err(invalid());
    }
    signature(&host.path, APP_ID, Some(&host.identity.team_id)).await?;
    let disk_hash = cdhash(&host.path).await?;
    let requirement = running_requirement(&host.identity.team_id, &disk_hash)?;
    running_signature(&target, &requirement).await?;
    if cdhash(&target).await? != disk_hash || fingerprint(&host.path)? != host.identity.fingerprint
    {
        return Err(invalid());
    }
    // Recheck dynamic validity after all metadata/filesystem queries, then
    // verify that the process still exists. This does not replace kqueue.
    running_signature(&target, &requirement).await?;
    if unsafe { libc::kill(pid as i32, 0) } != 0 {
        return Err(invalid());
    }
    Ok(())
}

fn assessment_enabled(output: &Output) -> bool {
    output.success && output.stdout == b"assessments enabled\n" && output.stderr.is_empty()
}

fn contains_override(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            key.to_ascii_lowercase().contains("override") || contains_override(value)
        }),
        Value::Array(values) => values.iter().any(contains_override),
        _ => false,
    }
}

fn notarized_assessment(json: &[u8]) -> Result<()> {
    // This is a necessary assessment-shape check, not a notarization proof:
    // local policy can give an arbitrary rule this same source label.
    if json.len() > OUTPUT_LIMIT {
        return Err(policy_unavailable());
    }
    let value: Value = serde_json::from_slice(json).map_err(|_| policy_unavailable())?;
    let object = value.as_object().ok_or_else(policy_unavailable)?;
    let authority = object
        .get("assessment:authority")
        .and_then(Value::as_object)
        .ok_or_else(policy_unavailable)?;
    if object.get("assessment:verdict") != Some(&Value::Bool(true))
        || authority
            .get("assessment:authority:source")
            .and_then(Value::as_str)
            != Some("Notarized Developer ID")
        || contains_override(&value)
    {
        return Err(policy_unavailable());
    }
    Ok(())
}

fn notarization_proof(json: &[u8], ticket_check: &Output) -> Result<()> {
    notarized_assessment(json)?;
    if !ticket_check.success {
        return Err(policy_unavailable());
    }
    Ok(())
}

async fn notarization(path: &Path) -> Result<()> {
    let status_args = [OsString::from("--status")];
    if !assessment_enabled(&command("/usr/sbin/spctl", &status_args).await?) {
        return Err(policy_unavailable());
    }
    // Raw authority is necessary: exit zero alone can mean a user/global
    // override. Its source label is NOT sufficient notarization evidence.
    // Ignore and do not populate the assessment cache. No CLT/stapler and no
    // --check-notarization (which would force a separate network-only check).
    let assessment = command(
        "/usr/sbin/spctl",
        &args(
            &[
                "--assess",
                "--type",
                "execute",
                "--raw",
                "--ignore-cache",
                "--no-cache",
            ],
            path,
        ),
    )
    .await?;
    if !assessment.success || assessment.stdout.len() as u64 > PLIST_LIMIT {
        return Err(policy_unavailable());
    }
    let json = command_input(
        "/usr/bin/plutil",
        &args(&["-convert", "json", "-o", "-"], Path::new("-")),
        Some(&assessment.stdout),
    )
    .await?;
    if !json.success {
        return Err(policy_unavailable());
    }
    notarized_assessment(&json.stdout)?;
    // Assess first so macOS can ingest a stapled ticket, then independently
    // test Apple's notarization requirement. A custom spctl rule labelled
    // "Notarized Developer ID" cannot satisfy this predicate. This is an
    // OUTER-app check: the nested helper retains its own ID/team verification.
    let ticket_check = command(
        "/usr/bin/codesign",
        &args(
            &[
                "--verify",
                "--strict",
                "--all-architectures",
                "--test-requirement",
                "=notarized",
            ],
            path,
        ),
    )
    .await?;
    notarization_proof(&json.stdout, &ticket_check)?;
    if !assessment_enabled(&command("/usr/sbin/spctl", &status_args).await?) {
        return Err(policy_unavailable());
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct Info {
    version: String,
    build: String,
    minimum: Vec<u32>,
}

fn field<'a>(object: &'a serde_json::Map<String, Value>, key: &str) -> Result<&'a str> {
    let value = object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(invalid)?;
    if value.is_empty() || value.len() > 128 || !value.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(invalid());
    }
    Ok(value)
}

fn os_version(version: &str) -> Result<Vec<u32>> {
    let parts = version.split('.').collect::<Vec<_>>();
    if !(2..=3).contains(&parts.len()) {
        return Err(invalid());
    }
    let mut numbers = Vec::new();
    for part in parts {
        if part.is_empty() || part.len() > 5 || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid());
        }
        numbers.push(part.parse::<u32>().map_err(|_| invalid())?);
    }
    numbers.resize(3, 0);
    if numbers[0] < 10 {
        return Err(invalid());
    }
    Ok(numbers)
}

fn positive_build(build: &str) -> bool {
    !build.is_empty()
        && build.bytes().all(|byte| byte.is_ascii_digit())
        && build.parse::<u64>().is_ok_and(|number| number > 0)
}

fn parse_info(json: &[u8], expected_version: &str) -> Result<Info> {
    if json.len() > OUTPUT_LIMIT {
        return Err(invalid());
    }
    let value: Value = serde_json::from_slice(json).map_err(|_| invalid())?;
    let object = value.as_object().ok_or_else(invalid)?;
    if field(object, "CFBundleIdentifier")? != APP_ID
        || field(object, "CFBundleExecutable")? != "api-tester"
        || field(object, "CFBundlePackageType")? != "APPL"
        || field(object, "CFBundleShortVersionString")? != expected_version
        // Per-architecture minimums would introduce a second source of truth.
        || object.contains_key("LSMinimumSystemVersionByArchitecture")
    {
        return Err(invalid());
    }
    let build = field(object, "CFBundleVersion")?;
    if !positive_build(build) {
        return Err(invalid());
    }
    Ok(Info {
        version: expected_version.to_owned(),
        build: build.to_owned(),
        minimum: os_version(field(object, "LSMinimumSystemVersion")?)?,
    })
}

async fn info(path: &Path, version: &str) -> Result<Info> {
    let file = open_relative(&open_root(path)?, Path::new("Contents/Info.plist"))?;
    let metadata = file.metadata().map_err(|_| invalid())?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > PLIST_LIMIT {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    file.take(PLIST_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    if bytes.len() as u64 != metadata.len() || bytes.len() as u64 > PLIST_LIMIT {
        return Err(invalid());
    }
    let output = command_input(
        "/usr/bin/plutil",
        &args(&["-convert", "json", "-o", "-"], Path::new("-")),
        Some(&bytes),
    )
    .await?;
    if !output.success {
        return Err(invalid());
    }
    parse_info(&output.stdout, version)
}

fn macho_header(header: &[u8], arch: &str) -> Result<()> {
    if header.len() != 32 || header[..4] != [0xcf, 0xfa, 0xed, 0xfe] {
        return Err(invalid());
    }
    let word = |offset| u32::from_le_bytes(header[offset..offset + 4].try_into().unwrap());
    let cpu = match arch {
        "arm64" => 0x0100_000c,
        "x64" => 0x0100_0007,
        _ => return Err(invalid()),
    };
    // MH_EXECUTE only. Reject universal, 32-bit, big-endian, dylibs, and objects.
    if word(4) != cpu || word(12) != 2 {
        return Err(invalid());
    }
    // V1 ships baseline arm64 / x86_64, not arm64e or x86_64h.
    let subtype = if arch == "arm64" { 0 } else { 3 };
    if word(8) != subtype {
        return Err(invalid());
    }
    Ok(())
}

fn executable(bundle: &Path, relative: &str, arch: &str) -> Result<()> {
    let mut file = open_relative(&open_root(bundle)?, Path::new(relative))?;
    let metadata = file.metadata().map_err(|_| invalid())?;
    if !metadata.is_file() || metadata.mode() & 0o111 == 0 {
        return Err(invalid());
    }
    let mut header = [0; 32];
    file.read_exact(&mut header).map_err(|_| invalid())?;
    macho_header(&header, arch)
}

async fn verify(path: &Path, version: &str, team: &str) -> Result<BundleIdentity> {
    let before = fingerprint(path)?;
    signature(path, APP_ID, Some(team)).await?;
    let info = info(path, version).await?;
    let arch = env!("RESOLVED_UPDATER_ARCH");
    executable(path, "Contents/MacOS/api-tester", arch)?;
    let helper = path.join("Contents/Helpers/resolved-updater");
    executable(path, "Contents/Helpers/resolved-updater", arch)?;
    signature(&helper, HELPER_ID, Some(team)).await?;
    let output = command("/usr/bin/sw_vers", &[OsString::from("-productVersion")]).await?;
    if !output.success {
        return Err(invalid());
    }
    let running = std::str::from_utf8(&output.stdout).map_err(|_| invalid())?;
    if info.minimum > os_version(running.trim_end_matches('\n'))? {
        return Err(invalid());
    }
    notarization(path).await?;
    if before != fingerprint(path)? {
        return Err(invalid());
    }
    Ok(BundleIdentity {
        version: info.version,
        build: info.build,
        team_id: team.to_owned(),
        fingerprint: before,
    })
}

fn derive_host_path(executable: &Path) -> Result<PathBuf> {
    if executable.file_name() != Some(OsStr::new("resolved-updater")) {
        return Err(invalid());
    }
    let helpers = executable.parent().ok_or_else(invalid)?;
    let contents = helpers.parent().ok_or_else(invalid)?;
    let bundle = contents.parent().ok_or_else(invalid)?;
    if helpers.file_name() != Some(OsStr::new("Helpers"))
        || contents.file_name() != Some(OsStr::new("Contents"))
        || bundle.file_name() != Some(OsStr::new("Resolved.app"))
    {
        return Err(invalid());
    }
    Ok(bundle.to_owned())
}

/// Locate only; this does not establish signature, publisher, or policy trust.
pub fn host_path() -> Result<PathBuf> {
    derive_host_path(&std::env::current_exe().map_err(|_| invalid())?)
}

fn stable_version(version: &str) -> bool {
    let parts = version.split('.').collect::<Vec<_>>();
    (2..=3).contains(&parts.len())
        && parts.iter().all(|part| {
            !part.is_empty() && part.len() <= 10 && part.bytes().all(|byte| byte.is_ascii_digit())
        })
}

pub async fn host() -> Result<Host> {
    let version = env!("RESOLVED_UPDATER_APP_VERSION");
    if cfg!(debug_assertions)
        || env!("RESOLVED_BUILD_VERSION") != version
        || !stable_version(version)
    {
        return Err(invalid());
    }
    let path = host_path()?;
    // Establish native Apple/Developer ID trust BEFORE treating metadata as a
    // publisher identity. No feed field supplies this value.
    let before = fingerprint(&path)?;
    signature(&path, APP_ID, None).await?;
    let team = signing_team(&path).await?;
    let identity = verify(&path, version, &team).await?;
    if identity.fingerprint != before {
        return Err(invalid());
    }
    Ok(Host { path, identity })
}

pub async fn candidate(
    bundle: &Path,
    host: &Host,
    artifact: &UpdateArtifact,
) -> Result<BundleIdentity> {
    if artifact.arch != env!("RESOLVED_UPDATER_ARCH") || !stable_version(&artifact.version) {
        return Err(invalid());
    }
    verify(bundle, &artifact.version, &host.identity.team_id).await
}

pub async fn validate_identity(bundle: &Path, expected: &BundleIdentity) -> Result<()> {
    let actual = verify(bundle, &expected.version, &expected.team_id).await?;
    if actual.version != expected.version
        || actual.build != expected.build
        || actual.team_id != expected.team_id
        || actual.fingerprint != expected.fingerprint
    {
        return Err(invalid());
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct Limits {
    depth: usize,
    entries: usize,
    bytes: u64,
}

const TREE_LIMITS: Limits = Limits {
    depth: 64,
    entries: 100_000,
    bytes: 4 * 1024 * 1024 * 1024,
};

#[derive(Debug, PartialEq, Eq)]
struct Stamp {
    device: u64,
    inode: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    links: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

fn stamp(metadata: &Metadata) -> Result<Stamp> {
    if (!metadata.is_file() && !metadata.is_dir())
        || metadata.mode() & 0o7022 != 0
        || (metadata.uid() != 0 && metadata.uid() != unsafe { libc::geteuid() })
        || (metadata.is_file() && metadata.nlink() != 1)
    {
        return Err(invalid());
    }
    Ok(Stamp {
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.mode(),
        uid: metadata.uid(),
        gid: metadata.gid(),
        links: metadata.nlink(),
        size: metadata.len(),
        modified: (metadata.mtime(), metadata.mtime_nsec()),
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    })
}

fn open_root(path: &Path) -> Result<File> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
        || fs::canonicalize(path).map_err(|_| invalid())? != path
    {
        return Err(invalid());
    }
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| invalid())
}

fn open_child(parent: &File, name: &OsStr) -> Result<File> {
    let name = CString::new(name.as_bytes()).map_err(|_| invalid())?;
    let mut before = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            before.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(invalid());
    }
    let before = unsafe { before.assume_init() };
    if !matches!(
        before.st_mode as u32 & libc::S_IFMT as u32,
        mode if mode == libc::S_IFREG as u32 || mode == libc::S_IFDIR as u32
    ) {
        return Err(invalid());
    }
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(invalid());
    }
    let file = unsafe { File::from_raw_fd(fd) };
    let after = file.metadata().map_err(|_| invalid())?;
    if before.st_dev as u64 != after.dev() || before.st_ino != after.ino() {
        return Err(invalid());
    }
    Ok(file)
}

fn open_relative(root: &File, path: &Path) -> Result<File> {
    let mut file = root.try_clone().map_err(|_| invalid())?;
    for part in path.components() {
        let Component::Normal(name) = part else {
            return Err(invalid());
        };
        file = open_child(&file, name)?;
    }
    Ok(file)
}

#[derive(Debug, PartialEq, Eq)]
struct Entry {
    path: PathBuf,
    stamp: Stamp,
}

struct Directory(*mut libc::DIR);

impl Drop for Directory {
    fn drop(&mut self) {
        unsafe { libc::closedir(self.0) };
    }
}

fn names(directory: &File, limit: usize) -> Result<Vec<OsString>> {
    // Open "." rather than dup: each traversal needs its own directory offset.
    let file = open_child(directory, OsStr::new("."))?;
    let stream = unsafe { libc::fdopendir(file.as_raw_fd()) };
    if stream.is_null() {
        return Err(invalid());
    }
    let _ = file.into_raw_fd(); // fdopendir now owns the descriptor.
    let stream = Directory(stream);
    let mut names = Vec::new();
    loop {
        #[cfg(target_os = "macos")]
        let errno = unsafe { libc::__error() };
        #[cfg(not(target_os = "macos"))]
        let errno = unsafe { libc::__errno_location() };
        unsafe { *errno = 0 };
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            if unsafe { *errno } != 0 {
                return Err(invalid());
            }
            return Ok(names);
        }
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        if names.len() >= limit
            || name.is_empty()
            || !name.iter().all(|byte| (0x20..=0x7e).contains(byte))
            || name.contains(&b'\\')
        {
            return Err(invalid());
        }
        names.push(OsStr::from_bytes(name).to_owned());
    }
}

fn collect(
    directory: &File,
    relative: &Path,
    limits: Limits,
    entries: &mut Vec<Entry>,
    bytes: &mut u64,
) -> Result<()> {
    if relative.components().count() > limits.depth || entries.len() >= limits.entries {
        return Err(invalid());
    }
    let metadata = directory.metadata().map_err(|_| invalid())?;
    let before = stamp(&metadata)?;
    entries.push(Entry {
        path: relative.to_owned(),
        stamp: before,
    });
    // Enumerate and open relative to retained descriptors, never following a
    // directory symlink if an attacker replaces a path during traversal.
    for name in names(directory, limits.entries - entries.len())? {
        if entries.len() >= limits.entries {
            return Err(invalid());
        }
        let child_path = relative.join(&name);
        if child_path.as_os_str().as_bytes().len() > 4096
            || child_path.components().count() > limits.depth
        {
            return Err(invalid());
        }
        let child = open_child(directory, &name)?;
        let metadata = child.metadata().map_err(|_| invalid())?;
        let child_stamp = stamp(&metadata)?;
        if metadata.dev() != directory.metadata().map_err(|_| invalid())?.dev() {
            return Err(invalid());
        }
        if metadata.is_dir() {
            collect(&child, &child_path, limits, entries, bytes)?;
        } else {
            *bytes = bytes.checked_add(metadata.len()).ok_or_else(invalid)?;
            if *bytes > limits.bytes {
                return Err(invalid());
            }
            entries.push(Entry {
                path: child_path,
                stamp: child_stamp,
            });
        }
    }
    if stamp(&directory.metadata().map_err(|_| invalid())?)? != stamp(&metadata)? {
        return Err(invalid());
    }
    Ok(())
}

fn inventory(root: &File, limits: Limits) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    collect(root, Path::new(""), limits, &mut entries, &mut 0)?;
    entries.sort_by(|left, right| {
        left.path
            .as_os_str()
            .as_bytes()
            .cmp(right.path.as_os_str().as_bytes())
    });
    Ok(entries)
}

fn fingerprint_with_limits(bundle: &Path, limits: Limits) -> Result<String> {
    let root = open_root(bundle)?;
    let entries = inventory(&root, limits)?;
    let mut hash = Sha256::new();
    hash.update(b"Resolved bundle fingerprint v1\0");
    for entry in &entries {
        let mut file = open_relative(&root, &entry.path)?;
        let metadata = file.metadata().map_err(|_| invalid())?;
        if stamp(&metadata)? != entry.stamp {
            return Err(invalid());
        }
        let path = entry.path.as_os_str().as_bytes();
        hash.update((path.len() as u64).to_le_bytes());
        hash.update(path);
        hash.update(if metadata.is_dir() { b"D" } else { b"F" });
        hash.update((metadata.mode() & 0o7777).to_le_bytes());
        if metadata.is_file() {
            hash.update(metadata.len().to_le_bytes());
            let mut remaining = metadata.len();
            let mut buffer = [0; 64 * 1024];
            while remaining > 0 {
                let wanted = usize::try_from(remaining.min(buffer.len() as u64)).unwrap();
                let count = file.read(&mut buffer[..wanted]).map_err(|_| invalid())?;
                if count == 0 {
                    return Err(invalid());
                }
                remaining -= count as u64;
                hash.update(&buffer[..count]);
            }
            if file.read(&mut buffer[..1]).map_err(|_| invalid())? != 0 {
                return Err(invalid());
            }
        }
        if stamp(&file.metadata().map_err(|_| invalid())?)? != entry.stamp {
            return Err(invalid());
        }
    }
    // Detect insertions, removals, replacements and concurrent content/mode
    // mutations, including replacement of the bundle directory itself.
    if inventory(&root, limits)? != entries
        || stamp(&open_root(bundle)?.metadata().map_err(|_| invalid())?)? != entries[0].stamp
    {
        return Err(invalid());
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn fingerprint(bundle: &Path) -> Result<String> {
    fingerprint_with_limits(bundle, TREE_LIMITS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn plist() -> Value {
        serde_json::json!({
            "CFBundleIdentifier": APP_ID,
            "CFBundleExecutable": "api-tester",
            "CFBundlePackageType": "APPL",
            "CFBundleShortVersionString": "1.2.3",
            "CFBundleVersion": "42",
            "LSMinimumSystemVersion": "12.0"
        })
    }

    #[test]
    fn plist_types_and_metadata_are_strict() {
        assert!(parse_info(&serde_json::to_vec(&plist()).unwrap(), "1.2.3").is_ok());
        for json in [b"".as_slice(), b"[]", b"null", b"{", b"{\"a\":NaN}"] {
            assert!(parse_info(json, "1.2.3").is_err());
        }
        for key in [
            "CFBundleIdentifier",
            "CFBundleExecutable",
            "CFBundlePackageType",
            "CFBundleShortVersionString",
            "CFBundleVersion",
            "LSMinimumSystemVersion",
        ] {
            for value in [
                Value::Null,
                serde_json::json!(42),
                serde_json::json!(true),
                serde_json::json!([]),
                serde_json::json!(""),
                serde_json::json!("x".repeat(129)),
                serde_json::json!("bad\nvalue"),
                serde_json::json!("wrong"),
            ] {
                let mut json = plist();
                json[key] = value;
                assert!(parse_info(&serde_json::to_vec(&json).unwrap(), "1.2.3").is_err());
            }
            let mut json = plist();
            json.as_object_mut().unwrap().remove(key);
            assert!(parse_info(&serde_json::to_vec(&json).unwrap(), "1.2.3").is_err());
        }
        assert!(parse_info(&serde_json::to_vec(&plist()).unwrap(), "1.2.4").is_err());
        for build in ["0", "-1", "+1", "1.0", "18446744073709551616"] {
            assert!(!positive_build(build));
        }
        assert!(os_version("12.0").unwrap() < os_version("12.0.1").unwrap());
        for minimum in ["", "12", "12.0.1.0", "12.-1", "12. 0", "12.0\n"] {
            assert!(os_version(minimum).is_err());
        }
    }

    fn header(arch: &str) -> [u8; 32] {
        let mut header = [0; 32];
        header[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
        let (cpu, subtype): (u32, u32) = if arch == "arm64" {
            (0x0100_000c, 0)
        } else {
            (0x0100_0007, 3)
        };
        header[4..8].copy_from_slice(&cpu.to_le_bytes());
        header[8..12].copy_from_slice(&subtype.to_le_bytes());
        header[12..16].copy_from_slice(&2u32.to_le_bytes());
        header
    }

    #[test]
    fn thin_native_executable_only() {
        for arch in ["arm64", "x64"] {
            let good = header(arch);
            assert!(macho_header(&good, arch).is_ok());
            assert!(macho_header(&good[..31], arch).is_err());
            assert!(macho_header(&good, "other").is_err());
            for offset in [0, 4, 8, 12] {
                let mut bad = good;
                bad[offset] ^= 1;
                assert!(macho_header(&bad, arch).is_err());
            }
            for filetype in [1u32, 6, 8, 9] {
                let mut bad = good;
                bad[12..16].copy_from_slice(&filetype.to_le_bytes());
                assert!(macho_header(&bad, arch).is_err());
            }
        }
        assert!(macho_header(&header("arm64"), "x64").is_err());
        assert!(macho_header(&header("x64"), "arm64").is_err());
        for magic in [[0xca, 0xfe, 0xba, 0xbe], [0xfe, 0xed, 0xfa, 0xcf]] {
            let mut bad = header("arm64");
            bad[..4].copy_from_slice(&magic);
            assert!(macho_header(&bad, "arm64").is_err());
        }
    }

    #[test]
    fn publisher_requirement_cannot_be_injected() {
        let requirement = requirement(APP_ID, Some("ABCDE12345")).unwrap();
        assert!(requirement.contains("anchor apple generic"));
        assert!(requirement.contains("1.2.840.113635.100.6.1.13"));
        assert!(requirement.contains("1.2.840.113635.100.6.2.6"));
        assert!(requirement.contains("subject.OU] = \"ABCDE12345\""));
        for team in [
            "",
            "adhoc",
            "ABCDEFGHIJK",
            "ABCD 12345",
            "ABCD\"12345",
            "éBCD12345",
        ] {
            assert!(super::requirement(APP_ID, Some(team)).is_err());
        }
        assert!(super::requirement("x\" or true", None).is_err());
        assert_eq!(
            parse_team(b"Authority=ignored\nTeamIdentifier=ABCDE12345\n").unwrap(),
            "ABCDE12345"
        );
        for metadata in [
            b"TeamIdentifier=not set\n".as_slice(),
            b"TeamIdentifier=ABCDE12345\nTeamIdentifier=ABCDE12345\n",
            b" TeamIdentifier=ABCDE12345\n",
            b"TeamIdentifier=ABCDE12345 \n",
            b"Signature=adhoc\n",
        ] {
            assert!(parse_team(metadata).is_err());
        }
    }

    #[test]
    fn live_process_hash_and_requirement_are_unambiguous() {
        let hash = "0123456789abcdef0123456789ABCDEF01234567";
        assert_eq!(hash.len(), 40);
        let text = format!("CandidateCDHash sha256=ignored\nCDHash={hash}\n");
        assert_eq!(
            parse_cdhash(text.as_bytes()).unwrap(),
            hash.to_ascii_lowercase()
        );
        for text in [
            String::new(),
            format!("CDHash={hash}\nCDHash={hash}\n"),
            format!("CDHash={hash} \n"),
            format!(" CDHash={hash}\n"),
            format!("CandidateCDHash sha256={hash}\n"),
            format!("CDHash={}g\n", "a".repeat(39)),
            format!("CDHash={}\n", "a".repeat(39)),
            format!("CDHash={}\n", "a".repeat(41)),
        ] {
            assert!(parse_cdhash(text.as_bytes()).is_err());
        }
        assert!(parse_cdhash(b"CDHash=\xff").is_err());
        let requirement = running_requirement("ABCDE12345", hash).unwrap();
        assert!(requirement.starts_with("=anchor apple generic"));
        assert!(requirement.contains("identifier \"dev.apitester.desktop\""));
        assert!(requirement.contains("certificate leaf[subject.OU] = \"ABCDE12345\""));
        assert!(requirement.ends_with(&format!("and cdhash H\"{}\"", hash.to_ascii_lowercase())));
        for hash in [
            "",
            "x",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"",
            "a\" or true",
        ] {
            assert!(running_requirement("ABCDE12345", hash).is_err());
        }
        assert!(running_requirement("ABCD\"12345", hash).is_err());
        for pid in [0, i32::MAX as u32 + 1, u32::MAX] {
            assert!(process_target(pid).is_err());
        }
        assert_eq!(process_target(123).unwrap(), Path::new("+123"));
        assert_eq!(
            process_target(i32::MAX as u32).unwrap(),
            PathBuf::from(format!("+{}", i32::MAX))
        );
    }

    #[test]
    fn matching_policy_label_cannot_replace_native_notarization_proof() {
        // A local custom rule can forge this label even without an override
        // field. The parser alone cannot distinguish that from built-in policy.
        let matching_label = serde_json::to_vec(&serde_json::json!({
            "assessment:verdict": true,
            "assessment:authority": {
                "assessment:authority:source": "Notarized Developer ID"
            }
        }))
        .unwrap();
        assert!(notarized_assessment(&matching_label).is_ok());
        let mut ticket_check = Output {
            success: false,
            stdout: Vec::new(),
            stderr: b"arbitrary native diagnostic".to_vec(),
        };
        assert!(notarization_proof(&matching_label, &ticket_check).is_err());
        ticket_check.success = true;
        assert!(notarization_proof(&matching_label, &ticket_check).is_ok());
        // The independent ticket predicate also cannot replace assessment.
        assert!(notarization_proof(b"{}", &ticket_check).is_err());
        let overridden = serde_json::to_vec(&serde_json::json!({
            "assessment:verdict": true,
            "assessment:authority": {
                "assessment:authority:source": "Notarized Developer ID",
                "assessment:authority:override": true
            }
        }))
        .unwrap();
        assert!(notarization_proof(&overridden, &ticket_check).is_err());
    }

    #[test]
    fn notarization_requires_builtin_authority_without_overrides() {
        let good = serde_json::json!({
            "assessment:verdict": true,
            "assessment:authority": {
                "assessment:authority:source": "Notarized Developer ID"
            }
        });
        assert!(notarized_assessment(&serde_json::to_vec(&good).unwrap()).is_ok());
        for source in [
            "Developer ID",
            "Unnotarized Developer ID",
            "User",
            "Apple System",
            "Notarized Developer ID ",
            "",
        ] {
            let mut bad = good.clone();
            bad["assessment:authority"]["assessment:authority:source"] = Value::from(source);
            assert!(notarized_assessment(&serde_json::to_vec(&bad).unwrap()).is_err());
        }
        for verdict in [
            Value::Null,
            Value::Bool(false),
            Value::from("true"),
            Value::from(1),
        ] {
            let mut bad = good.clone();
            bad["assessment:verdict"] = verdict;
            assert!(notarized_assessment(&serde_json::to_vec(&bad).unwrap()).is_err());
        }
        for location in ["assessment:authority", "root"] {
            let mut bad = good.clone();
            let object = if location == "root" {
                &mut bad
            } else {
                &mut bad[location]
            };
            object["assessment:authority:override"] = Value::Bool(false);
            assert!(notarized_assessment(&serde_json::to_vec(&bad).unwrap()).is_err());
        }
        for bad in [b"{}".as_slice(), b"[]", b"null", b"invalid"] {
            assert!(notarized_assessment(bad).is_err());
        }
        for (success, stdout, stderr, accepted) in [
            (
                true,
                b"assessments enabled\n".as_slice(),
                b"".as_slice(),
                true,
            ),
            (false, b"assessments enabled\n", b"", false),
            (true, b"assessments disabled\n", b"", false),
            (true, b"assessments enabled\nextra", b"", false),
            (true, b"assessments enabled\n", b"warning", false),
        ] {
            assert_eq!(
                assessment_enabled(&Output {
                    success,
                    stdout: stdout.to_vec(),
                    stderr: stderr.to_vec()
                }),
                accepted
            );
        }
    }

    #[test]
    fn host_location_and_release_channel() {
        assert_eq!(
            derive_host_path(Path::new(
                "/Applications/Resolved.app/Contents/Helpers/resolved-updater"
            ))
            .unwrap(),
            Path::new("/Applications/Resolved.app")
        );
        for path in [
            "/tmp/resolved-updater",
            "/Applications/Other.app/Contents/Helpers/resolved-updater",
            "/Applications/Resolved.app/Contents/MacOS/resolved-updater",
        ] {
            assert!(derive_host_path(Path::new(path)).is_err());
        }
        for version in ["1.2.3-debug", "nightly", "1.2.3+1", "1", "1.2.3\n"] {
            assert!(!stable_version(version));
        }
        assert!(stable_version("1.2.3"));
    }

    #[test]
    fn fingerprint_covers_paths_content_modes_and_empty_directories() {
        let temp = tempfile::tempdir().unwrap();
        let path = fs::canonicalize(temp.path()).unwrap();
        fs::write(path.join("file"), b"first").unwrap();
        let first = fingerprint(&path).unwrap();
        assert_eq!(first, fingerprint(&path).unwrap());
        fs::write(path.join("file"), b"other").unwrap();
        assert_ne!(first, fingerprint(&path).unwrap());
        fs::write(path.join("file"), b"first").unwrap();
        assert_eq!(first, fingerprint(&path).unwrap());
        fs::rename(path.join("file"), path.join("renamed")).unwrap();
        assert_ne!(first, fingerprint(&path).unwrap());
        fs::rename(path.join("renamed"), path.join("file")).unwrap();
        fs::set_permissions(path.join("file"), fs::Permissions::from_mode(0o755)).unwrap();
        assert_ne!(first, fingerprint(&path).unwrap());
        let before = fingerprint(&path).unwrap();
        fs::create_dir(path.join("empty")).unwrap();
        assert_ne!(before, fingerprint(&path).unwrap());
    }

    #[test]
    fn fingerprint_rejects_unsafe_entries_and_limits() {
        let temp = tempfile::tempdir().unwrap();
        let path = fs::canonicalize(temp.path()).unwrap();
        fs::write(path.join("file"), b"1234").unwrap();
        for limits in [
            Limits {
                bytes: 3,
                ..TREE_LIMITS
            },
            Limits {
                entries: 1,
                ..TREE_LIMITS
            },
            Limits {
                depth: 0,
                ..TREE_LIMITS
            },
        ] {
            assert!(fingerprint_with_limits(&path, limits).is_err());
        }
        symlink("file", path.join("link")).unwrap();
        assert!(fingerprint(&path).is_err());
        fs::remove_file(path.join("link")).unwrap();
        fs::hard_link(path.join("file"), path.join("link")).unwrap();
        assert!(fingerprint(&path).is_err());
        fs::remove_file(path.join("link")).unwrap();
        fs::set_permissions(path.join("file"), fs::Permissions::from_mode(0o666)).unwrap();
        assert!(fingerprint(&path).is_err());
        fs::set_permissions(path.join("file"), fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(path.join("non-ascii-é"), b"").unwrap();
        assert!(fingerprint(&path).is_err());
    }

    #[tokio::test]
    async fn subprocess_output_is_bounded() {
        let bytes = vec![b'x'; OUTPUT_LIMIT + 1];
        assert!(bounded_read(bytes.as_slice()).await.is_err());
        assert_eq!(bounded_read(b"ok".as_slice()).await.unwrap(), b"ok");
        assert!(command("/bin/sh", &[]).await.is_err());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn cancellation_kills_and_reaps_native_child() {
        // plutil waits for EOF on stdin. This uses no signing fixture, shell,
        // bundle execution, or process outside the production allowlist.
        let mut child = OwnedChild(
            tokio::process::Command::new("/usr/bin/plutil")
                .args(["-convert", "json", "-o", "-", "--", "-"])
                .env_clear()
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .unwrap(),
        );
        let pid = child.0.id().unwrap();
        let stdin = child.0.stdin.take().unwrap();
        let stdout = child.0.stdout.take().unwrap();
        let future = async move {
            let result = bounded_read(stdout).await;
            drop(stdin);
            drop(child);
            result
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(30), future)
                .await
                .is_err()
        );
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(pid as i32, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }

    #[test]
    fn fingerprint_rejects_special_files_and_directory_links() {
        let temp = tempfile::tempdir().unwrap();
        let path = fs::canonicalize(temp.path()).unwrap();
        let fifo = CString::new(path.join("fifo").as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(fingerprint(&path).is_err());
        fs::remove_file(path.join("fifo")).unwrap();
        symlink(&path, path.join("recursive")).unwrap();
        assert!(fingerprint(&path).is_err());
        fs::remove_file(path.join("recursive")).unwrap();
        fs::create_dir(path.join("a")).unwrap();
        fs::create_dir(path.join("a/b")).unwrap();
        assert!(
            fingerprint_with_limits(
                &path,
                Limits {
                    depth: 1,
                    ..TREE_LIMITS
                }
            )
            .is_err()
        );
    }
}
