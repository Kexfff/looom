//! Acceptance harness, never installed on the target system.
use anyhow::{Result, ensure};
use looom::{credentials, installer::Plan, util::*};
use serde_json::{Value, json as value};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use zeroize::Zeroizing;
const BASE: &str = "/var/lib/looom/dev/installer-20261009";
fn guard() -> Result<()> {
    root()?;
    credentials::disable_dumps()?;
    ensure!(
        matches!(
            output("systemd-detect-virt", &["--vm"])?.as_str(),
            "qemu" | "kvm"
        ) && fs::read_to_string("/sys/class/net/enp1s0/address")?.trim() == "52:54:00:7b:23:63",
        "original development VM only"
    );
    ensure!(
        output("lsblk", &["-dnro", "SERIAL", "/dev/vdb"])? == "looom-installer-2026"
            && output("blockdev", &["--getsize64", "/dev/vdb"])? == "51539607552",
        "explicit separate installer disk only"
    );
    Ok(())
}
fn binary() -> PathBuf {
    Path::new(BASE).join("looom")
}
fn call(args: &[&str], point: Option<&str>, input: bool, success: bool) -> Result<()> {
    let mut cmd = Command::new(binary());
    cmd.args(args);
    cmd.stderr(Stdio::piped());
    if let Some(p) = point {
        cmd.env("LOOOM_FAIL_AFTER", p);
    }
    if input {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
    let mut child = cmd.spawn()?;
    if input {
        let bytes = credentials::read_private(&Path::new(BASE).join("private/passwords.secret"))?;
        child.stdin.take().unwrap().write_all(bytes.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    std::io::stderr().write_all(&output.stderr)?;
    ensure!(
        output.status.success() == success,
        "unexpected installer exit"
    );
    if let Some(p) = point {
        ensure!(
            String::from_utf8_lossy(&output.stderr)
                .contains(&format!("Injected interruption after {p}")),
            "requested failpoint was not reached"
        );
    }
    Ok(())
}
fn plan(work: &Path) -> Result<Plan> {
    call(
        &[
            "install",
            "plan",
            "/dev/vdb",
            &format!("{BASE}/base.yaml"),
            string(work)?,
            "--ssh-key",
            &format!("{BASE}/management.pub"),
            "--no-nvram",
        ],
        None,
        false,
        true,
    )?;
    Ok(serde_json::from_slice(&fs::read(work.join("plan.json"))?)?)
}
fn confirmation(_plan: &Plan) -> String {
    "YES".into()
}
fn primary() -> Result<Value> {
    Ok(
        value!({"profile":hash_file(Path::new("/var/lib/looom/machine.json"))?,"root_hash":hash_file(Path::new("/var/lib/looom/credentials/root.hash"))?,"user_hash":hash_file(Path::new("/var/lib/looom/credentials/codex.hash"))?,"release":fs::read_to_string("/etc/looom/release-id")?,"grubenv":output("grub-editenv", &["/efi/looom/grub/grubenv","list"])?,"efi":hash_file(Path::new("/efi/EFI/Linux/looom-bootstrap.efi"))?}),
    )
}
fn prepare() -> Result<()> {
    let base = Path::new(BASE);
    mkdir(base, 0o700)?;
    ensure!(
        !base.join("private/baseline.json").exists(),
        "acceptance already prepared"
    );
    mkdir(&base.join("private"), 0o700)?;
    mkdir(&base.join("public"), 0o700)?;
    json(&base.join("private/baseline.json"), &primary()?, 0o600)?;
    let mut random = [0u8; 32];
    fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
    let password = Zeroizing::new(format!("Installer-{}", hash(&random)));
    let input = Zeroizing::new(format!(
        "{{\"root\":\"{}\",\"user\":\"{}\"}}",
        password.as_str(),
        password.as_str()
    ));
    credentials::private_atomic(&base.join("private/passwords.secret"), input.as_bytes())?;
    let mut cfg: Value = serde_saphyr::from_str(include_str!("../../configs/native/gc/base.yaml"))?;
    cfg["system"]["hostname"] = "looom-installed".into();
    cfg["accounts"]["user"]["name"] = "owner".into();
    cfg["accounts"]["user"]["password_secret"] = "login-owner".into();
    cfg["packages"] = value!(["intel-ucode", "amd-ucode"]);
    json(&base.join("base.yaml"), &cfg, 0o600)?;
    println!("Prepared private baseline and fresh test credentials; primary disk untouched");
    Ok(())
}
fn negatives() -> Result<()> {
    let work = Path::new(BASE).join("negatives");
    let p = plan(&work)?;
    let before = output("sfdisk", &["--dump", "/dev/vdb"]).unwrap_or_default();
    call(
        &[
            "install",
            "apply",
            string(&work)?,
            "--confirm",
            "WRONG",
            "--passwords-stdin",
        ],
        None,
        true,
        false,
    )?;
    call(&["install", "resume", string(&work)?], None, false, false)?;
    call(
        &[
            "install",
            "plan",
            "/dev/vda",
            &format!("{BASE}/base.yaml"),
            &format!("{BASE}/refuse-primary"),
        ],
        None,
        false,
        false,
    )?;
    let rejected = Command::new(binary())
        .args([
            "install",
            "plan",
            "/dev/vda2",
            &format!("{BASE}/base.yaml"),
            &format!("{BASE}/refuse-btrfs"),
        ])
        .output()?;
    ensure!(
        !rejected.status.success()
            && String::from_utf8_lossy(&rejected.stderr).contains("selected disk is mounted at /"),
        "Btrfs anonymous mount guard did not reject root"
    );
    std::io::stderr().write_all(&rejected.stderr)?;
    let foreign = work.join("target/usr");
    mkdir(&foreign, 0o700)?;
    command("mount", &["--bind", "/usr", string(&foreign)?])?;
    let rejected = Command::new(binary())
        .args([
            "install",
            "apply",
            string(&work)?,
            "--confirm",
            &confirmation(&p),
        ])
        .output()?;
    command("umount", &[string(&foreign)?])?;
    ensure!(
        !rejected.status.success()
            && String::from_utf8_lossy(&rejected.stderr).contains("foreign/nested mount"),
        "foreign target bind guard failed"
    );
    std::io::stderr().write_all(&rejected.stderr)?;
    let mut bad: Value = serde_json::from_slice(&fs::read(work.join("plan.json"))?)?;
    bad["disk"]["serial"] = "wrong-disk".into();
    json(&work.join("plan.json"), &bad, 0o600)?;
    call(&["install", "show", string(&work)?], None, false, false)?;
    json(&work.join("plan.json"), &p, 0o600)?;
    bad = serde_json::from_slice(&fs::read(work.join("base.yaml"))?)?;
    bad["system"]["hostname"] = "changed".into();
    json(&work.join("base.yaml"), &bad, 0o600)?;
    call(&["install", "show", string(&work)?], None, false, false)?;
    json(&work.join("base.yaml"), &p.config, 0o600)?;
    let link = Path::new(BASE).join("workspace-link");
    std::os::unix::fs::symlink(&work, &link)?;
    call(&["install", "show", string(&link)?], None, false, false)?;
    ensure!(
        !work.join("journal.json").exists()
            && before == output("sfdisk", &["--dump", "/dev/vdb"]).unwrap_or_default(),
        "negative test mutated disk/journal"
    );
    println!(
        "PASS: wrong confirmation, missing resume journal, active system disk, changed identity/config, symlink workspace; selected disk layout unchanged"
    );
    Ok(())
}
fn apply(work: &Path, point: Option<&str>) -> Result<()> {
    let plan = plan(work)?;
    // Reuse only the VM's existing public package cache. The installer still verifies signatures and hashes.
    mkdir(&work.join("cache"), 0o700)?;
    for entry in fs::read_dir("/var/cache/pacman/pkg")? {
        let path = entry?.path();
        if path.is_file()
            && path
                .file_name()
                .is_some_and(|v| v.to_string_lossy().contains(".pkg.tar."))
        {
            let destination = work.join("cache").join(path.file_name().unwrap());
            command(
                "cp",
                &["--reflink=auto", string(&path)?, string(&destination)?],
            )?;
        }
    }
    call(
        &[
            "install",
            "apply",
            string(work)?,
            "--confirm",
            &confirmation(&plan),
            "--passwords-stdin",
        ],
        point,
        true,
        point.is_none(),
    )
}
fn destructive() -> Result<()> {
    let work = Path::new(BASE).join("format-interruption");
    apply(&work, Some("install-action-root-format"))?;
    let before = output("blkid", &["-p", "-s", "UUID", "-o", "value", "/dev/vdb2"])?;
    call(&["install", "resume", string(&work)?], None, false, false)?;
    ensure!(
        before == output("blkid", &["-p", "-s", "UUID", "-o", "value", "/dev/vdb2"])?
            && !work.join("top").exists(),
        "destructive resume modified disk"
    );
    println!(
        "PASS: interruption after actual format, before checkpoint; resume refuses to format or mount again"
    );
    Ok(())
}
fn report() -> Result<()> {
    let before: Value =
        serde_json::from_slice(&fs::read(Path::new(BASE).join("private/baseline.json"))?)?;
    ensure!(
        before == primary()?,
        "primary system/credentials/boot selection changed"
    );
    println!(
        "PASS: development VM profile, saved/next, recovery image, credentials and running release unchanged"
    );
    let work = Path::new(BASE).join("install");
    let journal: Value = serde_json::from_slice(&fs::read(work.join("journal.json"))?)?;
    ensure!(
        journal["completed"] == 12 && journal["pending"].is_null(),
        "installation incomplete"
    );
    ensure!(
        output("findmnt", &["-rn", "-S", "/dev/vdb2"])
            .unwrap_or_default()
            .is_empty()
            && output("findmnt", &["-rn", "-S", "/dev/vdb1"])
                .unwrap_or_default()
                .is_empty(),
        "target mounts retained"
    );
    println!("PASS: all 12 installer stages complete; target disk detached from mount namespace");
    Ok(())
}
fn run() -> Result<()> {
    guard()?;
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str).unwrap_or("") {
        "prepare" => prepare(),
        "negatives" => negatives(),
        "destructive" => destructive(),
        "report" => report(),
        "start" => apply(
            &Path::new(BASE).join("install"),
            Some("install-root-format"),
        ),
        "resume" => call(
            &["install", "resume", &format!("{BASE}/install")],
            args.get(1).map(String::as_str),
            false,
            args.len() == 1,
        ),
        _ => anyhow::bail!("prepare|negatives|destructive|start|resume [failpoint]|report"),
    }
}
fn main() {
    if let Err(e) = run() {
        eprintln!("{e:#}");
        std::process::exit(1);
    }
}
