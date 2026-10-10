use looom::apps;
use std::{fs, process::Command};
fn vm() {
    assert_eq!(
        std::env::var("LOOOM_TEST_VM").as_deref(),
        Ok("1"),
        "tests only on VM"
    );
    assert_eq!(unsafe { libc::geteuid() }, 0);
    assert!(matches!(
        looom::util::output("systemd-detect-virt", &["--vm"])
            .unwrap()
            .as_str(),
        "qemu" | "kvm"
    ));
    assert_eq!(
        fs::read_to_string("/sys/class/net/enp1s0/address")
            .unwrap()
            .trim(),
        "52:54:00:7b:23:63"
    );
}
#[test]
fn rejects_ambiguous_apps_and_executable_sources() {
    vm();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("apps.yaml");
    fs::write(&path, apps::TEMPLATE).unwrap();
    assert!(apps::load(&path).is_ok());
    for content in [
        "schema: 1\nschema: 1\n",
        "schema: 1\nunknown: true\n",
        "schema: 1\nflatpak: [--system]\n",
        "schema: 1\nflatpak: [org.test.App, org.test.App]\n",
        "schema: 1\narch: {packages: [\"foo; touch /tmp/host\"]}\n",
        "schema: 1\narch: {image: archlinux}\n",
        "schema: 1\narch: {image: \"-docker.io/x/y\"}\n",
        "schema: 1\nflatpak: &apps []\n",
        "schema: 1\nappimages: {test: {url: 'http://example.org/x', sha256: abc}}\n",
        "schema: 1\n---\nschema: 1\n",
    ] {
        fs::write(&path, content).unwrap();
        assert!(apps::load(&path).is_err(), "accepted {content}");
    }
    let digest = "a".repeat(64);
    fs::write(
        &path,
        format!(
            "schema: 1\nappimages:\n  test:\n    source: ./test.AppImage\n    sha256: {digest}\n"
        ),
    )
    .unwrap();
    assert!(apps::load(&path).is_ok());
    for extra in [
        "    url: https://example.org/x\n",
        "    name: \"bad\\nentry\"\n",
    ] {
        let base = fs::read_to_string(&path).unwrap();
        fs::write(&path, format!("{base}{extra}")).unwrap();
        assert!(apps::load(&path).is_err());
        fs::write(&path, base).unwrap();
    }
}
#[test]
fn root_cannot_apply_or_run_installers() {
    vm();
    for args in [
        vec!["apps", "apply"],
        vec!["apps", "exec", "--", "pacman", "-S", "tree"],
        vec!["apps", "run-script", "/tmp/installer.sh"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_looom"))
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("regular user")
        );
    }
}
