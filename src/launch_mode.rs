//! Select the process mode before any desktop runtime initialization.

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LaunchMode {
    Desktop,
    Mcp,
}

pub(crate) fn parse(
    args: impl IntoIterator<Item = std::ffi::OsString>,
) -> Result<LaunchMode, &'static str> {
    let args: Vec<_> = args.into_iter().collect();
    if !args.iter().any(|arg| arg == "--mcp") {
        // Preserve existing desktop launch arguments, including OS-supplied ones.
        return Ok(LaunchMode::Desktop);
    }
    if args.len() != 1 {
        return Err("usage: resolved --mcp (no additional arguments)");
    }
    Ok(LaunchMode::Mcp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<LaunchMode, &'static str> {
        parse(args.iter().map(std::ffi::OsString::from))
    }

    #[test]
    fn selects_desktop_without_mcp_flag() {
        assert_eq!(parse_args(&[]), Ok(LaunchMode::Desktop));
        assert_eq!(parse_args(&["-psn_0_123"]), Ok(LaunchMode::Desktop));
    }

    #[test]
    fn selects_mcp_only_for_a_standalone_flag() {
        assert_eq!(parse_args(&["--mcp"]), Ok(LaunchMode::Mcp));
        assert!(parse_args(&["--mcp", "unexpected"]).is_err());
        assert!(parse_args(&["unexpected", "--mcp"]).is_err());
        assert!(parse_args(&["--mcp", "--mcp"]).is_err());
    }
}
