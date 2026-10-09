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
const BASE: &str = "/var/lib/looom/dev/gc-20261009";
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
fn update_record() -> Result<PathBuf> {
    let pointer = Path::new(STATE).join("bootstrap-current.json");
    if pointer.exists() {
        let id: String = serde_json::from_str(&credentials::read_private(&pointer)?)?;
        ensure!(looom::config::identifier(&id), "invalid generation pointer");
        Ok(Path::new(STATE)
            .join("bootstrap-history")
            .join(id)
            .join("journal.json"))
    } else {
        Ok(Path::new(STATE).join("bootstrap-update/journal.json"))
    }
}
fn audit() -> Result<()> {
    let machine = Machine::load()?;
    let mut secrets = Vec::new();
    for user in ["root", machine.user.as_str()] {
        secrets.push(credentials::read_private(
            &private().join(format!("{user}.hash")),
        )?);
        secrets.push(credentials::read_private(
            &Path::new(STATE)
                .join("credentials")
                .join(format!("{user}.hash")),
        )?);
    }
    let temporary = Path::new(STATE).join("private-password-test");
    if temporary.exists() {
        for name in ["original.hash", "password.secret"] {
            secrets.push(credentials::read_private(&temporary.join(name))?);
        }
    }
    let mut count = 0;
    for entry in fs::read_dir(Path::new(BASE).join("public"))? {
        let path = entry?.path();
        owned_regular(&path)?;
        let bytes = fs::read(&path)?;
        for secret in &secrets {
            ensure!(
                !bytes
                    .windows(secret.trim().len())
                    .any(|v| v == secret.trim().as_bytes()),
                "private credential leaked into public artifact"
            );
        }
        let text = String::from_utf8_lossy(&bytes);
        ensure!(
            !text.contains("-----BEGIN OPENSSH PRIVATE KEY-----"),
            "private access material in public artifact"
        );
        count += 1;
    }
    println!(
        "PASS: {count} public artifacts contain no inspected private values or SSH private key"
    );
    Ok(())
}
fn fixture() -> Result<()> {
    let manager = Manager::installed(Machine::load()?)?;
    let _lock = manager.lock()?;
    let id = "gc-powercut-sync";
    let root = manager.top.join(format!("@root-{id}"));
    let record = manager.state.join("releases").join(format!("{id}.json"));
    let image = manager.efi().join(format!("looom-{id}.efi"));
    ensure!(
        !root.exists() && !record.exists() && !image.exists(),
        "fixture already exists"
    );
    command("btrfs", &["subvolume", "create", string(&root)?])?;
    atomic(
        &image,
        b"GC fixture, intentionally not a bootable EFI image",
        0o600,
    )?;
    let metadata = looom::releases::Metadata {
        schema_version: 1,
        id: id.into(),
        phase: "validated".into(),
        root_subvolume: format!("@root-{id}"),
        kernel_package: "linux".into(),
        kernel_version: output("uname", &["-r"])?,
        root_uuid: manager.machine.root_uuid.clone(),
        esp_uuid: manager.machine.esp_uuid.clone(),
        uki_sha256: hash_file(&image)?,
        declarative_value: id.into(),
        created_at: looom::releases::timestamp(),
        engine: Some("rust-0.2".into()),
    };
    manager.save(&metadata)?;
    manager.operation(id, "validated")?;
    ensure!(
        !manager
            .menu(None)?
            .contains("--id looom-gc-powercut-sync {"),
        "fixture entered boot menu"
    );
    println!("PASS: uniquely owned unpublished GC power-cut fixture created");
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
    let record = update_record()?;
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
        "generation-traversal",
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
            "generation-traversal" => changed["generation"] = "../escape".into(),
            "source-traversal" => changed["source"] = "../escape".into(),
            "subvolume-traversal" => changed["subvolume"] = "../escape".into(),
            "schema" => changed["schema"] = 99.into(),
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
    let pointer = Path::new(STATE).join("bootstrap-current.json");
    let pointer_data = credentials::read_private(&pointer)?;
    let _pointer_restore = Restore {
        path: pointer.clone(),
        data: pointer_data.as_bytes().into(),
    };
    for bad in ["../escape", "missing-generation"] {
        json(&pointer, &bad, 0o600)?;
        ensure!(
            !succeeds(string(&executable)?, &["bootstrap-recover"]),
            "unsafe generation pointer accepted"
        );
        ensure!(
            hash_file(&Path::new(STATE).join("machine.json"))? == before_profile
                && hash_file(Path::new("/efi/EFI/Linux/looom-bootstrap.efi"))? == before_uki
                && output("grub-editenv", &["/efi/looom/grub/grubenv", "list"])? == before_env,
            "pointer negative case changed system"
        );
        println!("PASS: rejected generation pointer {bad}");
    }
    atomic(&pointer, pointer_data.as_bytes(), 0o600)?;
    ensure!(
        !succeeds(string(&executable)?, &["bootstrap-confirm"]),
        "confirmation without actual candidate boot accepted"
    );
    println!("PASS: candidate confirmation refused from working release");
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
    let update: serde_json::Value =
        serde_json::from_str(&credentials::read_private(&update_record()?)?)?;
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
    let mut generations = Vec::new();
    for entry in fs::read_dir(Path::new(STATE).join("bootstrap-history"))? {
        let path = entry?.path().join("journal.json");
        let generation: serde_json::Value =
            serde_json::from_str(&credentials::read_private(&path)?)?;
        generations.push(generation);
    }
    json(
        &Path::new(BASE).join("public/bootstrap-history.json"),
        &generations,
        0o600,
    )?;
    json(
        &Path::new(BASE).join("public/release-registry.json"),
        &manager.list()?,
        0o600,
    )?;
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
    ensure!(args.len() == 1, "prepare|negatives|report|audit|fixture");
    match args[0].as_str() {
        "prepare" => prepare(),
        "fixture" => fixture(),
        "negatives" => negatives(),
        "report" => report(),
        "audit" => audit(),
        _ => anyhow::bail!("unknown bootstrap acceptance operation"),
    }
}
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("VM GC acceptance: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
