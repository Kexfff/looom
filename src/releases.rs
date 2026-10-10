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
    io::{Read, Write},
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
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Publication {
    id: String,
    uki_sha256: String,
    pending_uki: String,
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
        let path = self.record_path(rid)?;
        crate::credentials::trusted_dir(path.parent().context("release metadata parent")?, false)?;
        owned_regular(&path)?;
        let metadata: Metadata = serde_json::from_slice(&fs::read(path)?)?;
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
            let (path, marker) = if self.machine.bootloader == crate::machine::Bootloader::Limine {
                (
                    crate::limine::config_path(self),
                    format!("/looom-{}\n", metadata.id),
                )
            } else {
                (
                    self.grub().join("grub.cfg"),
                    format!("--id looom-{} {{", metadata.id),
                )
            };
            ensure!(
                fs::read_to_string(path)?.contains(&marker),
                "release absent from boot menu"
            );
        }
        Ok(())
    }
    pub fn environment(&self) -> Result<BTreeMap<String, String>> {
        if self.machine.bootloader == crate::machine::Bootloader::Limine {
            return crate::limine::environment(self);
        }
        Ok(output(
            "grub-editenv",
            &[string(&self.grub().join("grubenv"))?, "list"],
        )?
        .lines()
        .filter_map(|s| s.split_once('=').map(|(k, v)| (k.into(), v.into())))
        .collect())
    }
    pub fn set_environment(&self, values: &[String]) -> Result<()> {
        if self.machine.bootloader == crate::machine::Bootloader::Limine {
            return crate::limine::set_environment(self, values);
        }
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
        if self.machine.bootloader == crate::machine::Bootloader::Limine {
            return crate::limine::menu(self, include);
        }
        let uuid = &self.machine.esp_uuid;
        let mut text = format!(
            "set timeout=5\nset timeout_style=menu\ninsmod part_gpt\ninsmod fat\ninsmod chain\nsearch --no-floppy --fs-uuid --set=esp {uuid}\n"
        );
        if self.machine.serial_console {
            text.push_str("serial --unit=0 --speed=115200 --word=8 --parity=no --stop=1\nterminal_input console serial\nterminal_output console serial\n");
        }
        text.push_str("if [ -s ($esp)/looom/grub/grubenv ]; then\n load_env -f ($esp)/looom/grub/grubenv\nfi\nif [ \"$next_entry\" ]; then\n set default=\"$next_entry\"\n set next_entry=\n save_env -f ($esp)/looom/grub/grubenv next_entry\nelif [ \"$saved_entry\" ]; then\n set default=\"$saved_entry\"\nelse\n set default=looom-bootstrap\nfi\nmenuentry 'looom bootstrap (recovery)' --id looom-bootstrap {\n chainloader ($esp)/EFI/Linux/looom-bootstrap.efi\n}\n");
        text.push_str(&crate::bootstrap_update::menu_entries(self)?);
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
        if self.machine.bootloader == crate::machine::Bootloader::Limine {
            return crate::limine::write_menu(self, text);
        }
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
        self.clean_publication(rid)?;
        mkdir(&self.efi(), 0o700)?;
        let source = self
            .top
            .join(&metadata.root_subvolume)
            .join("boot")
            .join(format!("looom-{rid}.efi"));
        let destination = self.efi().join(format!("looom-{rid}.efi"));
        if destination.exists() && hash_file(&destination)? != metadata.uki_sha256 {
            // FAT recovery can retain the renamed UKI with a damaged cluster
            // chain. Only an unselected, uncommitted candidate can be replaced;
            // published/confirmed releases require explicit diagnosis.
            ensure!(
                metadata.phase == "validated"
                    && !self
                        .environment()?
                        .values()
                        .any(|v| v == &format!("looom-{rid}")),
                "damaged published/selected UKI requires explicit recovery"
            );
            use std::os::unix::fs::MetadataExt;
            let info = fs::symlink_metadata(&destination)?;
            ensure!(info.is_file() && info.uid() == 0, "untrusted damaged UKI");
            fs::remove_file(&destination)?;
            sync_dir(&self.efi())?;
            println!("Recreating damaged unselected candidate UKI: {rid}");
        }
        if !destination.exists() {
            let mut file = tempfile::Builder::new()
                .prefix(".uki-")
                .tempfile_in(self.efi())?;
            let journals = self.state.join("publications");
            mkdir(&journals, 0o700)?;
            json(
                &journals.join(format!("{rid}.json")),
                &Publication {
                    id: rid.into(),
                    uki_sha256: metadata.uki_sha256.clone(),
                    pending_uki: file
                        .path()
                        .file_name()
                        .context("pending UKI name")?
                        .to_str()
                        .context("pending UKI name")?
                        .into(),
                },
                0o600,
            )?;
            let mut source = File::open(source)?;
            let mut buffer = vec![0; 1024 * 1024];
            let mut copied = 0;
            loop {
                let count = source.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                file.write_all(&buffer[..count])?;
                copied += count;
                if copied == 16 * 1024 * 1024 {
                    failpoint("uki-copy")?;
                }
            }
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
        failpoint("menu")?;
        if metadata.phase != "confirmed" {
            metadata.phase = "published".into();
        }
        self.save(&metadata)?;
        self.operation(rid, &metadata.phase)?;
        self.clean_publication(rid)?;
        println!("Published {rid}; permanent boot choice unchanged");
        Ok(())
    }
    fn clean_publication(&self, rid: &str) -> Result<()> {
        let journal = self.state.join("publications").join(format!("{rid}.json"));
        if !journal.exists() {
            return Ok(());
        }
        let pending: Publication = serde_json::from_slice(&fs::read(&journal)?)?;
        ensure!(
            pending.id == rid && pending.uki_sha256 == self.load(rid)?.uki_sha256,
            "publication journal identity mismatch"
        );
        ensure!(
            pending.pending_uki.starts_with(".uki-")
                && pending.pending_uki.len() <= 64
                && pending
                    .pending_uki
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)),
            "invalid pending UKI name"
        );
        let path = self.efi().join(&pending.pending_uki);
        match fs::symlink_metadata(&path) {
            Ok(info) => {
                use std::os::unix::fs::MetadataExt;
                ensure!(info.is_file() && info.uid() == 0, "untrusted pending UKI");
                fs::remove_file(path)?;
                sync_dir(&self.efi())?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        fs::remove_file(journal)?;
        sync_dir(&self.state.join("publications"))
    }
    pub fn recover_publications(&self) -> Result<()> {
        let directory = self.state.join("publications");
        if !directory.exists() {
            return Ok(());
        }
        for item in fs::read_dir(directory)? {
            let path = item?.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                let id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .context("publication ID")?;
                ensure!(crate::config::identifier(id), "invalid publication ID");
                self.clean_publication(id)?;
            }
        }
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
        if cfg.desktop.graphical() {
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
        println!(
            "Bootloader {:?}: {}",
            self.machine.bootloader,
            serde_json::to_string(&self.environment()?)?
        );
        for metadata in self.list()? {
            println!(
                "{} {} {} {}",
                metadata.id, metadata.phase, metadata.kernel_version, metadata.root_subvolume
            );
        }
        Ok(())
    }
    pub fn mounted_subvolume(&self, subvolume: &str) -> Result<bool> {
        // A bind from State or a tmpfs mounted inside the root has a different
        // FSROOT. Protect its target path before checking Btrfs source aliases.
        let target = self.top.join(subvolume);
        let target = string(&target)?;
        let targets = output("findmnt", &["-rn", "-o", "TARGET"])?;
        if targets
            .lines()
            .any(|path| path == target || path.starts_with(&format!("{target}/")))
        {
            return Ok(true);
        }
        let prefix = format!("/{subvolume}");
        let mounts = output("findmnt", &["-rn", "-t", "btrfs", "-o", "UUID,FSROOT"])?;
        Ok(mounts.lines().any(|line| {
            let mut fields = line.split_whitespace();
            fields.next() == Some(self.machine.root_uuid.as_str())
                && fields
                    .next()
                    .is_some_and(|root| root == prefix || root.starts_with(&format!("{prefix}/")))
        }))
    }
    pub fn delete_subvolume(&self, subvolume: &str) -> Result<()> {
        ensure!(
            subvolume
                .strip_prefix("@root-")
                .or_else(|| subvolume.strip_prefix("@bootstrap-"))
                .is_some_and(crate::config::identifier),
            "invalid managed root"
        );
        ensure!(
            !self.mounted_subvolume(subvolume)?,
            "cannot remove mounted root: {subvolume}"
        );
        crate::credentials::trusted_dir(&self.top, false)?;
        let root = self.top.join(subvolume);
        match fs::symlink_metadata(&root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
            Ok(metadata) => {
                use std::os::unix::fs::MetadataExt;
                ensure!(
                    metadata.is_dir() && metadata.uid() == 0 && metadata.mode() & 0o022 == 0,
                    "unsafe managed root"
                );
            }
        }
        ensure!(
            output("findmnt", &["-nro", "UUID", "-T", string(&root)?])? == self.machine.root_uuid,
            "root belongs to another filesystem"
        );
        command("btrfs", &["subvolume", "show", string(&root)?])?;
        command(
            "btrfs",
            &["subvolume", "delete", "--commit-after", string(&root)?],
        )?;
        sync_dir(&self.top)?;
        Ok(())
    }
    fn gc_protected(&self, keep: usize) -> Result<BTreeSet<String>> {
        let entries = self.list()?;
        let mut confirmed: Vec<_> = entries.iter().filter(|m| m.phase == "confirmed").collect();
        confirmed.sort_by_key(|m| (m.created_at, m.id.clone()));
        let mut protected: BTreeSet<String> = confirmed
            .iter()
            .rev()
            .take(keep)
            .map(|m| m.id.clone())
            .collect();
        // I/O failure must not silently turn a working root into an eligible one.
        match fs::read_to_string("/etc/looom/release-id") {
            Ok(id) => {
                protected.insert(id.trim().into());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        for value in self.environment()?.values() {
            if let Some(id) = value.strip_prefix("looom-") {
                protected.insert(id.into());
            }
        }
        for metadata in &entries {
            if self.mounted_subvolume(&metadata.root_subvolume)? {
                protected.insert(metadata.id.clone());
            }
        }
        protected.extend(crate::bootstrap_update::protected_sources(self)?);
        for entry in fs::read_dir(self.state.join("operations"))? {
            let path = entry?.path();
            if path.extension().and_then(|v| v.to_str()) != Some("json") {
                continue;
            }
            owned_regular(&path)?;
            let operation: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
            if operation["phase"] == "configured" {
                let id = operation["id"]
                    .as_str()
                    .context("configured operation ID")?;
                ensure!(
                    crate::config::identifier(id),
                    "invalid configured operation ID"
                );
                protected.insert(id.into());
            }
        }
        Ok(protected)
    }
    pub fn gc(&self, keep: usize, apply: bool) -> Result<()> {
        ensure!(keep >= 2, "retain at least two confirmed releases");
        // Validate bootstrap history before deleting any ordinary release.
        crate::bootstrap_update::gc(self, false)?;
        let protected = self.gc_protected(keep)?;
        for mut metadata in self.list()? {
            if protected.contains(&metadata.id) {
                println!("GC retain release: {}", metadata.id);
                continue;
            }
            // Published candidates require explicit reject, allowing a user to
            // prepare a trial without losing it to an unrelated GC invocation.
            if !matches!(
                metadata.phase.as_str(),
                "confirmed" | "rejected" | "validated" | "removing"
            ) {
                continue;
            }
            println!("GC candidate: {} ({})", metadata.id, metadata.phase);
            if apply {
                metadata.phase = "removing".into();
                self.save(&metadata)?;
                self.operation(&metadata.id, "removing")?;
                failpoint("gc-journal")?;
                self.finish_removal(&metadata)?;
            }
        }
        if apply {
            crate::bootstrap_update::gc(self, true)?;
        }
        println!(
            "GC {} complete; frozen inputs, evidence and persistent data retained",
            if apply { "apply" } else { "preview" }
        );
        Ok(())
    }
    pub fn finish_removal(&self, metadata: &Metadata) -> Result<()> {
        let metadata = self.load(&metadata.id)?;
        ensure!(metadata.phase == "removing", "not a removal operation");
        ensure!(
            !self.gc_protected(2)?.contains(&metadata.id),
            "cannot remove protected release: {}",
            metadata.id
        );
        // Remove the menu entry durably before deleting either root or image.
        self.write_menu(&self.menu(None)?)?;
        command("sync", &["-f", string(&self.esp)?])?;
        failpoint("gc-menu")?;
        self.clean_publication(&metadata.id)?;
        self.delete_subvolume(&metadata.root_subvolume)?;
        failpoint("gc-root")?;
        let image = self.efi().join(format!("looom-{}.efi", metadata.id));
        if image.try_exists()? {
            owned_regular(&image)?;
            fs::remove_file(image)?;
        }
        sync_dir(&self.efi())?;
        command("sync", &["-f", string(&self.esp)?])?;
        failpoint("gc-uki")?;
        self.operation(&metadata.id, "removed")?;
        owned_regular(&self.record_path(&metadata.id)?)?;
        fs::remove_file(self.record_path(&metadata.id)?)?;
        sync_dir(&self.state.join("releases"))?;
        Ok(())
    }
}
pub fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |v| v.as_secs())
}
