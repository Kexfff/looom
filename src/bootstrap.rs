//! Connect an existing prepared Arch installation. Never repartitions a disk.
use crate::{
    config::Config,
    machine::{self, STATE},
    util::*,
};
use anyhow::{Context, Result, ensure};
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
pub fn prepare(config: &Config) -> Result<()> {
    root()?;
    ensure!(
        Path::new("/sys/firmware/efi").is_dir() && std::env::consts::ARCH == "x86_64",
        "x86_64 UEFI required"
    );
    ensure!(
        !Path::new(STATE).join("machine.json").exists(),
        "machine is already registered; use init/verify"
    );
    ensure!(
        !Path::new("/efi/looom").exists() && !Path::new("/efi/EFI/looom").exists(),
        "existing looom boot namespace; explicit recovery required"
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
    let _state = crate::credentials::trusted_dir(Path::new(STATE), true)?;
    let original_fstab = fs::read_to_string("/etc/fstab")?;
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
    let kernel = &kernels[0];
    let directory = Path::new("/efi/EFI/Linux");
    mkdir(directory, 0o700)?;
    let destination = directory.join("looom-bootstrap.efi");
    ensure!(
        !destination.exists(),
        "existing bootstrap UKI; explicit recovery required"
    );
    let work = tempfile::Builder::new()
        .prefix("looom-bootstrap-")
        .tempdir_in("/run")?;
    let conf = work.path().join("mkinitcpio.conf");
    let cmdline = work.path().join("cmdline");
    fs::write(&conf, INITRAMFS)?;
    fs::write(
        &cmdline,
        format!("root=UUID={uuid} rootflags=subvol={subvol} rw console=tty0\n"),
    )?;
    let pending = directory.join("looom-bootstrap.pending.efi");
    command(
        "mkinitcpio",
        &[
            "-k",
            kernel,
            "-c",
            string(&conf)?,
            "-U",
            string(&pending)?,
            "--cmdline",
            string(&cmdline)?,
        ],
    )?;
    fs::File::open(&pending)?.sync_all()?;
    fs::rename(&pending, &destination)?;
    sync_dir(directory)?;
    command(
        "grub-install",
        &[
            "--target=x86_64-efi",
            "--efi-directory=/efi",
            "--boot-directory=/efi/looom",
            "--bootloader-id=looom",
            "--no-nvram",
        ],
    )?;
    // Retain the vendor-generated loader, then install a self-contained
    // dispatcher with an emergency entry independent of the external menu.
    let loader = Path::new("/efi/EFI/looom/grubx64.efi");
    fs::rename(loader, loader.with_file_name("grubx64.vendor.efi"))?;
    recovery_loader(
        &output("findmnt", &["-nro", "UUID", "/efi"])?,
        matches!(
            output("systemd-detect-virt", &[])
                .unwrap_or_default()
                .as_str(),
            "qemu" | "kvm"
        ),
        loader,
    )?;
    let grub = Path::new("/efi/looom/grub");
    command("grub-editenv", &[string(&grub.join("grubenv"))?, "create"])?;
    command(
        "grub-editenv",
        &[
            string(&grub.join("grubenv"))?,
            "set",
            "saved_entry=looom-bootstrap",
        ],
    )?;
    machine::initialize(config)?;
    let profile = machine::Machine::load()?;
    profile.guard()?;
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
    println!(
        "Prepared native bootstrap; register its UEFI entry with looom boot-entry, then build/publish/try a release"
    );
    Ok(())
}
pub fn boot_entry() -> Result<()> {
    let machine = machine::Machine::load()?;
    machine.guard()?;
    let _lock = Lock::acquire(&Path::new(STATE).join("release-control.lock"))?;
    ensure!(
        Path::new("/efi/EFI/looom/grubx64.efi").is_file(),
        "native GRUB loader is missing"
    );
    register_entry(&machine, "looom", "\\EFI\\looom\\grubx64.efi", false)?;
    println!("Registered looom UEFI entry; existing fallback loader retained");
    Ok(())
}
fn register_entry(
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
    let before = output("efibootmgr", &[])?;
    // Creating a named entry leaves the vendor fallback loader intact.
    ensure!(
        !before
            .lines()
            .any(|l| l.starts_with("Boot") && l.split_whitespace().nth(1) == Some(label)),
        "{label} UEFI entry already exists; use firmware boot menu"
    );
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
