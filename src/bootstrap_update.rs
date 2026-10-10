//! A recovery-system candidate is tested once before replacing the stable UKI.
//! Btrfs holds the journal and both EFI checkpoints; FAT is never the sole copy.
use crate::{
    bootstrap::copy_checkpoint,
    credentials,
    machine::{Machine, STATE},
    releases::Manager,
    util::*,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

#[derive(Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum Phase {
    Staging,
    Ready,
    Committing,
    Complete,
    Removing,
    Removed,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Update {
    schema: u32,
    #[serde(default)]
    generation: Option<String>,
    source: String,
    kernel: String,
    subvolume: String,
    old_machine: Machine,
    manager_sha256: String,
    uki_sha256: Option<String>,
    phase: Phase,
}
fn history() -> PathBuf {
    Path::new(STATE).join("bootstrap-history")
}
fn pointer() -> PathBuf {
    Path::new(STATE).join("bootstrap-current.json")
}
fn present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}
fn private_directory(path: &Path) -> Result<()> {
    if !present(path)? {
        mkdir(path, 0o700)?;
        sync_dir(path.parent().context("directory parent")?)?;
    }
    credentials::trusted_dir(path, true)?;
    Ok(())
}
fn directory() -> Result<PathBuf> {
    if present(&pointer())? {
        let id: String = serde_json::from_str(&credentials::read_private(&pointer())?)?;
        ensure!(
            crate::config::identifier(&id),
            "invalid bootstrap generation pointer"
        );
        Ok(history().join(id))
    } else {
        Ok(Path::new(STATE).join("bootstrap-update"))
    }
}
fn record() -> Result<PathBuf> {
    let path = directory()?.join("journal.json");
    ensure!(
        !present(&pointer())? || present(&path)?,
        "active bootstrap journal missing"
    );
    Ok(path)
}
fn read_at(path: &Path) -> Result<Update> {
    credentials::trusted_dir(path, true)?;
    let update: Update =
        serde_json::from_str(&credentials::read_private(&path.join("journal.json"))?)?;
    let id = update.generation.as_deref().unwrap_or(&update.source);
    ensure!(
        matches!(update.schema, 1 | 2)
            && (update.schema == 1 && update.generation.is_none()
                || update.schema == 2 && update.generation.is_some())
            && crate::config::identifier(&update.source)
            && crate::config::identifier(id)
            && update.subvolume == format!("@bootstrap-{id}")
            && update.subvolume.len() <= 64
            && !update.kernel.is_empty()
            && update
                .kernel
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._+-".contains(&c))
            && update.manager_sha256.len() == 64
            && update.manager_sha256.bytes().all(|c| c.is_ascii_hexdigit())
            && update
                .uki_sha256
                .as_ref()
                .is_none_or(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit())),
        "invalid bootstrap update journal"
    );
    if path.parent() == Some(history().as_path()) {
        ensure!(
            path.file_name().and_then(|v| v.to_str()) == Some(id),
            "bootstrap generation name mismatch"
        );
    }
    update.old_machine.validate()?;
    ensure!(
        update.phase == Phase::Staging || update.uki_sha256.is_some(),
        "bootstrap digest missing"
    );
    Ok(update)
}
fn read() -> Result<Update> {
    let update = read_at(&directory()?)?;
    ensure!(
        !matches!(update.phase, Phase::Removing | Phase::Removed),
        "active bootstrap generation cannot be removed"
    );
    Ok(update)
}
fn save(update: &Update) -> Result<()> {
    json(&record()?, update, 0o600)
}
fn same_identity(update: &Update, machine: &Machine) -> Result<()> {
    let mut normalized = machine.clone();
    normalized.bootstrap_uki_sha256 = update.old_machine.bootstrap_uki_sha256.clone();
    ensure!(
        serde_json::to_vec(&normalized)? == serde_json::to_vec(&update.old_machine)?,
        "bootstrap journal belongs to another machine"
    );
    Ok(())
}
fn archive_legacy(update: &Update) -> Result<()> {
    if directory()? != Path::new(STATE).join("bootstrap-update") {
        return Ok(());
    }
    ensure!(
        update.phase == Phase::Complete,
        "finish bootstrap recovery before migration"
    );
    private_directory(&history())?;
    let destination = history().join(&update.source);
    private_directory(&destination)?;
    if destination.join("journal.json").exists() {
        ensure!(
            serde_json::to_vec(&read_at(&destination)?)? == serde_json::to_vec(update)?,
            "bootstrap archive collision"
        );
    }
    copy_checkpoint(
        &directory()?.join("candidate.efi"),
        &destination.join("candidate.efi"),
        update.uki_sha256.as_ref().unwrap(),
    )?;
    copy_checkpoint(
        &directory()?.join("previous.efi"),
        &destination.join("previous.efi"),
        &update.old_machine.bootstrap_uki_sha256,
    )?;
    json(&destination.join("journal.json"), update, 0o600)?;
    sync_dir(&history())?;
    Ok(())
}
fn matching_machine(update: &Update) -> Result<Machine> {
    let mut current = Machine::load()?;
    current.guard_mounts()?;
    ensure!(
        current.bootstrap_uki_sha256 == update.old_machine.bootstrap_uki_sha256
            || update.uki_sha256.as_ref() == Some(&current.bootstrap_uki_sha256),
        "unknown bootstrap profile digest"
    );
    current.bootstrap_uki_sha256 = update.old_machine.bootstrap_uki_sha256.clone();
    ensure!(
        serde_json::to_vec(&current)? == serde_json::to_vec(&update.old_machine)?,
        "bootstrap journal belongs to another machine"
    );
    Ok(current)
}
fn manager(update: &Update) -> Result<Manager> {
    let machine = matching_machine(update)?;
    Ok(Manager {
        top: machine.ensure_top()?,
        machine,
        state: STATE.into(),
        esp: "/efi".into(),
    })
}
struct Mounted(PathBuf);
impl Drop for Mounted {
    fn drop(&mut self) {
        if succeeds("umount", &[self.0.to_str().unwrap()]) {
            let _ = fs::remove_dir(&self.0);
        } else {
            eprintln!(
                "Bootstrap mount retained for diagnosis: {}",
                self.0.display()
            );
        }
    }
}
fn candidate_valid(manager: &Manager, update: &Update) -> Result<()> {
    let root = manager.top.join(&update.subvolume);
    ensure!(
        output("btrfs", &["property", "get", "-ts", string(&root)?, "ro"])? == "ro=false",
        "bootstrap candidate must be writable"
    );
    ensure!(
        hash_file(&root.join("boot/looom-bootstrap.efi"))?
            == *update.uki_sha256.as_ref().context("candidate not ready")?
            && hash_file(&root.join("usr/bin/looom"))? == update.manager_sha256
            && fs::read_to_string(root.join("etc/kernel/cmdline"))?
                .split_whitespace()
                .any(|v| v == format!("rootflags=subvol={}", update.subvolume))
            && !root.join("etc/looom/release-id").exists(),
        "bootstrap candidate changed"
    );
    ensure!(
        hash_file(&directory()?.join("candidate.efi"))? == *update.uki_sha256.as_ref().unwrap(),
        "candidate checkpoint changed"
    );
    Ok(())
}

pub fn stage(source: &str, generation: Option<&str>) -> Result<()> {
    let generation = generation.unwrap_or(source);
    ensure!(
        crate::config::identifier(source)
            && crate::config::identifier(generation)
            && format!("@bootstrap-{generation}").len() <= 64,
        "invalid bootstrap source ID"
    );
    let manager = Manager::installed(Machine::load()?)?;
    let _lock = manager.lock()?;
    manager.health(source)?;
    ensure!(
        manager.load(source)?.phase == "confirmed",
        "bootstrap update requires confirmed running release"
    );
    let env = manager.environment()?;
    ensure!(
        env.get("saved_entry") == Some(&format!("looom-{source}"))
            && env.get("next_entry").is_none_or(String::is_empty),
        "confirm running default and finish pending trial first"
    );
    let executable = std::env::current_exe()?;
    let executable_sha = hash_file(&executable)?;
    let existing = if present(&record()?)? {
        Some(read()?)
    } else {
        None
    };
    if let Some(update) = &existing {
        matching_machine(update)?;
        let same = update.source == source
            && update.manager_sha256 == executable_sha
            && update.generation.as_deref().unwrap_or(&update.source) == generation;
        if update.phase == Phase::Complete && same {
            println!("Bootstrap update already completed");
            return Ok(());
        }
        ensure!(
            update.phase == Phase::Complete || same,
            "another bootstrap candidate is pending"
        );
        ensure!(
            !matches!(
                update.phase,
                Phase::Committing | Phase::Removing | Phase::Removed
            ),
            "run bootstrap-recover before staging"
        );
    }
    let mut update = if existing
        .as_ref()
        .is_some_and(|v| v.phase != Phase::Complete)
    {
        existing.unwrap()
    } else {
        if let Some(previous) = &existing {
            archive_legacy(previous)?;
        } else {
            ensure!(
                !directory()?.exists()
                    && !manager.efi().join("looom-bootstrap.previous.efi").exists(),
                "existing bootstrap namespace; retain for diagnosis"
            );
        }
        // A complete candidate slot is disposable, but selected slots never are.
        clean_trial_slot(&manager, existing.as_ref(), true)?;
        private_directory(&history())?;
        let destination = history().join(generation);
        let next = Update {
            schema: 2,
            generation: Some(generation.into()),
            source: source.into(),
            kernel: manager.load(source)?.kernel_version,
            subvolume: format!("@bootstrap-{generation}"),
            old_machine: manager.machine.clone(),
            manager_sha256: executable_sha,
            uki_sha256: None,
            phase: Phase::Staging,
        };
        if present(&destination)? && present(&destination.join("journal.json"))? {
            let interrupted = read_at(&destination)?;
            ensure!(
                serde_json::to_vec(&interrupted)? == serde_json::to_vec(&next)?,
                "bootstrap generation exists; choose a new generation ID"
            );
        } else {
            ensure!(
                !manager.top.join(&next.subvolume).exists(),
                "unowned bootstrap root exists"
            );
            private_directory(&destination)?;
            ensure!(
                fs::read_dir(&destination)?.next().is_none(),
                "unowned incomplete bootstrap directory"
            );
            json(&destination.join("journal.json"), &next, 0o600)?;
        }
        // Prepare old image before switching the pointer; every crash leaves a usable journal.
        copy_checkpoint(
            &manager.efi().join("looom-bootstrap.efi"),
            &destination.join("previous.efi"),
            &next.old_machine.bootstrap_uki_sha256,
        )?;
        sync_dir(&history())?;
        failpoint("bootstrap-generation-prepared")?;
        json(&pointer(), &generation, 0o600)?;
        failpoint("bootstrap-generation-selected")?;
        next
    };
    if update.phase == Phase::Ready {
        candidate_valid(&manager, &update)?;
        println!("Bootstrap candidate ready; use bootstrap-try, reboot and bootstrap-confirm");
        return Ok(());
    }
    failpoint("bootstrap-update-journal")?;
    copy_checkpoint(
        &manager.efi().join("looom-bootstrap.efi"),
        &directory()?.join("previous.efi"),
        &update.old_machine.bootstrap_uki_sha256,
    )?;
    // Our unfinished candidate can be recreated, but never delete a mounted/running root.
    let candidate = manager.top.join(&update.subvolume);
    if candidate.exists() {
        ensure!(
            !manager.mounted_subvolume(&update.subvolume)?,
            "unfinished bootstrap candidate is mounted; unmount it explicitly"
        );
        command(
            "btrfs",
            &["subvolume", "delete", "--commit-after", string(&candidate)?],
        )?;
    }
    manager.validate(&manager.load(source)?, true)?;
    command(
        "btrfs",
        &[
            "subvolume",
            "snapshot",
            string(&manager.top.join(format!("@root-{source}")))?,
            string(&candidate)?,
        ],
    )?;
    failpoint("bootstrap-update-snapshot")?;
    atomic(
        &candidate.join("etc/fstab"),
        manager.machine.fstab(&update.subvolume, false)?.as_bytes(),
        0o644,
    )?;
    remove_if_exists(&candidate.join("etc/looom/release-id"))?;
    fs::copy(&executable, candidate.join("usr/bin/looom"))?;
    crate::builder::install_sources(&candidate)?;
    let previous_cmdline = fs::read_to_string(candidate.join("etc/kernel/cmdline"))?;
    let extra = previous_cmdline
        .split_whitespace()
        .filter(|v| {
            !v.starts_with("root=") && !v.starts_with("rootflags=") && !matches!(*v, "ro" | "rw")
        })
        .collect::<Vec<_>>()
        .join(" ");
    atomic(
        &candidate.join("etc/kernel/cmdline"),
        format!(
            "root=UUID={} rootflags=subvol={} rw {extra}\n",
            manager.machine.root_uuid, update.subvolume
        )
        .as_bytes(),
        0o644,
    )?;
    // Keep account templates and persistent bind mounts from the confirmed release.
    // Disable desktop startup in rescue, leaving its packages available for diagnosis.
    command(
        "ln",
        &[
            "-sfn",
            "/usr/lib/systemd/system/multi-user.target",
            string(&candidate.join("etc/systemd/system/default.target"))?,
        ],
    )?;
    for entry in fs::read_dir(candidate.join("etc/mkinitcpio.d"))? {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) == Some("preset") {
            fs::remove_file(path)?;
        }
    }
    atomic(&candidate.join("etc/mkinitcpio.d/looom.preset"), b"ALL_config=\"/etc/mkinitcpio.conf\"\nALL_kver=\"/boot/vmlinuz-looom\"\nPRESETS=('default')\ndefault_uki=\"/boot/looom-bootstrap.efi\"\ndefault_options=\"--cmdline /etc/kernel/cmdline\"\n", 0o644)?;
    let mount = Path::new("/run/looom-bootstrap-update-root");
    ensure!(
        !succeeds("mountpoint", &["-q", string(mount)?]),
        "bootstrap build mount occupied"
    );
    mkdir(mount, 0o700)?;
    command(
        "mount",
        &[
            "-o",
            &format!("subvol={},rw", update.subvolume),
            &format!("UUID={}", manager.machine.root_uuid),
            string(mount)?,
        ],
    )?;
    let mounted = Mounted(mount.into());
    command("mount", &["--make-rprivate", string(mount)?])?;
    chroot(mount, "mkinitcpio", &["-P"])?;
    command("sync", &["-f", string(mount)?])?;
    drop(mounted);
    let image = candidate.join("boot/looom-bootstrap.efi");
    let digest = hash_file(&image)?;
    copy_checkpoint(&image, &directory()?.join("candidate.efi"), &digest)?;
    update.uki_sha256 = Some(digest);
    update.phase = Phase::Ready;
    save(&update)?;
    failpoint("bootstrap-update-ready")?;
    println!(
        "Bootstrap candidate ready; use bootstrap-try, reboot and bootstrap-confirm. Saved release is unchanged"
    );
    Ok(())
}

pub fn trial() -> Result<()> {
    let update = read()?;
    let manager = manager(&update)?;
    let _lock = manager.lock()?;
    manager.machine.guard()?;
    ensure!(
        update.phase == Phase::Ready,
        "bootstrap candidate is not ready"
    );
    candidate_valid(&manager, &update)?;
    ensure!(
        manager
            .environment()?
            .get("next_entry")
            .is_none_or(String::is_empty),
        "another trial is pending"
    );
    copy_checkpoint(
        &directory()?.join("candidate.efi"),
        &manager.efi().join("looom-bootstrap-candidate.efi"),
        update.uki_sha256.as_ref().unwrap(),
    )?;
    manager.write_menu(&manager.menu(None)?)?;
    manager.set_environment(&["next_entry=looom-bootstrap-candidate".into()])?;
    println!("Next boot only: bootstrap candidate; reboot separately. Saved release unchanged");
    Ok(())
}

pub fn confirm() -> Result<()> {
    let mut update = read()?;
    let manager = manager(&update)?;
    let _lock = manager.lock()?;
    ensure!(
        update.phase == Phase::Ready,
        "bootstrap candidate is not awaiting confirmation"
    );
    manager.machine.guard()?;
    candidate_valid(&manager, &update)?;
    ensure!(
        output("findmnt", &["-nro", "FSROOT", "/"])? == format!("/{}", update.subvolume)
            && output("findmnt", &["-nro", "OPTIONS", "/"])?
                .split(',')
                .any(|v| v == "rw")
            && output("uname", &["-r"])? == update.kernel
            && hash_file(Path::new("/usr/bin/looom"))? == update.manager_sha256,
        "boot the expected bootstrap candidate before confirmation"
    );
    ensure!(
        manager
            .environment()?
            .get("next_entry")
            .is_none_or(String::is_empty),
        "boot trial was not consumed"
    );
    for unit in [
        "looom-accounts.service",
        "sshd.service",
        "NetworkManager.service",
    ] {
        ensure!(
            output("systemctl", &["is-active", unit])? == "active",
            "bootstrap service inactive: {unit}"
        );
    }
    if manager.machine.guest_agent {
        ensure!(
            output("systemctl", &["is-active", "qemu-guest-agent.service"])? == "active",
            "guest agent inactive"
        );
    }
    ensure!(
        output("systemctl", &["--failed", "--no-legend", "--plain"])?.is_empty(),
        "bootstrap has failed units"
    );
    ensure!(
        output("id", &["-u", &manager.machine.user])? == manager.machine.uid.to_string()
            && output("id", &["-g", &manager.machine.user])? == manager.machine.gid.to_string(),
        "bootstrap user identity differs"
    );
    for path in [
        "/etc/looom-local",
        "/etc/NetworkManager/system-connections",
        "/root/.ssh",
    ] {
        ensure!(
            output("findmnt", &["-nro", "UUID", path])? == manager.machine.root_uuid
                && output("findmnt", &["-nro", "OPTIONS", path])?
                    .split(',')
                    .any(|v| v == "rw"),
            "bootstrap persistent bind missing: {path}"
        );
    }
    credentials::Accounts::installed(&manager.machine).generate()?;
    command(
        "pwck",
        &["-qr", "/etc/passwd", "/run/looom/accounts/shadow"],
    )?;
    command(
        "grpck",
        &["-r", "/etc/group", "/run/looom/accounts/gshadow"],
    )?;
    update.phase = Phase::Committing;
    save(&update)?;
    failpoint("bootstrap-update-commit")?;
    finish(&manager, &mut update)
}
fn finish(manager: &Manager, update: &mut Update) -> Result<()> {
    candidate_valid(manager, update)?;
    copy_checkpoint(
        &directory()?.join("previous.efi"),
        &manager.efi().join("looom-bootstrap.previous.efi"),
        &update.old_machine.bootstrap_uki_sha256,
    )?;
    // Crossing filesystems is journaled: recovery accepts only the recorded old/new profile.
    copy_checkpoint(
        &directory()?.join("candidate.efi"),
        &manager.efi().join("looom-bootstrap.efi"),
        update.uki_sha256.as_ref().unwrap(),
    )?;
    command("sync", &["-f", "/efi"])?;
    failpoint("bootstrap-update-uki")?;
    let mut profile = update.old_machine.clone();
    profile.bootstrap_uki_sha256 = update.uki_sha256.clone().unwrap();
    json(&Path::new(STATE).join("machine.json"), &profile, 0o600)?;
    failpoint("bootstrap-update-profile")?;
    update.phase = Phase::Complete;
    save(update)?;
    manager.write_menu(&manager.menu(None)?)?;
    println!("Bootstrap update completed; previous root/image retained, saved release unchanged");
    Ok(())
}
/// Invoke before strict Machine::guard, otherwise the old-profile/new-UKI window
/// would make recovery inaccessible. Mount identity and journal are still checked.
pub fn recover() -> Result<()> {
    root()?;
    if !present(&record()?)? {
        return Ok(());
    }
    let mut update = read()?;
    let manager = manager(&update)?;
    let _lock = manager.lock()?;
    if update.phase == Phase::Committing {
        finish(&manager, &mut update)?;
    }
    if update.phase == Phase::Complete {
        copy_checkpoint(
            &directory()?.join("candidate.efi"),
            &manager.efi().join("looom-bootstrap.efi"),
            update.uki_sha256.as_ref().unwrap(),
        )?;
        copy_checkpoint(
            &directory()?.join("previous.efi"),
            &manager.efi().join("looom-bootstrap.previous.efi"),
            &update.old_machine.bootstrap_uki_sha256,
        )?;
        let mut profile = update.old_machine.clone();
        profile.bootstrap_uki_sha256 = update.uki_sha256.clone().unwrap();
        json(&Path::new(STATE).join("machine.json"), &profile, 0o600)?;
        Machine::load()?.guard()?;
        manager.write_menu(&manager.menu(None)?)?;
    } else {
        println!(
            "Bootstrap candidate remains unselected; repeat bootstrap-update or bootstrap-try explicitly"
        );
    }
    Ok(())
}
/// Called only for the installed manager, not synthetic test Managers.
pub fn menu_entries(manager: &Manager) -> Result<String> {
    if manager.state != Path::new(STATE)
        || manager.esp != Path::new("/efi")
        || !present(&record()?)?
    {
        return Ok(String::new());
    }
    let update = read()?;
    matching_machine(&update)?;
    let mut text = String::new();
    if matches!(update.phase, Phase::Ready | Phase::Committing)
        && manager.efi().join("looom-bootstrap-candidate.efi").exists()
    {
        ensure!(
            hash_file(&manager.efi().join("looom-bootstrap-candidate.efi"))?
                == *update.uki_sha256.as_ref().unwrap(),
            "bootstrap trial image mismatch"
        );
        if manager.machine.bootloader == crate::machine::Bootloader::Limine {
            text.push_str(&crate::limine::entry(
                "looom-bootstrap-candidate",
                "One-shot recovery trial",
                "looom-bootstrap-candidate.efi",
            ));
        } else {
            text.push_str("menuentry 'looom bootstrap candidate (one-shot trial)' --id looom-bootstrap-candidate {\n chainloader ($esp)/EFI/Linux/looom-bootstrap-candidate.efi\n}\n");
        }
    }
    if manager.efi().join("looom-bootstrap.previous.efi").exists() {
        let digest = hash_file(&manager.efi().join("looom-bootstrap.previous.efi"))?;
        let known = if matches!(update.phase, Phase::Committing | Phase::Complete) {
            digest == update.old_machine.bootstrap_uki_sha256
        } else {
            history_entries()?.iter().any(|(_, old)| {
                old.phase == Phase::Complete && old.old_machine.bootstrap_uki_sha256 == digest
            })
        };
        ensure!(known, "previous bootstrap image mismatch");
        if manager.machine.bootloader == crate::machine::Bootloader::Limine {
            text.push_str(&crate::limine::entry(
                "looom-bootstrap-previous",
                "Previous recovery",
                "looom-bootstrap.previous.efi",
            ));
        } else {
            text.push_str("menuentry 'looom previous bootstrap (recovery)' --id looom-bootstrap-previous {\n chainloader ($esp)/EFI/Linux/looom-bootstrap.previous.efi\n}\n");
        }
    }
    Ok(text)
}

fn history_entries() -> Result<Vec<(PathBuf, Update)>> {
    let mut entries = Vec::new();
    if !history().exists() {
        return Ok(entries);
    }
    credentials::trusted_dir(&history(), true)?;
    for entry in fs::read_dir(history())? {
        let path = entry?.path();
        if !present(&path.join("journal.json"))? {
            credentials::trusted_dir(&path, true)?;
            ensure!(
                fs::read_dir(&path)?.next().is_none(),
                "bootstrap history without journal"
            );
            continue;
        }
        entries.push((path.clone(), read_at(&path)?));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries)
}
fn clean_trial_slot(manager: &Manager, update: Option<&Update>, apply: bool) -> Result<()> {
    let slot = manager.efi().join("looom-bootstrap-candidate.efi");
    if !slot.try_exists()? {
        return Ok(());
    }
    let update = update.context("unowned bootstrap candidate slot")?;
    if update.phase != Phase::Complete {
        return Ok(());
    }
    ensure!(
        !manager
            .environment()?
            .values()
            .any(|v| v == "looom-bootstrap-candidate"),
        "bootstrap trial slot selected"
    );
    ensure!(
        hash_file(&slot)? == *update.uki_sha256.as_ref().unwrap(),
        "bootstrap trial slot changed"
    );
    owned_regular(&slot)?;
    println!("GC candidate: completed bootstrap trial EFI slot");
    if apply {
        fs::remove_file(slot)?;
        sync_dir(&manager.efi())?;
        command("sync", &["-f", string(&manager.esp)?])?;
    }
    Ok(())
}
pub fn protected_sources(manager: &Manager) -> Result<BTreeSet<String>> {
    let mut sources = BTreeSet::new();
    if manager.state == Path::new(STATE) && present(&record()?)? {
        let update = read()?;
        matching_machine(&update)?;
        if matches!(
            update.phase,
            Phase::Staging | Phase::Ready | Phase::Committing
        ) {
            sources.insert(update.source);
        }
    }
    if manager.state == Path::new(STATE) {
        for (_, update) in history_entries()? {
            same_identity(&update, &manager.machine)?;
            if matches!(
                update.phase,
                Phase::Staging | Phase::Ready | Phase::Committing
            ) {
                sources.insert(update.source);
            }
        }
    }
    Ok(sources)
}
pub fn list() -> Result<()> {
    let manager = Manager::installed(Machine::load()?)?;
    let _lock = manager.lock()?;
    if present(&record()?)? {
        let current = read()?;
        matching_machine(&current)?;
        println!(
            "Current bootstrap operation: {} ({})",
            current.subvolume,
            serde_json::to_string(&current.phase)?
        );
    }
    for (_, update) in history_entries()? {
        same_identity(&update, &manager.machine)?;
        println!(
            "{} {} source={} kernel={}",
            update.subvolume,
            serde_json::to_string(&update.phase)?,
            update.source,
            update.kernel
        );
    }
    Ok(())
}
/// Called under the common release-control lock. Keep stable, previous and mounted
/// roots regardless of count; only journal-owned history is eligible for deletion.
pub fn gc(manager: &Manager, apply: bool) -> Result<()> {
    gc_inner(manager, apply, false)
}
pub fn recover_gc(manager: &Manager) -> Result<()> {
    gc_inner(manager, true, true)
}
fn gc_inner(manager: &Manager, apply: bool, recover_only: bool) -> Result<()> {
    if manager.state != Path::new(STATE) || manager.esp != Path::new("/efi") {
        return Ok(());
    }
    let current = if present(&record()?)? {
        Some(read()?)
    } else {
        None
    };
    if let Some(update) = &current {
        matching_machine(update)?;
    }
    let mut digests = BTreeSet::from([manager.machine.bootstrap_uki_sha256.clone()]);
    let previous = manager.efi().join("looom-bootstrap.previous.efi");
    if previous.try_exists()? {
        digests.insert(hash_file(&previous)?);
    }
    if let Some(update) = &current {
        digests.insert(update.old_machine.bootstrap_uki_sha256.clone());
        if let Some(digest) = &update.uki_sha256 {
            digests.insert(digest.clone());
        }
    }
    // Verify the menu before unlinking a stale fixed candidate slot.
    manager.menu(None)?;
    if !recover_only {
        clean_trial_slot(manager, current.as_ref(), apply)?;
    }
    for (path, mut update) in history_entries()? {
        same_identity(&update, &manager.machine)?;
        if path == directory()?
            || update.phase == Phase::Removed
            || recover_only && update.phase != Phase::Removing
        {
            continue;
        }
        if !matches!(update.phase, Phase::Complete | Phase::Removing)
            || update
                .uki_sha256
                .as_ref()
                .is_some_and(|d| digests.contains(d))
            || manager.mounted_subvolume(&update.subvolume)?
        {
            println!("GC retain bootstrap: {}", update.subvolume);
            continue;
        }
        println!("GC bootstrap candidate: {}", update.subvolume);
        if !apply {
            continue;
        }
        update.phase = Phase::Removing;
        json(&path.join("journal.json"), &update, 0o600)?;
        failpoint("gc-bootstrap-journal")?;
        manager.delete_subvolume(&update.subvolume)?;
        failpoint("gc-bootstrap-root")?;
        for name in ["candidate.efi", "previous.efi"] {
            let image = path.join(name);
            if image.try_exists()? {
                owned_regular(&image)?;
                fs::remove_file(image)?;
            }
        }
        sync_dir(&path)?;
        update.phase = Phase::Removed;
        json(&path.join("journal.json"), &update, 0o600)?;
    }
    Ok(())
}
