//! Cargo.toml `[workspace.package] version` is the only number you edit.
//! `scripts/bump-version.sh` copies it into Info.plist, the marketing pages,
//! and Cargo.lock so a release is not a scavenger hunt.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn bump_script() -> PathBuf {
    repo_root().join("scripts/bump-version.sh")
}

fn run_bump(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new("bash")
        .arg(bump_script())
        .args(args)
        .env("NEVER_SLEEP_ROOT", root)
        .output()
        .unwrap_or_else(|err| panic!("run bump-version.sh: {err}"))
}

fn workspace_version_from_cargo(cargo: &str) -> &str {
    let workspace = cargo
        .split("[workspace.package]")
        .nth(1)
        .expect("[workspace.package]");
    workspace
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("version = \"")
                .and_then(|rest| rest.strip_suffix('"'))
        })
        .expect("workspace.package version")
}

fn bundle_version(semver: &str) -> String {
    let mut parts = semver.split('.');
    let major: u32 = parts.next().unwrap().parse().unwrap();
    let minor: u32 = parts.next().unwrap().parse().unwrap();
    let patch: u32 = parts.next().unwrap().parse().unwrap();
    (major * 10_000 + minor * 100 + patch).to_string()
}

#[test]
fn bump_version_script_is_the_release_entry_point() {
    assert!(
        bump_script().is_file(),
        "scripts/bump-version.sh must exist so a release is one command, not four file edits"
    );
}

#[test]
fn print_reads_workspace_package_version() {
    let cargo = fs::read_to_string(repo_root().join("Cargo.toml")).unwrap();
    let expected = workspace_version_from_cargo(&cargo);
    let output = run_bump(&repo_root(), &["--print"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        expected,
        "bump-version.sh --print must match [workspace.package] version"
    );
}

#[test]
fn check_is_green_when_derived_files_match_cargo() {
    let output = run_bump(&repo_root(), &["--check"]);
    assert!(
        output.status.success(),
        "derived version files drifted from Cargo.toml:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn bundle_version_encodes_semver_as_a_monotonic_integer() {
    for (semver, build) in [
        ("0.0.1", "1"),
        ("0.3.3", "303"),
        ("0.3.4", "304"),
        ("1.0.0", "10000"),
        ("1.2.3", "10203"),
    ] {
        assert_eq!(bundle_version(semver), build, "{semver}");
    }
}

#[test]
fn bump_rewrites_plist_pages_and_lock_from_the_new_semver() {
    let tmp = repo_root().join("target/test-bump-version");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("packaging")).unwrap();
    fs::create_dir_all(tmp.join("site/zh")).unwrap();
    fs::create_dir_all(tmp.join("scripts")).unwrap();
    fs::copy(bump_script(), tmp.join("scripts/bump-version.sh")).unwrap();

    fs::write(
        tmp.join("Cargo.toml"),
        r#"[workspace.package]
edition = "2021"
version = "0.3.3"
"#,
    )
    .unwrap();
    fs::write(
        tmp.join("packaging/Info.plist"),
        r#"  <key>CFBundleShortVersionString</key>
  <string>0.3.3</string>
  <key>CFBundleVersion</key>
  <string>5</string>
"#,
    )
    .unwrap();
    let page = r#"        "softwareVersion": "0.3.3",
        <span data-release-tag>v0.3.3</span>
          <p class="release-ver"><a href="https://example.test" data-release-tag>v0.3.3</a></p>
      <p>Never Sleep · <span data-release-tag>v0.3.3</span></p>
"#;
    fs::write(tmp.join("site/index.html"), page).unwrap();
    fs::write(tmp.join("site/zh/index.html"), page).unwrap();
    fs::write(
        tmp.join("Cargo.lock"),
        r#"name = "never-sleep"
version = "0.3.3"
dependencies = [
name = "never-sleep-core"
version = "0.3.3"
dependencies = [
name = "objc2-app-kit"
version = "0.3.2"
"#,
    )
    .unwrap();

    let output = run_bump(&tmp, &["0.4.1"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cargo = fs::read_to_string(tmp.join("Cargo.toml")).unwrap();
    assert_eq!(workspace_version_from_cargo(&cargo), "0.4.1");

    let plist = fs::read_to_string(tmp.join("packaging/Info.plist")).unwrap();
    assert!(
        plist.contains("<string>0.4.1</string>"),
        "CFBundleShortVersionString: {plist}"
    );
    assert!(
        plist.contains("<string>401</string>"),
        "CFBundleVersion must be derived (0.4.1 → 401), got {plist}"
    );
    assert!(
        !plist.contains("<string>5</string>"),
        "must not leave a hand-maintained build number: {plist}"
    );

    for rel in ["site/index.html", "site/zh/index.html"] {
        let html = fs::read_to_string(tmp.join(rel)).unwrap();
        assert!(
            html.contains(r#""softwareVersion": "0.4.1""#),
            "{rel} JSON-LD: {html}"
        );
        assert_eq!(
            html.matches("v0.4.1").count(),
            3,
            "{rel} data-release-tag fallbacks: {html}"
        );
        assert!(
            !html.contains("0.3.3"),
            "{rel} still mentions the old version: {html}"
        );
    }

    let lock = fs::read_to_string(tmp.join("Cargo.lock")).unwrap();
    assert!(lock.contains("name = \"never-sleep\"\nversion = \"0.4.1\""));
    assert!(lock.contains("name = \"never-sleep-core\"\nversion = \"0.4.1\""));
    assert!(
        lock.contains("name = \"objc2-app-kit\"\nversion = \"0.3.2\""),
        "must not rewrite unrelated crate versions: {lock}"
    );

    let check = run_bump(&tmp, &["--check"]);
    assert!(
        check.status.success(),
        "tree should be consistent after bump:\n{}",
        String::from_utf8_lossy(&check.stderr)
    );

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn check_fails_when_a_marketing_page_is_stale() {
    let tmp = repo_root().join("target/test-bump-version-stale");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("packaging")).unwrap();
    fs::create_dir_all(tmp.join("site/zh")).unwrap();
    fs::create_dir_all(tmp.join("scripts")).unwrap();
    fs::copy(bump_script(), tmp.join("scripts/bump-version.sh")).unwrap();
    fs::copy(repo_root().join("Cargo.toml"), tmp.join("Cargo.toml")).unwrap();
    fs::copy(
        repo_root().join("packaging/Info.plist"),
        tmp.join("packaging/Info.plist"),
    )
    .unwrap();
    fs::copy(
        repo_root().join("site/index.html"),
        tmp.join("site/index.html"),
    )
    .unwrap();
    let cargo = fs::read_to_string(tmp.join("Cargo.toml")).unwrap();
    let current = workspace_version_from_cargo(&cargo);
    let mut zh = fs::read_to_string(repo_root().join("site/zh/index.html")).unwrap();
    zh = zh.replace(current, "0.0.1");
    fs::write(tmp.join("site/zh/index.html"), zh).unwrap();
    fs::copy(repo_root().join("Cargo.lock"), tmp.join("Cargo.lock")).unwrap();

    let output = run_bump(&tmp, &["--check"]);
    assert!(
        !output.status.success(),
        "stale zh/index.html must fail --check"
    );
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("zh/index.html") || err.contains("softwareVersion") || err.contains("stale"),
        "stderr should name the drifted file, got {err}"
    );
    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn release_workflow_reads_the_workspace_version() {
    let release = fs::read_to_string(repo_root().join(".github/workflows/release.yml")).unwrap();
    assert!(
        release.contains("bump-version.sh") && release.contains("--print"),
        "release.yml must take the tag version from Cargo.toml via bump-version.sh --print, not Info.plist"
    );
    assert!(
        !release.contains("Print :CFBundleShortVersionString"),
        "Info.plist is derived; do not let a stale plist name the GitHub Release"
    );
}

#[test]
fn package_macos_stamps_the_copied_plist_from_cargo() {
    let script = fs::read_to_string(repo_root().join("scripts/package-macos.sh")).unwrap();
    assert!(
        script.contains("bump-version.sh") && script.contains("--stamp-plist"),
        "package-macos.sh must stamp dist Info.plist from the workspace version so a forgotten bump still ships the crate version"
    );
}

#[test]
fn stamp_plist_overwrites_short_and_build_versions() {
    let tmp = repo_root().join("target/test-stamp-plist");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("packaging")).unwrap();
    fs::create_dir_all(tmp.join("scripts")).unwrap();
    fs::copy(bump_script(), tmp.join("scripts/bump-version.sh")).unwrap();
    fs::write(
        tmp.join("Cargo.toml"),
        r#"[workspace.package]
version = "1.2.3"
"#,
    )
    .unwrap();
    let dest = tmp.join("Info.plist");
    fs::write(
        &dest,
        r#"  <key>CFBundleShortVersionString</key>
  <string>0.0.0</string>
  <key>CFBundleVersion</key>
  <string>1</string>
"#,
    )
    .unwrap();

    let output = run_bump(&tmp, &["--stamp-plist", dest.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plist = fs::read_to_string(&dest).unwrap();
    assert!(plist.contains("<string>1.2.3</string>"), "{plist}");
    assert!(plist.contains("<string>10203</string>"), "{plist}");
    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn agents_doc_tells_you_not_to_hand_edit_derived_versions() {
    let agents = fs::read_to_string(repo_root().join("AGENTS.md")).unwrap();
    assert!(
        agents.contains("bump-version.sh"),
        "AGENTS.md must point at scripts/bump-version.sh as the release bump"
    );
}
