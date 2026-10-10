use looom::{
    credentials::{self, Accounts},
    machine::Machine,
    releases::{Manager, Metadata},
    util::*,
};
use std::{
    fs::{self, File},
    io::Write,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    sync::Arc,
};

fn vm() {
    assert_eq!(
        std::env::var("LOOOM_TEST_VM").as_deref(),
        Ok("1"),
        "tests only on authorized VM"
    );
    assert_eq!(
        unsafe { libc::geteuid() },
        0,
        "runtime tests require root in VM"
    );
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
}
struct Mounted(PathBuf);
impl Drop for Mounted {
    fn drop(&mut self) {
        // Never recursively remove a mountpoint, including when unmount fails.
        if succeeds("umount", &[self.0.to_str().unwrap()]) {
            let _ = fs::remove_dir(&self.0);
        } else {
            eprintln!(
                "Fixture mount retained for explicit cleanup: {}",
                self.0.display()
            );
        }
    }
}
#[test]
fn native_runtime_suite() {
    vm();
    let workspace = Path::new("/workspace/.native-fixtures");
    mkdir(workspace, 0o700).unwrap();
    let temp = tempfile::Builder::new()
        .prefix("rust-")
        .tempdir_in(workspace)
        .unwrap();
    credential_cases(temp.path());
    publication_and_gc_cases(temp.path());
}
fn credential_cases(root: &Path) {
    let fixture = root.join("accounts");
    mkdir(&fixture, 0o700).unwrap();
    for name in ["credentials", "templates"] {
        mkdir(&fixture.join(name), 0o700).unwrap();
    }
    let runtime = tempfile::Builder::new()
        .prefix("looom-native-accounts-")
        .tempdir_in("/run")
        .unwrap();
    fs::set_permissions(runtime.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let accounts = Arc::new(Accounts {
        credentials: fixture.join("credentials"),
        templates: fixture.join("templates"),
        runtime: runtime.path().into(),
        managed: vec!["root".into(), "codex".into()],
    });
    fs::write(
        accounts.templates.join("shadow"),
        "root:!:20000:0:99999:7:::\ncodex:!:20000:0:99999:7:::\n",
    )
    .unwrap();
    fs::write(accounts.templates.join("gshadow"), "root:!::\ncodex:!::\n").unwrap();
    for user in &accounts.managed {
        let value = credentials::hash_password("test-only-fixture").unwrap();
        credentials::private_atomic(
            &accounts.credentials.join(format!("{user}.hash")),
            value.as_bytes(),
        )
        .unwrap();
    }
    accounts.generate().unwrap();
    for path in [
        accounts.credentials.join("codex.hash"),
        accounts.runtime.join("shadow"),
    ] {
        assert!(!succeeds(
            "runuser",
            &["-u", "codex", "--", "test", "-r", string(&path).unwrap()]
        ));
    }
    let path = accounts.credentials.join("codex.hash");
    let original = fs::read(&path).unwrap();
    let before = fs::read(accounts.runtime.join("shadow")).unwrap();
    for bad in ["", "$garbage", "$6$bad", "!", "\n"] {
        fs::write(&path, bad).unwrap();
        assert!(accounts.generate().is_err());
        assert_eq!(before, fs::read(accounts.runtime.join("shadow")).unwrap());
    }
    fs::write(&path, &original).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(accounts.generate().is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    command("chown", &["1000:1000", string(&path).unwrap()]).unwrap();
    assert!(accounts.generate().is_err());
    command("chown", &["0:0", string(&path).unwrap()]).unwrap();
    command("setfacl", &["-m", "u:1000:r", string(&path).unwrap()]).unwrap();
    assert!(accounts.generate().is_err());
    command("setfacl", &["-b", string(&path).unwrap()]).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    command(
        "setfacl",
        &["-dm", "u:1000:r-x", string(&accounts.credentials).unwrap()],
    )
    .unwrap();
    assert!(accounts.generate().is_err());
    command("setfacl", &["-k", string(&accounts.credentials).unwrap()]).unwrap();
    fs::remove_file(&path).unwrap();
    symlink(accounts.credentials.join("root.hash"), &path).unwrap();
    assert!(accounts.generate().is_err());
    fs::remove_file(&path).unwrap();
    assert!(accounts.generate().is_err());
    credentials::private_atomic(&path, &original).unwrap();
    // One runtime suite owns failpoint environment; no other test thread changes it.
    unsafe {
        std::env::set_var("LOOOM_FAIL_AFTER", "credential");
    }
    assert!(
        accounts
            .set_password("codex", "new-test-fixture-password")
            .is_err()
    );
    unsafe {
        std::env::remove_var("LOOOM_FAIL_AFTER");
    }
    assert_eq!(before, fs::read(accounts.runtime.join("shadow")).unwrap());
    accounts.generate().unwrap();
    let workers: Vec<_> = (0..8)
        .map(|index| {
            let accounts = accounts.clone();
            std::thread::spawn(move || {
                accounts
                    .set_password("codex", &format!("test-fixture-{index}"))
                    .unwrap()
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    let value = credentials::read_private(&path).unwrap();
    assert_eq!(
        value.trim(),
        fs::read_to_string(accounts.runtime.join("shadow"))
            .unwrap()
            .lines()
            .nth(1)
            .unwrap()
            .split(':')
            .nth(1)
            .unwrap()
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &fs::read(accounts.credentials.join(".transaction.json")).unwrap()
        )
        .unwrap()["phase"],
        "synchronized"
    );
    println!(
        "PASS: native credentials permissions, owners, symlinks, access/default ACL, missing/malformed inputs, interruption, eight concurrent updates"
    );
}
fn publication_and_gc_cases(root: &Path) {
    let suffix = root.file_name().unwrap().to_str().unwrap();
    let top_path = PathBuf::from(format!("/run/looom-native-top-{suffix}"));
    mkdir(&top_path, 0o700).unwrap();
    let uuid = output("blkid", &["-s", "UUID", "-o", "value", "/dev/vda2"]).unwrap();
    command(
        "mount",
        &[
            "-o",
            "subvolid=5,rw",
            &format!("UUID={uuid}"),
            string(&top_path).unwrap(),
        ],
    )
    .unwrap();
    let top = Mounted(top_path);
    let image = root.join("esp.img");
    File::create(&image)
        .unwrap()
        .set_len(512 * 1024 * 1024)
        .unwrap();
    command("mkfs.fat", &["-F", "32", string(&image).unwrap()]).unwrap();
    let esp_path = PathBuf::from(format!("/run/looom-native-esp-{suffix}"));
    mkdir(&esp_path, 0o700).unwrap();
    command(
        "mount",
        &[
            "-o",
            "loop,umask=0077",
            string(&image).unwrap(),
            string(&esp_path).unwrap(),
        ],
    )
    .unwrap();
    let esp = Mounted(esp_path);
    let profile = Machine {
        schema: 1,
        bootloader: looom::machine::Bootloader::Grub,
        root_uuid: uuid.clone(),
        esp_uuid: output("findmnt", &["-nro", "UUID", string(&esp.0).unwrap()]).unwrap(),
        bootstrap_uki_sha256: "0".repeat(64),
        home_subvolume: "@home".into(),
        var_subvolume: "@var".into(),
        state_subvolume: "@state".into(),
        user: "codex".into(),
        uid: 1000,
        gid: 1000,
        serial_console: true,
        guest_agent: true,
        passwordless_sudo: false,
    };
    let state = root.join("release-state");
    mkdir(&state, 0o700).unwrap();
    for name in ["releases", "operations"] {
        mkdir(&state.join(name), 0o700).unwrap();
    }
    let manager = Manager {
        machine: profile,
        state,
        top: top.0.clone(),
        esp: esp.0.clone(),
    };
    // Any surviving confirmed native release can supply this immutable fixture.
    // Real GC is free to remove obsolete releases, including the old mvp-b.
    let records = top.0.join("@state/releases");
    let mut available: Vec<Metadata> = fs::read_dir(records)
        .unwrap()
        .map(|e| serde_json::from_slice(&fs::read(e.unwrap().path()).unwrap()).unwrap())
        .filter(|m: &Metadata| m.phase == "confirmed" && top.0.join(&m.root_subvolume).exists())
        .collect();
    available.sort_by_key(|m| (m.created_at, m.id.clone()));
    let mut metadata = available
        .pop()
        .expect("a confirmed source release is required");
    let id = metadata.id.clone();
    let source = top.0.join(&metadata.root_subvolume);
    metadata.phase = "validated".into();
    metadata.esp_uuid = manager.machine.esp_uuid.clone();
    manager.save(&metadata).unwrap();
    manager.operation(&id, "validated").unwrap();
    mkdir(&manager.efi(), 0o700).unwrap();
    let grub = manager.esp.join("looom/grub");
    mkdir(&grub, 0o700).unwrap();
    command(
        "grub-editenv",
        &[string(&grub.join("grubenv")).unwrap(), "create"],
    )
    .unwrap();
    manager
        .set_environment(&["saved_entry=looom-bootstrap".into()])
        .unwrap();
    let original = "set timeout=5\nmenuentry 'recovery' { true; }\n";
    atomic(&grub.join("grub.cfg"), original.as_bytes(), 0o600).unwrap();
    let environment = manager.environment().unwrap();
    let journals = manager.state.join("publications");
    mkdir(&journals, 0o700).unwrap();
    let journal = journals.join(format!("{id}.json"));
    let victim = esp.0.join("victim");
    fs::write(&victim, b"must remain intact").unwrap();
    for pending in ["../victim", ".uki-link"] {
        let mut checked = manager.clone();
        if pending == ".uki-link" {
            // FAT cannot contain symlinks; exercise the rejection on the
            // fixture's Btrfs directory without touching the real ESP.
            checked.esp = root.join("symlink-esp");
            mkdir(&checked.efi(), 0o700).unwrap();
            symlink(&victim, checked.efi().join(pending)).unwrap();
        }
        json(&journal, &serde_json::json!({"id":&id, "uki_sha256": metadata.uki_sha256, "pending_uki":pending}), 0o600).unwrap();
        assert!(checked.recover_publications().is_err());
        assert_eq!(fs::read(&victim).unwrap(), b"must remain intact");
        fs::remove_file(&journal).unwrap();
        if pending == ".uki-link" {
            fs::remove_file(checked.efi().join(pending)).unwrap();
        }
    }
    println!("PASS: publication recovery rejects traversal and symlink journal targets");
    let mut info = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let path = std::ffi::CString::new(esp.0.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(
        unsafe { libc::statvfs(path.as_ptr(), info.as_mut_ptr()) },
        0
    );
    let info = unsafe { info.assume_init() };
    let mut left = info.f_bavail * info.f_frsize - 1024 * 1024;
    let mut filler = File::create(esp.0.join("filler")).unwrap();
    let block = vec![0u8; 1024 * 1024];
    while left > 0 {
        let count = left.min(block.len() as u64) as usize;
        filler.write_all(&block[..count]).unwrap();
        left -= count as u64;
    }
    filler.sync_all().unwrap();
    drop(filler);
    assert!(manager.publish(&id).is_err());
    assert_eq!(fs::read_to_string(grub.join("grub.cfg")).unwrap(), original);
    assert_eq!(manager.environment().unwrap(), environment);
    assert!(!manager.efi().join(format!("looom-{id}.efi")).exists());
    assert_eq!(manager.load(&id).unwrap().phase, "validated");
    fs::remove_file(esp.0.join("filler")).unwrap();
    unsafe {
        std::env::set_var("LOOOM_FAIL_AFTER", "uki");
    }
    assert!(manager.publish(&id).is_err());
    unsafe {
        std::env::remove_var("LOOOM_FAIL_AFTER");
    }
    manager.write_menu(&manager.menu(None).unwrap()).unwrap();
    assert!(
        !fs::read_to_string(grub.join("grub.cfg"))
            .unwrap()
            .contains(&format!("--id looom-{id} {{"))
    );
    assert_eq!(manager.environment().unwrap(), environment);
    // A FAT power cut may corrupt a renamed but uncommitted UKI. Rebuild it
    // from the verified read-only root; never repair a published slot this way.
    let published_uki = manager.efi().join(format!("looom-{id}.efi"));
    fs::write(&published_uki, b"damaged candidate").unwrap();
    manager.publish(&id).unwrap();
    manager.validate(&manager.load(&id).unwrap(), true).unwrap();
    assert_eq!(manager.environment().unwrap(), environment);
    fs::write(&published_uki, b"damaged published slot").unwrap();
    assert!(manager.publish(&id).is_err());
    assert_eq!(fs::read(&published_uki).unwrap(), b"damaged published slot");
    fs::copy(source.join(format!("boot/looom-{id}.efi")), &published_uki).unwrap();
    println!("PASS: damaged uncommitted UKI rebuilt; damaged published UKI preserved and rejected");
    println!(
        "PASS: native publication on actual FAT ENOSPC, interruption, recovery and unchanged saved choice"
    );
    let gc_id = format!(
        "rust-gc-{}-{}",
        suffix.to_ascii_lowercase(),
        "x".repeat(64 - 9 - suffix.len())
    );
    let owned = top.0.join(format!("@root-{gc_id}"));
    command("btrfs", &["subvolume", "create", string(&owned).unwrap()]).unwrap();
    let mut garbage = metadata.clone();
    garbage.id = gc_id.clone();
    garbage.root_subvolume = format!("@root-{gc_id}");
    garbage.phase = "validated".into();
    manager.save(&garbage).unwrap();
    manager.gc(2, false).unwrap();
    assert!(owned.exists());
    // Saved and one-shot targets, mounted roots and symlinks are protected.
    manager
        .set_environment(&[format!("next_entry=looom-{gc_id}")])
        .unwrap();
    manager.gc(2, true).unwrap();
    assert!(owned.exists());
    manager
        .set_environment(&["next_entry=".into(), format!("saved_entry=looom-{gc_id}")])
        .unwrap();
    manager.gc(2, true).unwrap();
    assert!(owned.exists());
    manager
        .set_environment(&["saved_entry=looom-bootstrap".into()])
        .unwrap();
    manager.operation(&gc_id, "configured").unwrap();
    manager.gc(2, true).unwrap();
    assert!(owned.exists());
    manager.operation(&gc_id, "validated").unwrap();
    mkdir(&owned.join("nested"), 0o700).unwrap();
    let mounted_path = PathBuf::from(format!("/run/looom-gc-mounted-{suffix}"));
    mkdir(&mounted_path, 0o700).unwrap();
    command(
        "mount",
        &[
            "-o",
            &format!("subvol=@root-{gc_id},rw"),
            &format!("UUID={uuid}"),
            string(&mounted_path).unwrap(),
        ],
    )
    .unwrap();
    let mounted = Mounted(mounted_path);
    manager.gc(2, true).unwrap();
    assert!(owned.exists());
    let mut removing = garbage.clone();
    removing.phase = "removing".into();
    manager.save(&removing).unwrap();
    assert!(manager.finish_removal(&removing).is_err());
    let nested_path = PathBuf::from(format!("/run/looom-gc-nested-{suffix}"));
    mkdir(&nested_path, 0o700).unwrap();
    command(
        "mount",
        &[
            "--bind",
            string(&mounted.0.join("nested")).unwrap(),
            string(&nested_path).unwrap(),
        ],
    )
    .unwrap();
    let nested = Mounted(nested_path);
    drop(mounted);
    assert!(manager.finish_removal(&removing).is_err());
    assert!(owned.exists());
    drop(nested);
    let shared = root.join("shared-bind");
    mkdir(&shared, 0o700).unwrap();
    fs::write(shared.join("marker"), b"persistent data must survive").unwrap();
    command(
        "mount",
        &[
            "--bind",
            string(&shared).unwrap(),
            string(&owned.join("nested")).unwrap(),
        ],
    )
    .unwrap();
    let shared_bind = Mounted(owned.join("nested"));
    assert!(manager.finish_removal(&removing).is_err());
    assert!(owned.exists());
    drop(shared_bind);
    mkdir(&owned.join("nested"), 0o700).unwrap();
    command(
        "mount",
        &[
            "-t",
            "tmpfs",
            "-o",
            "size=1M,mode=0700",
            "none",
            string(&owned.join("nested")).unwrap(),
        ],
    )
    .unwrap();
    let foreign_mount = Mounted(owned.join("nested"));
    assert!(manager.finish_removal(&removing).is_err());
    assert!(owned.exists());
    drop(foreign_mount);
    assert_eq!(
        fs::read(shared.join("marker")).unwrap(),
        b"persistent data must survive"
    );
    for point in ["gc-journal", "gc-menu", "gc-root", "gc-uki"] {
        unsafe {
            std::env::set_var("LOOOM_FAIL_AFTER", point);
        }
        assert!(manager.gc(2, true).is_err());
        unsafe {
            std::env::remove_var("LOOOM_FAIL_AFTER");
        }
        assert_eq!(manager.load(&gc_id).unwrap().phase, "removing");
    }
    manager.finish_removal(&removing).unwrap();
    assert!(!owned.exists());
    assert!(manager.load(&gc_id).is_err());
    assert_eq!(manager.load(&id).unwrap().phase, "published");
    // A registry entry cannot turn a symlink into an owned Btrfs root.
    symlink(&source, &owned).unwrap();
    manager.save(&garbage).unwrap();
    assert!(manager.gc(2, true).is_err());
    assert!(source.exists());
    fs::remove_file(&owned).unwrap();
    manager.finish_removal(&removing).unwrap();
    assert!(source.exists());
    println!(
        "PASS: GC preview, selected/mounted protection, symlink refusal and four interrupted removals"
    );
}
