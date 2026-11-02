fn main() {
    // Embed the current git tag (exact match) as VWSD_VERSION.
    // Falls back to `git describe --tags` (e.g. "v1.0.0-5-gabcdef") for
    // non-tagged commits, and finally to "v<CARGO_PKG_VERSION>" when git
    // is not available.
    let version = git_exact_tag()
        .or_else(git_describe)
        .unwrap_or_else(|| format!("v{}", env!("CARGO_PKG_VERSION")));

    println!("cargo:rustc-env=VWSD_VERSION={}", version);

    // Re-run this script when the package version or git state changes.
    // The .git paths are optional; cargo silently ignores missing paths.
    println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/packed-refs");
    // `.git/refs/tags` mtime changes when a new loose tag file is added.
    println!("cargo:rerun-if-changed=.git/refs/tags");
}

fn git_exact_tag() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["describe", "--tags", "--exact-match"])
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8(output.stdout).ok()?.trim().to_string())
    } else {
        None
    }
}

fn git_describe() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["describe", "--tags"])
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8(output.stdout).ok()?.trim().to_string())
    } else {
        None
    }
}
