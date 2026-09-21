use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::{self, Write},
    process::ExitCode,
};

fn parse(args: impl IntoIterator<Item = OsString>) -> Result<(), &'static str> {
    let args = args
        .into_iter()
        .map(|arg| arg.into_string().map_err(|_| "invalid_utf8"))
        .collect::<Result<Vec<_>, _>>()?;
    if args.first().map(String::as_str) != Some("--protocol-version") {
        return Err("missing_protocol_version");
    }
    if args.get(1).map(String::as_str) != Some("1") {
        return Err("unsupported_protocol_version");
    }
    if args.len() != 3 {
        return Err("invalid_arguments");
    }
    if args[2] != "health" {
        return Err("unsupported_command");
    }
    Ok(())
}

fn response(args: impl IntoIterator<Item = OsString>) -> (Value, bool) {
    match parse(args) {
        Ok(()) => (
            json!({
                "protocol_version": 1,
                "kind": "health",
                "name": "resolved-updater",
                "app_version": env!("RESOLVED_UPDATER_APP_VERSION"),
                "build_version": env!("RESOLVED_BUILD_VERSION"),
                "build_number": env!("API_TESTER_BUILD_NUMBER"),
                "os": "macos",
                "arch": env!("RESOLVED_UPDATER_ARCH"),
                "feed_url": "https://apiworkbench.dev/downloads.json",
                "capabilities": ["health"],
                "installation_enabled": false
            }),
            true,
        ),
        Err(code) => (
            json!({
                "protocol_version": 1,
                "kind": "error",
                "name": "resolved-updater",
                "error": {"code": code},
                "installation_enabled": false
            }),
            false,
        ),
    }
}

fn main() -> ExitCode {
    let (value, success) = response(std::env::args_os().skip(1));
    let mut line = serde_json::to_vec(&value).expect("JSON value serialization");
    line.push(b'\n');
    if io::stdout().lock().write_all(&line).is_err() {
        return ExitCode::FAILURE;
    }
    if success {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
#[path = "../build_support.rs"]
mod build_support;

#[cfg(test)]
mod tests {
    use super::build_support::*;
    use super::*;

    #[test]
    fn parsing_is_exact() {
        for (args, expected) in [
            (vec!["--protocol-version", "1", "health"], Ok(())),
            (vec![], Err("missing_protocol_version")),
            (vec!["health"], Err("missing_protocol_version")),
            (
                vec!["--protocol-version"],
                Err("unsupported_protocol_version"),
            ),
            (
                vec!["--protocol-version", "2", "health"],
                Err("unsupported_protocol_version"),
            ),
            (vec!["--protocol-version", "1"], Err("invalid_arguments")),
            (
                vec!["--protocol-version", "1", "health", "extra"],
                Err("invalid_arguments"),
            ),
            (
                vec!["--protocol-version", "1", "check"],
                Err("unsupported_command"),
            ),
            (
                vec!["--protocol-version", "1", "download"],
                Err("unsupported_command"),
            ),
            (
                vec!["--protocol-version", "1", "install"],
                Err("unsupported_command"),
            ),
            (
                vec!["--protocol-version", "1", "unknown"],
                Err("unsupported_command"),
            ),
        ] {
            assert_eq!(parse(args.into_iter().map(OsString::from)), expected);
        }
    }

    #[test]
    fn invalid_utf8_fails_closed() {
        use std::os::unix::ffi::OsStringExt;
        let (value, success) = response([OsString::from_vec(vec![0xff])]);
        assert!(!success);
        assert_eq!(value["error"]["code"], "invalid_utf8");
    }

    #[test]
    fn metadata_is_bounded_and_cannot_inject_directives() {
        for value in [
            "",
            "\n",
            "1\ncargo:rustc-env=X=Y",
            "1\r2",
            "1\0",
            " 1",
            "💥",
        ] {
            assert!(validate_version(value).is_err());
        }
        assert!(validate_version(&"a".repeat(129)).is_err());
        for value in ["1.2.3", "1.2.3-nightly.abc+1", "nightly-abc123"] {
            assert!(validate_version(value).is_ok());
        }
        for value in ["", "0", "-1", "+1", "1\n", "1.0", "18446744073709551616"] {
            assert!(validate_build_number(value).is_err());
        }
        assert!(validate_build_number("1").is_ok());
        assert!(validate_build_number("18446744073709551615").is_ok());
    }

    #[test]
    fn build_entry_point_and_target_are_required() {
        assert!(
            validate_target("", "macos", "aarch64")
                .unwrap_err()
                .contains("scripts/bundle-macos.sh")
        );
        assert!(validate_target("1", "linux", "aarch64").is_err());
        assert!(validate_target("1", "macos", "arm").is_err());
        assert_eq!(validate_target("1", "macos", "aarch64"), Ok("arm64"));
        assert_eq!(validate_target("1", "macos", "x86_64"), Ok("x64"));
    }
}
