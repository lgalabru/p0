//! Pre-compile + ad-hoc sign the Swift Keychain/Touch ID helper and embed it.
fn main() {
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("p0-keychain");
    println!("cargo::rerun-if-changed=src/macos/helper.swift");
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64".to_owned(),
        Ok(a) => a.to_owned(),
        Err(_) => "arm64".to_owned(),
    };
    let ok = std::process::Command::new("/usr/bin/swiftc")
        .args(["-O", "-target", &format!("{arch}-apple-macos12"), "-o"])
        .arg(&out)
        .arg("src/macos/helper.swift")
        .status()
        .is_ok_and(|s| s.success())
        && std::process::Command::new("/usr/bin/codesign")
            .args(["-s", "-", "-f", "--options", "kill,hard"])
            .arg(&out)
            .status()
            .is_ok_and(|s| s.success());
    if !ok {
        panic!("failed to build Swift keychain helper (need Xcode command line tools)");
    }
}
