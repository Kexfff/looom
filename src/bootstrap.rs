//! Connect an existing prepared Arch installation. Never repartitions a disk.
use crate::{
    config::Config,
    machine::{self, STATE},
    util::*,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

const INITRAMFS: &str = "MODULES=(btrfs)\nBINARIES=()\nFILES=()\nHOOKS=(base systemd microcode modconf kms keyboard sd-vconsole block filesystems fsck)\nCOMPRESSION=\"zstd\"\n";
/// The emergency entry lives in the EFI image, independently of mutable FAT
/// menu files. Keep it first so GRUB's index-zero fallback remains bootable
/// when a truncated external menu selects an entry that no longer exists.
pub fn recovery_loader(esp_uuid: &str, serial: bool, destination: &Path) -> Result<()> {
    ensure!(
        !esp_uuid.is_empty() && esp_uuid.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-'),
        "invalid ESP UUID"
    );
    let work = tempfile::Builder::new()
        .prefix("looom-grub-rescue-")
        .tempdir_in("/run")?;
    let config = work.path().join("grub.cfg");
    let serial = if serial {
        "serial --unit=0 --speed=115200 --word=8 --parity=no --stop=1\nterminal_input console serial\nterminal_output console serial\n"
    } else {
        ""
    };
    atomic(&config, format!("set timeout=5\nset default=looom-emergency\ninsmod part_gpt\ninsmod fat\ninsmod chain\nsearch --no-floppy --fs-uuid --set=esp {esp_uuid}\n{serial}menuentry 'looom emergency (embedded recovery)' --id looom-emergency {{\n chainloader ($esp)/EFI/Linux/looom-bootstrap.efi\n}}\nif [ -f ($esp)/looom/grub/grub.cfg ]; then\n source ($esp)/looom/grub/grub.cfg\nfi\n").as_bytes(), 0o600)?;
    command("grub-script-check", &[string(&config)?])?;
    let image = work.path().join("grubx64.efi");
    command(
        "grub-mkstandalone",
        &[
            "--format=x86_64-efi",
            "--locales=",
            "--fonts=",
            "--themes=",
            "--modules=part_gpt fat chain search search_fs_uuid normal configfile serial",
            "--output",
            string(&image)?,
            &format!("boot/grub/grub.cfg={}", config.display()),
        ],
    )?;
    // A new loader is published once. Updating an installed loader must use a
    // different path and a separate UEFI entry, preserving the previous one.
    ensure!(
        !destination.exists(),
        "loader already exists; do not replace a working EFI loader"
    );
    mkdir(destination.parent().context("loader directory")?, 0o700)?;
    let mut pending = tempfile::Builder::new()
        .prefix(".looom-loader-")
        .tempfile_in(destination.parent().unwrap())?;
    std::io::copy(&mut fs::File::open(&image)?, &mut pending)?;
    pending.as_file().sync_all()?;
    ensure!(
        hash_file(pending.path())? == hash_file(&image)?,
        "EFI loader copy mismatch"
    );
    pending
        .persist_noclobber(destination)
        .map_err(|e| e.error)?;
    sync_dir(destination.parent().unwrap())
}

/// Add an immutable rescue dispatcher to an existing installation. It reads
/// the same saved/next environment but does not change firmware boot order.
pub fn boot_recovery() -> Result<()> {
    let machine = machine::Machine::load()?;
    machine.guard()?;
    let _lock = Lock::acquire(&Path::new(STATE).join("release-control.lock"))?;
    if machine.bootloader == machine::Bootloader::Limine {
        let image = Path::new("/efi/EFI/Linux/looom-bootstrap.efi");
        ensure!(
            hash_file(image)? == machine.bootstrap_uki_sha256,
            "recovery UKI integrity mismatch"
        );
        register_entry(
            &machine,
            &format!("looom-recovery-{}", &machine.root_uuid[..8]),
            "\\EFI\\Linux\\looom-bootstrap.efi",
            true,
        )?;
        println!(
            "Registered direct recovery UKI; Limine configuration is not required for this entry"
        );
        return Ok(());
    }
    let loader = Path::new("/efi/EFI/looom/safex64.efi");
    let receipt = Path::new(STATE).join("boot-recovery.json");
    if loader.exists() {
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&receipt).context(
            "unregistered recovery loader without receipt; original loader is unchanged",
        )?)?;
        ensure!(
            saved["esp_uuid"].as_str() == Some(machine.esp_uuid.as_str())
                && saved["loader_sha256"].as_str() == Some(hash_file(loader)?.as_str()),
            "recovery loader differs from receipt"
        );
    } else {
        recovery_loader(&machine.esp_uuid, machine.serial_console, loader)?;
        json(
            &receipt,
            &serde_json::json!({"esp_uuid":machine.esp_uuid,"loader_sha256":hash_file(loader)?}),
            0o600,
        )?;
    }
    register_entry(&machine, "looom-safe", "\\EFI\\looom\\safex64.efi", true)?;
    println!(
        "Registered looom-safe without changing BootOrder; test it with firmware BootNext before selecting it permanently"
    );
    Ok(())
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Installation {
    schema: u32,
    config_sha256: String,
    root_uuid: String,
    esp_uuid: String,
    root_subvolume: String,
    shared_fsroots: [String; 3],
    original_fstab: String,
    kernel: String,
    uki_sha256: Option<String>,
    loader_sha256: Option<String>,
    complete: bool,
}

/// Copy from a checkpoint on Btrfs, retaining that checkpoint across FAT writes.
/// Destination names are fixed by the caller, never taken from a journal.
pub(crate) fn copy_checkpoint(source: &Path, destination: &Path, expected: &str) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let checkpoint = fs::symlink_metadata(source)?;
    ensure!(
        checkpoint.is_file() && checkpoint.uid() == 0 && checkpoint.mode() & 0o022 == 0,
        "unsafe bootstrap checkpoint"
    );
    ensure!(
        hash_file(source)? == expected,
        "bootstrap checkpoint integrity mismatch"
    );
    crate::credentials::trusted_dir(destination.parent().context("destination parent")?, true)?;
    use std::os::unix::fs::OpenOptionsExt;
    let validate_existing = |path: &Path| -> Result<bool> {
        match fs::symlink_metadata(path) {
            Ok(info) => {
                ensure!(
                    info.is_file() && info.uid() == 0,
                    "unsafe bootstrap destination"
                );
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    };
    // Fixed, owned scratch name: a power cut cannot leak untracked random images.
    let pending_path = destination.with_extension("pending");
    if validate_existing(&pending_path)? {
        fs::remove_file(&pending_path)?;
        sync_dir(destination.parent().unwrap())?;
    }
    if validate_existing(destination)? && hash_file(destination)? == expected {
        return Ok(());
    }
    let mut pending = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&pending_path)?;
    std::io::copy(&mut fs::File::open(source)?, &mut pending)?;
    pending.sync_all()?;
    ensure!(
        hash_file(&pending_path)? == expected,
        "bootstrap copy mismatch"
    );
    fs::rename(&pending_path, destination)?;
    sync_dir(destination.parent().unwrap())
}

pub fn prepare(config: &Config) -> Result<()> {
    root()?;
    ensure!(
        Path::new("/sys/firmware/efi").is_dir() && std::env::consts::ARCH == "x86_64",
        "x86_64 UEFI required"
    );
    for item in fs::read_dir("/sys/firmware/efi/efivars")? {
        let path = item?.path();
        if path
            .file_name()
            .is_some_and(|s| s.to_string_lossy().starts_with("SecureBoot-"))
        {
            ensure!(
                fs::read(path)?.get(4).copied().unwrap_or(0) == 0,
                "Secure Boot is unsupported"
            );
        }
    }
    let uuid = output("findmnt", &["-nro", "UUID", "/"])?;
    let subvol = output("findmnt", &["-nro", "FSROOT", "/"])?;
    ensure!(
        output("findmnt", &["-nro", "FSTYPE", "/"])? == "btrfs"
            && output("findmnt", &["-nro", "OPTIONS", "/"])?
                .split(',')
                .any(|s| s == "rw"),
        "prepared root must be writable Btrfs"
    );
    let subvol = subvol.strip_prefix('/').context("root subvolume")?;
    ensure!(
        subvol.starts_with('@')
            && subvol
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"@_-".contains(&c)),
        "named top-level root subvolume required"
    );
    let source = output("findmnt", &["-nro", "SOURCE", "/"])?;
    let device = source.split('[').next().context("root device")?;
    ensure!(
        !output("lsblk", &["-ndo", "TYPE", device])?
            .lines()
            .any(|s| s == "crypt"),
        "encrypted root is unsupported"
    );
    ensure!(
        output("findmnt", &["-nro", "FSTYPE", "/efi"])? == "vfat",
        "FAT ESP must be mounted at /efi"
    );
    for path in ["/home", "/var", STATE] {
        ensure!(
            output("findmnt", &["-nro", "UUID", path])? == uuid
                && output("findmnt", &["-nro", "FSROOT", path])?.starts_with("/@")
                && output("findmnt", &["-nro", "OPTIONS", path])?
                    .split(',')
                    .any(|s| s == "rw"),
            "prepared shared subvolume required: {path}"
        );
    }
    let shared_fsroots = [
        output("findmnt", &["-nro", "FSROOT", "/home"])?,
        output("findmnt", &["-nro", "FSROOT", "/var"])?,
        output("findmnt", &["-nro", "FSROOT", STATE])?,
    ];
    let _state = crate::credentials::trusted_dir(Path::new(STATE), true)?;
    let _lock = Lock::acquire(&Path::new(STATE).join("bootstrap-install.lock"))?;
    let workdir = Path::new(STATE).join("bootstrap-install");
    let record = workdir.join("journal.json");
    let previous: Option<Installation> = if record.exists() {
        Some(serde_json::from_str(&crate::credentials::read_private(
            &record,
        )?)?)
    } else {
        None
    };
    let config_sha256 = hash(&serde_json::to_vec(config)?);
    let esp_uuid = output("findmnt", &["-nro", "UUID", "/efi"])?;
    if let Some(journal) = &previous {
        ensure!(
            journal.schema == 1
                && journal.config_sha256 == config_sha256
                && journal.root_uuid == uuid
                && journal.esp_uuid == esp_uuid
                && journal.root_subvolume == subvol
                && journal.shared_fsroots == shared_fsroots,
            "bootstrap resume identity/declaration differs"
        );
        if journal.complete {
            let profile = machine::Machine::load()?;
            profile.guard()?;
            profile.config_contract(config)?;
            ensure!(
                journal.uki_sha256.as_deref() == Some(profile.bootstrap_uki_sha256.as_str()),
                "initial bootstrap was superseded; use bootstrap-update"
            );
            println!("Bootstrap already completed; no boot choice or state changed");
            return Ok(());
        }
    } else {
        ensure!(
            !Path::new(STATE).join("machine.json").exists()
                && !Path::new("/efi/looom").exists()
                && !Path::new("/efi/EFI/looom").exists()
                && !Path::new("/efi/EFI/Linux/looom-bootstrap.efi").exists()
                && !workdir.exists(),
            "existing bootstrap namespace without matching journal; explicit diagnosis required"
        );
    }
    if !previous.as_ref().is_some_and(|j| j.complete) && Path::new(STATE).join("releases").exists()
    {
        ensure!(
            !fs::read_dir(Path::new(STATE).join("releases"))?
                .any(|e| e.is_ok_and(|e| e.path().extension().is_some_and(|s| s == "json"))),
            "initial bootstrap cannot reset an existing release registry"
        );
    }
    let original_fstab = previous
        .as_ref()
        .map(|j| j.original_fstab.clone())
        .unwrap_or(fs::read_to_string("/etc/fstab")?);
    for line in original_fstab
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
    {
        let target = line
            .split_whitespace()
            .nth(1)
            .context("invalid fstab line")?;
        ensure!(
            [
                "/",
                "/home",
                "/var",
                STATE,
                "/efi",
                "/tmp",
                "/etc/looom-local",
                "/etc/NetworkManager/system-connections",
                "/root/.ssh"
            ]
            .contains(&target),
            "additional fstab resources require a future handler: {target}"
        );
    }
    command("ssh-keygen", &["-A"])?;
    let mut kernels = Vec::new();
    for entry in fs::read_dir("/usr/lib/modules")? {
        let path = entry?.path();
        if fs::read_to_string(path.join("pkgbase")).is_ok_and(|s| s.trim() == config.kernel.package)
        {
            kernels.push(
                path.file_name()
                    .and_then(|s| s.to_str())
                    .context("kernel version")?
                    .to_owned(),
            );
        }
    }
    ensure!(
        kernels.len() == 1,
        "one installed declared kernel required for bootstrap"
    );
    mkdir(&workdir, 0o700)?;
    let mut journal = previous.unwrap_or(Installation {
        schema: 1,
        config_sha256,
        root_uuid: uuid.clone(),
        esp_uuid: esp_uuid.clone(),
        root_subvolume: subvol.into(),
        shared_fsroots,
        original_fstab: original_fstab.clone(),
        kernel: kernels[0].clone(),
        uki_sha256: None,
        loader_sha256: None,
        complete: false,
    });
    ensure!(
        journal.kernel == kernels[0],
        "prepared kernel changed during bootstrap"
    );
    ensure!(
        serde_json::to_vec(&journal)?.len() <= 16000,
        "bootstrap journal too large (fstab)"
    );
    json(&record, &journal, 0o600)?;
    failpoint("bootstrap-journal")?;
    let image = workdir.join("bootstrap.efi");
    if journal.uki_sha256.is_none() {
        let work = tempfile::Builder::new()
            .prefix("looom-bootstrap-")
            .tempdir_in("/run")?;
        let conf = work.path().join("mkinitcpio.conf");
        let cmdline = work.path().join("cmdline");
        fs::write(&conf, INITRAMFS)?;
        let serial = matches!(
            output("systemd-detect-virt", &[])
                .unwrap_or_default()
                .as_str(),
            "qemu" | "kvm"
        );
        fs::write(
            &cmdline,
            format!(
                "root=UUID={uuid} rootflags=subvol={subvol} rw console=tty0{}\n",
                if serial {
                    " console=ttyS0,115200n8"
                } else {
                    ""
                }
            ),
        )?;
        command(
            "mkinitcpio",
            &[
                "-k",
                &journal.kernel,
                "-c",
                string(&conf)?,
                "-U",
                string(&image)?,
                "--cmdline",
                string(&cmdline)?,
            ],
        )?;
        fs::File::open(&image)?.sync_all()?;
        sync_dir(&workdir)?;
        journal.uki_sha256 = Some(hash_file(&image)?);
        json(&record, &journal, 0o600)?;
    }
    let directory = Path::new("/efi/EFI/Linux");
    mkdir(directory, 0o700)?;
    copy_checkpoint(
        &image,
        &directory.join("looom-bootstrap.efi"),
        journal.uki_sha256.as_deref().unwrap(),
    )?;
    failpoint("bootstrap-uki")?;
    let loader_checkpoint = workdir.join("liminex64.efi");
    if journal.loader_sha256.is_none() {
        // A crash before recording the digest may leave our own unregistered checkpoint.
        remove_if_exists(&loader_checkpoint)?;
        crate::limine::copy_loader(&loader_checkpoint)?;
        journal.loader_sha256 = Some(hash_file(&loader_checkpoint)?);
        json(&record, &journal, 0o600)?;
    }
    mkdir(Path::new("/efi/EFI/looom"), 0o700)?;
    copy_checkpoint(
        &loader_checkpoint,
        Path::new("/efi/EFI/looom/liminex64.efi"),
        journal.loader_sha256.as_deref().unwrap(),
    )?;
    failpoint("bootstrap-loader")?;
    machine::initialize(config)?;
    let profile = machine::Machine::load()?;
    profile.guard()?;
    failpoint("bootstrap-profile")?;
    atomic(
        &Path::new(STATE).join("bootstrap-original-fstab"),
        original_fstab.as_bytes(),
        0o600,
    )?;
    mkdir(Path::new("/etc/looom-local"), 0o700)?;
    atomic(
        Path::new("/etc/fstab"),
        profile.fstab(subvol, false)?.as_bytes(),
        0o644,
    )?;
    // No root validation is needed for an empty bootstrap menu. Avoid leaving
    // a top-level mount behind when invoked inside a prepared-install chroot.
    let manager = crate::releases::Manager {
        machine: profile,
        state: Path::new(STATE).into(),
        top: Path::new("/run/looom-top").into(),
        esp: Path::new("/efi").into(),
    };
    ensure!(
        manager.list()?.is_empty(),
        "bootstrap requires empty release registry"
    );
    let _lock = manager.lock()?;
    manager.write_menu(&manager.menu(None)?)?;
    command("sync", &["-f", "/efi"])?;
    failpoint("bootstrap-menu")?;
    journal.complete = true;
    json(&record, &journal, 0o600)?;
    println!(
        "Prepared native bootstrap; register its UEFI entry with looom boot-entry, then build/publish/try a release"
    );
    Ok(())
}
pub fn boot_entry() -> Result<()> {
    let machine = machine::Machine::load()?;
    machine.guard()?;
    let _lock = Lock::acquire(&Path::new(STATE).join("release-control.lock"))?;
    let loader = if machine.bootloader == machine::Bootloader::Limine {
        "\\EFI\\looom\\liminex64.efi"
    } else {
        "\\EFI\\looom\\grubx64.efi"
    };
    ensure!(
        Path::new("/efi")
            .join(loader.trim_start_matches('\\').replace('\\', "/"))
            .is_file(),
        "native EFI loader missing"
    );
    register_entry(&machine, "looom", loader, false)?;
    println!("Registered looom UEFI entry; existing fallback loader retained");
    Ok(())
}
pub(crate) fn register_entry(
    machine: &machine::Machine,
    label: &str,
    loader: &str,
    create_only: bool,
) -> Result<()> {
    let partition = fs::canonicalize(format!("/dev/disk/by-uuid/{}", machine.esp_uuid))?;
    let name = partition
        .file_name()
        .and_then(|s| s.to_str())
        .context("ESP partition")?;
    let number = fs::read_to_string(Path::new("/sys/class/block").join(name).join("partition"))?;
    let number: u32 = number.trim().parse()?;
    ensure!(number > 0, "ESP partition number required");
    let parent = output("lsblk", &["-ndo", "PKNAME", string(&partition)?])?;
    ensure!(
        !parent.is_empty()
            && parent
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)),
        "plain ESP disk required"
    );
    let before = output("efibootmgr", &["-v"])?;
    if let Some(line) = before
        .lines()
        .find(|l| l.starts_with("Boot") && l.split_whitespace().nth(1) == Some(label))
    {
        let partuuid = output(
            "blkid",
            &["-s", "PARTUUID", "-o", "value", string(&partition)?],
        )?;
        ensure!(
            line.to_ascii_lowercase()
                .contains(&loader.to_ascii_lowercase())
                && line
                    .to_ascii_lowercase()
                    .contains(&partuuid.to_ascii_lowercase()),
            "UEFI label points to another device/loader"
        );
        return Ok(());
    }
    command(
        "efibootmgr",
        &[
            if create_only {
                "--create-only"
            } else {
                "--create"
            },
            "--disk",
            &format!("/dev/{parent}"),
            "--part",
            &number.to_string(),
            "--label",
            label,
            "--loader",
            loader,
        ],
    )?;
    Ok(())
}
