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
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Update {
    schema: u32,
    source: String,
    kernel: String,
    subvolume: String,
    old_machine: Machine,
    manager_sha256: String,
    uki_sha256: Option<String>,
    phase: Phase,
}
fn directory() -> PathBuf {
    Path::new(STATE).join("bootstrap-update")
}
fn record() -> PathBuf {
    directory().join("journal.json")
}
fn read() -> Result<Update> {
    credentials::trusted_dir(&directory(), true)?;
    let update: Update = serde_json::from_str(&credentials::read_private(&record())?)?;
    ensure!(
        update.schema == 1
            && crate::config::identifier(&update.source)
            && update.subvolume == format!("@bootstrap-{}", update.source)
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
    update.old_machine.validate()?;
    ensure!(
        update.phase == Phase::Staging || update.uki_sha256.is_some(),
        "bootstrap digest missing"
    );
    Ok(update)
}
fn save(update: &Update) -> Result<()> {
    json(&record(), update, 0o600)
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
        hash_file(&directory().join("candidate.efi"))? == *update.uki_sha256.as_ref().unwrap(),
        "candidate checkpoint changed"
    );
    Ok(())
}

pub fn stage(source: &str) -> Result<()> {
    ensure!(
        crate::config::identifier(source) && format!("@bootstrap-{source}").len() <= 64,
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
    let mut update = if record().exists() {
        let update = read()?;
        matching_machine(&update)?;
        ensure!(
            update.source == source && update.manager_sha256 == executable_sha,
            "another bootstrap update already exists; retain its journal/checkpoints"
        );
        ensure!(
            update.phase != Phase::Committing,
            "run bootstrap-recover before staging"
        );
        if update.phase == Phase::Complete {
            println!("Bootstrap update already completed");
            return Ok(());
        }
        update
    } else {
        ensure!(
            !directory().exists()
                && !manager.top.join(format!("@bootstrap-{source}")).exists()
                && !manager.efi().join("looom-bootstrap-candidate.efi").exists()
                && !manager.efi().join("looom-bootstrap.previous.efi").exists(),
            "existing bootstrap update namespace; retain for diagnosis"
        );
        mkdir(&directory(), 0o700)?;
        let update = Update {
            schema: 1,
            source: source.into(),
            kernel: manager.load(source)?.kernel_version,
            subvolume: format!("@bootstrap-{source}"),
            old_machine: manager.machine.clone(),
            manager_sha256: executable_sha,
            uki_sha256: None,
            phase: Phase::Staging,
        };
        save(&update)?;
        update
    };
    if update.phase == Phase::Ready {
        candidate_valid(&manager, &update)?;
        println!("Bootstrap candidate ready; use bootstrap-try, reboot and bootstrap-confirm");
        return Ok(());
    }
    failpoint("bootstrap-update-journal")?;
    copy_checkpoint(
        &manager.efi().join("looom-bootstrap.efi"),
        &directory().join("previous.efi"),
        &update.old_machine.bootstrap_uki_sha256,
    )?;
    // Our unfinished candidate can be recreated, but never delete a mounted/running root.
    let candidate = manager.top.join(&update.subvolume);
    if candidate.exists() {
        let mounts = output("findmnt", &["-rn", "-t", "btrfs", "-o", "UUID,FSROOT"])?;
        ensure!(
            !mounts.lines().any(|l| {
                let mut v = l.split_whitespace();
                v.next() == Some(update.old_machine.root_uuid.as_str())
                    && v.next() == Some(format!("/{}", update.subvolume).as_str())
            }),
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
    copy_checkpoint(&image, &directory().join("candidate.efi"), &digest)?;
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
        &directory().join("candidate.efi"),
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
        &directory().join("previous.efi"),
        &manager.efi().join("looom-bootstrap.previous.efi"),
        &update.old_machine.bootstrap_uki_sha256,
    )?;
    // Crossing filesystems is journaled: recovery accepts only the recorded old/new profile.
    copy_checkpoint(
        &directory().join("candidate.efi"),
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
    if !record().exists() {
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
            &directory().join("candidate.efi"),
            &manager.efi().join("looom-bootstrap.efi"),
            update.uki_sha256.as_ref().unwrap(),
        )?;
        copy_checkpoint(
            &directory().join("previous.efi"),
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
    if manager.state != Path::new(STATE) || manager.esp != Path::new("/efi") || !record().exists() {
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
        text.push_str("menuentry 'looom bootstrap candidate (one-shot trial)' --id looom-bootstrap-candidate {\n chainloader ($esp)/EFI/Linux/looom-bootstrap-candidate.efi\n}\n");
    }
    if matches!(update.phase, Phase::Committing | Phase::Complete)
        && manager.efi().join("looom-bootstrap.previous.efi").exists()
    {
        ensure!(
            hash_file(&manager.efi().join("looom-bootstrap.previous.efi"))?
                == update.old_machine.bootstrap_uki_sha256,
            "previous bootstrap image mismatch"
        );
        text.push_str("menuentry 'looom previous bootstrap (recovery)' --id looom-bootstrap-previous {\n chainloader ($esp)/EFI/Linux/looom-bootstrap.previous.efi\n}\n");
    }
    Ok(text)
}
