//! Destructive acceptance fixtures, restricted to the original disposable VM.
use anyhow::{Context, Result, ensure};
use looom::{
    machine::Machine,
    releases::{Manager, Metadata},
    util::*,
};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

const BASE: &str = "/var/lib/looom/dev/durability-20261008/fixture";
const TOP: &str = "/run/looom-durability-btrfs";
const ESP: &str = "/run/looom-durability-fat";
const ID: &str = "candidate";
struct FaultChild(std::process::Child);
impl Drop for FaultChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn guard() -> Result<()> {
    root()?;
    ensure!(
        matches!(
            output("systemd-detect-virt", &["--vm"])?.as_str(),
            "qemu" | "kvm"
        ),
        "VM only"
    );
    ensure!(
        fs::read_to_string("/sys/class/net/enp1s0/address")?.trim() == "52:54:00:7b:23:63",
        "unexpected VM"
    );
    Ok(())
}
fn paths() -> (PathBuf, PathBuf) {
    (
        Path::new(BASE).join("btrfs.img"),
        Path::new(BASE).join("fat.img"),
    )
}
fn mount_images() -> Result<()> {
    let (btrfs, fat) = paths();
    for (image, mount) in [(&btrfs, TOP), (&fat, ESP)] {
        mkdir(Path::new(mount), 0o700)?;
        if !succeeds("mountpoint", &["-q", mount]) {
            command(
                "mount",
                &[
                    "-o",
                    if mount == ESP {
                        "loop,umask=0077"
                    } else {
                        "loop,noatime"
                    },
                    string(image)?,
                    mount,
                ],
            )?;
        }
    }
    Ok(())
}
fn manager() -> Result<Manager> {
    mount_images()?;
    let profile: Machine =
        serde_json::from_slice(&fs::read(Path::new(BASE).join("machine.json"))?)?;
    ensure!(
        output("findmnt", &["-nro", "UUID", TOP])? == profile.root_uuid
            && output("findmnt", &["-nro", "UUID", ESP])? == profile.esp_uuid,
        "fixture UUID mismatch"
    );
    Ok(Manager {
        machine: profile,
        top: TOP.into(),
        esp: ESP.into(),
        state: Path::new(TOP).join("state"),
    })
}
fn initialize() -> Result<()> {
    ensure!(
        !Path::new(BASE).exists(),
        "fixture already exists; clean it explicitly"
    );
    mkdir(Path::new(BASE), 0o700)?;
    let (btrfs, fat) = paths();
    for image in [&btrfs, &fat] {
        File::create(image)?.set_len(512 * 1024 * 1024)?;
    }
    command("mkfs.btrfs", &["-f", string(&btrfs)?])?;
    command("mkfs.fat", &["-F", "32", string(&fat)?])?;
    mount_images()?;
    let profile = Machine {
        schema: 1,
        bootloader: looom::machine::Bootloader::Grub,
        root_uuid: output("findmnt", &["-nro", "UUID", TOP])?,
        esp_uuid: output("findmnt", &["-nro", "UUID", ESP])?,
        bootstrap_uki_sha256: "0".repeat(64),
        home_subvolume: "@home".into(),
        var_subvolume: "@var".into(),
        state_subvolume: "@state".into(),
        user: "codex".into(),
        uid: 1000,
        gid: 1000,
        serial_console: true,
        guest_agent: true,
        passwordless_sudo: false,
    };
    json(&Path::new(BASE).join("machine.json"), &profile, 0o600)?;
    let m = manager()?;
    for name in ["releases", "operations", "publications"] {
        mkdir(&m.state.join(name), 0o700)?;
    }
    mkdir(&m.efi(), 0o700)?;
    let grub = m.esp.join("looom/grub");
    mkdir(&grub, 0o700)?;
    command("grub-editenv", &[string(&grub.join("grubenv"))?, "create"])?;
    m.set_environment(&["saved_entry=looom-baseline".into(), "next_entry=".into()])?;
    for (id, size, phase) in [
        ("baseline", 64 * 1024, "confirmed"),
        (ID, 128 * 1024 * 1024, "validated"),
    ] {
        let root = m.top.join(format!("@root-{id}"));
        command("btrfs", &["subvolume", "create", string(&root)?])?;
        for name in [
            "boot",
            "etc/looom",
            "etc/kernel",
            "usr/lib/modules/test-kernel",
            "usr/lib/looom/accounts",
        ] {
            mkdir(&root.join(name), 0o755)?;
        }
        fs::write(root.join("etc/looom/release-id"), id)?;
        fs::write(
            root.join("etc/kernel/cmdline"),
            format!("rootflags=subvol=@root-{id} ro\n"),
        )?;
        for name in ["shadow", "gshadow"] {
            fs::write(
                root.join("usr/lib/looom/accounts").join(name),
                "root:!::::::::\ncodex:!::::::::\n",
            )?;
        }
        let uki = root.join(format!("boot/looom-{id}.efi"));
        let mut file = File::create(&uki)?;
        let block = vec![0x5a; 1024 * 1024];
        let mut left = size;
        while left > 0 {
            let count = left.min(block.len());
            file.write_all(&block[..count])?;
            left -= count;
        }
        file.sync_all()?;
        let metadata = Metadata {
            schema_version: 1,
            id: id.into(),
            phase: phase.into(),
            root_subvolume: format!("@root-{id}"),
            kernel_package: "linux".into(),
            kernel_version: "test-kernel".into(),
            root_uuid: m.machine.root_uuid.clone(),
            esp_uuid: m.machine.esp_uuid.clone(),
            uki_sha256: hash_file(&uki)?,
            declarative_value: id.into(),
            created_at: 1,
            engine: None,
        };
        command(
            "btrfs",
            &["property", "set", "-ts", string(&root)?, "ro", "true"],
        )?;
        m.save(&metadata)?;
        m.operation(id, phase)?;
        if id == "baseline" {
            fs::copy(uki, m.efi().join("looom-baseline.efi"))?;
        }
    }
    m.write_menu(&m.menu(None)?)?;
    command("sync", &["-f", TOP])?;
    command("sync", &["-f", ESP])?;
    // Loop files live on the VM's outer Btrfs. Persist both filesystem
    // layers before the experiment; faults affect subsequent writes only.
    for image in [&btrfs, &fat] {
        File::open(image)?.sync_all()?;
    }
    sync_dir(Path::new(BASE))?;
    command("sync", &["-f", BASE])?;
    println!("Prepared isolated 512 MiB Btrfs and FAT fixtures; main VM boot choice untouched");
    Ok(())
}
fn recover(m: &Manager) -> Result<()> {
    let _lock = m.lock()?;
    m.recover_publications()?;
    m.write_menu(&m.menu(None)?)?;
    Ok(())
}
fn check(m: &Manager) -> Result<()> {
    let env = m.environment()?;
    ensure!(
        env.get("saved_entry").map(String::as_str) == Some("looom-baseline")
            && env.get("next_entry").is_none_or(String::is_empty),
        "boot choice changed"
    );
    let baseline = m.load("baseline")?;
    m.validate(&baseline, false)?;
    ensure!(
        hash_file(&m.efi().join("looom-baseline.efi"))? == baseline.uki_sha256,
        "baseline ESP UKI changed"
    );
    m.validate(&m.load(ID)?, false)?;
    ensure!(
        m.load(ID)?.phase == "validated",
        "interrupted candidate unexpectedly published"
    );
    println!("PASS: baseline UKI/root and saved choice intact; candidate remains validated");
    Ok(())
}
fn finish(m: &Manager) -> Result<()> {
    check(m)?;
    recover(m)?;
    m.validate(&m.load("baseline")?, true)?;
    ensure!(
        !fs::read_to_string(m.esp.join("looom/grub/grub.cfg"))?.contains("--id looom-candidate {"),
        "uncommitted candidate in recovered menu"
    );
    for file in fs::read_dir(m.efi())? {
        ensure!(
            !file?.file_name().to_string_lossy().starts_with(".uki-"),
            "pending UKI leaked"
        );
    }
    let before = m.environment()?;
    let _lock = m.lock()?;
    m.publish(ID)?;
    m.publish(ID)?;
    m.validate(&m.load(ID)?, true)?;
    ensure!(m.environment()? == before, "publish changed boot choice");
    ensure!(
        fs::read_dir(m.state.join("publications"))?.next().is_none(),
        "publication journal leaked"
    );
    println!(
        "PASS: recovery removed unfinished UKI; explicit publication is repeatable; baseline remains saved"
    );
    Ok(())
}
fn enospc() -> Result<()> {
    initialize()?;
    let m = manager()?;
    let before = m.environment()?;
    let baseline = fs::read(m.state.join("releases/baseline.json"))?;
    let child = FaultChild(
        Command::new(std::env::current_exe()?)
            .arg("publish")
            .env("LOOOM_FAIL_AFTER", "menu")
            .env("LOOOM_FAIL_MODE", "stop")
            .spawn()?,
    );
    let pid = child.0.id() as i32;
    let mut status = 0;
    ensure!(
        unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) } == pid
            && libc::WIFSTOPPED(status),
        "publication did not pause"
    );
    let filler_path = m.top.join("filler");
    let mut filler = File::create(&filler_path)?;
    let mut random = File::open("/dev/urandom")?;
    let mut bytes = vec![0; 1024 * 1024];
    let error = loop {
        random.read_exact(&mut bytes)?;
        if let Err(e) = filler.write_all(&bytes) {
            break e;
        }
    };
    ensure!(
        error.raw_os_error() == Some(libc::ENOSPC),
        "expected real ENOSPC: {error}"
    );
    let _ = filler.sync_all();
    drop(filler);
    // Metadata is small. Consume remaining metadata/data slack with independent files
    // until the very same atomic write used by the manager fails with ENOSPC.
    let mut slot = 0;
    loop {
        match atomic(&m.state.join("probe.json"), &vec![0x62; 4096], 0o600) {
            Err(e) => {
                println!("Atomic state write after confirmed ENOSPC: {e:#}");
                break;
            }
            Ok(()) => {
                fs::rename(
                    m.state.join("probe.json"),
                    m.top.join(format!("slack-{slot}")),
                )?;
                slot += 1;
                ensure!(slot < 10000, "could not exhaust metadata slack");
            }
        }
    }
    ensure!(
        unsafe { libc::kill(pid, libc::SIGCONT) } == 0,
        "resume child"
    );
    ensure!(
        unsafe { libc::waitpid(pid, &mut status, 0) } == pid
            && libc::WIFEXITED(status)
            && libc::WEXITSTATUS(status) != 0,
        "full state did not fail publication"
    );
    ensure!(
        fs::read(m.state.join("releases/baseline.json"))? == baseline && m.environment()? == before,
        "baseline changed at ENOSPC"
    );
    fs::remove_file(filler_path)?;
    for n in 0..slot {
        fs::remove_file(m.top.join(format!("slack-{n}")))?;
    }
    command("sync", &["-f", TOP])?;
    finish(&m)?;
    println!(
        "PASS: real Btrfs ENOSPC after FAT menu commit, unchanged metadata/boot choice and successful recovery"
    );
    Ok(())
}
fn clean() -> Result<()> {
    for path in [ESP, TOP] {
        if succeeds("mountpoint", &["-q", path]) {
            command("umount", &[path])?;
        }
    }
    // Only known fixture files are removed; never recursively delete a mountpoint.
    let (btrfs, fat) = paths();
    for path in [btrfs, fat, Path::new(BASE).join("machine.json")] {
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    if Path::new(BASE).exists() {
        fs::remove_dir(BASE)?;
    }
    Ok(())
}
fn audit(directory: &Path) -> Result<()> {
    looom::credentials::trusted_dir(directory, true)?;
    let machine = Machine::load()?;
    let mut secrets = Vec::new();
    for name in ["root", machine.user.as_str()] {
        secrets.push(looom::credentials::read_private(
            &Path::new(looom::machine::STATE)
                .join("credentials")
                .join(format!("{name}.hash")),
        )?);
    }
    let private = Path::new(looom::machine::STATE).join("private-password-test");
    if private.exists() {
        for name in ["original.hash", "password.secret"] {
            secrets.push(looom::credentials::read_private(&private.join(name))?);
        }
    }
    let mut count = 0;
    for item in fs::read_dir(directory)? {
        let path = item?.path();
        ensure!(
            fs::symlink_metadata(&path)?.is_file(),
            "unexpected public entry"
        );
        let bytes = fs::read(&path)?;
        for secret in &secrets {
            let needle = secret.trim().as_bytes();
            ensure!(needle.len() >= 12, "unexpected private value");
            ensure!(
                !bytes.windows(needle.len()).any(|s| s == needle),
                "private value in {}; do not export",
                path.display()
            );
        }
        count += 1;
    }
    ensure!(count > 0, "empty export");
    println!(
        "PASS: {count} public artifacts contain no actual credential hashes or temporary password"
    );
    Ok(())
}
fn run() -> Result<()> {
    guard()?;
    let op = std::env::args()
        .nth(1)
        .context("prepare|publish|check|recover|finish|enospc|clean")?;
    match op.as_str() {
        "prepare" => initialize(),
        "clean" => clean(),
        "audit" => audit(Path::new("/var/lib/looom/dev/durability-20261008/public")),
        "audit-bootstrap" => audit(Path::new("/var/lib/looom/dev/bootstrap-20261009/public")),
        "enospc" => enospc(),
        "publish" => {
            let m = manager()?;
            let _lock = m.lock()?;
            m.publish(ID)
        }
        "check" => check(&manager()?),
        "recover" => recover(&manager()?),
        "finish" => finish(&manager()?),
        _ => anyhow::bail!("unknown operation"),
    }
}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("VM durability: {e:#}");
            ExitCode::FAILURE
        }
    }
}
