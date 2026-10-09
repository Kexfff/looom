//! Acceptance helpers confined to the disposable VM; never installed in releases.
use anyhow::{Context, Result, ensure};
use looom::{
    credentials,
    machine::{Machine, STATE},
    releases::Manager,
    util::*,
};
use std::{
    fs,
    path::{Path, PathBuf},
};
const BASE: &str = "/var/lib/looom/dev/bootstrap-20261009";
fn guard() -> Result<()> {
    root()?;
    credentials::disable_dumps()?;
    ensure!(
        matches!(
            output("systemd-detect-virt", &["--vm"])?.as_str(),
            "qemu" | "kvm"
        ) && fs::read_to_string("/sys/class/net/enp1s0/address")?.trim() == "52:54:00:7b:23:63",
        "original VM only"
    );
    Ok(())
}
fn private() -> PathBuf {
    Path::new(BASE).join("private")
}
fn prepare() -> Result<()> {
    let machine = Machine::load()?;
    machine.guard()?;
    ensure!(!private().exists(), "baseline already retained");
    mkdir(&private(), 0o700)?;
    for user in ["root", machine.user.as_str()] {
        let value = credentials::read_private(
            &Path::new(STATE)
                .join("credentials")
                .join(format!("{user}.hash")),
        )?;
        credentials::private_atomic(&private().join(format!("{user}.hash")), value.as_bytes())?;
    }
    println!("PASS: original credentials retained in VM-only private baseline");
    Ok(())
}
struct Restore {
    path: PathBuf,
    data: Vec<u8>,
}
impl Drop for Restore {
    fn drop(&mut self) {
        if atomic(&self.path, &self.data, 0o600).is_err() {
            eprintln!("Failed to restore fixture journal; inspect private VM state");
        }
    }
}
fn negatives() -> Result<()> {
    let record = Path::new(STATE).join("bootstrap-update/journal.json");
    let data = credentials::read_private(&record)?;
    let original: serde_json::Value = serde_json::from_str(&data)?;
    ensure!(
        original["phase"] == "ready",
        "test only an unselected ready candidate"
    );
    let _restore = Restore {
        path: record.clone(),
        data: data.as_bytes().into(),
    };
    let executable = std::env::current_exe()?.with_file_name("looom");
    let before_profile = hash_file(&Path::new(STATE).join("machine.json"))?;
    let before_uki = hash_file(Path::new("/efi/EFI/Linux/looom-bootstrap.efi"))?;
    let before_env = output("grub-editenv", &["/efi/looom/grub/grubenv", "list"])?;
    for name in [
        "source-traversal",
        "subvolume-traversal",
        "schema",
        "phase",
        "manager-digest",
        "machine-identity",
        "unknown-field",
    ] {
        let mut changed = original.clone();
        match name {
            "source-traversal" => changed["source"] = "../escape".into(),
            "subvolume-traversal" => changed["subvolume"] = "../escape".into(),
            "schema" => changed["schema"] = 2.into(),
            "phase" => changed["phase"] = "unknown".into(),
            "manager-digest" => changed["manager_sha256"] = "bad".into(),
            "machine-identity" => {
                changed["old_machine"]["bootstrap_uki_sha256"] = "f".repeat(64).into()
            }
            _ => changed["unknown_field"] = true.into(),
        }
        json(&record, &changed, 0o600)?;
        ensure!(
            !succeeds(string(&executable)?, &["bootstrap-recover"]),
            "unsafe journal was accepted: {name}"
        );
        ensure!(
            hash_file(&Path::new(STATE).join("machine.json"))? == before_profile
                && hash_file(Path::new("/efi/EFI/Linux/looom-bootstrap.efi"))? == before_uki
                && output("grub-editenv", &["/efi/looom/grub/grubenv", "list"])? == before_env,
            "negative case changed installed state"
        );
        println!("PASS: rejected {name}; profile, stable UKI and boot choice intact");
    }
    atomic(&record, data.as_bytes(), 0o600)?;
    ensure!(
        !succeeds(string(&executable)?, &["bootstrap-confirm"]),
        "confirmation without actual candidate boot accepted"
    );
    println!("PASS: candidate confirmation refused from working release");
    Ok(())
}
fn clean_trial_slot() -> Result<()> {
    let machine = Machine::load()?;
    machine.guard()?;
    let manager = Manager::installed(machine.clone())?;
    let _lock = manager.lock()?;
    let update: serde_json::Value = serde_json::from_str(&credentials::read_private(
        &Path::new(STATE).join("bootstrap-update/journal.json"),
    )?)?;
    ensure!(
        update["phase"] == "complete" && update["uki_sha256"] == machine.bootstrap_uki_sha256,
        "bootstrap not completed"
    );
    ensure!(
        !manager
            .environment()?
            .values()
            .any(|v| v == "looom-bootstrap-candidate")
            && !fs::read_to_string("/efi/looom/grub/grub.cfg")?
                .contains("--id looom-bootstrap-candidate"),
        "trial image is selected or still in menu"
    );
    let path = Path::new("/efi/EFI/Linux/looom-bootstrap-candidate.efi");
    use std::os::unix::fs::MetadataExt;
    let info = fs::symlink_metadata(path)?;
    ensure!(
        info.is_file() && info.uid() == 0 && hash_file(path)? == machine.bootstrap_uki_sha256,
        "unexpected candidate slot"
    );
    ensure!(
        hash_file(Path::new("/efi/EFI/Linux/looom-bootstrap.previous.efi"))?
            == update["old_machine"]["bootstrap_uki_sha256"]
                .as_str()
                .context("previous digest")?
            && hash_file(&Path::new(STATE).join("bootstrap-update/candidate.efi"))?
                == machine.bootstrap_uki_sha256,
        "retained backups differ"
    );
    fs::remove_file(path)?;
    sync_dir(path.parent().unwrap())?;
    command("sync", &["-f", "/efi"])?;
    println!(
        "PASS: removed only unselected VM trial slot ({} bytes); stable/previous UKI and Btrfs checkpoints retained",
        info.len()
    );
    Ok(())
}

fn report() -> Result<()> {
    let machine = Machine::load()?;
    machine.guard()?;
    let manager = Manager::installed(machine.clone())?;
    let current = fs::read_to_string("/etc/looom/release-id")?
        .trim()
        .to_owned();
    manager.health(&current)?;
    ensure!(
        manager.load(&current)?.phase == "confirmed",
        "final release unconfirmed"
    );
    let env = manager.environment()?;
    ensure!(
        env.get("saved_entry") == Some(&format!("looom-{current}"))
            && env.get("next_entry").is_none_or(String::is_empty),
        "final choice unsettled"
    );
    ensure!(
        !Path::new(STATE).join("private-password-test").exists(),
        "temporary password test not restored"
    );
    let shadow = credentials::read_private(Path::new("/run/looom/accounts/shadow"))?;
    for user in ["root", machine.user.as_str()] {
        let old = credentials::read_private(&private().join(format!("{user}.hash")))?;
        let current = credentials::read_private(
            &Path::new(STATE)
                .join("credentials")
                .join(format!("{user}.hash")),
        )?;
        ensure!(old.trim() == current.trim(), "original credential changed");
        ensure!(
            shadow
                .lines()
                .find(|s| s.split(':').next() == Some(user))
                .and_then(|s| s.split(':').nth(1))
                == Some(current.trim()),
            "runtime credentials differ"
        );
    }
    let update: serde_json::Value = serde_json::from_str(&credentials::read_private(
        &Path::new(STATE).join("bootstrap-update/journal.json"),
    )?)?;
    ensure!(
        update["phase"] == "complete" && update["uki_sha256"] == machine.bootstrap_uki_sha256,
        "bootstrap not completed"
    );
    let root = manager
        .top
        .join(update["subvolume"].as_str().context("bootstrap root")?);
    let mut sources = std::collections::BTreeMap::new();
    for name in [
        "Cargo.toml",
        "Cargo.lock",
        "src/lib.rs",
        "src/main.rs",
        "src/config.rs",
        "src/util.rs",
        "src/machine.rs",
        "src/credentials.rs",
        "src/packages.rs",
        "src/releases.rs",
        "src/builder.rs",
        "src/bootstrap.rs",
        "src/bootstrap_update.rs",
        "configs/bootstrap/packages.txt",
    ] {
        let installed = root.join("usr/lib/looom/source").join(name);
        ensure!(
            fs::read(&installed)? == fs::read(Path::new(STATE).join("dev/src").join(name))?,
            "bootstrap source differs: {name}"
        );
        sources.insert(name, hash_file(&installed)?);
    }
    let bundle: looom::builder::Bundle =
        serde_json::from_slice(&fs::read("/usr/lib/looom/declaration.json")?)?;
    ensure!(
        looom::packages::inventory(None)? == bundle.lock.inventory()
            && hash_file(Path::new("/usr/bin/looom"))?
                == bundle.request["recipe_sha256"]
                    .as_str()
                    .context("manager digest")?,
        "working release changed"
    );
    json(
        &Path::new(BASE).join("public/final-state.json"),
        &serde_json::json!({"schema":1,"running_release":manager.load(&current)?,"grub_environment":env,"bootstrap_update":update,"machine":machine,"original_credentials_preserved":true,"runtime_credentials_match":true,"package_count":bundle.lock.packages.len(),"bootstrap_source_sha256":sources,"working_manager_sha256":hash_file(Path::new("/usr/bin/looom"))?,"failed_units":output("systemctl", &["--failed","--no-legend","--plain"])?,"firmware":output("efibootmgr", &[])?}),
        0o600,
    )?;
    println!(
        "PASS: confirmed working release, completed bootstrap, original credentials/runtime, exact inventory and bootstrap source verified"
    );
    Ok(())
}
fn run() -> Result<()> {
    guard()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    ensure!(args.len() == 1, "prepare|negatives|report|clean-trial-slot");
    match args[0].as_str() {
        "prepare" => prepare(),
        "negatives" => negatives(),
        "report" => report(),
        "clean-trial-slot" => clean_trial_slot(),
        _ => anyhow::bail!("unknown bootstrap acceptance operation"),
    }
}
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("VM bootstrap acceptance: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
