use std::{fs, process::Command};
fn vm() {
    assert_eq!(
        std::env::var("LOOOM_TEST_VM").as_deref(),
        Ok("1"),
        "All tests must be run on the VM"
    );
}
fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    vm();
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("base.yaml");
    let base = fs::read_to_string("configs/mvp/base.yaml").unwrap();
    fs::create_dir(t.path().join("files")).unwrap();
    fs::copy(
        "configs/mvp/files/example.conf",
        t.path().join("files/example.conf"),
    )
    .unwrap();
    fs::write(&p, base).unwrap();
    (t, p)
}
fn check(p: &std::path::Path) -> bool {
    Command::new(env!("CARGO_BIN_EXE_looom"))
        .arg("check")
        .arg(p)
        .output()
        .unwrap()
        .status
        .success()
}
#[test]
fn accepts_declared_vm_resources() {
    let (_t, p) = fixture();
    assert!(check(&p));
}
#[test]
fn rejects_unsafe_or_ambiguous_inputs() {
    let (_t, p) = fixture();
    let original = fs::read_to_string(&p).unwrap();
    for candidate in [
        original.replace("schema: 1", "schema: 2"),
        original.replace("schema: 1", "schema: 1\nschema: 1"),
        original.replace("schema: 1", "schema: 1\nunknown: true"),
        original.replace("profile: arch", "profile: &anchor arch"),
        original.replace("profile: arch", "profile: !custom arch"),
        original.replace("profile: arch", "profile: yes"),
        original.replace("uid: 1000", "uid: \"1000\""),
        original.replace("2026-10-03", "2026-02-30"),
        original.replace("2026-10-03", "ééééé"),
        original.replace("sshd.service: enabled", "sshd.service: masked"),
        original.replace("/etc/looom-example.conf:", "/etc/fstab:"),
        original.replace("/etc/looom-example.conf:", "/etc/looom-local/file.conf:"),
        original.replace("source: files/example.conf", "source: ../outside.conf"),
        original.replace("mode: \"0644\"", "mode: 644"),
        original.replace("mode: \"0644\"", "mode: \"4755\""),
        original.replace("packages:\n  - tree", "packages:\n  - tree\n  - tree"),
        format!("{original}\n---\nschema: 1\n"),
    ] {
        fs::write(&p, &candidate).unwrap();
        assert!(!check(&p), "accepted unsafe config: {candidate}");
    }
}
#[test]
fn rejects_source_symlink() {
    let (t, p) = fixture();
    fs::remove_file(t.path().join("files/example.conf")).unwrap();
    std::os::unix::fs::symlink("/etc/hostname", t.path().join("files/example.conf")).unwrap();
    assert!(!check(&p));
}

#[test]
fn rejects_source_directory_symlink() {
    let (t, p) = fixture();
    fs::rename(t.path().join("files"), t.path().join("real-files")).unwrap();
    std::os::unix::fs::symlink("real-files", t.path().join("files")).unwrap();
    assert!(!check(&p));
}

#[test]
fn accepts_generalized_user_and_rejects_bad_identity() {
    let (_t, p) = fixture();
    let original = fs::read_to_string(&p).unwrap();
    let personal = original.replace("codex", "owner").replace("1000", "1001");
    fs::write(&p, &personal).unwrap();
    assert!(check(&p));
    fs::write(&p, personal.replace("uid: 1001", "uid: 0")).unwrap();
    assert!(!check(&p));
}
