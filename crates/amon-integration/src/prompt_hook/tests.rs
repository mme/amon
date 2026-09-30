use super::*;

/// The asset carries the version stamp `state()` looks for, so a fresh install
/// always reads back as Current rather than Outdated.
#[test]
fn the_asset_stamps_the_version_state_checks_for() {
    assert!(
        ASSET.contains(&format!("AMON_PROMPT_HOOK_VERSION={VERSION}")),
        "the script and state() disagree on the version stamp"
    );
}

/// Registering into an empty file creates the whole structure, and a second
/// register is a no-op — the command's unique path makes it idempotent.
#[test]
fn registering_is_idempotent_and_creates_structure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let settings = dir.path().join("settings.json");
    let hook = dir.path().join("hooks").join("amon-prompt-state.sh");
    std::fs::write(&settings, "{}\n").expect("seed");

    register(&settings, &hook).expect("register");
    let once = std::fs::read_to_string(&settings).expect("read");
    assert!(once.contains(EVENT), "event not registered: {once}");
    assert!(once.contains(&command(&hook)), "command missing: {once}");

    register(&settings, &hook).expect("register again");
    let twice = std::fs::read_to_string(&settings).expect("read");
    assert_eq!(once, twice, "second register changed the file");
}

/// A hand-formatted file keeps its shape: registering touches only the bytes
/// it adds, because the splice is herdr's CST-preserving primitive — not a
/// whole-file reserialize. (Claude settings are strict JSON, so no comments —
/// the proof is that indentation and sibling keys survive byte-for-byte.)
#[test]
fn registering_preserves_surrounding_formatting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let settings = dir.path().join("settings.json");
    let hook = dir.path().join("hooks").join("amon-prompt-state.sh");
    let original = "{\n  \"model\": \"opus\",\n  \"hooks\": {\n    \"SessionStart\": []\n  }\n}\n";
    std::fs::write(&settings, original).expect("seed");

    register(&settings, &hook).expect("register");
    let updated = std::fs::read_to_string(&settings).expect("read");

    // The sibling key and the existing hook keep their exact bytes; a
    // whole-file reserialize would have re-quoted or re-indented them.
    assert!(
        updated.contains("  \"model\": \"opus\","),
        "disturbed a sibling key or its indent: {updated}"
    );
    assert!(
        updated.contains("\"SessionStart\": []"),
        "disturbed the session hook: {updated}"
    );
    assert!(updated.contains(EVENT), "did not add the event: {updated}");
}

/// Deregistering removes exactly our entry and prunes the husk it leaves, so
/// an uninstall is invisible in the file.
#[test]
fn deregistering_removes_only_ours_and_prunes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let settings = dir.path().join("settings.json");
    let hook = dir.path().join("hooks").join("amon-prompt-state.sh");
    std::fs::write(&settings, "{\n  \"hooks\": {}\n}\n").expect("seed");

    register(&settings, &hook).expect("register");
    deregister(&settings, &hook).expect("deregister");
    let after = std::fs::read_to_string(&settings).expect("read");

    assert!(
        !after.contains(&command(&hook)),
        "command survived: {after}"
    );
    assert!(
        !after.contains(EVENT),
        "empty event husk left behind: {after}"
    );
}

/// Deregistering leaves a hook the user added by hand for the same event
/// alone — matched by our exact command, not by the event.
#[test]
fn deregistering_leaves_a_users_own_hook_alone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let settings = dir.path().join("settings.json");
    let hook = dir.path().join("hooks").join("amon-prompt-state.sh");
    let mine = format!(
        "{{\"matcher\":\"*\",\"hooks\":[{{\"type\":\"command\",\"command\":{}}}]}}",
        serde_json::to_string(&command(&hook)).unwrap()
    );
    let theirs = "{\"hooks\":[{\"type\":\"command\",\"command\":\"echo hi\"}]}";
    std::fs::write(
        &settings,
        format!("{{\"hooks\":{{\"{EVENT}\":[{theirs},{mine}]}}}}"),
    )
    .expect("seed");

    deregister(&settings, &hook).expect("deregister");
    let after = std::fs::read_to_string(&settings).expect("read");
    assert!(
        after.contains("echo hi"),
        "removed the user's own hook: {after}"
    );
    assert!(!after.contains(&command(&hook)), "kept ours: {after}");
}

// ---- settings robustness (codex review) -------------------------------

/// A settings file that is not valid JSON is left exactly as it was — never
/// overwritten by a fresh minimal object. (Read errors and non-objects fail
/// the register, which the caller treats as non-fatal.)
#[test]
fn a_non_object_settings_file_is_refused_not_overwritten() {
    let dir = tempfile::tempdir().expect("tempdir");
    let settings = dir.path().join("settings.json");
    let hook = dir.path().join("hooks").join("amon-prompt-state.sh");
    let original = "[1, 2, 3]\n"; // valid JSON, but not an object
    std::fs::write(&settings, original).expect("seed");

    assert!(
        register(&settings, &hook).is_err(),
        "should refuse a non-object"
    );
    assert_eq!(
        std::fs::read_to_string(&settings).expect("read"),
        original,
        "the file was altered"
    );
}

/// A `hooks` value that is not an object is refused rather than shadowed by a
/// second `hooks` key.
#[test]
fn a_non_object_hooks_value_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let settings = dir.path().join("settings.json");
    let hook = dir.path().join("hooks").join("amon-prompt-state.sh");
    std::fs::write(&settings, "{\"hooks\": null}\n").expect("seed");

    assert!(
        register(&settings, &hook).is_err(),
        "should refuse non-object hooks"
    );
    assert!(
        std::fs::read_to_string(&settings)
            .expect("read")
            .contains("\"hooks\": null"),
        "the file was altered"
    );
}

/// A non-array event value is refused rather than shadowed by a duplicate key.
#[test]
fn a_non_array_event_value_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let settings = dir.path().join("settings.json");
    let hook = dir.path().join("hooks").join("amon-prompt-state.sh");
    std::fs::write(&settings, format!("{{\"hooks\": {{\"{EVENT}\": {{}}}}}}\n")).expect("seed");

    assert!(
        register(&settings, &hook).is_err(),
        "should refuse non-array event"
    );
}

/// Idempotency is by the parsed command, so the same command text sitting in
/// an unrelated place does not fool register into skipping — and a real second
/// call still adds nothing.
#[test]
fn idempotency_is_by_parsed_command_not_substring() {
    let dir = tempfile::tempdir().expect("tempdir");
    let settings = dir.path().join("settings.json");
    let hook = dir.path().join("hooks").join("amon-prompt-state.sh");
    let cmd = command(&hook);
    // The command text appears as an unrelated string value, not as a
    // registered UserPromptSubmit entry.
    std::fs::write(
        &settings,
        format!(
            "{{\"note\": {}, \"hooks\": {{}}}}\n",
            serde_json::to_string(&cmd).unwrap()
        ),
    )
    .expect("seed");

    register(&settings, &hook).expect("register");
    let after = std::fs::read_to_string(&settings).expect("read");
    assert!(
        after.contains(EVENT),
        "was fooled by the substring and skipped: {after}"
    );

    let before = after.clone();
    register(&settings, &hook).expect("register again");
    assert_eq!(
        std::fs::read_to_string(&settings).expect("read"),
        before,
        "second register changed the file"
    );
}
