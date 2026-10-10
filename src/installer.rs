//! Whole-disk installer: a reviewed nonsecret plan, then a journaled engine.
//! The terminal wizard and future graphical adapters use the same engine.
mod engine;

use crate::{config, credentials, util::*};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    io::{self, BufRead, Read, Write},
    path::{Path, PathBuf},
};

const GIB: u64 = 1024 * 1024 * 1024;
const TEMPLATE: &str = include_str!("../configs/installer/base.yaml");

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionDiskIdentity {
    pub boot_id: String,
    pub sysfs_path: String,
    pub diskseq: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Disk {
    pub path: String,
    pub size: u64,
    pub model: String,
    pub serial: String,
    pub wwn: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_identity: Option<SessionDiskIdentity>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub schema: u32,
    pub disk: Disk,
    pub original_layout_sha256: String,
    pub recipe_sha256: String,
    pub disk_guid: String,
    pub esp_partuuid: String,
    pub root_partuuid: String,
    pub root_uuid: String,
    pub esp_uuid: String,
    pub release: String,
    pub config: config::Config,
    pub ssh_public_key: Option<String>,
    pub register_firmware: bool,
}

fn text_field(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or("").trim().to_owned()
}
fn device_tree(path: &str) -> Result<Value> {
    let tree: Value = serde_json::from_str(&output(
        "lsblk",
        &[
            "--json",
            "--bytes",
            "--paths",
            "--output",
            "NAME,PATH,TYPE,SIZE,MODEL,SERIAL,WWN,RO,MAJ:MIN,MOUNTPOINTS,FSTYPE,UUID,PARTUUID,PARTTYPE",
            path,
        ],
    )?)?;
    let devices = tree["blockdevices"].as_array().context("lsblk devices")?;
    ensure!(devices.len() == 1, "one disk required");
    Ok(devices[0].clone())
}
fn disk_from_tree(tree: &Value) -> Result<Disk> {
    ensure!(
        tree["type"] == "disk" && tree["ro"] == false,
        "writable whole disk required"
    );
    let mut disk = Disk {
        path: text_field(tree, "path"),
        size: tree["size"].as_u64().context("disk size")?,
        model: text_field(tree, "model"),
        serial: text_field(tree, "serial"),
        wwn: text_field(tree, "wwn"),
        session_identity: None,
    };
    ensure!(
        disk.size >= 32 * GIB,
        "at least 32 GiB required (64 GiB recommended for Plasma)"
    );
    if disk.serial.is_empty() && disk.wwn.is_empty() {
        ensure!(
            output("systemd-detect-virt", &["--vm"])
                .is_ok_and(|kind| !kind.is_empty() && kind != "none"),
            "physical disk has no serial/WWN; a persistent identifier is required"
        );
        // An unlabelled VM disk is safe only within this live boot. diskseq
        // changes on removal/re-attachment, even if the path and size are reused.
        let sysfs = Path::new("/sys/class/block")
            .join(Path::new(&disk.path).file_name().context("disk name")?);
        let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id")?
            .trim()
            .to_owned();
        ensure!(uuid_valid(&boot_id), "invalid live session identity");
        let diskseq: u64 = fs::read_to_string(sysfs.join("diskseq"))?
            .trim()
            .parse()
            .context("virtual disk sequence unavailable; configure a serial")?;
        ensure!(
            diskseq > 0,
            "invalid virtual disk sequence; configure a serial"
        );
        disk.session_identity = Some(SessionDiskIdentity {
            boot_id,
            sysfs_path: string(&fs::canonicalize(sysfs)?)?.to_owned(),
            diskseq,
        });
    }
    Ok(disk)
}
fn each_device(tree: &Value, f: &mut impl FnMut(&Value) -> Result<()>) -> Result<()> {
    f(tree)?;
    if let Some(children) = tree["children"].as_array() {
        for child in children {
            each_device(child, f)?;
        }
    }
    Ok(())
}
/// Reject root/ESP/data disks, swaps, LVM/crypt/RAID holders and aliases.
/// Resume permits only our own target mounts; it never authorizes a format.
fn unused(tree: &Value, workspace: Option<&Path>) -> Result<()> {
    let mount_list: Value = serde_json::from_str(&output(
        "findmnt",
        &["--json", "--list", "--output", "TARGET,UUID"],
    )?)?;
    let swaps = output(
        "swapon",
        &["--show", "--noheadings", "--raw", "--output", "NAME"],
    )?;
    each_device(tree, &mut |device| {
        let path = text_field(device, "path");
        ensure!(
            ["disk", "part"].contains(&text_field(device, "type").as_str()),
            "mapped/RAID device on selected disk"
        );
        let holders = Path::new("/sys/class/block")
            .join(Path::new(&path).file_name().context("device name")?)
            .join("holders");
        ensure!(
            fs::read_dir(holders)?.next().is_none(),
            "selected disk has active holders"
        );
        for swap in swaps.lines() {
            ensure!(
                fs::canonicalize(swap).ok() != Some(PathBuf::from(&path)),
                "selected disk contains active swap"
            );
        }
        // Use mountinfo as well as lsblk: bind aliases are not reliably shown by lsblk.
        let number = text_field(device, "maj:min");
        let uuid = text_field(device, "uuid");
        let mut targets = Vec::new();
        for line in fs::read_to_string("/proc/self/mountinfo")?.lines() {
            let fields: Vec<_> = line.split_whitespace().collect();
            // Btrfs reports an anonymous superblock number, not the block dev_t.
            // Compare the source as well, including /dev/disk/by-* aliases.
            let source = fields
                .iter()
                .position(|f| *f == "-")
                .and_then(|i| fields.get(i + 2));
            let on_device = source
                .and_then(|s| fs::canonicalize(s).ok())
                .is_some_and(|p| p == Path::new(&path));
            if on_device || fields.get(2) == Some(&number.as_str()) {
                let target = fields.get(4).context("mountinfo target")?;
                ensure!(
                    !target.contains('\\'),
                    "escaped disk mount requires manual diagnosis"
                );
                targets.push((*target).to_owned());
            }
        }
        if !uuid.is_empty() {
            for entry in mount_list["filesystems"].as_array().context("mount list")? {
                if text_field(entry, "uuid").eq_ignore_ascii_case(&uuid) {
                    targets.push(text_field(entry, "target"));
                }
            }
        }
        for target in targets {
            let allowed = workspace.is_some_and(|w| {
                Path::new(&target).starts_with(w.join("target"))
                    || Path::new(&target).starts_with(w.join("top"))
            });
            ensure!(allowed, "selected disk is mounted at {target}");
        }
        Ok(())
    })
}
fn exclusive_disk(path: &str) -> Result<()> {
    use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_EXCL | libc::O_NOFOLLOW)
        .open(path)
        .context("disk is busy/inaccessible, including mounts outside this namespace")?;
    ensure!(
        file.metadata()?.file_type().is_block_device(),
        "block device required"
    );
    Ok(())
}
fn canonical_disk(path: &Path) -> Result<String> {
    let path = fs::canonicalize(path)?;
    ensure!(
        path.parent() == Some(Path::new("/dev")),
        "plain disk under /dev required"
    );
    Ok(string(&path)?.to_owned())
}
fn uuid() -> Result<String> {
    Ok(fs::read_to_string("/proc/sys/kernel/random/uuid")?
        .trim()
        .to_owned())
}
fn uuid_valid(v: &str) -> bool {
    v.len() == 36
        && v.bytes().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}
fn public_key(path: &Path) -> Result<String> {
    let value = fs::read_to_string(path)?;
    ensure!(value.len() <= 4096, "SSH public key too large");
    validate_key(value.trim())?;
    Ok(value.trim().to_owned())
}
fn validate_key(value: &str) -> Result<()> {
    ensure!(
        !value.contains(['\n', '\r', '\0']) && value.len() <= 4096,
        "one SSH public key required"
    );
    let parts: Vec<_> = value.split_whitespace().collect();
    ensure!(
        parts.len() >= 2
            && ["ssh-ed25519", "ssh-rsa", "ecdsa-sha2-nistp256"].contains(&parts[0])
            && parts[1]
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"+/=".contains(&c)),
        "invalid SSH public key (options are unsupported)"
    );
    let mut input = tempfile::NamedTempFile::new()?;
    writeln!(input, "{value}")?;
    ensure!(
        succeeds("ssh-keygen", &["-l", "-f", string(input.path())?]),
        "invalid SSH key encoding"
    );
    Ok(())
}
fn private_directory(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "absolute workspace path required");
    if path.exists() {
        credentials::trusted_dir(path, true)?;
        return Ok(());
    }
    let parent = path.parent().context("workspace parent")?;
    credentials::trusted_dir(parent, false)?;
    fs::create_dir(path)?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    credentials::trusted_dir(path, true)?;
    sync_dir(parent)
}

pub fn create_plan(
    disk: &Path,
    base: &Path,
    work: &Path,
    key: Option<&Path>,
    firmware: bool,
) -> Result<Plan> {
    root()?;
    let (cfg, _) = config::load(base)?;
    ensure!(
        cfg.files.is_empty() && cfg.kernel.extra_command_line.is_empty(),
        "installer MVP accepts only the base profile, without custom files/cmdline"
    );
    ensure!(
        cfg.accounts.user.groups == ["wheel"] && cfg.accounts.user.shell == "/bin/bash",
        "installer MVP requires wheel and /bin/bash"
    );
    let path = canonical_disk(disk)?;
    let tree = device_tree(&path)?;
    unused(&tree, None)?;
    exclusive_disk(&path)?;
    let plan = Plan {
        schema: 1,
        disk: disk_from_tree(&tree)?,
        original_layout_sha256: hash(&serde_json::to_vec(&tree)?),
        recipe_sha256: hash_file(&std::env::current_exe()?)?,
        disk_guid: uuid()?,
        esp_partuuid: uuid()?,
        root_partuuid: uuid()?,
        root_uuid: uuid()?,
        esp_uuid: {
            let random = uuid()?.replace('-', "").to_ascii_uppercase();
            format!("{}-{}", &random[..4], &random[4..8])
        },
        release: "initial".into(),
        config: cfg,
        ssh_public_key: key.map(public_key).transpose()?,
        register_firmware: firmware,
    };
    ensure!(
        serde_json::to_vec_pretty(&plan)?.len() < 16384,
        "installer plan exceeds 16 KiB"
    );
    private_directory(work)?;
    ensure!(
        !work.join("plan.json").exists() && !work.join("journal.json").exists(),
        "workspace already has an installation; use resume"
    );
    let _lock = Lock::acquire(&work.join("installer.lock"))?;
    json(&work.join("base.yaml"), &plan.config, 0o600)?; // JSON is a YAML subset.
    json(&work.join("plan.json"), &plan, 0o600)?;
    validate(&plan, work)?;
    show(&plan, work);
    Ok(plan)
}
fn validate(plan: &Plan, work: &Path) -> Result<()> {
    ensure!(
        plan.schema == 1 && plan.release == "initial" && plan.disk.size >= 32 * GIB,
        "invalid installer schema/profile"
    );
    ensure!(
        [
            &plan.disk_guid,
            &plan.esp_partuuid,
            &plan.root_partuuid,
            &plan.root_uuid
        ]
        .iter()
        .all(|v| uuid_valid(v)),
        "invalid planned UUID"
    );
    ensure!(
        plan.esp_uuid.len() == 9
            && plan.esp_uuid.bytes().enumerate().all(|(i, c)| if i == 4 {
                c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }),
        "invalid FAT UUID"
    );
    ensure!(
        [&plan.recipe_sha256, &plan.original_layout_sha256]
            .iter()
            .all(|v| v.len() == 64 && v.bytes().all(|c| c.is_ascii_hexdigit())),
        "invalid plan digest"
    );
    ensure!(
        plan.recipe_sha256 == hash_file(&std::env::current_exe()?)?,
        "installer binary differs from saved plan"
    );
    if let Some(identity) = &plan.disk.session_identity {
        ensure!(
            fs::read_to_string("/proc/sys/kernel/random/boot_id")?.trim() == identity.boot_id,
            "virtual disk without serial/WWN: this plan belongs to a previous live boot; resume requires the same session"
        );
    }
    ensure!(
        canonical_disk(Path::new(&plan.disk.path))? == plan.disk.path
            && disk_from_tree(&device_tree(&plan.disk.path)?)? == plan.disk,
        "disk identity changed"
    );
    let (cfg, _) = config::load(&work.join("base.yaml"))?;
    ensure!(
        serde_json::to_vec(&cfg)? == serde_json::to_vec(&plan.config)?,
        "saved declaration differs from installation plan"
    );
    ensure!(
        cfg.files.is_empty()
            && cfg.kernel.extra_command_line.is_empty()
            && cfg.accounts.user.groups == ["wheel"]
            && cfg.accounts.user.shell == "/bin/bash",
        "unsupported installation contract"
    );
    if let Some(key) = &plan.ssh_public_key {
        validate_key(key)?;
    }
    Ok(())
}
fn load(work: &Path) -> Result<Plan> {
    credentials::trusted_dir(work, true)?;
    let plan = serde_json::from_str(&credentials::read_private(&work.join("plan.json"))?)?;
    validate(&plan, work)?;
    Ok(plan)
}
fn confirmation(plan: &Plan) -> String {
    format!(
        "ERASE {} {} {}",
        plan.disk.path,
        if plan.disk.serial.is_empty() {
            if plan.disk.wwn.is_empty() {
                "NO-ID"
            } else {
                &plan.disk.wwn
            }
        } else {
            &plan.disk.serial
        },
        plan.disk.size
    )
}
fn show(plan: &Plan, work: &Path) {
    println!(
        "looom installation plan\nDisk: {} | {} | serial={} | WWN={} | {:.1} GiB\nERASE ENTIRE DISK\nGPT: EFI 2 GiB (FAT32) + remaining Btrfs\nSubvolumes: @bootstrap, @home, @var, @state; read-only @root-initial\nArch snapshot: {}; kernel: {}; desktop: {}\nHost: {}; user: {} ({}/{})\nTimezone: {}; locale: {}; keymap: {}\nSSH public key: {}; UEFI entry: {}\nWorkspace: {}\nConfirmation: {}",
        plan.disk.path,
        plan.disk.model,
        plan.disk.serial,
        plan.disk.wwn,
        plan.disk.size as f64 / GIB as f64,
        plan.config.source.snapshot,
        plan.config.kernel.package,
        plan.config.desktop.environment,
        plan.config.system.hostname,
        plan.config.accounts.user.name,
        plan.config.accounts.user.uid,
        plan.config.accounts.user.gid,
        plan.config.system.timezone,
        plan.config.system.locale,
        plan.config.system.console_keymap,
        plan.ssh_public_key.is_some(),
        plan.register_firmware,
        work.display(),
        confirmation(plan)
    );
    if plan.disk.session_identity.is_some() {
        println!(
            "Virtual disk without serial/WWN: plan valid only in this live session; reconnecting the disk requires a new plan."
        );
    }
}
fn prompt(label: &str, default: &str) -> Result<String> {
    print!("{label} [{default}]: ");
    io::stdout().flush()?;
    let mut line = String::new();
    ensure!(
        io::stdin().lock().take(4097).read_line(&mut line)? > 0 && line.len() <= 4096,
        "input closed/too long"
    );
    let line = line.trim();
    Ok(if line.is_empty() { default } else { line }.to_owned())
}
struct DiskChoice {
    tree: Value,
    disk: Option<Disk>,
    unavailable: Option<String>,
}
fn display_field(value: &str) -> String {
    if value.is_empty() {
        return "none".into();
    }
    value.chars().filter(|c| !c.is_control()).take(80).collect()
}
fn select_disk() -> Result<Disk> {
    let inventory: Value = serde_json::from_str(&output(
        "lsblk",
        &[
            "--json",
            "--bytes",
            "--paths",
            "--nodeps",
            "--output",
            "PATH,TYPE",
        ],
    )?)?;
    let mut choices = Vec::new();
    for entry in inventory["blockdevices"]
        .as_array()
        .context("disk inventory")?
    {
        if text_field(entry, "type") != "disk" {
            continue;
        }
        let tree = device_tree(&text_field(entry, "path"))?;
        let checked = (|| -> Result<Disk> {
            let disk = disk_from_tree(&tree)?;
            unused(&tree, None)?;
            exclusive_disk(&disk.path)?;
            Ok(disk)
        })();
        let (disk, unavailable) = match checked {
            Ok(disk) => (Some(disk), None),
            Err(error) => (None, Some(error.to_string())),
        };
        choices.push(DiskChoice {
            tree,
            disk,
            unavailable,
        });
    }
    ensure!(!choices.is_empty(), "no whole disks found");
    println!("Installation disks (enter a number; 0 cancels):");
    for (index, choice) in choices.iter().enumerate() {
        println!(
            "{}) {} | {:.1} GiB | {}",
            index + 1,
            display_field(&text_field(&choice.tree, "path")),
            choice.tree["size"].as_u64().context("disk size")? as f64 / GIB as f64,
            display_field(&text_field(&choice.tree, "model"))
        );
        println!(
            "   Serial: {}; WWN: {}",
            display_field(&text_field(&choice.tree, "serial")),
            display_field(&text_field(&choice.tree, "wwn"))
        );
        if let Some(reason) = &choice.unavailable {
            println!("   Unavailable: {}", display_field(reason));
        } else if choice
            .disk
            .as_ref()
            .is_some_and(|d| d.session_identity.is_some())
        {
            println!("   Available (virtual disk without serial/WWN; same live session only)");
        } else {
            println!("   Available");
        }
    }
    loop {
        let answer = prompt("Disk number to erase", "0")?;
        if answer == "0" {
            bail!("installation cancelled; disk unchanged");
        }
        let choice = answer
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|i| choices.get(i));
        let Some(choice) = choice else {
            println!(
                "Enter a disk number from 1 to {}, or 0 to cancel.",
                choices.len()
            );
            continue;
        };
        let Some(disk) = &choice.disk else {
            println!(
                "Disk unavailable: {}",
                choice.unavailable.as_deref().unwrap_or("unknown reason")
            );
            continue;
        };
        let current = device_tree(&disk.path)?;
        ensure!(
            disk_from_tree(&current)? == *disk,
            "disk changed since listing; restart the wizard"
        );
        unused(&current, None)?;
        exclusive_disk(&disk.path)?;
        println!(
            "Selected disk {} ({:.1} GiB).",
            disk.path,
            disk.size as f64 / GIB as f64
        );
        return Ok(disk.clone());
    }
}
fn wizard() -> Result<()> {
    root()?;
    credentials::disable_dumps()?;
    println!("looom installer — UEFI / whole disk / GRUB / Btrfs / KDE Plasma");
    let disk = select_disk()?;
    let hostname = prompt("Computer name", "looom")?;
    let user = prompt("User name", "owner")?;
    let timezone = prompt("Timezone", "Europe/Moscow")?;
    let locale = prompt("Locale", "en_US.UTF-8")?;
    let keymap = prompt("Console keymap", "us")?;
    let key = prompt(
        "SSH public key file (required for SSH access; optional locally)",
        "",
    )?;
    let parent = Path::new("/run/looom-installer");
    private_directory(parent)?;
    if !succeeds("mountpoint", &["-q", string(parent)?]) && fs::read_dir(parent)?.next().is_none() {
        // /run itself is often capped at 20% RAM, too small for Plasma inputs.
        // A dedicated tmpfs has a lazy 50% limit and does not resize global /run.
        command(
            "mount",
            &[
                "-t",
                "tmpfs",
                "-o",
                "size=50%,mode=0700,nosuid,nodev",
                "looom-installer",
                string(parent)?,
            ],
        )?;
    }
    let work = parent.join(uuid()?);
    private_directory(&work)?;
    let mut cfg: Value = serde_saphyr::from_str(TEMPLATE)?;
    cfg["system"] = serde_json::json!({"hostname":hostname,"timezone":timezone,"locale":locale,"console_keymap":keymap});
    cfg["accounts"]["user"]["name"] = user.clone().into();
    cfg["accounts"]["user"]["password_secret"] = format!("login-{user}").into();
    cfg["packages"] = serde_json::json!(["intel-ucode", "amd-ucode"]);
    let input = work.join("wizard.yaml");
    json(&input, &cfg, 0o600)?;
    ensure!(
        disk_from_tree(&device_tree(&disk.path)?)? == disk,
        "selected disk changed while configuring the system; restart the wizard"
    );
    let plan = create_plan(
        Path::new(&disk.path),
        &input,
        &work,
        if key.is_empty() {
            None
        } else {
            Some(Path::new(&key))
        },
        true,
    )?;
    let answer = prompt(
        "Type the exact confirmation above (anything else cancels)",
        "",
    )?;
    ensure!(
        answer == confirmation(&plan),
        "installation cancelled; disk unchanged"
    );
    engine::apply(&work, &plan, false, false)
}

pub fn dispatch(args: &[String]) -> Result<()> {
    if args.is_empty() {
        return wizard();
    }
    if ["--help", "-h", "help"].contains(&args[0].as_str()) {
        println!(
            "looom install — terminal wizard\nlooom install plan <disk> <base.yaml> <absolute-workspace> [--ssh-key <public-key-file>] [--no-nvram]\nlooom install show <workspace>\nlooom install apply <workspace> --confirm <exact-confirmation> [--passwords-stdin]\nlooom install resume <workspace> [--passwords-stdin]\nPasswords: hidden TTY prompts, or bounded JSON on stdin {{\"root\":\"...\",\"user\":\"...\"}}.\nA pending partition/format operation fails closed; resume never repeats it.\nKeep the workspace on a different persistent disk for recovery after reboot."
        );
        return Ok(());
    }
    root()?;
    credentials::disable_dumps()?;
    match args[0].as_str() {
        "plan" => {
            ensure!(
                args.len() >= 4,
                "install plan <disk> <base.yaml> <workspace>"
            );
            let mut key = None;
            let mut firmware = true;
            let mut i = 4;
            while i < args.len() {
                match args[i].as_str() {
                    "--ssh-key" => {
                        i += 1;
                        key = Some(Path::new(args.get(i).context("missing public key file")?));
                    }
                    "--no-nvram" if firmware => firmware = false,
                    _ => bail!("unknown/repeated plan option"),
                }
                i += 1;
            }
            create_plan(
                Path::new(&args[1]),
                Path::new(&args[2]),
                Path::new(&args[3]),
                key,
                firmware,
            )?;
        }
        "show" => {
            ensure!(args.len() == 2, "install show <workspace>");
            let w = Path::new(&args[1]);
            show(&load(w)?, w);
        }
        "apply" | "resume" => {
            ensure!(args.len() >= 2, "installation workspace required");
            let work = Path::new(&args[1]);
            let plan = load(work)?;
            let resume = args[0] == "resume";
            let mut input = false;
            let mut confirm = None;
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--passwords-stdin" if !input => input = true,
                    "--confirm" if !resume && confirm.is_none() => {
                        i += 1;
                        confirm = Some(args.get(i).context("missing confirmation")?);
                    }
                    _ => bail!("unknown/repeated apply/resume option"),
                }
                i += 1;
            }
            ensure!(
                resume || confirm.map(String::as_str) == Some(confirmation(&plan).as_str()),
                "exact disk-bound --confirm required; run install show"
            );
            engine::apply(work, &plan, resume, input)?;
        }
        _ => bail!("unknown install operation; run looom install --help"),
    }
    Ok(())
}

/// Called only inside the newly provisioned target. Existing normal builds
/// keep their stricter refusal-to-overwrite contract.
pub fn first_release() -> Result<()> {
    use crate::{
        builder::{self, Bundle},
        machine::{Machine, STATE},
        packages,
        releases::Manager,
    };
    root()?;
    let directory = Path::new(STATE).join("installation");
    credentials::trusted_dir(&directory, true)?;
    let plan: Plan =
        serde_json::from_str(&credentials::read_private(&directory.join("plan.json"))?)?;
    ensure!(
        plan.schema == 1
            && plan.release == "initial"
            && plan.recipe_sha256 == hash_file(&std::env::current_exe()?)?,
        "initial installer identity differs"
    );
    let machine = Machine::load()?;
    ensure!(
        machine.root_uuid == plan.root_uuid && machine.esp_uuid == plan.esp_uuid,
        "installer belongs to another system"
    );
    let manager = Manager::installed(machine)?;
    let (cfg, files) = config::load(&directory.join("base.yaml"))?;
    ensure!(
        files.is_empty() && serde_json::to_vec(&cfg)? == serde_json::to_vec(&plan.config)?,
        "initial declaration changed"
    );
    let lock = packages::load(&directory.join("base.yaml"), &cfg)?;
    builder::recover_builds(&manager)?;
    let _lock = manager.lock()?;
    let metadata = manager.state.join("releases/initial.json");
    if metadata.exists() {
        let record = manager.load("initial")?;
        ensure!(
            ["validated", "published", "confirmed"].contains(&record.phase.as_str()),
            "first release is not usable"
        );
        manager.validate(&record, false)?;
        return Ok(());
    }
    let frozen = manager.state.join("inputs/initial");
    let private = manager.top.join("@build-initial");
    ensure!(
        !manager.top.join("@root-initial").exists()
            && !manager.efi().join("looom-initial.efi").exists(),
        "unregistered first-release output; diagnosis required"
    );
    if frozen.exists() {
        credentials::trusted_dir(&frozen, true)?;
        let operation: Value =
            serde_json::from_slice(&fs::read(manager.state.join("operations/initial.json"))?)?;
        ensure!(
            operation["id"] == "initial"
                && operation["phase"] == "building"
                && operation["engine"] == "rust-0.2",
            "only an unconfigured installer build may restart"
        );
        let old: Bundle = serde_json::from_slice(&fs::read(frozen.join("bundle.json"))?)?;
        ensure!(
            old.release == "initial"
                && old.files.is_empty()
                && old.request == packages::request(&cfg)?
                && serde_json::to_vec(&old.config)? == serde_json::to_vec(&cfg)?
                && serde_json::to_vec(&old.lock)? == serde_json::to_vec(&lock)?
                && hash_file(&frozen.join("looom"))? == plan.recipe_sha256,
            "interrupted build inputs differ"
        );
        ensure!(
            !manager.mounted_subvolume("@build-initial")?,
            "interrupted private root is still mounted; unmount it before resume"
        );
        if private.exists() {
            ensure!(
                fs::symlink_metadata(&private)?.is_dir()
                    && succeeds("btrfs", &["subvolume", "show", string(&private)?]),
                "unsafe private build root"
            );
            command(
                "btrfs",
                &[
                    "subvolume",
                    "delete",
                    "--recursive",
                    "--commit-after",
                    string(&private)?,
                ],
            )?;
        }
        let retired = directory.join("abandoned-builds");
        private_directory(&retired)?;
        let destination = retired.join(uuid()?);
        fs::rename(&frozen, &destination)?;
        sync_dir(frozen.parent().unwrap())?;
        sync_dir(&retired)?;
        println!(
            "Restarting an unconfigured first build; previous inputs retained at {}",
            destination.display()
        );
    }
    builder::build(
        &manager,
        Bundle {
            schema: 2,
            config: cfg,
            files,
            request: packages::request(&plan.config)?,
            lock,
            release: "initial".into(),
        },
    )
}

/// An outer private mount namespace contains crashes as well as normal exits.
/// This wrapper also releases our own mounts before arch-chroot's teardown.
pub fn target_session(args: &[String]) -> Result<()> {
    root()?;
    ensure!(
        matches!(args, [op] if op == "internal-install-build")
            || matches!(args, [op, id] if ["publish","try"].contains(&op.as_str()) && id == "initial")
            || matches!(args, [op, cfg] if op == "bootstrap" && cfg == "/var/lib/looom/installation/base.yaml"),
        "invalid installer target operation"
    );
    let plan: Plan = serde_json::from_str(&credentials::read_private(Path::new(
        "/var/lib/looom/installation/plan.json",
    ))?)?;
    ensure!(
        output("findmnt", &["-nro", "UUID", "/"])? == plan.root_uuid
            && output("findmnt", &["-nro", "FSROOT", "/"])? == "/@bootstrap"
            && plan.recipe_sha256 == hash_file(&std::env::current_exe()?)?,
        "installer target session identity differs"
    );
    let status = std::process::Command::new(std::env::current_exe()?)
        .args(args)
        .stdin(std::process::Stdio::null())
        .status();
    for (path, fsroot) in [
        ("/run/looom-build-initial", "/@build-initial"),
        ("/run/looom-top", "/"),
    ] {
        if succeeds("mountpoint", &["-q", path]) {
            ensure!(
                output("findmnt", &["-nro", "UUID", "--mountpoint", path])? == plan.root_uuid
                    && output("findmnt", &["-nro", "FSROOT", "--mountpoint", path])? == fsroot,
                "foreign mount in installer target session"
            );
            command("umount", &["--recursive", path])?;
        }
    }
    ensure!(status?.success(), "installer target command failed");
    Ok(())
}
