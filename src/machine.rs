use crate::{config::Config, credentials, util::*};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

pub const STATE: &str = "/var/lib/looom";
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Machine {
    pub schema: u32,
    pub root_uuid: String,
    pub esp_uuid: String,
    pub bootstrap_uki_sha256: String,
    pub home_subvolume: String,
    pub var_subvolume: String,
    pub state_subvolume: String,
    pub user: String,
    pub uid: u32,
    pub gid: u32,
    pub serial_console: bool,
    pub guest_agent: bool,
    pub passwordless_sudo: bool,
}
fn uuid(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-')
}
fn subvolume(s: &str) -> bool {
    s.starts_with('@')
        && s.len() <= 64
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"@_-".contains(&c))
}
impl Machine {
    pub fn fstab(&self, root_subvolume: &str, readonly: bool) -> Result<String> {
        ensure!(subvolume(root_subvolume), "invalid root subvolume");
        Ok(format!(
            "UUID={} / btrfs {},noatime,compress=zstd:3,subvol={} 0 0\nUUID={} /home btrfs rw,noatime,compress=zstd:3,subvol={} 0 0\nUUID={} /var btrfs rw,noatime,compress=zstd:3,subvol={} 0 0\nUUID={} /var/lib/looom btrfs rw,noatime,compress=zstd:3,subvol={} 0 0\nUUID={} /efi vfat rw,umask=0077 0 2\ntmpfs /tmp tmpfs rw,nosuid,nodev,mode=1777 0 0\n/var/lib/looom/local-etc/looom-local /etc/looom-local none bind,x-systemd.requires-mounts-for=/var/lib/looom 0 0\n/var/lib/looom/local-etc/NetworkManager/system-connections /etc/NetworkManager/system-connections none bind,x-systemd.requires-mounts-for=/var/lib/looom 0 0\n/var/lib/looom/root-ssh /root/.ssh none bind,x-systemd.requires-mounts-for=/var/lib/looom 0 0\n",
            self.root_uuid,
            if readonly { "ro" } else { "rw" },
            root_subvolume,
            self.root_uuid,
            self.home_subvolume,
            self.root_uuid,
            self.var_subvolume,
            self.root_uuid,
            self.state_subvolume,
            self.esp_uuid
        ))
    }
    pub fn load() -> Result<Self> {
        root()?;
        let path = Path::new(STATE).join("machine.json");
        let info = fs::symlink_metadata(&path)
            .context("machine not initialized; run looom init on prepared Btrfs system")?;
        ensure!(
            info.is_file() && info.uid() == 0 && info.mode() & 0o022 == 0,
            "untrusted machine profile"
        );
        let profile: Self = serde_json::from_slice(&fs::read(path)?)?;
        profile.validate()?;
        Ok(profile)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == 1 && uuid(&self.root_uuid) && uuid(&self.esp_uuid),
            "invalid machine UUID/schema"
        );
        ensure!(
            self.bootstrap_uki_sha256.len() == 64
                && self
                    .bootstrap_uki_sha256
                    .bytes()
                    .all(|c| c.is_ascii_hexdigit()),
            "invalid bootstrap digest"
        );
        ensure!(
            [
                &self.home_subvolume,
                &self.var_subvolume,
                &self.state_subvolume
            ]
            .iter()
            .all(|s| subvolume(s)),
            "invalid subvolume"
        );
        ensure!(
            self.home_subvolume != self.var_subvolume
                && self.home_subvolume != self.state_subvolume
                && self.var_subvolume != self.state_subvolume,
            "shared subvolumes conflict"
        );
        ensure!(
            crate::config::identifier(&self.user)
                && self.user != "root"
                && (1000..65534).contains(&self.uid)
                && (1000..65534).contains(&self.gid),
            "invalid managed user"
        );
        Ok(())
    }
    pub fn guard_mounts(&self) -> Result<()> {
        root()?;
        self.validate()?;
        ensure!(Path::new("/sys/firmware/efi").is_dir(), "UEFI required");
        ensure!(std::env::consts::ARCH == "x86_64", "x86_64 required");
        for item in fs::read_dir("/sys/firmware/efi/efivars")? {
            let path = item?.path();
            if path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("SecureBoot-"))
            {
                let value = fs::read(path)?;
                ensure!(
                    value.get(4).copied().unwrap_or(0) == 0,
                    "Secure Boot is not supported in this version"
                );
            }
        }
        ensure!(
            output("findmnt", &["-nro", "FSTYPE", "/"])? == "btrfs",
            "root must be Btrfs"
        );
        ensure!(
            output("findmnt", &["-nro", "UUID", "/"])? == self.root_uuid,
            "root UUID differs from registered machine"
        );
        ensure!(
            output("findmnt", &["-nro", "UUID", "/efi"])? == self.esp_uuid
                && output("findmnt", &["-nro", "FSTYPE", "/efi"])? == "vfat",
            "registered FAT ESP must be mounted at /efi"
        );
        for (path, subvol) in [
            ("/home", &self.home_subvolume),
            ("/var", &self.var_subvolume),
            (STATE, &self.state_subvolume),
        ] {
            ensure!(
                output("findmnt", &["-nro", "UUID", path])? == self.root_uuid
                    && output("findmnt", &["-nro", "FSROOT", path])? == format!("/{subvol}"),
                "incorrect persistent mount {path}"
            );
            ensure!(
                output("findmnt", &["-nro", "OPTIONS", path])?
                    .split(',')
                    .any(|v| v == "rw"),
                "persistent mount is not writable"
            );
        }
        let _dir = credentials::trusted_dir(Path::new(STATE), true)?;
        let _esp = credentials::trusted_dir(Path::new("/efi"), true)?;
        Ok(())
    }
    pub fn guard(&self) -> Result<()> {
        self.guard_mounts()?;
        ensure!(
            hash_file(Path::new("/efi/EFI/Linux/looom-bootstrap.efi"))?
                == self.bootstrap_uki_sha256,
            "bootstrap recovery UKI integrity mismatch"
        );
        Ok(())
    }
    pub fn ensure_top(&self) -> Result<PathBuf> {
        let top = PathBuf::from("/run/looom-top");
        mkdir(&top, 0o700)?;
        if !succeeds("mountpoint", &["-q", string(&top)?]) {
            command(
                "mount",
                &[
                    "-o",
                    "subvolid=5,rw,noatime,compress=zstd:3",
                    &format!("UUID={}", self.root_uuid),
                    string(&top)?,
                ],
            )?;
        }
        ensure!(
            output("findmnt", &["-nro", "UUID", string(&top)?])? == self.root_uuid
                && output("findmnt", &["-nro", "FSROOT", string(&top)?])? == "/",
            "incorrect top-level Btrfs mount"
        );
        Ok(top)
    }
    pub fn config_contract(&self, config: &Config) -> Result<()> {
        let user = &config.accounts.user;
        ensure!(
            user.name == self.user && user.uid == self.uid && user.gid == self.gid,
            "account migration is not supported; declaration must match registered identity"
        );
        for (path, id) in [
            ("/etc/looom-local", "local-settings"),
            (
                "/etc/NetworkManager/system-connections",
                "network-connections",
            ),
        ] {
            let d = config
                .persistent
                .directories
                .get(path)
                .context("required persistent directory missing")?;
            ensure!(
                d.id == id && d.mode == "0700" && d.owner == "root" && d.group == "root",
                "persistent contract requires root:root/0700"
            );
        }
        ensure!(
            config.persistent.directories.len() == 2,
            "persistent migrations are not supported yet"
        );
        Ok(())
    }
}

/// Register an already prepared machine, without repartitioning or touching its boot choice.
pub fn initialize(config: &Config) -> Result<()> {
    root()?;
    if Path::new(STATE).join("machine.json").exists() {
        let m = Machine::load()?;
        m.guard()?;
        m.config_contract(config)?;
        println!("Machine already initialized");
        return Ok(());
    }
    let get_sub = |path: &str| -> Result<String> {
        let value = output("findmnt", &["-nro", "FSROOT", path])?;
        Ok(value.strip_prefix('/').context("subvolume path")?.into())
    };
    let virt = output("systemd-detect-virt", &[]).unwrap_or_default();
    let profile = Machine {
        schema: 1,
        root_uuid: output("findmnt", &["-nro", "UUID", "/"])?,
        esp_uuid: output("findmnt", &["-nro", "UUID", "/efi"])?,
        bootstrap_uki_sha256: hash_file(Path::new("/efi/EFI/Linux/looom-bootstrap.efi"))?,
        home_subvolume: get_sub("/home")?,
        var_subvolume: get_sub("/var")?,
        state_subvolume: get_sub(STATE)?,
        user: config.accounts.user.name.clone(),
        uid: config.accounts.user.uid,
        gid: config.accounts.user.gid,
        serial_console: matches!(virt.as_str(), "qemu" | "kvm"),
        guest_agent: matches!(virt.as_str(), "qemu" | "kvm"),
        passwordless_sudo: false,
    };
    profile.guard()?;
    profile.config_contract(config)?;
    ensure!(
        output("id", &["-u", &profile.user])? == profile.uid.to_string()
            && output("id", &["-g", &profile.user])? == profile.gid.to_string(),
        "managed user must exist with declared IDs"
    );
    let memberships = output("id", &["-nG", &profile.user])?;
    ensure!(
        config
            .accounts
            .user
            .groups
            .iter()
            .all(|g| memberships.split_whitespace().any(|name| name == g)),
        "prepared user group membership differs"
    );
    let passwd = fs::read_to_string("/etc/passwd")?;
    ensure!(
        passwd
            .lines()
            .find(|l| l.split(':').next() == Some(profile.user.as_str()))
            .and_then(|l| l.split(':').nth(6))
            == Some(config.accounts.user.shell.as_str()),
        "prepared user shell differs"
    );
    let state = Path::new(STATE);
    // The shell prototype created its empty flock file with 0644. Migrate the
    // existing inode so an older process holding it keeps the same lock.
    let old_lock = state.join("release-control.lock");
    if old_lock.exists() {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&old_lock)?;
        let info = file.metadata()?;
        ensure!(
            info.is_file() && info.uid() == 0 && info.mode() & 0o022 == 0,
            "unsafe legacy lock"
        );
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    let _lock = Lock::acquire(&state.join("release-control.lock"))?;
    for name in [
        "releases",
        "operations",
        "inputs",
        "dev",
        "ssh",
        "account-registry",
        "local-etc/looom-local",
        "local-etc/NetworkManager/system-connections",
        "credentials",
        "root-ssh",
    ] {
        mkdir(&state.join(name), 0o700)?;
    }
    if !state.join("release-state-initialized").exists() {
        ensure!(
            output("findmnt", &["-nro", "OPTIONS", "/"])?
                .split(',')
                .any(|v| v == "rw"),
            "initial import requires writable prepared bootstrap"
        );
        credentials::import_shadow(config)?;
        failpoint("bootstrap-credentials")?;
        for name in ["passwd", "group", "shadow", "gshadow"] {
            let content = fs::read_to_string(Path::new("/etc").join(name))?;
            let data = credentials::locked_template(&content, name)?;
            atomic(
                &state.join("account-registry").join(name),
                data.as_bytes(),
                if name.contains("shadow") {
                    0o600
                } else {
                    0o644
                },
            )?;
        }
        command(
            "cp",
            &[
                "-a",
                "/etc/NetworkManager/system-connections/.",
                string(&state.join("local-etc/NetworkManager/system-connections"))?,
            ],
        )?;
        if Path::new("/etc/looom-local").is_dir() {
            command(
                "cp",
                &[
                    "-a",
                    "/etc/looom-local/.",
                    string(&state.join("local-etc/looom-local"))?,
                ],
            )?;
        }
        if Path::new("/root/.ssh").is_dir() {
            command(
                "cp",
                &["-a", "/root/.ssh/.", string(&state.join("root-ssh"))?],
            )?;
        }
        for entry in fs::read_dir("/etc/ssh")? {
            let p = entry?.path();
            if p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("ssh_host_"))
            {
                fs::copy(&p, state.join("ssh").join(p.file_name().unwrap()))?;
            }
        }
        fs::copy("/etc/machine-id", state.join("machine-id"))?;
        command("sync", &["-f", STATE])?;
        failpoint("bootstrap-import")?;
        atomic(
            &state.join("release-state-initialized"),
            b"Imported by native Rust initializer\n",
            0o600,
        )?;
    }
    ensure!(
        Path::new("/efi/EFI/Linux/looom-bootstrap.efi").is_file(),
        "prepared bootstrap recovery UKI is required; see installation guide"
    );
    for user in ["root", profile.user.as_str()] {
        credentials::validate_hash(
            credentials::read_private(&state.join("credentials").join(format!("{user}.hash")))?
                .trim(),
        )?;
    }
    json(&state.join("machine.json"), &profile, 0o600)?;
    println!("Registered Btrfs/UEFI machine; sudo requires a password by default");
    Ok(())
}
