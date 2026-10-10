use super::*;
use crate::{
    machine::Machine,
    packages::{self, PackageLock},
};
use serde_json::json as value;
use std::{
    io::Read,
    process::{Command, Stdio},
};
use zeroize::Zeroizing;

const STEPS: &[&str] = &[
    "inputs",
    "partition",
    "esp-format",
    "root-format",
    "subvolumes",
    "packages",
    "configure",
    "bootstrap",
    "build",
    "publish",
    "boot",
    "ready",
];
const DESTRUCTIVE: &[&str] = &["partition", "esp-format", "root-format"];
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema: u32,
    plan_sha256: String,
    inputs_sha256: Option<String>,
    completed: usize,
    pending: Option<String>,
}
fn save(work: &Path, j: &Journal) -> Result<()> {
    json(&work.join("journal.json"), j, 0o600)
}
fn preflight(plan: &Plan, work: &Path) -> Result<()> {
    ensure!(
        std::env::consts::ARCH == "x86_64" && Path::new("/sys/firmware/efi").is_dir(),
        "x86_64 UEFI required"
    );
    for entry in fs::read_dir("/sys/firmware/efi/efivars")? {
        let path = entry?.path();
        if path
            .file_name()
            .is_some_and(|v| v.to_string_lossy().starts_with("SecureBoot-"))
        {
            ensure!(
                fs::read(path)?.get(4) == Some(&0),
                "Secure Boot must be disabled"
            );
        }
    }
    for program in [
        "lsblk",
        "swapon",
        "sgdisk",
        "mkfs.fat",
        "mkfs.btrfs",
        "btrfs",
        "blkid",
        "mount",
        "umount",
        "pacstrap",
        "arch-chroot",
        "pacman",
        "curl",
        "ssh-keygen",
        "udevadm",
        "efibootmgr",
        "unshare",
    ] {
        ensure!(
            std::env::var_os("PATH")
                .is_some_and(|p| std::env::split_paths(&p).any(|dir| dir.join(program).is_file())),
            "missing installer tool: {program}"
        );
    }
    // Identity contract is shared with normal releases, including the fixed local /etc allowlist.
    let m = Machine {
        schema: 1,
        root_uuid: plan.root_uuid.clone(),
        esp_uuid: plan.esp_uuid.clone(),
        bootstrap_uki_sha256: plan.recipe_sha256.clone(),
        home_subvolume: "@home".into(),
        var_subvolume: "@var".into(),
        state_subvolume: "@state".into(),
        user: plan.config.accounts.user.name.clone(),
        uid: plan.config.accounts.user.uid,
        gid: plan.config.accounts.user.gid,
        serial_console: false,
        guest_agent: false,
        passwordless_sudo: false,
    };
    m.validate()?;
    m.config_contract(&plan.config)?;
    ensure!(
        Path::new("/usr/share/zoneinfo")
            .join(&plan.config.system.timezone)
            .is_file(),
        "timezone unavailable in installation environment"
    );
    let locales = fs::read_to_string("/usr/share/i18n/SUPPORTED")?;
    ensure!(
        locales
            .lines()
            .any(|l| l.split_whitespace().collect::<Vec<_>>()
                == [plan.config.system.locale.as_str(), "UTF-8"]),
        "unsupported UTF-8 locale"
    );
    fn keymap(directory: &Path, name: &str) -> Result<bool> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() && keymap(&entry.path(), name)? {
                return Ok(true);
            }
            if entry.file_name() == format!("{name}.map.gz").as_str()
                || entry.file_name() == format!("{name}.map").as_str()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    ensure!(
        keymap(
            Path::new("/usr/share/kbd/keymaps"),
            &plan.config.system.console_keymap
        )?,
        "unsupported console keymap"
    );
    if let Ok(existing) = output("getent", &["passwd", &plan.config.accounts.user.name]) {
        ensure!(
            existing.split(':').nth(2) == Some(plan.config.accounts.user.uid.to_string().as_str())
                && existing.split(':').nth(3)
                    == Some(plan.config.accounts.user.gid.to_string().as_str()),
            "user name conflicts with an existing system account"
        );
    }
    // The workspace must survive erasing the selected disk.
    let number = output("findmnt", &["-nro", "MAJ:MIN", "--target", string(work)?])?;
    each_device(&device_tree(&plan.disk.path)?, &mut |d| {
        ensure!(
            text_field(d, "maj:min") != number,
            "workspace is on the selected disk"
        );
        Ok(())
    })?;
    // A foreign bind inside target/usr could otherwise redirect package writes
    // to another disk even though the target root itself has the correct UUID.
    let mounted: Value = serde_json::from_str(&output(
        "findmnt",
        &["--json", "--list", "--output", "TARGET,UUID"],
    )?)?;
    let target = work.join("target");
    let expected = [
        work.join("top"),
        target.clone(),
        target.join("home"),
        target.join("var"),
        target.join("var/lib/looom"),
        target.join("efi"),
    ];
    for entry in mounted["filesystems"].as_array().context("mount list")? {
        let path = PathBuf::from(text_field(entry, "target"));
        if path.starts_with(&target) || path.starts_with(work.join("top")) {
            ensure!(
                expected.contains(&path)
                    && text_field(entry, "uuid").eq_ignore_ascii_case(
                        if path == target.join("efi") {
                            &plan.esp_uuid
                        } else {
                            &plan.root_uuid
                        }
                    ),
                "foreign/nested mount inside installer workspace: {}",
                path.display()
            );
        }
    }
    Ok(())
}
fn passwords(work: &Path, stdin: bool) -> Result<()> {
    let names = ["root", "user"];
    if names
        .iter()
        .all(|name| work.join(format!("{name}.hash")).exists())
    {
        for name in names {
            credentials::validate_hash(
                credentials::read_private(&work.join(format!("{name}.hash")))?.trim(),
            )?;
        }
        return Ok(());
    }
    let values: [Zeroizing<String>; 2] = if stdin {
        let mut input = Zeroizing::new(String::new());
        io::stdin().take(16385).read_to_string(&mut input)?;
        ensure!(input.len() <= 16384, "password input too large");
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            root: String,
            user: String,
        }
        let mut value: Input = serde_json::from_str(&input).map_err(|_| {
            anyhow::anyhow!("invalid password input; root/user JSON strings required")
        })?;
        [
            Zeroizing::new(std::mem::take(&mut value.root)),
            Zeroizing::new(std::mem::take(&mut value.user)),
        ]
    } else {
        let mut values = [Zeroizing::new(String::new()), Zeroizing::new(String::new())];
        for (i, name) in names.iter().enumerate() {
            values[i] = Zeroizing::new(rpassword::prompt_password(format!("{name} password: "))?);
            let again = Zeroizing::new(rpassword::prompt_password("Repeat password: ")?);
            ensure!(*values[i] == *again, "passwords differ");
        }
        values
    };
    // Hash both before publishing either; a partial pair is replaceable before provisioning.
    let hashes = [
        credentials::hash_password(&values[0])?,
        credentials::hash_password(&values[1])?,
    ];
    for (name, hash) in names.iter().zip(&hashes) {
        credentials::private_atomic(&work.join(format!("{name}.hash")), hash.as_bytes())?;
    }
    Ok(())
}
fn partition(plan: &Plan, n: u32) -> Result<PathBuf> {
    let table = output("sgdisk", &["--print", &plan.disk.path])?;
    ensure!(
        table.lines().any(|line| line
            .strip_prefix("Disk identifier (GUID): ")
            .is_some_and(|id| id.eq_ignore_ascii_case(&plan.disk_guid))),
        "GPT disk identity differs"
    );
    let tree = device_tree(&plan.disk.path)?;
    let children = tree["children"]
        .as_array()
        .context("partition table missing")?;
    let guid = if n == 1 {
        &plan.esp_partuuid
    } else {
        &plan.root_partuuid
    };
    let candidates: Vec<_> = children
        .iter()
        .filter(|d| text_field(d, "partuuid").eq_ignore_ascii_case(guid))
        .collect();
    ensure!(
        children.len() == 2 && candidates.len() == 1,
        "partition layout/identity differs from plan"
    );
    let node = candidates[0];
    let path = PathBuf::from(text_field(node, "path"));
    ensure!(
        fs::read_to_string(
            Path::new("/sys/class/block")
                .join(path.file_name().context("partition name")?)
                .join("partition")
        )?
        .trim()
            == n.to_string(),
        "partition number differs"
    );
    ensure!(
        node["type"] == "part"
            && text_field(node, "parttype").eq_ignore_ascii_case(if n == 1 {
                "c12a7328-f81f-11d2-ba4b-00a0c93ec93b"
            } else {
                "0fc63daf-8483-4772-8e79-3d69d8477de4"
            }),
        "partition type differs"
    );
    let size = node["size"].as_u64().context("partition size")?;
    ensure!(
        if n == 1 {
            size == 2 * GIB
        } else {
            size >= 29 * GIB
        },
        "partition size differs"
    );
    Ok(path)
}
fn filesystems(plan: &Plan) -> Result<()> {
    for (n, typ, uuid) in [(1, "vfat", &plan.esp_uuid), (2, "btrfs", &plan.root_uuid)] {
        let path = partition(plan, n)?;
        ensure!(
            output(
                "blkid",
                &["-p", "-s", "TYPE", "-o", "value", string(&path)?]
            )? == typ
                && output(
                    "blkid",
                    &["-p", "-s", "UUID", "-o", "value", string(&path)?]
                )?
                .eq_ignore_ascii_case(uuid),
            "filesystem differs from recorded installation"
        );
    }
    Ok(())
}
struct Mounts {
    paths: Vec<PathBuf>,
}
impl Drop for Mounts {
    fn drop(&mut self) {
        for path in self.paths.iter().rev() {
            if !succeeds("umount", &[string(path).unwrap_or("")]) {
                eprintln!(
                    "Retained mount {}; unmount it before moving the disk",
                    path.display()
                );
            }
        }
    }
}
fn mount_one(
    mounts: &mut Mounts,
    source: &Path,
    target: &Path,
    options: &str,
    uuid: &str,
    fsroot: &str,
    fstype: &str,
) -> Result<()> {
    if !target.exists() {
        mkdir(target, 0o700)?;
    }
    if !succeeds("mountpoint", &["-q", string(target)?]) {
        command("mount", &["-o", options, string(source)?, string(target)?])?;
    }
    ensure!(
        output(
            "findmnt",
            &["-nro", "UUID", "--mountpoint", string(target)?]
        )?
        .eq_ignore_ascii_case(uuid)
            && output(
                "findmnt",
                &["-nro", "FSROOT", "--mountpoint", string(target)?]
            )? == fsroot
            && output(
                "findmnt",
                &["-nro", "FSTYPE", "--mountpoint", string(target)?]
            )? == fstype,
        "unexpected installation mount {}",
        target.display()
    );
    mounts.paths.push(target.into());
    Ok(())
}
fn mounts(plan: &Plan, work: &Path, subvolumes: bool) -> Result<Mounts> {
    filesystems(plan)?;
    let mut mounts = Mounts { paths: vec![] };
    let btrfs = partition(plan, 2)?;
    let esp = partition(plan, 1)?;
    let top = work.join("top");
    mount_one(
        &mut mounts,
        &btrfs,
        &top,
        "subvolid=5,rw,noatime,compress=zstd:3",
        &plan.root_uuid,
        "/",
        "btrfs",
    )?;
    if subvolumes {
        for name in ["@bootstrap", "@home", "@var", "@state"] {
            let path = top.join(name);
            if !path.exists() {
                command("btrfs", &["subvolume", "create", string(&path)?])?;
            }
            ensure!(
                succeeds("btrfs", &["subvolume", "show", string(&path)?])
                    && output("btrfs", &["property", "get", "-ts", string(&path)?, "ro"])?
                        == "ro=false",
                "invalid installation subvolume"
            );
        }
        command("sync", &["-f", string(&top)?])?;
    }
    let root = work.join("target");
    for (name, dest) in [
        ("@bootstrap", root.clone()),
        ("@home", root.join("home")),
        ("@var", root.join("var")),
        ("@state", root.join("var/lib/looom")),
    ] {
        mount_one(
            &mut mounts,
            &btrfs,
            &dest,
            &format!("subvol={name},rw,noatime,compress=zstd:3"),
            &plan.root_uuid,
            &format!("/{name}"),
            "btrfs",
        )?;
    }
    mount_one(
        &mut mounts,
        &esp,
        &root.join("efi"),
        "rw,umask=0077",
        &plan.esp_uuid,
        "/",
        "vfat",
    )?;
    mkdir(&root.join("var/lib/looom"), 0o700)?;
    Ok(mounts)
}
fn copy_owned(source: &Path, target: &Path, mode: u32) -> Result<()> {
    ensure!(
        fs::symlink_metadata(source)?.is_file(),
        "source must be a regular file"
    );
    if target.exists() {
        ensure!(
            fs::symlink_metadata(target)?.is_file(),
            "unsafe copy destination"
        );
    }
    let parent = target.parent().context("copy parent")?;
    if !parent.exists() {
        mkdir(parent, 0o700)?;
    }
    ensure!(fs::symlink_metadata(parent)?.is_dir(), "unsafe copy parent");
    // Atomic rename also handles a same-filesystem cache hardlink without mutating its inode.
    atomic(target, &fs::read(source)?, mode)
}
fn inputs(plan: &Plan, work: &Path) -> Result<PackageLock> {
    let path = work.join("base.lock");
    let lock: PackageLock = serde_json::from_slice(&fs::read(path)?)?;
    ensure!(
        lock.request == packages::request(&plan.config)?,
        "package inputs differ from plan/binary"
    );
    lock.validate(&plan.config, false)?;
    for p in &lock.packages {
        for (file, digest) in [(&p.archive, &p.sha256), (&p.signature, &p.signature_sha256)] {
            let source = work.join("cache").join(file);
            ensure!(
                fs::symlink_metadata(&source)?.is_file() && hash_file(&source)? == *digest,
                "installer package input changed: {}",
                p.name
            );
        }
    }
    for (repo, record) in &lock.repositories {
        ensure!(
            hash_file(
                &work
                    .join("input-state/repository-cache")
                    .join(&lock.archive_date)
                    .join(format!("{repo}.db"))
            )? == record.database_sha256,
            "installer repository changed"
        );
    }
    Ok(lock)
}
fn provision(plan: &Plan, work: &Path) -> Result<()> {
    let lock = inputs(plan, work)?;
    let root = work.join("target");
    let conf = work.join("pacman.conf");
    atomic(
        &conf,
        packages::config(&plan.config.source.snapshot, false).as_bytes(),
        0o600,
    )?;
    let paths: Vec<_> = lock
        .packages
        .iter()
        .map(|p| work.join("cache").join(&p.archive))
        .collect();
    let mut args = vec!["-U", "-c", "-K", "-M", "-C", string(&conf)?, string(&root)?];
    for p in &paths {
        args.push(string(p)?);
    }
    command("pacstrap", &args)?;
    ensure!(
        packages::inventory(Some(&root))? == lock.inventory(),
        "bootstrap package inventory differs from frozen inputs"
    );
    let cache = root.join("var/cache/pacman/pkg");
    mkdir(&cache, 0o700)?;
    for package in &lock.packages {
        for file in [&package.archive, &package.signature] {
            let destination = cache.join(file);
            if !destination.exists() {
                copy_owned(&work.join("cache").join(file), &destination, 0o600)?;
            }
            ensure!(
                hash_file(&destination)? == hash_file(&work.join("cache").join(file))?,
                "target package cache differs"
            );
        }
    }
    let state = root.join("var/lib/looom");
    for (repo, record) in &lock.repositories {
        let destination = state
            .join("repository-cache")
            .join(&lock.archive_date)
            .join(format!("{repo}.db"));
        copy_owned(
            &work
                .join("input-state/repository-cache")
                .join(&lock.archive_date)
                .join(format!("{repo}.db")),
            &destination,
            0o600,
        )?;
        ensure!(
            hash_file(&destination)? == record.database_sha256,
            "copied database differs"
        );
    }
    for name in ["base.yaml", "base.lock", "plan.json"] {
        copy_owned(
            &work.join(name),
            &state.join("installation").join(name),
            0o600,
        )?;
    }
    // Save the plan-specific binary, never depend on a Rust compiler on the target.
    let binary = root.join("usr/bin/looom");
    copy_owned(&std::env::current_exe()?, &binary, 0o755)?;
    ensure!(
        hash_file(&binary)? == plan.recipe_sha256,
        "installed manager differs"
    );
    Ok(())
}
fn configure(plan: &Plan, work: &Path) -> Result<()> {
    let root = work.join("target");
    let cfg = &plan.config;
    let write = |path: &str, data: String| -> Result<()> {
        atomic(&root.join(path), data.as_bytes(), 0o644)
    };
    write("etc/hostname", format!("{}\n", cfg.system.hostname))?;
    write(
        "etc/hosts",
        format!(
            "127.0.0.1 localhost\n::1 localhost\n127.0.1.1 {}.localdomain {}\n",
            cfg.system.hostname, cfg.system.hostname
        ),
    )?;
    write("etc/locale.gen", format!("{} UTF-8\n", cfg.system.locale))?;
    write("etc/locale.conf", format!("LANG={}\n", cfg.system.locale))?;
    write(
        "etc/vconsole.conf",
        format!("KEYMAP={}\n", cfg.system.console_keymap),
    )?;
    let zone = root.join("usr/share/zoneinfo").join(&cfg.system.timezone);
    ensure!(
        zone.is_file(),
        "declared timezone missing from installed system"
    );
    let localtime = root.join("etc/localtime");
    remove_if_exists(&localtime)?;
    std::os::unix::fs::symlink(
        format!("/usr/share/zoneinfo/{}", cfg.system.timezone),
        localtime,
    )?;
    chroot(&root, "locale-gen", &[])?;
    chroot(&root, "systemd-machine-id-setup", &[])?;
    write(
        "etc/pacman.conf",
        packages::config(&cfg.source.snapshot, false),
    )?;
    let m = Machine {
        schema: 1,
        root_uuid: plan.root_uuid.clone(),
        esp_uuid: plan.esp_uuid.clone(),
        bootstrap_uki_sha256: plan.recipe_sha256.clone(),
        home_subvolume: "@home".into(),
        var_subvolume: "@var".into(),
        state_subvolume: "@state".into(),
        user: cfg.accounts.user.name.clone(),
        uid: cfg.accounts.user.uid,
        gid: cfg.accounts.user.gid,
        serial_console: false,
        guest_agent: false,
        passwordless_sudo: false,
    };
    write("etc/fstab", m.fstab("@bootstrap", false)?)?;
    let user = &cfg.accounts.user;
    if !succeeds(
        "arch-chroot",
        &[string(&root)?, "getent", "group", &user.name],
    ) {
        chroot(
            &root,
            "groupadd",
            &["-g", &user.gid.to_string(), &user.name],
        )?;
    }
    if !succeeds("arch-chroot", &[string(&root)?, "id", "-u", &user.name]) {
        chroot(
            &root,
            "useradd",
            &[
                "-m",
                "-u",
                &user.uid.to_string(),
                "-g",
                &user.gid.to_string(),
                "-G",
                "wheel",
                "-s",
                "/bin/bash",
                &user.name,
            ],
        )?;
    }
    ensure!(
        chroot_output(&root, "id", &["-u", &user.name])? == user.uid.to_string()
            && chroot_output(&root, "id", &["-g", &user.name])? == user.gid.to_string(),
        "existing account identity differs"
    );
    let mut data = Zeroizing::new(String::new());
    for (name, file) in [("root", "root"), (user.name.as_str(), "user")] {
        let hash = credentials::read_private(&work.join(format!("{file}.hash")))?;
        credentials::validate_hash(hash.trim())?;
        use std::fmt::Write;
        writeln!(data, "{name}:{}", hash.trim())?;
    }
    let mut child = Command::new("arch-chroot")
        .args([string(&root)?, "chpasswd", "-e"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    child
        .stdin
        .take()
        .context("password pipe")?
        .write_all(data.as_bytes())?;
    ensure!(
        child.wait()?.success(),
        "setting account credentials failed"
    );
    mkdir(&root.join("etc/sudoers.d"), 0o755)?;
    atomic(
        &root.join("etc/sudoers.d/10-looom-installer"),
        b"%wheel ALL=(ALL:ALL) ALL\n",
        0o440,
    )?;
    chroot(&root, "visudo", &["-cf", "/etc/sudoers"])?;
    for path in [
        "etc/looom-local",
        "etc/NetworkManager/system-connections",
        "root/.ssh",
    ] {
        mkdir(&root.join(path), 0o700)?;
    }
    if let Some(key) = &plan.ssh_public_key {
        atomic(
            &root.join("root/.ssh/authorized_keys"),
            format!("{key}\n").as_bytes(),
            0o600,
        )?;
        let home = root.join("home").join(&user.name).join(".ssh");
        mkdir(&home, 0o700)?;
        atomic(
            &home.join("authorized_keys"),
            format!("{key}\n").as_bytes(),
            0o600,
        )?;
        chroot(
            &root,
            "chown",
            &[
                "-R",
                &format!("{}:{}", user.uid, user.gid),
                &format!("/home/{}/.ssh", user.name),
            ],
        )?;
    }
    chroot(&root, "ssh-keygen", &["-A"])?;
    chroot(&root, "sshd", &["-t"])?;
    for unit in [
        "NetworkManager.service",
        "sshd.service",
        "systemd-timesyncd.service",
        "sddm.service",
    ] {
        if unit != "sddm.service" || cfg.desktop.environment == "plasma" {
            chroot(&root, "systemctl", &["enable", unit])?;
        }
    }
    chroot(
        &root,
        "systemctl",
        &[
            "set-default",
            if cfg.desktop.environment == "plasma" {
                "graphical.target"
            } else {
                "multi-user.target"
            },
        ],
    )?;
    command("sync", &["-f", string(&root)?])
}
fn target_command(work: &Path, args: &[&str]) -> Result<()> {
    let root = work.join("target");
    let mut command_args = vec![
        "--mount",
        "--propagation",
        "private",
        "arch-chroot",
        string(&root)?,
        "/usr/bin/looom",
        "internal-install-run",
    ];
    command_args.extend(args);
    command("unshare", &command_args)
}
fn build(plan: &Plan, work: &Path) -> Result<()> {
    ensure!(plan.release == "initial", "unexpected first release");
    target_command(work, &["internal-install-build"])
}
fn boot(plan: &Plan, work: &Path) -> Result<()> {
    let root = work.join("target");
    let esp = root.join("efi");
    let image = esp.join("EFI/looom/grubx64.efi");
    ensure!(
        hash_file(&image)? == hash_file(&root.join("var/lib/looom/bootstrap-install/grubx64.efi"))?,
        "installer GRUB integrity mismatch"
    );
    let fallback = esp.join("EFI/BOOT/BOOTX64.EFI");
    // Only the freshly formatted, UUID-checked target ESP is written.
    copy_owned(&image, &fallback, 0o600)?;
    command("sync", &["-f", string(&esp)?])?;
    if plan.register_firmware {
        let label = format!("looom-{}", &plan.root_uuid[..8]);
        let entries = output("efibootmgr", &["-v"])?;
        if let Some(line) = entries
            .lines()
            .find(|line| line.split_whitespace().nth(1) == Some(label.as_str()))
        {
            ensure!(
                line.to_ascii_lowercase()
                    .contains(&plan.esp_partuuid.to_ascii_lowercase())
                    && line
                        .to_ascii_lowercase()
                        .contains("\\efi\\looom\\grubx64.efi"),
                "firmware entry with our label points elsewhere"
            );
        } else {
            command(
                "efibootmgr",
                &[
                    "--create",
                    "--disk",
                    &plan.disk.path,
                    "--part",
                    "1",
                    "--label",
                    &label,
                    "--loader",
                    "\\EFI\\looom\\grubx64.efi",
                ],
            )?;
        }
    }
    Ok(())
}
pub(super) fn apply(work: &Path, plan: &Plan, resume: bool, stdin: bool) -> Result<()> {
    root()?;
    credentials::disable_dumps()?;
    validate(plan, work)?;
    let _lock = Lock::acquire(&work.join("installer.lock"))?;
    let locks = Path::new("/run/looom-installer-locks");
    private_directory(locks)?;
    let _disk_lock =
        Lock::acquire(&locks.join(format!("{}.lock", hash(&serde_json::to_vec(&plan.disk)?))))?;
    preflight(plan, work)?;
    let digest = hash(&serde_json::to_vec(plan)?);
    let path = work.join("journal.json");
    let mut journal: Journal = if path.exists() {
        ensure!(resume, "installation already started; use resume");
        serde_json::from_str(&credentials::read_private(&path)?)?
    } else {
        ensure!(
            !resume,
            "no installation journal; run apply with confirmation"
        );
        unused(&device_tree(&plan.disk.path)?, None)?;
        ensure!(
            hash(&serde_json::to_vec(&device_tree(&plan.disk.path)?)?)
                == plan.original_layout_sha256,
            "disk layout changed since plan"
        );
        Journal {
            schema: 1,
            plan_sha256: digest.clone(),
            inputs_sha256: None,
            completed: 0,
            pending: None,
        }
    };
    ensure!(
        journal.schema == 1 && journal.plan_sha256 == digest && journal.completed <= STEPS.len(),
        "journal identity/phase differs"
    );
    ensure!(
        journal
            .pending
            .as_deref()
            .is_none_or(|p| STEPS.get(journal.completed).copied() == Some(p)),
        "invalid pending step"
    );
    if let Some(pending) = &journal.pending {
        ensure!(
            !DESTRUCTIVE.contains(&pending.as_str()),
            "interrupted destructive step {pending}: resume refuses to partition/format again; retain journal for diagnosis"
        );
    }
    if journal.completed > 0 {
        ensure!(
            journal.inputs_sha256.as_deref() == Some(hash_file(&work.join("base.lock"))?.as_str()),
            "frozen installer lock changed"
        );
    }
    if journal.completed == STEPS.len() {
        println!(
            "Installation already completed. Boot the target disk, run looom verify, then looom confirm. No selection was changed."
        );
        return Ok(());
    }
    unused(
        &device_tree(&plan.disk.path)?,
        if journal.completed >= 4 {
            Some(work)
        } else {
            None
        },
    )?;
    if journal.completed < 4 {
        exclusive_disk(&plan.disk.path)?;
    }
    passwords(work, stdin)?;
    save(work, &journal)?;
    let mut mounted: Option<Mounts> = None;
    if journal.completed >= 5 {
        mounted = Some(mounts(plan, work, false)?);
    }
    while journal.completed < STEPS.len() {
        // Recheck hardware identity after long downloads as well as on resume.
        // A serial-less device replaced at the same /dev path has a new diskseq.
        validate(plan, work)?;
        let step = STEPS[journal.completed];
        println!("[{}/{}] {step}", journal.completed + 1, STEPS.len());
        journal.pending = Some(step.into());
        save(work, &journal)?;
        failpoint(&format!("install-before-{step}"))?;
        match step {
            "inputs" => {
                packages::resolve_at(
                    &plan.config,
                    &work.join("base.lock"),
                    &work.join("input-state"),
                    &work.join("cache"),
                )?;
                inputs(plan, work)?;
            }
            "partition" => {
                // Repeat safety checks after downloads and immediately before the first disk write.
                unused(&device_tree(&plan.disk.path)?, None)?;
                exclusive_disk(&plan.disk.path)?;
                ensure!(
                    hash(&serde_json::to_vec(&device_tree(&plan.disk.path)?)?)
                        == plan.original_layout_sha256,
                    "disk changed while preparing inputs"
                );
                inputs(plan, work)?;
                command("sgdisk", &["--zap-all", &plan.disk.path])?;
                command(
                    "sgdisk",
                    &[
                        "--clear",
                        &format!("--disk-guid={}", plan.disk_guid),
                        "--new=1:0:+2G",
                        "--typecode=1:ef00",
                        "--change-name=1:looom-efi",
                        &format!("--partition-guid=1:{}", plan.esp_partuuid),
                        "--new=2:0:0",
                        "--typecode=2:8300",
                        "--change-name=2:looom-root",
                        &format!("--partition-guid=2:{}", plan.root_partuuid),
                        &plan.disk.path,
                    ],
                )?;
                command("udevadm", &["settle"])?;
                partition(plan, 1)?;
                partition(plan, 2)?;
            }
            "esp-format" => {
                unused(&device_tree(&plan.disk.path)?, None)?;
                command(
                    "mkfs.fat",
                    &[
                        "-F",
                        "32",
                        "-i",
                        &plan.esp_uuid.replace('-', ""),
                        "-n",
                        "LOOOM_EFI",
                        string(&partition(plan, 1)?)?,
                    ],
                )?;
            }
            "root-format" => {
                unused(&device_tree(&plan.disk.path)?, None)?;
                command(
                    "mkfs.btrfs",
                    &[
                        "-f",
                        "-U",
                        &plan.root_uuid,
                        "-L",
                        "looom",
                        string(&partition(plan, 2)?)?,
                    ],
                )?;
                command("udevadm", &["settle"])?;
                filesystems(plan)?;
            }
            "subvolumes" => {
                mounted = Some(mounts(plan, work, true)?);
            }
            "packages" => provision(plan, work)?,
            "configure" => configure(plan, work)?,
            "bootstrap" => target_command(
                work,
                &["bootstrap", "/var/lib/looom/installation/base.yaml"],
            )?,
            "build" => build(plan, work)?,
            "publish" => target_command(work, &["publish", &plan.release])?,
            "boot" => boot(plan, work)?,
            "ready" => {
                target_command(work, &["try", &plan.release])?;
                let root = work.join("target");
                let state = root.join("var/lib/looom/installation");
                json(
                    &state.join("result.json"),
                    &value!({"schema":1,"plan_sha256":digest,"recipe_sha256":plan.recipe_sha256,"release":plan.release,"status":"awaiting-first-boot-confirmation"}),
                    0o600,
                )?;
                command("sync", &["-f", string(&root)?])?;
                command("sync", &["-f", string(&root.join("efi"))?])?;
            }
            _ => unreachable!(),
        }
        failpoint(&format!("install-action-{step}"))?;
        if step == "inputs" {
            journal.inputs_sha256 = Some(hash_file(&work.join("base.lock"))?);
        }
        journal.completed += 1;
        journal.pending = None;
        save(work, &journal)?;
        if journal.completed >= 6 {
            copy_owned(
                &path,
                &work.join("target/var/lib/looom/installation/journal.json"),
                0o600,
            )?;
        }
        failpoint(&format!("install-{step}"))?;
    }
    drop(mounted);
    println!(
        "Installation complete. First boot is a read-only trial; recovery remains the saved default.\nBoot the target disk, run: looom verify && looom confirm\nRetain workspace {} for the installation record; it contains private password hashes.",
        work.display()
    );
    Ok(())
}
