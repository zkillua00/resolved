fn main() {
    println!("cargo:rerun-if-env-changed=RESOLVED_BUILD_VERSION");
    println!("cargo:rerun-if-changed=../../Cargo.toml");
    let version = std::env::var("RESOLVED_BUILD_VERSION").unwrap_or_else(|_| {
        // Keep the root package's literal version as the single source of truth:
        // release tooling reads and updates it without TOML workspace inheritance.
        let manifest =
            std::fs::read_to_string("../../Cargo.toml").expect("root Cargo.toml must be readable");
        let mut in_package = false;
        manifest
            .lines()
            .find_map(|line| {
                let line = line.trim();
                if line.starts_with('[') {
                    in_package = line == "[package]";
                }
                in_package
                    .then(|| line.strip_prefix("version = \"")?.strip_suffix('"'))
                    .flatten()
            })
            .expect("root [package] must contain a literal version")
            .to_owned()
    });
    println!("cargo:rustc-env=RESOLVED_BUILD_VERSION={version}");
}
