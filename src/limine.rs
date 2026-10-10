//! Limine UEFI backend. Persistent choice lives in the config; trials use BLI.
use crate::{
    machine::{Bootloader, Machine, STATE},
    releases::Manager,
    util::*,
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    os::unix::{fs::OpenOptionsExt, io::AsRawFd},
    path::{Path, PathBuf},
};

const GUID: &str = "4a67b082-0a4c-41cf-b6c7-440b29bb8c4f";
pub const LOADER: &str = "EFI/looom/liminex64.efi";
pub fn config_path(manager: &Manager) -> PathBuf {
    manager.esp.join("EFI/looom/limine.conf")
}
fn variable(name: &str) -> PathBuf {
    Path::new("/sys/firmware/efi/efivars").join(format!("{name}-{GUID}"))
}
fn read_variable(name: &str) -> Result<Option<String>> {
    let data = match fs::read(variable(name)) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    ensure!(
        data.len() >= 6 && data.len() <= 1028 && data.len() % 2 == 0,
        "invalid EFI selection variable"
    );
    let wide: Vec<u16> = data[4..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .take_while(|v| *v != 0)
        .collect();
    Ok(Some(String::from_utf16(&wide)?))
}
fn oneshot(entry: &str) -> Result<()> {
    root()?;
    let path = variable("LoaderEntryOneShot");
    // efivarfs marks unfamiliar variables immutable. Change only this named BLI variable.
    let existing = if path.try_exists()? {
        Some(
            fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&path)?,
        )
    } else {
        None
    };
    let mut flags: libc::c_long = 0;
    if let Some(file) = &existing {
        ensure!(
            unsafe { libc::ioctl(file.as_raw_fd(), 0x80086601 as libc::c_ulong, &mut flags) } == 0,
            "cannot inspect EFI variable flags"
        );
        if flags & 0x10 != 0 {
            let writable = flags & !0x10;
            ensure!(
                unsafe { libc::ioctl(file.as_raw_fd(), 0x40086602 as libc::c_ulong, &writable) }
                    == 0,
                "cannot unlock EFI selection variable"
            );
        }
    }
    let result = (|| -> Result<()> {
        if entry.is_empty() {
            remove_if_exists(&path)?;
            return Ok(());
        }
        ensure!(
            entry.starts_with("looom-") && crate::config::identifier(entry),
            "invalid trial entry"
        );
        let mut data = 7u32.to_le_bytes().to_vec(); // NV | boot service | runtime
        for code in entry.encode_utf16().chain(Some(0)) {
            data.extend(code.to_le_bytes());
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)?;
        file.write_all(&data)?;
        ensure!(
            read_variable("LoaderEntryOneShot")?.as_deref() == Some(entry),
            "EFI trial readback mismatch"
        );
        Ok(())
    })();
    if flags & 0x10 != 0 && path.try_exists()? {
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)?;
        ensure!(
            unsafe { libc::ioctl(file.as_raw_fd(), 0x40086602 as libc::c_ulong, &flags) } == 0,
            "cannot restore EFI variable flags"
        );
    }
    result
}
pub fn entry(id: &str, title: &str, image: &str) -> String {
    format!("/{id}\n    comment: {title}\n    protocol: efi\n    path: boot():/EFI/Linux/{image}\n")
}
pub fn environment(manager: &Manager) -> Result<BTreeMap<String, String>> {
    let text = fs::read_to_string(config_path(manager))?;
    let saved = text
        .lines()
        .find_map(|l| l.strip_prefix("default_entry: "))
        .context("Limine default missing")?;
    ensure!(
        saved.starts_with("looom-") && crate::config::identifier(saved),
        "invalid Limine default"
    );
    let mut env = BTreeMap::from([("saved_entry".into(), saved.into())]);
    if let Some(next) = read_variable("LoaderEntryOneShot")?.filter(|v| !v.is_empty()) {
        // A foreign BLI selection also blocks GC/confirm; do not silently overwrite it.
        ensure!(
            next.starts_with("looom-") && crate::config::identifier(&next),
            "another bootloader has a pending one-shot request"
        );
        env.insert("next_entry".into(), next);
    }
    Ok(env)
}
pub fn menu(manager: &Manager, include: Option<&str>) -> Result<String> {
    let saved = if config_path(manager).try_exists()? {
        environment(manager)?["saved_entry"].clone()
    } else {
        "looom-bootstrap".into()
    };
    render_menu(manager, include, &saved)
}
fn render_menu(manager: &Manager, include: Option<&str>, saved: &str) -> Result<String> {
    let mut text = format!(
        "# Generated by looom; use try/confirm/rollback.\ntimeout: 5\nremember_last_entry: no\ndefault_entry: {saved}\ninterface_branding: looom\n"
    );
    text.push_str(&entry(
        "looom-bootstrap",
        "Writable recovery",
        "looom-bootstrap.efi",
    ));
    text.push_str(&crate::bootstrap_update::menu_entries(manager)?);
    for metadata in manager.list()?.iter().filter(|m| {
        matches!(m.phase.as_str(), "published" | "confirmed") || include == Some(m.id.as_str())
    }) {
        manager.validate(metadata, false)?;
        let image = format!("looom-{}.efi", metadata.id);
        ensure!(
            hash_file(&manager.efi().join(&image))? == metadata.uki_sha256,
            "menu UKI integrity mismatch"
        );
        text.push_str(&entry(
            &format!("looom-{}", metadata.id),
            &format!("{} ({})", metadata.id, metadata.kernel_version),
            &image,
        ));
    }
    ensure!(
        text.lines().any(|l| l == format!("/{saved}")),
        "default absent from Limine menu"
    );
    Ok(text)
}
/// Explicit repair may reset a lost/invalid permanent choice to recovery.
/// Normal publish/GC never guesses a default when configuration is damaged.
pub fn recover_menu(manager: &Manager) -> Result<()> {
    ensure!(
        hash_file(&manager.efi().join("looom-bootstrap.efi"))?
            == manager.machine.bootstrap_uki_sha256,
        "recovery UKI integrity mismatch"
    );
    let saved = fs::read_to_string(config_path(manager))
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix("default_entry: ").map(str::to_owned))
        })
        .filter(|id| {
            id == "looom-bootstrap"
                || id
                    .strip_prefix("looom-")
                    .filter(|s| crate::config::identifier(s))
                    .and_then(|id| manager.load(id).ok())
                    .is_some_and(|m| matches!(m.phase.as_str(), "published" | "confirmed"))
        })
        .unwrap_or_else(|| "looom-bootstrap".into());
    write_menu(manager, &render_menu(manager, None, &saved)?)?;
    println!("Limine menu repaired; permanent choice: {saved}");
    Ok(())
}
pub fn write_menu(manager: &Manager, text: &str) -> Result<()> {
    ensure!(
        text.starts_with("# Generated by looom;") && text.contains("/looom-bootstrap\n"),
        "invalid generated Limine menu"
    );
    let path = config_path(manager);
    mkdir(path.parent().unwrap(), 0o700)?;
    // Both paths must lose GC candidates before either UKI or root is removed.
    atomic(&path, text.as_bytes(), 0o600)?;
    mkdir(&manager.esp.join("EFI/BOOT"), 0o700)?;
    atomic(
        &manager.esp.join("EFI/BOOT/limine.conf"),
        text.as_bytes(),
        0o600,
    )?;
    command("sync", &["-f", string(&manager.esp)?])
}
pub fn set_environment(manager: &Manager, values: &[String]) -> Result<()> {
    let mut text = fs::read_to_string(config_path(manager))?;
    let mut next = None;
    for value in values {
        let (key, entry) = value.split_once('=').context("invalid selection")?;
        ensure!(
            entry.is_empty()
                || (entry.starts_with("looom-")
                    && crate::config::identifier(entry)
                    && text.lines().any(|l| l == format!("/{entry}"))),
            "entry absent from Limine menu"
        );
        match key {
            "saved_entry" => {
                ensure!(!entry.is_empty(), "empty permanent boot choice");
                let old = text
                    .lines()
                    .find(|l| l.starts_with("default_entry: "))
                    .context("default missing")?
                    .to_owned();
                text = text.replacen(&old, &format!("default_entry: {entry}"), 1);
            }
            "next_entry" => next = Some(entry),
            _ => anyhow::bail!("unknown boot selection key"),
        }
    }
    write_menu(manager, &text)?;
    if let Some(entry) = next {
        oneshot(entry)?;
    }
    Ok(())
}
pub fn copy_loader(destination: &Path) -> Result<()> {
    let source = Path::new("/usr/share/limine/BOOTX64.EFI");
    ensure!(
        source.is_file(),
        "Limine package missing; install the pinned package before bootstrap/migration"
    );
    // Require the BLI implementation used by looom, rather than assuming all versions support it.
    ensure!(
        fs::read_to_string("/usr/share/doc/limine/CONFIG.md")?.contains("LoaderEntryOneShot"),
        "Limine with Boot Loader Interface one-shot support required"
    );
    ensure!(
        !destination.try_exists()?,
        "loader checkpoint already exists"
    );
    mkdir(destination.parent().context("loader parent")?, 0o700)?;
    atomic(destination, &fs::read(source)?, 0o600)?;
    ensure!(
        hash_file(destination)? == hash_file(source)?,
        "Limine copy mismatch"
    );
    Ok(())
}
/// Migrate an existing installation; GRUB and its fallback remain available.
pub fn migrate() -> Result<()> {
    let mut manager = Manager::installed(Machine::load()?)?;
    let _lock = manager.lock()?;
    if manager.machine.bootloader == Bootloader::Limine {
        println!("Limine already selected");
        return Ok(());
    }
    let env = manager.environment()?;
    ensure!(
        env.get("next_entry").is_none_or(String::is_empty),
        "finish the pending GRUB trial before migration"
    );
    let saved = env.get("saved_entry").context("GRUB default missing")?;
    if saved != "looom-bootstrap" {
        let id = saved
            .strip_prefix("looom-")
            .context("invalid GRUB default")?;
        ensure!(
            manager.load(id)?.phase == "confirmed",
            "migration requires a confirmed default"
        );
        manager.validate(&manager.load(id)?, true)?;
    }
    ensure!(
        read_variable("LoaderEntryOneShot")?.is_none_or(|s| s.is_empty()),
        "another EFI trial is pending"
    );
    let loader = manager.esp.join(LOADER);
    if loader.try_exists()? {
        ensure!(
            hash_file(&loader)? == hash_file(Path::new("/usr/share/limine/BOOTX64.EFI"))?,
            "existing Limine loader differs"
        );
    } else {
        copy_loader(&loader)?;
    }
    manager.machine.bootloader = Bootloader::Limine;
    write_menu(&manager, &menu(&manager, None)?)?;
    set_environment(&manager, &[format!("saved_entry={saved}")])?;
    let binary = Path::new(STATE).join("boot-manager");
    atomic(&binary, &fs::read(std::env::current_exe()?)?, 0o700)?;
    // Register only after the complete menu/loader is durable; keep the GRUB profile on failure.
    crate::bootstrap::register_entry(
        &manager.machine,
        "looom-limine",
        "\\EFI\\looom\\liminex64.efi",
        false,
    )?;
    json(
        &Path::new(STATE).join("bootloader.json"),
        &Bootloader::Limine,
        0o600,
    )?;
    println!(
        "Limine selected; GRUB retained. Reboot, verify the running release, then continue with this manager."
    );
    Ok(())
}
