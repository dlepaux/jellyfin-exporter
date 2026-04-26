//! Build-time metadata for the `jellyfin_exporter_build_info` metric.
//!
//! Reads `BUILD_GIT_SHA` and `BUILD_DATE` from the environment when set
//! (CI / Docker passes them in), with a `git rev-parse` fallback for local
//! `cargo build` invocations. Falls back to `"unknown"` when neither is
//! available (sandboxed builds, source tarball releases).
//!
//! Intentionally has no third-party dependencies — `vergen`, `built`, and
//! `shadow-rs` would all pull in heavyweight build-script machinery for what
//! amounts to two string bakes.

fn main() {
    println!("cargo:rerun-if-env-changed=BUILD_GIT_SHA");
    println!("cargo:rerun-if-env-changed=BUILD_DATE");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");

    let git_sha = std::env::var("BUILD_GIT_SHA")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(git_sha_from_repo)
        .unwrap_or_else(|| "unknown".into());

    let build_date = std::env::var("BUILD_DATE")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());

    println!("cargo:rustc-env=BUILD_GIT_SHA={git_sha}");
    println!("cargo:rustc-env=BUILD_DATE={build_date}");
}

fn git_sha_from_repo() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8(output.stdout).ok()?;
    let trimmed = sha.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}
