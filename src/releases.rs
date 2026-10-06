use crate::{
    config::Config,
    machine::{Machine, STATE},
    util::*,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::Write,
    path::PathBuf,
    time::UNIX_EPOCH,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Metadata {
    pub schema_version: u32,
    pub id: String,
    pub phase: String,
    pub root_subvolume: String,
    pub kernel_package: String,
    pub kernel_version: String,
    pub root_uuid: String,
    pub esp_uuid: String,
    pub uki_sha256: String,
    pub declarative_value: String,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub engine: Option<String>,
}
#[derive(Clone)]
pub struct Manager {
    pub machine: Machine,
    pub state: PathBuf,
    pub top: PathBuf,
    pub esp: PathBuf,
}
impl Manager {
    pub fn installed(machine: Machine) -> Result<Self> {
        machine.guard()?;
        let top = machine.ensure_top()?;
        Ok(Self {
            machine,
            top,
            state: PathBuf::from(STATE),
            esp: PathBuf::from("/efi"),
        })
    }
    fn grub(&self) -> PathBuf {
        self.esp.join("looom/grub")
    }
    pub fn efi(&self) -> PathBuf {
        self.esp.join("EFI/Linux")
    }
    pub fn lock(&self) -> Result<Lock> {
        Lock::acquire(&self.state.join("release-control.lock"))
    }
    fn record_path(&self, rid: &str) -> Result<PathBuf> {
        ensure!(crate::config::identifier(rid), "invalid release ID");
        Ok(self.state.join("releases").join(format!("{rid}.json")))
    }
    pub fn load(&self, rid: &str) -> Result<Metadata> {
        let metadata: Metadata = serde_json::from_slice(&fs::read(self.record_path(rid)?)?)?;
        ensure!(
            metadata.id == rid
                && metadata.root_subvolume == format!("@root-{rid}")
                && metadata.schema_version == 1,
            "invalid release metadata"
        );
        ensure!(
            metadata.root_uuid == self.machine.root_uuid
                && metadata.esp_uuid == self.machine.esp_uuid,
            "release belongs to a different filesystem"
        );
        ensure!(
            !metadata.kernel_version.is_empty()
                && metadata
                    .kernel_version
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._+-".contains(&c)),
            "invalid kernel metadata"
        );
        Ok(metadata)
    }
    pub fn list(&self) -> Result<Vec<Metadata>> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(self.state.join("releases"))? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                entries.push(
                    self.load(
                        path.file_stem()
                            .and_then(|v| v.to_str())
                            .context("release name")?,
                    )?,
                );
            }
        }
        entries.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(entries)
    }
    pub fn save(&self, metadata: &Metadata) -> Result<()> {
        json(&self.record_path(&metadata.id)?, metadata, 0o600)
    }
    pub fn operation(&self, rid: &str, phase: &str) -> Result<()> {
        ensure!(crate::config::identifier(rid), "invalid ID");
        json(
            &self.state.join("operations").join(format!("{rid}.json")),
            &serde_json::json!({"id":rid,"phase":phase,"engine":"rust-0.2"}),
            0o600,
        )
    }
    pub fn validate(&self, metadata: &Metadata, published: bool) -> Result<()> {
        let root = self.top.join(&metadata.root_subvolume);
        ensure!(
            output("btrfs", &["property", "get", "-ts", string(&root)?, "ro"])? == "ro=true",
            "release root must be read-only"
        );
        ensure!(
            fs::read_to_string(root.join("etc/looom/release-id"))?.trim() == metadata.id
                && root
                    .join("usr/lib/modules")
                    .join(&metadata.kernel_version)
                    .is_dir(),
            "root/kernel does not match metadata"
        );
        ensure!(
            hash_file(&root.join("boot").join(format!("looom-{}.efi", metadata.id)))?
                == metadata.uki_sha256,
            "UKI differs from metadata"
        );
        ensure!(
            fs::read_to_string(root.join("etc/kernel/cmdline"))?
                .split_whitespace()
                .any(|s| s == format!("rootflags=subvol={}", metadata.root_subvolume)),
            "wrong UKI root command line"
        );
        for name in ["shadow", "gshadow"] {
            ensure!(
                fs::read_to_string(root.join("usr/lib/looom/accounts").join(name))?
                    .lines()
                    .all(|l| l.split(':').nth(1) == Some("!")),
                "release contains unlocked credential template"
            );
        }
        if published {
            ensure!(
                hash_file(&self.efi().join(format!("looom-{}.efi", metadata.id)))?
                    == metadata.uki_sha256,
                "published UKI integrity mismatch"
            );
            ensure!(
                fs::read_to_string(self.grub().join("grub.cfg"))?
                    .contains(&format!("--id looom-{} {{", metadata.id)),
                "release absent from boot menu"
            );
        }
        Ok(())
    }
    pub fn environment(&self) -> Result<BTreeMap<String, String>> {
        Ok(output(
            "grub-editenv",
            &[string(&self.grub().join("grubenv"))?, "list"],
        )?
        .lines()
        .filter_map(|s| s.split_once('=').map(|(k, v)| (k.into(), v.into())))
        .collect())
    }
    pub fn set_environment(&self, values: &[String]) -> Result<()> {
        let directory = self.grub();
        let temporary = tempfile::Builder::new()
            .prefix(".grubenv-")
            .tempfile_in(&directory)?;
        fs::copy(directory.join("grubenv"), temporary.path())?;
        let mut args = vec![string(temporary.path())?, "set"];
        args.extend(values.iter().map(String::as_str));
        command("grub-editenv", &args)?;
        temporary.as_file().sync_all()?;
        let check = output("grub-editenv", &[string(temporary.path())?, "list"])?;
        for value in values {
            ensure!(
                check.lines().any(|line| line == value),
                "GRUB environment verification failed"
            );
        }
        temporary
            .persist(directory.join("grubenv"))
            .map_err(|e| e.error)?;
        sync_dir(&directory)
    }
    pub fn menu(&self, include: Option<&str>) -> Result<String> {
        let uuid = &self.machine.esp_uuid;
        let mut text = format!(
            "set timeout=5\nset timeout_style=menu\ninsmod part_gpt\ninsmod fat\ninsmod chain\nsearch --no-floppy --fs-uuid --set=esp {uuid}\n"
        );
        if self.machine.serial_console {
            text.push_str("serial --unit=0 --speed=115200 --word=8 --parity=no --stop=1\nterminal_input console serial\nterminal_output console serial\n");
        }
        text.push_str("if [ -s ($esp)/looom/grub/grubenv ]; then\n load_env -f ($esp)/looom/grub/grubenv\nfi\nif [ \"$next_entry\" ]; then\n set default=\"$next_entry\"\n set next_entry=\n save_env -f ($esp)/looom/grub/grubenv next_entry\nelif [ \"$saved_entry\" ]; then\n set default=\"$saved_entry\"\nelse\n set default=looom-bootstrap\nfi\nmenuentry 'looom bootstrap (recovery)' --id looom-bootstrap {\n chainloader ($esp)/EFI/Linux/looom-bootstrap.efi\n}\n");
        for metadata in self.list()?.iter().filter(|m| {
            matches!(m.phase.as_str(), "published" | "confirmed") || include == Some(m.id.as_str())
        }) {
            self.validate(metadata, false)?;
            ensure!(
                hash_file(&self.efi().join(format!("looom-{}.efi", metadata.id)))?
                    == metadata.uki_sha256,
                "menu UKI integrity mismatch"
            );
            text.push_str(&format!("menuentry 'looom {} ({})' --id looom-{} {{\n chainloader ($esp)/EFI/Linux/looom-{}.efi\n}}\n", metadata.id, metadata.kernel_version, metadata.id, metadata.id));
        }
        Ok(text)
    }
    pub fn write_menu(&self, text: &str) -> Result<()> {
        let directory = self.grub();
        mkdir(&directory, 0o700)?;
        let mut temporary = tempfile::Builder::new()
            .prefix(".grub-menu-")
            .tempfile_in(&directory)?;
        temporary.write_all(text.as_bytes())?;
        temporary.as_file().sync_all()?;
        command("grub-script-check", &[string(temporary.path())?])?;
        let previous = directory.join("grub.cfg");
        if previous.exists() {
            atomic(
                &directory.join("grub.cfg.previous"),
                &fs::read(&previous)?,
                0o600,
            )?;
        }
        temporary.persist(previous).map_err(|e| e.error)?;
        sync_dir(&directory)
    }
    pub fn publish(&self, rid: &str) -> Result<()> {
        let mut metadata = self.load(rid)?;
        ensure!(
            !matches!(metadata.phase.as_str(), "rejected" | "removing" | "removed"),
            "release cannot be published in this phase"
        );
        self.validate(&metadata, false)?;
        mkdir(&self.efi(), 0o700)?;
        let source = self
            .top
            .join(&metadata.root_subvolume)
            .join("boot")
            .join(format!("looom-{rid}.efi"));
        let destination = self.efi().join(format!("looom-{rid}.efi"));
        if !destination.exists() {
            let mut file = tempfile::Builder::new()
                .prefix(".uki-")
                .tempfile_in(self.efi())?;
            std::io::copy(&mut File::open(source)?, &mut file)?;
            file.as_file().sync_all()?;
            ensure!(
                hash_file(file.path())? == metadata.uki_sha256,
                "copied UKI digest mismatch"
            );
            file.persist(&destination).map_err(|e| e.error)?;
            sync_dir(&self.efi())?;
        }
        ensure!(
            hash_file(&destination)? == metadata.uki_sha256,
            "existing published UKI differs"
        );
        failpoint("uki")?;
        self.write_menu(&self.menu(Some(rid))?)?;
        if metadata.phase != "confirmed" {
            metadata.phase = "published".into();
        }
        self.save(&metadata)?;
        self.operation(rid, &metadata.phase)?;
        println!("Published {rid}; permanent boot choice unchanged");
        Ok(())
    }
    pub fn health(&self, rid: &str) -> Result<()> {
        ensure!(
            fs::read_to_string("/etc/looom/release-id")?.trim() == rid,
            "running release differs"
        );
        let metadata = self.load(rid)?;
        ensure!(
            output("findmnt", &["-nro", "FSROOT", "/"])? == format!("/@root-{rid}")
                && output("findmnt", &["-nro", "OPTIONS", "/"])?
                    .split(',')
                    .any(|v| v == "ro"),
            "running root is not the expected read-only subvolume"
        );
        self.machine.guard()?;
        ensure!(
            output("id", &["-u", &self.machine.user])? == self.machine.uid.to_string()
                && output("id", &["-g", &self.machine.user])? == self.machine.gid.to_string(),
            "personal identity changed"
        );
        let declaration: serde_json::Value =
            serde_json::from_slice(&fs::read("/usr/lib/looom/declaration.json")?)?;
        let cfg: Config = serde_json::from_value(declaration["config"].clone())?;
        self.machine.config_contract(&cfg)?;
        for path in ["/etc/looom-local", "/etc/NetworkManager/system-connections"] {
            ensure!(
                output("findmnt", &["-nro", "UUID", path])? == self.machine.root_uuid
                    && output("findmnt", &["-nro", "OPTIONS", path])?
                        .split(',')
                        .any(|v| v == "rw"),
                "persistent local mount failed"
            );
        }
        let mut required: Vec<&str> = vec![
            "looom-accounts.service",
            "sshd.service",
            "NetworkManager.service",
            "systemd-timesyncd.service",
        ];
        if self.machine.guest_agent {
            required.push("qemu-guest-agent.service");
        }
        if cfg.desktop.environment == "plasma" {
            required.push("sddm.service");
        }
        required.extend(cfg.health.required_units.iter().map(String::as_str));
        for name in required {
            ensure!(
                output("systemctl", &["is-active", name])? == "active",
                "required unit inactive: {name}"
            );
        }
        ensure!(
            output("systemctl", &["--failed", "--no-legend", "--plain"])?.is_empty(),
            "failed system units"
        );
        ensure!(
            output("uname", &["-r"])? == metadata.kernel_version,
            "running kernel differs"
        );
        self.validate(&metadata, true)
    }
    pub fn select(&self, op: &str, rid: &str) -> Result<()> {
        let mut metadata = self.load(rid)?;
        ensure!(
            matches!(metadata.phase.as_str(), "published" | "confirmed"),
            "release must be published"
        );
        self.validate(&metadata, true)?;
        match op {
            "try" => {
                self.set_environment(&[format!("next_entry=looom-{rid}")])?;
                println!("Next boot only: {rid}");
            }
            "rollback" => {
                ensure!(
                    metadata.phase == "confirmed",
                    "rollback requires previously confirmed release"
                );
                self.set_environment(&[
                    format!("saved_entry=looom-{rid}"),
                    format!("next_entry=looom-{rid}"),
                ])?;
                println!("Rollback selected; reboot required: {rid}");
            }
            "confirm" => {
                self.health(rid)?;
                ensure!(
                    self.environment()?
                        .get("next_entry")
                        .is_none_or(String::is_empty),
                    "trial selection was not consumed by bootloader"
                );
                self.set_environment(&[format!("saved_entry=looom-{rid}")])?;
                metadata.phase = "confirmed".into();
                self.save(&metadata)?;
                self.operation(rid, "confirmed")?;
                println!("Confirmed healthy running release: {rid}");
            }
            _ => anyhow::bail!("invalid selection operation"),
        }
        Ok(())
    }
    pub fn reject(&self, rid: &str) -> Result<()> {
        let mut metadata = self.load(rid)?;
        ensure!(
            fs::read_to_string("/etc/looom/release-id")
                .unwrap_or_default()
                .trim()
                != rid
                && metadata.phase != "confirmed",
            "cannot reject running/confirmed release"
        );
        ensure!(
            !self
                .environment()?
                .values()
                .any(|v| v == &format!("looom-{rid}")),
            "cannot reject selected release"
        );
        metadata.phase = "rejected".into();
        self.save(&metadata)?;
        self.operation(rid, "rejected")?;
        self.write_menu(&self.menu(None)?)?;
        println!("Rejected candidate retained for diagnosis: {rid}");
        Ok(())
    }
    pub fn status(&self) -> Result<()> {
        println!(
            "Running: {}",
            fs::read_to_string("/etc/looom/release-id")
                .unwrap_or_else(|_| "bootstrap".into())
                .trim()
        );
        println!("GRUB: {}", serde_json::to_string(&self.environment()?)?);
        for metadata in self.list()? {
            println!(
                "{} {} {} {}",
                metadata.id, metadata.phase, metadata.kernel_version, metadata.root_subvolume
            );
        }
        Ok(())
    }
    pub fn gc(&self, keep: usize, apply: bool) -> Result<()> {
        ensure!(keep >= 2, "retain at least two confirmed releases");
        let entries = self.list()?;
        let mut confirmed: Vec<_> = entries.iter().filter(|m| m.phase == "confirmed").collect();
        confirmed.sort_by_key(|m| (m.created_at, m.id.clone()));
        let mut protected: BTreeSet<String> = confirmed
            .iter()
            .rev()
            .take(keep)
            .map(|m| m.id.clone())
            .collect();
        protected.insert(
            fs::read_to_string("/etc/looom/release-id")
                .unwrap_or_default()
                .trim()
                .into(),
        );
        for value in self.environment()?.values() {
            if let Some(id) = value.strip_prefix("looom-") {
                protected.insert(id.into());
            }
        }
        for mut metadata in entries {
            if protected.contains(&metadata.id)
                || !matches!(
                    metadata.phase.as_str(),
                    "confirmed" | "rejected" | "validated" | "removing"
                )
            {
                continue;
            }
            println!("GC candidate: {} ({})", metadata.id, metadata.phase);
            if apply {
                metadata.phase = "removing".into();
                self.save(&metadata)?;
                self.operation(&metadata.id, "removing")?;
                self.write_menu(&self.menu(None)?)?;
                self.finish_removal(&metadata)?;
            }
        }
        Ok(())
    }
    pub fn finish_removal(&self, metadata: &Metadata) -> Result<()> {
        ensure!(metadata.phase == "removing", "not a removal operation");
        ensure!(
            !self
                .environment()?
                .values()
                .any(|v| v == &format!("looom-{}", metadata.id))
                && fs::read_to_string("/etc/looom/release-id")
                    .unwrap_or_default()
                    .trim()
                    != metadata.id,
            "cannot remove selected/running release"
        );
        let root = self.top.join(&metadata.root_subvolume);
        if root.exists() {
            command(
                "btrfs",
                &["subvolume", "delete", "--commit-after", string(&root)?],
            )?;
        }
        remove_if_exists(&self.efi().join(format!("looom-{}.efi", metadata.id)))?;
        sync_dir(&self.efi())?;
        self.operation(&metadata.id, "removed")?;
        fs::remove_file(self.record_path(&metadata.id)?)?;
        sync_dir(&self.state.join("releases"))?;
        // Frozen inputs and evidence remain for reproducibility, never garbage collect credentials.
        Ok(())
    }
}
pub fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |v| v.as_secs())
}
