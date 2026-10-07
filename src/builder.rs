use crate::{
    config::Config,
    credentials,
    machine::Machine,
    packages::{self, PackageLock},
    releases::{Manager, Metadata, timestamp},
    util::*,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
};

#[derive(Deserialize, Serialize)]
pub struct Bundle {
    pub schema: u32,
    pub config: Config,
    pub files: BTreeMap<String, Vec<u8>>,
    pub request: Value,
    pub lock: PackageLock,
    pub release: String,
}
const INITRAMFS: &str = "MODULES=(btrfs)\nBINARIES=()\nFILES=()\nHOOKS=(base systemd microcode modconf kms keyboard sd-vconsole block filesystems fsck)\nCOMPRESSION=\"zstd\"\n";
fn write(root: &Path, name: &str, data: &str, mode: u32) -> Result<()> {
    let path = root.join(name.trim_start_matches('/'));
    mkdir(path.parent().context("parent")?, 0o755)?;
    remove_if_exists(&path)?;
    atomic(&path, data.as_bytes(), mode)
}
fn link(root: &Path, name: &str, target: &str) -> Result<()> {
    let path = root.join(name.trim_start_matches('/'));
    mkdir(path.parent().context("parent")?, 0o755)?;
    remove_if_exists(&path)?;
    symlink(target, path)?;
    Ok(())
}
struct BuildMount {
    path: PathBuf,
}
impl BuildMount {
    fn mount(machine: &Machine, rid: &str) -> Result<Self> {
        let path = PathBuf::from(format!("/run/looom-build-{rid}"));
        mkdir(&path, 0o700)?;
        if !succeeds("mountpoint", &["-q", string(&path)?]) {
            command(
                "mount",
                &[
                    "-o",
                    &format!("subvol=@build-{rid},rw,noatime,compress=zstd:3"),
                    &format!("UUID={}", machine.root_uuid),
                    string(&path)?,
                ],
            )?;
        }
        ensure!(
            output("findmnt", &["-nro", "UUID", string(&path)?])? == machine.root_uuid
                && output("findmnt", &["-nro", "FSROOT", string(&path)?])?
                    == format!("/@build-{rid}"),
            "unexpected private build mount"
        );
        Ok(Self { path })
    }
}
impl Drop for BuildMount {
    fn drop(&mut self) {
        if succeeds("umount", &[self.path.to_str().unwrap_or("")]) {
            let _ = fs::remove_dir(&self.path);
        }
    }
}

pub fn build(manager: &Manager, bundle: Bundle) -> Result<()> {
    let rid = &bundle.release;
    ensure!(crate::config::identifier(rid), "invalid release ID");
    manager.machine.config_contract(&bundle.config)?;
    bundle.lock.validate(&bundle.config, true)?;
    ensure!(
        bundle.request == packages::request(&bundle.config)?,
        "recipe identity changed"
    );
    for user in ["root", manager.machine.user.as_str()] {
        credentials::validate_hash(
            credentials::read_private(
                &manager
                    .state
                    .join("credentials")
                    .join(format!("{user}.hash")),
            )?
            .trim(),
        )?;
    }
    let inputs = manager.state.join("inputs").join(rid);
    ensure!(
        !inputs.exists()
            && !manager.top.join(format!("@build-{rid}")).exists()
            && !manager.top.join(format!("@root-{rid}")).exists()
            && !manager
                .state
                .join("releases")
                .join(format!("{rid}.json"))
                .exists(),
        "refusing to overwrite an existing build/release"
    );
    let info = output("df", &["-B1", "--output=avail", string(&manager.top)?])?;
    let available: u64 = info.lines().last().context("df")?.trim().parse()?;
    ensure!(
        available
            > if bundle.config.desktop.environment == "plasma" {
                10 * 1024 * 1024 * 1024
            } else {
                4 * 1024 * 1024 * 1024
            },
        "insufficient space for private build"
    );
    mkdir(&inputs, 0o700)?;
    json(&inputs.join("bundle.json"), &bundle, 0o600)?;
    json(&inputs.join("machine.json"), &manager.machine, 0o600)?;
    let executable = std::env::current_exe()?;
    fs::copy(&executable, inputs.join("looom"))?;
    fs::set_permissions(inputs.join("looom"), fs::Permissions::from_mode(0o700))?;
    fs::File::open(inputs.join("looom"))?.sync_all()?;
    sync_dir(&inputs)?;
    ensure!(
        hash_file(&inputs.join("looom"))?
            == bundle.request["recipe_sha256"]
                .as_str()
                .context("binary recipe hash")?,
        "frozen binary differs"
    );
    manager.operation(rid, "building")?;
    command(
        "btrfs",
        &[
            "subvolume",
            "create",
            string(&manager.top.join(format!("@build-{rid}")))?,
        ],
    )?;
    let mount = BuildMount::mount(&manager.machine, rid)?;
    let root = &mount.path;
    mkdir(&root.join("etc"), 0o755)?;
    let registry = registry_directory(&manager.state)?;
    for name in ["passwd", "group", "shadow", "gshadow"] {
        let data = fs::read_to_string(registry.join(name))?;
        ensure!(
            !name.contains("shadow") || data.lines().all(|l| l.split(':').nth(1) == Some("!")),
            "registry must contain locked shadow fields"
        );
        atomic(
            &root.join("etc").join(name),
            data.as_bytes(),
            if name.contains("shadow") {
                0o600
            } else {
                0o644
            },
        )?;
    }
    let conf = inputs.join("pacman.conf");
    atomic(
        &conf,
        packages::config(&bundle.config.source.snapshot, false).as_bytes(),
        0o600,
    )?;
    let paths: Vec<String> = bundle
        .lock
        .packages
        .iter()
        .map(|p| format!("/var/cache/pacman/pkg/{}", p.archive))
        .collect();
    let mut args = vec!["-U", "-c", "-K", "-M", "-C", string(&conf)?, string(root)?];
    args.extend(paths.iter().map(String::as_str));
    command("pacstrap", &args)?;
    mkdir(&root.join("usr/lib/looom"), 0o755)?;
    fs::rename(
        root.join("var/lib/pacman"),
        root.join("usr/lib/looom/pacman"),
    )?;
    link(root, "var/lib/pacman", "/usr/lib/looom/pacman")?;
    mkdir(&root.join("usr/lib/looom/pacman/sync"), 0o755)?;
    for (repo, record) in &bundle.lock.repositories {
        let source = manager
            .state
            .join("repository-cache")
            .join(&bundle.lock.archive_date)
            .join(format!("{repo}.db"));
        ensure!(
            hash_file(&source)? == record.database_sha256,
            "repository database hash mismatch"
        );
        fs::copy(
            source,
            root.join("usr/lib/looom/pacman/sync")
                .join(format!("{repo}.db")),
        )?;
    }
    write(
        root,
        "etc/pacman.conf",
        &packages::config(&bundle.config.source.snapshot, true),
        0o644,
    )?;
    configure(manager, root, &bundle)?;
    ensure!(
        packages::inventory(Some(root))? == bundle.lock.inventory(),
        "installed closure differs from input lock"
    );
    bundle.lock.validate(&bundle.config, true)?;
    sanitize_accounts(manager, root, &bundle)?;
    let evidence = manager.state.join("release-evidence").join(rid);
    mkdir(&evidence, 0o700)?;
    let packages = bundle
        .lock
        .packages
        .iter()
        .map(|p| format!("{} {}\n", p.name, p.version))
        .collect::<String>();
    atomic(&evidence.join("packages.txt"), packages.as_bytes(), 0o600)?;
    json(&evidence.join("input.lock.json"), &bundle.lock, 0o600)?;
    for name in ["etc/fstab", "etc/kernel/cmdline"] {
        fs::copy(
            root.join(name),
            evidence.join(Path::new(name).file_name().unwrap()),
        )?;
    }
    json(
        &evidence.join("build.json"),
        &json!({"engine":"rust-0.2","binary_sha256":hash_file(&inputs.join("looom"))?,"uki_sha256":hash_file(&root.join("boot").join(format!("looom-{rid}.efi")))?,"package_count":bundle.lock.packages.len(),"machine":manager.machine}),
        0o600,
    )?;
    command("sync", &["-f", string(root)?])?;
    manager.operation(rid, "configured")?;
    failpoint("build")?;
    finalize_mounted(manager, &bundle, root)?;
    drop(mount);
    delete_private(manager, rid)?;
    println!("Validated read-only Rust release {rid}; publish explicitly");
    Ok(())
}
fn configure(manager: &Manager, root: &Path, bundle: &Bundle) -> Result<()> {
    let cfg = &bundle.config;
    let m = &manager.machine;
    let rid = &bundle.release;
    for path in [
        "home",
        "efi",
        "etc/looom-local",
        "etc/NetworkManager/system-connections",
        "var/lib/looom",
    ] {
        mkdir(&root.join(path), 0o755)?;
    }
    let fstab = m.fstab(&format!("@root-{rid}"), true)?;
    write(root, "etc/fstab", &fstab, 0o644)?;
    write(
        root,
        "etc/machine-id",
        &fs::read_to_string(manager.state.join("machine-id"))?,
        0o444,
    )?;
    ensure!(
        root.join("usr/share/zoneinfo")
            .join(&cfg.system.timezone)
            .is_file(),
        "unknown timezone"
    );
    link(
        root,
        "etc/localtime",
        &format!("/usr/share/zoneinfo/{}", cfg.system.timezone),
    )?;
    write(
        root,
        "etc/hostname",
        &format!("{}\n", cfg.system.hostname),
        0o644,
    )?;
    write(
        root,
        "etc/hosts",
        &format!(
            "127.0.0.1 localhost\n::1 localhost\n127.0.1.1 {}\n",
            cfg.system.hostname
        ),
        0o644,
    )?;
    write(
        root,
        "etc/locale.gen",
        &format!("{} UTF-8\n", cfg.system.locale),
        0o644,
    )?;
    write(
        root,
        "etc/locale.conf",
        &format!("LANG={}\n", cfg.system.locale),
        0o644,
    )?;
    write(
        root,
        "etc/vconsole.conf",
        &format!("KEYMAP={}\n", cfg.system.console_keymap),
        0o644,
    )?;
    chroot(root, "locale-gen", &[])?;
    ensure!(
        chroot_output(root, "id", &["-u", &m.user])? == m.uid.to_string()
            && chroot_output(root, "id", &["-g", &m.user])? == m.gid.to_string(),
        "personal UID/GID differs"
    );
    write(
        root,
        "etc/sudoers.d/10-looom-vm",
        &format!(
            "{} ALL=(ALL:ALL) {}ALL\n",
            m.user,
            if m.passwordless_sudo {
                "NOPASSWD: "
            } else {
                ""
            }
        ),
        0o440,
    )?;
    write(
        root,
        "etc/ssh/sshd_config.d/10-looom-vm.conf",
        &format!(
            "PubkeyAuthentication yes\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPermitRootLogin prohibit-password\nAllowUsers root {}\nHostKey /var/lib/looom/ssh/ssh_host_ed25519_key\nHostKey /var/lib/looom/ssh/ssh_host_rsa_key\n",
            m.user
        ),
        0o644,
    )?;
    if root.join("root/.ssh").exists() {
        fs::remove_dir_all(root.join("root/.ssh"))?;
    }
    mkdir(&root.join("root/.ssh"), 0o700)?;
    write(
        root,
        "etc/NetworkManager/conf.d/10-looom-readonly.conf",
        "[main]\nrc-manager=unmanaged\n",
        0o644,
    )?;
    link(root, "etc/resolv.conf", "/run/NetworkManager/resolv.conf")?;
    write(root, "etc/looom/release-id", &format!("{rid}\n"), 0o644)?;
    write(
        root,
        "etc/looom/declarative-value",
        &format!("{rid}\n"),
        0o644,
    )?;
    fs::copy(
        manager.state.join("inputs").join(rid).join("looom"),
        root.join("usr/bin/looom"),
    )?;
    fs::set_permissions(
        root.join("usr/bin/looom"),
        fs::Permissions::from_mode(0o755),
    )?;
    link(root, "usr/bin/looom-password", "looom")?;
    link(root, "usr/bin/looom-release", "looom")?;
    write(
        root,
        "etc/systemd/system/looom-accounts.service",
        "[Unit]\nDescription=Native looom runtime credentials\nDefaultDependencies=no\nRequiresMountsFor=/var/lib/looom\nAfter=local-fs.target\nBefore=systemd-tmpfiles-setup.service sysinit.target\nConflicts=shutdown.target\nBefore=shutdown.target\n[Service]\nType=oneshot\nExecStart=/usr/bin/looom accounts generate\nRemainAfterExit=yes\n[Install]\nRequiredBy=sysinit.target\n",
        0o644,
    )?;
    link(
        root,
        "etc/systemd/system/systemd-sysusers.service",
        "/dev/null",
    )?;
    for unit in ["sshd", "NetworkManager", "systemd-timesyncd", "shadow"] {
        write(
            root,
            &format!("etc/systemd/system/{unit}.service.d/10-looom-accounts.conf"),
            "[Unit]\nRequires=looom-accounts.service\nAfter=looom-accounts.service\n",
            0o644,
        )?;
    }
    write(
        root,
        "etc/systemd/system/shadow.service.d/20-native-check.conf",
        "[Service]\nExecStart=\nExecStart=/usr/bin/looom accounts check\n",
        0o644,
    )?;
    if cfg.desktop.environment == "plasma" {
        write(
            root,
            "etc/sddm.conf.d/10-looom.conf",
            "[Theme]\nCurrent=breeze\n[General]\nDisplayServer=x11\n",
            0o644,
        )?;
        write(
            root,
            "etc/systemd/system/sddm.service.d/10-looom-accounts.conf",
            "[Unit]\nRequires=looom-accounts.service\nAfter=looom-accounts.service\n",
            0o644,
        )?;
    }
    apply_files(root, bundle)?;
    apply_units(root, bundle, m)?;
    let serial = if m.serial_console {
        " console=ttyS0,115200n8"
    } else {
        ""
    };
    write(
        root,
        "etc/kernel/cmdline",
        &format!(
            "root=UUID={} rootflags=subvol=@root-{rid} ro console=tty0{serial}\n",
            m.root_uuid
        ),
        0o644,
    )?;
    // Portable initramfs: include all storage drivers instead of detecting only the build machine.
    write(root, "etc/mkinitcpio.conf", INITRAMFS, 0o644)?;
    let kernel = kernel_version(root)?;
    fs::copy(
        root.join("usr/lib/modules").join(&kernel).join("vmlinuz"),
        root.join("boot/vmlinuz-looom"),
    )?;
    for entry in fs::read_dir(root.join("etc/mkinitcpio.d"))? {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) == Some("preset") {
            fs::remove_file(path)?;
        }
    }
    write(
        root,
        "etc/mkinitcpio.d/looom.preset",
        &format!(
            "ALL_config=\"/etc/mkinitcpio.conf\"\nALL_kver=\"/boot/vmlinuz-looom\"\nPRESETS=('default')\ndefault_uki=\"/boot/looom-{rid}.efi\"\ndefault_options=\"--cmdline /etc/kernel/cmdline\"\n"
        ),
        0o644,
    )?;
    chroot(root, "mkinitcpio", &["-P"])?;
    chroot(
        root,
        "ssh-keygen",
        &[
            "-q",
            "-t",
            "ed25519",
            "-N",
            "",
            "-f",
            "/etc/ssh/looom-validation.key",
        ],
    )?;
    let result = chroot(
        root,
        "sshd",
        &["-t", "-o", "HostKey=/etc/ssh/looom-validation.key"],
    );
    remove_if_exists(&root.join("etc/ssh/looom-validation.key"))?;
    remove_if_exists(&root.join("etc/ssh/looom-validation.key.pub"))?;
    result?;
    chroot(root, "visudo", &["-cf", "/etc/sudoers"])?;
    chroot(root, "pacman", &["-Dk"])?;
    chroot(
        root,
        "runuser",
        &["-u", "dbus", "--", "test", "-r", "/etc/machine-id"],
    )?;
    json(&root.join("usr/lib/looom/declaration.json"), bundle, 0o644)?;
    install_sources(root)?;
    Ok(())
}
fn apply_files(root: &Path, bundle: &Bundle) -> Result<()> {
    for (name, file) in &bundle.config.files {
        let mut path = root.to_path_buf();
        for component in Path::new(name).components().skip(1) {
            path.push(component.as_os_str());
            ensure!(
                !fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()),
                "target symlink rejected"
            );
        }
        ensure!(
            !path.exists() || file.replace_package_file,
            "explicit replace_package_file required: {name}"
        );
        mkdir(path.parent().unwrap(), 0o755)?;
        atomic(
            &path,
            bundle.files.get(name).context("missing frozen file")?,
            u32::from_str_radix(&file.mode, 8)?,
        )?;
    }
    Ok(())
}
fn apply_units(root: &Path, bundle: &Bundle, m: &Machine) -> Result<()> {
    let mut vendor = Vec::new();
    for base in ["etc/systemd/system", "usr/lib/systemd/system"] {
        for entry in fs::read_dir(root.join(base))? {
            let directory = entry?.path();
            if directory.extension().and_then(|s| s.to_str()) != Some("wants")
                || !fs::symlink_metadata(&directory)?.is_dir()
            {
                continue;
            }
            for entry in fs::read_dir(&directory)? {
                let path = entry?.path();
                if !fs::symlink_metadata(&path)?.file_type().is_symlink() {
                    continue;
                }
                let logical = format!("/{}", path.strip_prefix(root)?.display());
                let keep = if base.starts_with("usr") {
                    let owner = chroot_output(root, "pacman", &["-Qqo", &logical])?;
                    ["systemd", "dbus", "dbus-broker-units", "filesystem"].contains(&owner.as_str())
                        || logical == "/usr/lib/systemd/system/timers.target.wants/shadow.timer"
                } else {
                    false
                };
                if keep {
                    vendor.push(logical);
                } else {
                    fs::remove_file(path)?;
                }
            }
        }
    }
    let mut units: BTreeMap<String, String> = [
        "looom-accounts.service",
        "NetworkManager.service",
        "sshd.service",
        "systemd-timesyncd.service",
    ]
    .into_iter()
    .map(|s| (s.into(), "enabled".into()))
    .collect();
    if m.serial_console {
        units.insert("serial-getty@ttyS0.service".into(), "enabled".into());
    }
    if bundle.config.desktop.environment == "plasma" {
        units.insert("sddm.service".into(), "enabled".into());
    }
    for (name, state) in &bundle.config.units {
        units.insert(
            name.clone(),
            match state {
                crate::config::UnitState::Enabled => "enabled",
                crate::config::UnitState::Disabled => "disabled",
                crate::config::UnitState::Masked => "masked",
            }
            .into(),
        );
    }
    for (name, state) in &units {
        let action = match state.as_str() {
            "enabled" => "enable",
            "disabled" => "disable",
            _ => "mask",
        };
        command(
            "systemctl",
            &[&format!("--root={}", root.display()), action, name],
        )?;
        if state == "masked" {
            // mkinitcpio's systemd hook copies vendor units and their /etc
            // drop-ins, but does not preserve /etc masks in the initramfs.
            // /dev/null cannot be a directory, so this condition is always
            // false and also prevents the masked unit running before switch-root.
            write(
                root,
                &format!("etc/systemd/system/{name}.d/00-looom-initrd-mask.conf"),
                "[Unit]\nConditionPathExists=/dev/null/looom-masked\n",
                0o644,
            )?;
        }
        if state == "enabled" {
            let enabled = output(
                "systemctl",
                &[&format!("--root={}", root.display()), "is-enabled", name],
            )?;
            ensure!(
                ["enabled", "alias", "enabled-runtime"].contains(&enabled.as_str()),
                "unit has no enablement contract: {name}"
            );
        }
    }
    for name in &bundle.config.health.required_units {
        ensure!(
            root.join("etc/systemd/system").join(name).exists()
                || root.join("usr/lib/systemd/system").join(name).exists(),
            "missing health unit: {name}"
        );
    }
    command(
        "systemctl",
        &[
            &format!("--root={}", root.display()),
            "set-default",
            if bundle.config.desktop.environment == "plasma" {
                "graphical.target"
            } else {
                "multi-user.target"
            },
        ],
    )?;
    json(
        &root.join("usr/lib/looom/unit-plan.json"),
        &json!({"units":units,"core_vendor_dependencies":vendor,"device_activated_guest_agent":m.guest_agent}),
        0o644,
    )
}
fn registry_directory(state: &Path) -> Result<PathBuf> {
    let pointer = state.join("current-registry.json");
    if pointer.exists() {
        let id: String = serde_json::from_slice(&fs::read(pointer)?)?;
        ensure!(
            crate::config::identifier(&id),
            "invalid account registry pointer"
        );
        Ok(state.join("registry-generations").join(id))
    } else {
        Ok(state.join("account-registry"))
    }
}
fn sanitize_accounts(manager: &Manager, root: &Path, bundle: &Bundle) -> Result<()> {
    let previous = registry_directory(&manager.state)?;
    let generation = manager
        .state
        .join("registry-generations")
        .join(&bundle.release);
    mkdir(&generation, 0o700)?;
    mkdir(&root.join("usr/lib/looom/accounts"), 0o755)?;
    for name in ["passwd", "group", "shadow", "gshadow"] {
        let content = fs::read_to_string(root.join("etc").join(name))?;
        let locked = credentials::locked_template(&content, name)?;
        if ["passwd", "group"].contains(&name) {
            let map = |s: &str| -> BTreeMap<String, Vec<String>> {
                s.lines()
                    .map(|line| {
                        let f: Vec<_> = line.split(':').collect();
                        (
                            f[0].into(),
                            f[2..if name == "passwd" { 4 } else { 3 }]
                                .iter()
                                .map(|s| (*s).into())
                                .collect(),
                        )
                    })
                    .collect()
            };
            let old = map(&fs::read_to_string(previous.join(name))?);
            let new = map(&locked);
            ensure!(
                old.iter().all(|(name, ids)| new.get(name) == Some(ids)),
                "stable account IDs changed"
            );
            ensure!(
                new.values().map(|v| &v[0]).collect::<BTreeSet<_>>().len() == new.len(),
                "duplicate account IDs"
            );
        }
        let mode = if name.contains("shadow") {
            0o600
        } else {
            0o644
        };
        atomic(&generation.join(name), locked.as_bytes(), mode)?;
        atomic(
            &root.join("usr/lib/looom/accounts").join(name),
            locked.as_bytes(),
            mode,
        )?;
        if name.contains("shadow") {
            link(
                root,
                &format!("etc/{name}"),
                &format!("/run/looom/accounts/{name}"),
            )?;
        }
    }
    // One atomic pointer publishes the complete append-only allocation map.
    json(
        &manager.state.join("current-registry.json"),
        &bundle.release,
        0o600,
    )?;
    chroot(
        root,
        "gpgconf",
        &["--homedir", "/etc/pacman.d/gnupg", "--kill", "all"],
    )?;
    for name in ["passwd-", "group-", "shadow-", "gshadow-"] {
        remove_if_exists(&root.join("etc").join(name))?;
    }
    for entry in fs::read_dir(root.join("etc/ssh"))? {
        let path = entry?.path();
        if path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("ssh_host_"))
        {
            remove_if_exists(&path)?;
        }
    }
    let private = root.join("etc/pacman.d/gnupg/private-keys-v1.d");
    if private.exists() {
        fs::remove_dir_all(private)?;
    }
    for entry in fs::read_dir(root.join("etc"))? {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) == Some("pacnew") {
            remove_if_exists(&path)?;
        }
    }
    Ok(())
}
fn kernel_version(root: &Path) -> Result<String> {
    let entries =
        fs::read_dir(root.join("usr/lib/modules"))?.collect::<std::io::Result<Vec<_>>>()?;
    ensure!(
        entries.len() == 1 && entries[0].file_type()?.is_dir(),
        "exactly one kernel required"
    );
    entries[0]
        .file_name()
        .into_string()
        .map_err(|_| anyhow::anyhow!("kernel name"))
}
fn finalize_mounted(manager: &Manager, bundle: &Bundle, root: &Path) -> Result<()> {
    let rid = &bundle.release;
    ensure!(
        packages::inventory(Some(root))? == bundle.lock.inventory(),
        "final inventory differs"
    );
    ensure!(
        fs::read_to_string(root.join("etc/looom/release-id"))?.trim() == rid,
        "build ID differs"
    );
    for name in ["shadow", "gshadow"] {
        ensure!(
            fs::read_to_string(root.join("usr/lib/looom/accounts").join(name))?
                .lines()
                .all(|l| l.split(':').nth(1) == Some("!")),
            "unlocked template"
        );
    }
    ensure!(
        fs::read_link(root.join("etc/shadow"))? == Path::new("/run/looom/accounts/shadow"),
        "wrong shadow link"
    );
    let evidence: Value = serde_json::from_slice(&fs::read(
        manager
            .state
            .join("release-evidence")
            .join(rid)
            .join("build.json"),
    )?)?;
    let uki = root.join("boot").join(format!("looom-{rid}.efi"));
    let digest = hash_file(&uki)?;
    ensure!(
        evidence["uki_sha256"].as_str() == Some(digest.as_str()),
        "UKI differs from configured build"
    );
    ensure!(
        hash_file(&root.join("usr/bin/looom"))?
            == bundle.request["recipe_sha256"].as_str().unwrap_or(""),
        "installed manager differs from frozen recipe"
    );
    command("sync", &["-f", string(root)?])?;
    let destination = manager.top.join(format!("@root-{rid}"));
    if destination.exists() {
        ensure!(
            output(
                "btrfs",
                &["property", "get", "-ts", string(&destination)?, "ro"]
            )? == "ro=true"
                && hash_file(&destination.join("boot").join(format!("looom-{rid}.efi")))? == digest,
            "orphan root differs"
        );
    } else {
        command(
            "btrfs",
            &[
                "subvolume",
                "snapshot",
                "-r",
                string(root)?,
                string(&destination)?,
            ],
        )?;
    }
    command("sync", &["-f", string(&manager.top)?])?;
    failpoint("snapshot")?;
    let metadata = Metadata {
        schema_version: 1,
        id: rid.clone(),
        phase: "validated".into(),
        root_subvolume: format!("@root-{rid}"),
        kernel_package: bundle.config.kernel.package.clone(),
        kernel_version: kernel_version(root)?,
        root_uuid: manager.machine.root_uuid.clone(),
        esp_uuid: manager.machine.esp_uuid.clone(),
        uki_sha256: digest,
        declarative_value: rid.clone(),
        created_at: timestamp(),
        engine: Some("rust-0.2".into()),
    };
    manager.save(&metadata)?;
    manager.operation(rid, "validated")?;
    Ok(())
}
fn delete_private(manager: &Manager, rid: &str) -> Result<()> {
    let path = manager.top.join(format!("@build-{rid}"));
    if path.exists() {
        command(
            "btrfs",
            &[
                "subvolume",
                "delete",
                "--recursive",
                "--commit-after",
                string(&path)?,
            ],
        )?;
    }
    Ok(())
}
pub fn finalize(manager: &Manager, inputs: &Path) -> Result<()> {
    ensure!(
        inputs.parent() == Some(manager.state.join("inputs").as_path()),
        "frozen input directory required"
    );
    let bundle: Bundle = serde_json::from_slice(&fs::read(inputs.join("bundle.json"))?)?;
    ensure!(
        inputs.file_name().and_then(|s| s.to_str()) == Some(bundle.release.as_str()),
        "frozen release ID differs"
    );
    let operation: Value = serde_json::from_slice(&fs::read(
        manager
            .state
            .join("operations")
            .join(format!("{}.json", bundle.release)),
    )?)?;
    ensure!(
        operation["phase"] == "configured" || operation["phase"] == "validated",
        "private build is not configured"
    );
    manager.machine.config_contract(&bundle.config)?;
    let mount = BuildMount::mount(&manager.machine, &bundle.release)?;
    finalize_mounted(manager, &bundle, &mount.path)?;
    drop(mount);
    delete_private(manager, &bundle.release)?;
    println!("Recovered configured Rust build {}", bundle.release);
    Ok(())
}
pub fn recover_builds(manager: &Manager) -> Result<()> {
    for entry in fs::read_dir(manager.state.join("operations"))? {
        let path = entry?.path();
        if path.extension().and_then(|v| v.to_str()) != Some("json") {
            continue;
        }
        let record: Value = serde_json::from_slice(&fs::read(&path)?)?;
        let rid = record["id"].as_str().context("operation ID")?;
        ensure!(crate::config::identifier(rid), "invalid operation ID");
        if record["phase"] != "configured" || record["engine"] != "rust-0.2" {
            continue;
        }
        if let Ok(metadata) = manager.load(rid) {
            let _lock = manager.lock()?;
            manager.operation(rid, &metadata.phase)?;
            delete_private(manager, rid)?;
            continue;
        }
        let inputs = manager.state.join("inputs").join(rid);
        let bundle: Bundle = serde_json::from_slice(&fs::read(inputs.join("bundle.json"))?)?;
        let binary = inputs.join("looom");
        ensure!(
            hash_file(&binary)? == bundle.request["recipe_sha256"].as_str().unwrap_or(""),
            "frozen recovery binary differs"
        );
        command(string(&binary)?, &["internal-finalize", string(&inputs)?])?;
    }
    Ok(())
}
fn install_sources(root: &Path) -> Result<()> {
    let source = root.join("usr/lib/looom/source");
    for (name, data) in [
        ("Cargo.toml", include_str!("../Cargo.toml")),
        ("Cargo.lock", include_str!("../Cargo.lock")),
        ("src/lib.rs", include_str!("lib.rs")),
        ("src/main.rs", include_str!("main.rs")),
        ("src/config.rs", include_str!("config.rs")),
        ("src/util.rs", include_str!("util.rs")),
        ("src/machine.rs", include_str!("machine.rs")),
        ("src/credentials.rs", include_str!("credentials.rs")),
        ("src/packages.rs", include_str!("packages.rs")),
        ("src/releases.rs", include_str!("releases.rs")),
        ("src/builder.rs", include_str!("builder.rs")),
        ("src/bootstrap.rs", include_str!("bootstrap.rs")),
        (
            "configs/bootstrap/packages.txt",
            include_str!("../configs/bootstrap/packages.txt"),
        ),
    ] {
        write(&source, name, data, 0o644)?;
    }
    Ok(())
}
pub fn plan(cfg: &Config, files: &BTreeMap<String, Vec<u8>>, lock: &PackageLock) -> Result<Value> {
    let current = packages::inventory(None)?;
    let target = lock.inventory();
    let add: BTreeMap<_, _> = target
        .iter()
        .filter(|(k, _)| !current.contains_key(*k))
        .collect();
    let remove: BTreeMap<_, _> = current
        .iter()
        .filter(|(k, _)| !target.contains_key(*k))
        .collect();
    let upgrade: BTreeMap<_, _> = target
        .iter()
        .filter_map(|(k, v)| {
            current
                .get(k)
                .filter(|old| *old != v)
                .map(|old| (k, json!({"old":old,"new":v})))
        })
        .collect();
    let previous: Value = fs::read("/usr/lib/looom/declaration.json")
        .ok()
        .map(|v| serde_json::from_slice(&v))
        .transpose()?
        .unwrap_or_else(|| json!({"config":{"files":{},"units":{}},"files":{}}));
    let old_files: BTreeMap<String, Vec<u8>> = serde_json::from_value(previous["files"].clone())?;
    let file_add: Vec<_> = files
        .keys()
        .filter(|k| !old_files.contains_key(*k))
        .collect();
    let file_remove: Vec<_> = old_files
        .keys()
        .filter(|k| !files.contains_key(*k))
        .collect();
    let changed: Vec<_> = files
        .iter()
        .filter(|(k, v)| {
            old_files.get(*k).is_some_and(|old| old != *v)
                || old_files.contains_key(*k)
                    && previous["config"]["files"][*k]
                        != serde_json::to_value(&cfg.files[*k]).unwrap_or(Value::Null)
        })
        .map(|(k, _)| k)
        .collect();
    let new_units = serde_json::to_value(&cfg.units)?;
    let old_units = previous["config"]["units"]
        .as_object()
        .context("unit map")?;
    let mut unit_changes = BTreeMap::new();
    for name in old_units
        .keys()
        .chain(cfg.units.keys())
        .collect::<BTreeSet<_>>()
    {
        if previous["config"]["units"][name] != new_units[name] {
            unit_changes.insert(
                name,
                json!({"old":previous["config"]["units"][name],"new":new_units[name]}),
            );
        }
    }
    Ok(
        json!({"engine":"rust-0.2","packages":{"add":add,"remove":remove,"upgrade":upgrade},"file_changes":{"add":file_add,"remove":file_remove,"change":changed},"unit_changes":unit_changes,"target_packages":target.len(),"explicit_requests":packages::requested(cfg),"system":cfg.system,"kernel":cfg.kernel,"desktop":cfg.desktop,"health":cfg.health,"persistent":cfg.persistent}),
    )
}
