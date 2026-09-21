mod build_support;

fn input(name: &str) -> String {
    println!("cargo:rerun-if-env-changed={name}");
    std::env::var(name).unwrap_or_default()
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build_support.rs");
    let marker = input("RESOLVED_UPDATER_BUILD");
    let os = input("CARGO_CFG_TARGET_OS");
    let arch = input("CARGO_CFG_TARGET_ARCH");
    let wire_arch = build_support::validate_target(&marker, &os, &arch)
        .unwrap_or_else(|error| panic!("{error}"));
    for name in [
        "RESOLVED_UPDATER_APP_VERSION",
        "RESOLVED_BUILD_VERSION",
        "API_TESTER_BUILD_NUMBER",
    ] {
        let value = input(name);
        let result = if name == "API_TESTER_BUILD_NUMBER" {
            build_support::validate_build_number(&value)
        } else {
            build_support::validate_version(&value)
        };
        result.unwrap_or_else(|error| panic!("{name}: {error}"));
        println!("cargo:rustc-env={name}={value}");
    }
    println!("cargo:rustc-env=RESOLVED_UPDATER_ARCH={wire_arch}");
}
