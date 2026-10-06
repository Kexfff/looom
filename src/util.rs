use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
    },
    path::Path,
    process::{Command, Stdio},
};

pub fn command(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("start {program}"))?;
    ensure!(status.success(), "{program} failed ({status})");
    Ok(())
}
pub fn output(program: &str, args: &[&str]) -> Result<String> {
    let value = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("start {program}"))?;
    ensure!(
        value.status.success(),
        "{program} failed: {}",
        String::from_utf8_lossy(&value.stderr).trim()
    );
    Ok(String::from_utf8(value.stdout)?.trim().to_string())
}
pub fn succeeds(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
pub fn string(path: &Path) -> Result<&str> {
    path.to_str().context("non-UTF8 path")
}
pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("read {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut bytes = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        digest.update(&bytes[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
pub fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}
pub fn mkdir(path: &Path, mode: u32) -> Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}
pub fn atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let parent = path.parent().context("missing parent")?;
    let mut file = tempfile::Builder::new()
        .prefix(".looom-")
        .tempfile_in(parent)?;
    file.as_file()
        .set_permissions(fs::Permissions::from_mode(mode))?;
    use std::os::unix::fs::MetadataExt;
    let metadata = file.as_file().metadata()?;
    if metadata.uid() != 0 || metadata.gid() != 0 {
        ensure!(
            unsafe { libc::fchown(file.as_file().as_raw_fd(), 0, 0) } == 0,
            "cannot set root file ownership"
        );
    }
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    sync_dir(parent)
}
pub fn json<T: Serialize>(path: &Path, value: &T, mode: u32) -> Result<()> {
    let mut data = serde_json::to_vec_pretty(value)?;
    data.push(b'\n');
    atomic(path, &data, mode)
}
pub fn root() -> Result<()> {
    ensure!(unsafe { libc::geteuid() } == 0, "run as root");
    Ok(())
}
pub struct Lock(File);
impl Lock {
    pub fn acquire(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(path)?;
        let m = file.metadata()?;
        use std::os::unix::fs::MetadataExt;
        ensure!(
            m.is_file() && m.uid() == 0 && m.mode() & 0o077 == 0,
            "unsafe lock file"
        );
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Self(file))
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
pub fn failpoint(point: &str) -> Result<()> {
    if std::env::var("LOOOM_FAIL_AFTER").as_deref() == Ok(point) {
        bail!("Injected interruption after {point}");
    }
    Ok(())
}
pub fn chroot(root: &Path, program: &str, args: &[&str]) -> Result<()> {
    let mut all = vec![string(root)?, program];
    all.extend(args);
    command("arch-chroot", &all)
}
pub fn chroot_output(root: &Path, program: &str, args: &[&str]) -> Result<String> {
    let mut all = vec![string(root)?, program];
    all.extend(args);
    output("arch-chroot", &all)
}
pub fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => bail!("refusing to unlink directory {}", path.display()),
        Ok(_) => fs::remove_file(path)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
