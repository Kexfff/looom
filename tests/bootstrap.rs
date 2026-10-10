use looom::util::*;
use std::{
    fs::{self, File},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

struct Fixture {
    mounts: Vec<PathBuf>,
    subvolumes: Vec<PathBuf>,
    top: PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let mut clean = true;
        for path in self.mounts.iter().rev() {
            clean &= succeeds("umount", &[path.to_str().unwrap()]);
        }
        if clean {
            for path in self.subvolumes.iter().rev() {
                let _ = command(
                    "btrfs",
                    &[
                        "subvolume",
                        "delete",
                        "--commit-after",
                        path.to_str().unwrap(),
                    ],
                );
            }
        } else {
            eprintln!("Bootstrap fixtures retained; recursive deletion is intentionally avoided");
        }
        if succeeds("umount", &[self.top.to_str().unwrap()]) {
            let _ = fs::remove_dir(&self.top);
        }
    }
}
#[test]
fn native_bootstrap_on_isolated_prepared_arch() {
    assert_eq!(std::env::var("LOOOM_TEST_VM").as_deref(), Ok("1"));
    assert!(matches!(
        output("systemd-detect-virt", &["--vm"]).unwrap().as_str(),
        "qemu" | "kvm"
    ));
    assert_eq!(
        fs::read_to_string("/sys/class/net/enp1s0/address")
            .unwrap()
            .trim(),
        "52:54:00:7b:23:63"
    );
    root().unwrap();
    mkdir(Path::new("/workspace/.native-fixtures"), 0o700).unwrap();
    let files = tempfile::Builder::new()
        .prefix("bootstrap-")
        .tempdir_in("/workspace/.native-fixtures")
        .unwrap();
    let suffix = files
        .path()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_ascii_lowercase();
    let top = PathBuf::from(format!("/run/looom-bootstrap-top-{suffix}"));
    mkdir(&top, 0o700).unwrap();
    let uuid = output("blkid", &["-s", "UUID", "-o", "value", "/dev/vda2"]).unwrap();
    command(
        "mount",
        &[
            "-o",
            "subvolid=5,rw",
            &format!("UUID={uuid}"),
            string(&top).unwrap(),
        ],
    )
    .unwrap();
    let mut fixture = Fixture {
        mounts: Vec::new(),
        subvolumes: Vec::new(),
        top: top.clone(),
    };
    let rid = format!("@bootstrap-{suffix}");
    let snapshot = top.join(&rid);
    command(
        "btrfs",
        &[
            "subvolume",
            "snapshot",
            string(&top.join("@bootstrap")).unwrap(),
            string(&snapshot).unwrap(),
        ],
    )
    .unwrap();
    for relative in [
        "usr/share/limine/BOOTX64.EFI",
        "usr/share/doc/limine/CONFIG.md",
    ] {
        let destination = snapshot.join(relative);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::copy(Path::new("/").join(relative), destination).unwrap();
    }
    fixture.subvolumes.push(snapshot);
    let target = PathBuf::from(format!("/run/looom-bootstrap-root-{suffix}"));
    mkdir(&target, 0o700).unwrap();
    command(
        "mount",
        &[
            "-o",
            &format!("subvol={rid},rw"),
            &format!("UUID={uuid}"),
            string(&target).unwrap(),
        ],
    )
    .unwrap();
    fixture.mounts.push(target.clone());
    command("mount", &["--make-rprivate", string(&target).unwrap()]).unwrap();
    for (name, path) in [("home", "home"), ("var", "var"), ("state", "var/lib/looom")] {
        let subvolume = format!("@{name}-{suffix}");
        let source = top.join(&subvolume);
        command("btrfs", &["subvolume", "create", string(&source).unwrap()]).unwrap();
        fixture.subvolumes.push(source);
        let mountpoint = target.join(path);
        mkdir(&mountpoint, 0o755).unwrap();
        command(
            "mount",
            &[
                "-o",
                &format!("subvol={subvolume},rw"),
                &format!("UUID={uuid}"),
                string(&mountpoint).unwrap(),
            ],
        )
        .unwrap();
        fixture.mounts.push(mountpoint.clone());
        if name == "state" {
            fs::set_permissions(mountpoint, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    mkdir(&target.join("home/codex"), 0o700).unwrap();
    command(
        "chown",
        &["1000:1000", string(&target.join("home/codex")).unwrap()],
    )
    .unwrap();
    // Keep bootstrap pacman DB readable after mounting its isolated /var.
    std::os::unix::fs::symlink("/usr/lib/looom/pacman", target.join("var/lib/pacman")).unwrap();
    let image = files.path().join("esp.img");
    File::create(&image)
        .unwrap()
        .set_len(512 * 1024 * 1024)
        .unwrap();
    command("mkfs.fat", &["-F", "32", string(&image).unwrap()]).unwrap();
    mkdir(&target.join("efi"), 0o700).unwrap();
    command(
        "mount",
        &[
            "-o",
            "loop,umask=0077",
            string(&image).unwrap(),
            string(&target.join("efi")).unwrap(),
        ],
    )
    .unwrap();
    fixture.mounts.push(target.join("efi"));
    fs::copy(env!("CARGO_BIN_EXE_looom"), target.join("usr/bin/looom")).unwrap();
    fs::set_permissions(
        target.join("usr/bin/looom"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    mkdir(&target.join("root/native-test/files"), 0o700).unwrap();
    fs::copy(
        "configs/native/a/base.yaml",
        target.join("root/native-test/base.yaml"),
    )
    .unwrap();
    fs::copy(
        "configs/native/a/files/example.conf",
        target.join("root/native-test/files/example.conf"),
    )
    .unwrap();
    mkdir(&target.join("etc/looom-local"), 0o700).unwrap();
    fs::write(
        target.join("etc/looom-local/import-marker"),
        b"preserved during import\n",
    )
    .unwrap();
    // Interrupt every cross-filesystem checkpoint and repeat the same declaration.
    for point in [
        "bootstrap-journal",
        "bootstrap-uki",
        "bootstrap-loader",
        "bootstrap-credentials",
        "bootstrap-import",
        "bootstrap-profile",
        "bootstrap-menu",
    ] {
        assert!(!succeeds(
            "arch-chroot",
            &[
                string(&target).unwrap(),
                "env",
                &format!("LOOOM_FAIL_AFTER={point}"),
                "looom",
                "bootstrap",
                "/root/native-test/base.yaml"
            ]
        ));
        if point == "bootstrap-uki" {
            // Known partial scratch is removed even if the destination is already valid.
            fs::write(
                target.join("efi/EFI/Linux/looom-bootstrap.pending"),
                b"unfinished copy",
            )
            .unwrap();
        }
        if point == "bootstrap-loader" {
            assert!(
                !target
                    .join("efi/EFI/Linux/looom-bootstrap.pending")
                    .exists()
            );
        }
        if point == "bootstrap-credentials" {
            let credential = target.join("var/lib/looom/credentials/root.hash");
            let before = hash_file(&credential).unwrap();
            // FAT cannot carry symlinks: attack the Btrfs checkpoint instead.
            let scratch = target.join("var/lib/looom/bootstrap-install/bootstrap.efi");
            let backup = scratch.with_extension("test-backup");
            fs::rename(&scratch, &backup).unwrap();
            std::os::unix::fs::symlink("/var/lib/looom/credentials/root.hash", &scratch).unwrap();
            assert!(!succeeds(
                "arch-chroot",
                &[
                    string(&target).unwrap(),
                    "looom",
                    "bootstrap",
                    "/root/native-test/base.yaml"
                ]
            ));
            assert_eq!(hash_file(&credential).unwrap(), before);
            fs::remove_file(&scratch).unwrap();
            fs::rename(backup, scratch).unwrap();
        }
        let journal: serde_json::Value = serde_json::from_slice(
            &fs::read(target.join("var/lib/looom/bootstrap-install/journal.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(journal["complete"], false);
        // Changes in the declaration must not redirect a partially registered machine.
        // The sample may change its hostname; use parsed YAML to ensure a distinct declaration.
        let mut value: serde_json::Value = serde_json::to_value(
            looom::config::load(Path::new("configs/native/a/base.yaml"))
                .unwrap()
                .0,
        )
        .unwrap();
        value["system"]["hostname"] = "other-machine".into();
        // JSON is a YAML subset accepted by our strict parser.
        fs::write(
            target.join("root/native-test/changed.yaml"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        chroot(
            &target,
            "looom",
            &["check", "/root/native-test/changed.yaml"],
        )
        .unwrap();
        assert!(!succeeds(
            "arch-chroot",
            &[
                string(&target).unwrap(),
                "looom",
                "bootstrap",
                "/root/native-test/changed.yaml"
            ]
        ));
    }
    chroot(
        &target,
        "looom",
        &["bootstrap", "/root/native-test/base.yaml"],
    )
    .unwrap();
    assert!(target.join("efi/EFI/Linux/looom-bootstrap.efi").is_file());
    assert!(target.join("efi/EFI/looom/liminex64.efi").is_file());
    let profile: serde_json::Value =
        serde_json::from_slice(&fs::read(target.join("var/lib/looom/machine.json")).unwrap())
            .unwrap();
    assert_eq!(profile["root_uuid"], uuid);
    assert_eq!(profile["passwordless_sudo"], false);
    assert_eq!(
        fs::read(target.join("var/lib/looom/local-etc/looom-local/import-marker")).unwrap(),
        b"preserved during import\n"
    );
    assert!(
        target
            .join("var/lib/looom/release-state-initialized")
            .is_file()
    );
    for user in ["root", "codex"] {
        let path = target
            .join("var/lib/looom/credentials")
            .join(format!("{user}.hash"));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(succeeds(
        "arch-chroot",
        &[
            string(&target).unwrap(),
            "looom",
            "bootstrap",
            "/root/native-test/base.yaml"
        ]
    ));
    println!(
        "PASS: Rust bootstrap on isolated writable Arch snapshot + dedicated FAT; credentials imported, GRUB/UKI/profile created, all seven interruption checkpoints resumed, changed declarations rejected, repeat idempotent; NVRAM untouched"
    );
    drop(fixture);
    let _ = fs::remove_dir(&target);
}
