//! The release machinery's contracts with itself.
//!
//! A release crosses files that nothing compiles together: two GitHub
//! workflows, the curl installer, the toolchain pins, and the changelog they
//! feed. Every failure mode here is silent — a tarball published under one
//! name and downloaded under another, a version bumped in a field the
//! workflow no longer finds — so, as with the desktop's QML, reading the
//! sources and insisting they agree is the only test there is.

const RELEASE_PR: &str = include_str!("../../../.github/workflows/release-pr.yml");
const PUBLISH: &str = include_str!("../../../.github/workflows/publish.yml");
const INSTALLER: &str = include_str!("../../../install.sh");
const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");
const MANIFEST: &str = include_str!("../../../Cargo.toml");
const MISE: &str = include_str!("../../../.mise.toml");

/// The one name the binary tarball is published under and fetched from.
///
/// `publish.yml` builds it and attaches it to the GitHub Release; install.sh
/// downloads it from there. The two spell the version differently -
/// `${VERSION}` in the workflow's shell, `$TAG` in the installer's - so the
/// parts on either side of it are what can be held identical.
#[test]
fn the_tarball_is_published_under_the_name_the_installer_fetches() {
    assert!(
        PUBLISH.contains("amon-v${VERSION}-x86_64-linux.tar.gz"),
        "publish.yml builds the tarball under the agreed name"
    );
    assert!(
        INSTALLER.contains("amon-$TAG-x86_64-linux.tar.gz"),
        "install.sh downloads the tarball under the agreed name"
    );
    assert!(
        INSTALLER.contains("/releases/download/$TAG/"),
        "and fetches it from the GitHub Release the publish created"
    );
}

/// The checksum the publish writes is the checksum the installer checks.
///
/// Both sides name the sidecar `<tarball>.sha256`; a publish that stopped
/// writing it would brick every install, and nothing but this test connects
/// the two files.
#[test]
fn the_installer_verifies_the_checksum_the_publish_writes() {
    assert!(
        PUBLISH.contains(".tar.gz.sha256"),
        "publish.yml writes the sha256 sidecar"
    );
    assert!(
        INSTALLER.contains("$TARBALL.sha256"),
        "install.sh downloads the sidecar"
    );
    assert!(
        INSTALLER.contains("sha256sum -c"),
        "and refuses on mismatch"
    );
}

/// The release builds with the toolchain the repo itself pins.
///
/// The shadow terminal's libghostty-vt builds with Zig and fails loudly on
/// any version but the one .mise.toml names. The workflow therefore
/// installs its tools *from* .mise.toml, so
/// there is one pin and nothing to drift; this holds the mechanism in place
/// and the pin where the mechanism reads it.
#[test]
fn the_release_builds_with_the_pinned_toolchain() {
    assert!(
        PUBLISH.contains("jdx/mise-action"),
        "publish installs the toolchain from .mise.toml"
    );
    assert!(
        MISE.contains("zig"),
        ".mise.toml still carries the zig pin libghostty-vt needs"
    );
}

/// The bump edits the version where it actually lives.
///
/// The workspace has exactly one `version = "…"` line — every crate inherits
/// it — and the workflow bumps it with an anchored substitution. If the field
/// moves, or a second top-level `version =` line appears for the substitution
/// to hit, the release PR quietly bumps the wrong thing or nothing.
#[test]
fn the_bump_edits_the_one_version_line_the_manifest_has() {
    let version_lines = MANIFEST
        .lines()
        .filter(|line| line.starts_with("version = \""))
        .count();
    assert_eq!(
        version_lines, 1,
        "the workspace version is declared exactly once at the top level"
    );
    assert!(
        RELEASE_PR.contains("s/^version = "),
        "the bump anchors to the start of that line"
    );
}

/// The changelog carries the marker the release PR inserts under, and the
/// publish reads sections back out by the same heading shape it wrote.
#[test]
fn the_changelog_is_written_and_read_by_the_same_marks() {
    assert!(
        CHANGELOG.contains("<!-- next -->"),
        "the changelog carries the insertion marker"
    );
    assert!(
        RELEASE_PR.contains("<!-- next -->"),
        "the release PR inserts under the marker"
    );
    assert!(
        PUBLISH.contains("## v"),
        "the publish extracts the section by its heading"
    );
}

/// Merging the release PR is what publishes: the publish workflow watches the
/// file the release PR bumps.
#[test]
fn the_merge_is_the_publish_trigger() {
    assert!(
        PUBLISH.contains("Cargo.toml"),
        "publish triggers on the file that carries the version"
    );
    assert!(
        PUBLISH.contains("workflow_dispatch"),
        "and can be re-run by hand when a publish needs healing"
    );
}

/// The tag points at the commit that was tested and packaged.
///
/// `gh release create` tags the default branch's *latest* commit when left to
/// itself. A commit landing on main mid-run would then be what the tag — and
/// everything downstream fetches — names, while the attached
/// binary came from the commit before it.
#[test]
fn the_tag_points_at_the_commit_that_was_tested() {
    assert!(
        PUBLISH.contains("--target \"$GITHUB_SHA\""),
        "the release is created against the run's own commit"
    );
}

/// A prerelease version never gets as far as being public.
///
/// Cargo allows `1.0.0-rc.1`, and half the pipeline downstream - the tarball
/// name, the installer's tag check, the version the panel prints - assumes
/// plain x.y.z. The gate refuses anything else before a tag or Release
/// exists, so there is never a half-published version to clean up by hand.
#[test]
fn a_prerelease_version_never_ships() {
    assert!(
        PUBLISH.contains("^[0-9]+\\.[0-9]+\\.[0-9]+$"),
        "the gate holds versions to the shape everything downstream accepts"
    );
}

/// An open release PR is never rebuilt out from under its reviewer.
///
/// The PR body invites editing CHANGELOG.md on the branch; a re-dispatch that
/// force-pushed over those edits would destroy them silently. The workflow
/// refuses instead — closing the PR is how you ask for a rebuild.
#[test]
fn an_open_release_pr_is_never_overwritten() {
    assert!(
        RELEASE_PR.contains("gh pr list --head \"release/v$NEW\""),
        "the workflow looks for an open release PR before touching the branch"
    );
}
