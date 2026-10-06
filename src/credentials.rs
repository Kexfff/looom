use crate::{
    config::Config,
    machine::{Machine, STATE},
    util::*,
};
use anyhow::{Context, Result, bail, ensure};
use std::{
    ffi::{CStr, CString},
    fs::{self, File},
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd},
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};
use zeroize::Zeroizing;

fn cstring(value: &str) -> Result<CString> {
    Ok(CString::new(value)?)
}
fn opened(fd: libc::c_int) -> Result<File> {
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn check(file: &File, directory: bool, private: bool) -> Result<()> {
    let m = file.metadata()?;
    ensure!(
        m.uid() == 0 && if directory { m.is_dir() } else { m.is_file() },
        "credential object must be root-owned with expected type"
    );
    ensure!(
        m.mode() & if private || !directory { 0o077 } else { 0o022 } == 0,
        "unsafe credential permissions"
    );
    for attribute in ["system.posix_acl_access", "system.posix_acl_default"] {
        let attribute = cstring(attribute)?;
        let count = unsafe {
            libc::fgetxattr(
                file.as_raw_fd(),
                attribute.as_ptr(),
                std::ptr::null_mut(),
                0,
            )
        };
        if count >= 0 {
            bail!("credential ACLs are unsupported");
        }
        let error = std::io::Error::last_os_error();
        ensure!(
            matches!(
                error.raw_os_error(),
                Some(libc::ENODATA) | Some(libc::ENOTSUP)
            ),
            "cannot check credential ACL"
        );
    }
    Ok(())
}
pub fn trusted_dir(path: &Path, private: bool) -> Result<File> {
    ensure!(path.is_absolute(), "absolute trusted directory required");
    let slash = cstring("/")?;
    let mut directory = opened(unsafe {
        libc::open(
            slash.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    })?;
    check(&directory, true, false)?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                let name = cstring(name.to_str().context("non-UTF8 credential path")?)?;
                directory = opened(unsafe {
                    libc::openat(
                        directory.as_raw_fd(),
                        name.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    )
                })?;
                check(&directory, true, false)?;
            }
            _ => bail!("untrusted credential directory"),
        }
    }
    check(&directory, true, private)?;
    Ok(directory)
}
fn open_at(directory: &File, name: &str, flags: i32, mode: u32) -> Result<File> {
    let name = cstring(name)?;
    opened(unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            mode,
        )
    })
}
pub fn read_private(path: &Path) -> Result<Zeroizing<String>> {
    let directory = trusted_dir(path.parent().context("missing credential parent")?, false)?;
    let file = open_at(
        &directory,
        path.file_name()
            .and_then(|s| s.to_str())
            .context("credential name")?,
        libc::O_RDONLY,
        0,
    )?;
    check(&file, false, true)?;
    let mut value = Zeroizing::new(String::new());
    file.take(16385).read_to_string(&mut value)?;
    ensure!(value.len() <= 16384, "credential object too large");
    Ok(value)
}
pub fn private_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let directory = trusted_dir(path.parent().context("credential parent")?, false)?;
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .context("credential name")?;
    match open_at(&directory, name, libc::O_RDONLY, 0) {
        Ok(file) => check(&file, false, true)?,
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) => {}
        Err(e) => return Err(e),
    }
    let mut random = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut random)?;
    let temporary = format!(".{}.{}", name, hash(&random));
    let result = (|| -> Result<()> {
        let mut file = open_at(
            &directory,
            &temporary,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            0o600,
        )?;
        check(&file, false, true)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        let src = cstring(&temporary)?;
        let dest = cstring(name)?;
        if unsafe {
            libc::renameat(
                directory.as_raw_fd(),
                src.as_ptr(),
                directory.as_raw_fd(),
                dest.as_ptr(),
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        directory.sync_all()?;
        Ok(())
    })();
    let temp = cstring(&temporary)?;
    unsafe {
        libc::unlinkat(directory.as_raw_fd(), temp.as_ptr(), 0);
    }
    result
}
pub fn validate_hash(value: &str) -> Result<()> {
    let parts: Vec<_> = value.split('$').collect();
    let alphabet = |s: &str| {
        s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"./".contains(&c))
    };
    if parts.len() == 5
        && parts[0].is_empty()
        && parts[1] == "y"
        && parts[2] == "j9T"
        && (16..=86).contains(&parts[3].len())
        && parts[4].len() == 43
        && alphabet(parts[3])
        && alphabet(parts[4])
    {
        return Ok(());
    }
    if parts.first() == Some(&"") && parts.get(1) == Some(&"6") {
        let index = if parts.len() == 5 && parts[2].starts_with("rounds=") {
            let rounds: u32 = parts[2][7..].parse().context("unsupported password hash")?;
            ensure!(
                (1000..=1000000).contains(&rounds),
                "unsupported password hash cost"
            );
            3
        } else if parts.len() == 4 {
            2
        } else {
            bail!("unsupported password hash");
        };
        if (1..=16).contains(&parts[index].len())
            && parts[index + 1].len() == 86
            && alphabet(parts[index])
            && alphabet(parts[index + 1])
        {
            return Ok(());
        }
    }
    bail!("unsupported or malformed password hash")
}
pub fn hash_password(password: &str) -> Result<Zeroizing<String>> {
    ensure!(
        !password.is_empty() && password.len() <= 4096 && !password.contains(['\0', '\r', '\n']),
        "password must be nonempty, single-line and at most 4096 bytes"
    );
    let mut input = Zeroizing::new(password.as_bytes().to_vec());
    input.push(0);
    let mut setting = Zeroizing::new(vec![0u8; 192]);
    let mut workspace = Zeroizing::new(vec![0u8; 32768]);
    unsafe {
        let library = libloading::Library::new("libcrypt.so.2").context("libxcrypt required")?;
        let salt: libloading::Symbol<
            unsafe extern "C" fn(
                *const libc::c_char,
                libc::c_ulong,
                *const libc::c_char,
                libc::c_int,
                *mut libc::c_char,
                libc::c_int,
            ) -> *mut libc::c_char,
        > = library.get(b"crypt_gensalt_rn")?;
        let crypt: libloading::Symbol<
            unsafe extern "C" fn(
                *const libc::c_char,
                *const libc::c_char,
                *mut libc::c_void,
                libc::c_int,
            ) -> *mut libc::c_char,
        > = library.get(b"crypt_rn")?;
        ensure!(
            !salt(
                c"$y$".as_ptr(),
                5,
                std::ptr::null(),
                0,
                setting.as_mut_ptr().cast(),
                setting.len() as i32
            )
            .is_null(),
            "yescrypt salt generation failed"
        );
        let result = crypt(
            input.as_ptr().cast(),
            setting.as_ptr().cast(),
            workspace.as_mut_ptr().cast(),
            workspace.len() as i32,
        );
        ensure!(!result.is_null(), "yescrypt hashing failed");
        let value = Zeroizing::new(CStr::from_ptr(result).to_str()?.to_owned());
        validate_hash(&value)?;
        Ok(value)
    }
}

#[derive(Clone)]
pub struct Accounts {
    pub credentials: PathBuf,
    pub templates: PathBuf,
    pub runtime: PathBuf,
    pub managed: Vec<String>,
}
impl Accounts {
    pub fn installed(machine: &Machine) -> Self {
        Self {
            credentials: Path::new(STATE).join("credentials"),
            templates: PathBuf::from("/usr/lib/looom/accounts"),
            runtime: PathBuf::from("/run/looom/accounts"),
            managed: vec!["root".into(), machine.user.clone()],
        }
    }
    fn lock(&self) -> Result<Lock> {
        let _dir = trusted_dir(&self.credentials, true)?;
        Lock::acquire(&self.credentials.join(".lock"))
    }
    pub fn generate(&self) -> Result<()> {
        let _lock = self.lock()?;
        self.generate_locked()
    }
    fn generate_locked(&self) -> Result<()> {
        let source = fs::read_to_string(self.templates.join("shadow"))?;
        let mut lines = Zeroizing::new(String::new());
        let mut seen = std::collections::BTreeSet::new();
        for line in source.lines() {
            let mut fields: Vec<String> = line.split(':').map(str::to_owned).collect();
            ensure!(
                fields.len() == 9 && fields[1] == "!",
                "expected locked shadow template"
            );
            if self.managed.contains(&fields[0]) {
                ensure!(seen.insert(fields[0].clone()), "duplicate managed account");
                let value = read_private(&self.credentials.join(format!("{}.hash", fields[0])))?;
                validate_hash(value.trim_end_matches('\n'))?;
                fields[1] = value.trim_end_matches('\n').to_owned();
            }
            for (index, field) in fields.iter().enumerate() {
                if index > 0 {
                    lines.push(':');
                }
                lines.push_str(field);
            }
            lines.push('\n');
            use zeroize::Zeroize;
            fields.zeroize();
        }
        ensure!(
            seen.len() == self.managed.len(),
            "managed user missing from template"
        );
        let gshadow = fs::read_to_string(self.templates.join("gshadow"))?;
        locked_template(&gshadow, "gshadow")?;
        ensure!(
            gshadow
                .lines()
                .all(|line| line.split(':').nth(1) == Some("!")),
            "expected locked gshadow template"
        );
        mkdir(&self.runtime, 0o755)?;
        let _dir = trusted_dir(&self.runtime, false)?;
        private_atomic(&self.runtime.join("gshadow"), gshadow.as_bytes())?;
        private_atomic(&self.runtime.join("shadow"), lines.as_bytes())?;
        private_atomic(
            &self.credentials.join(".transaction.json"),
            b"{\"phase\":\"synchronized\"}\n",
        )?;
        Ok(())
    }
    pub fn set_password(&self, user: &str, password: &str) -> Result<()> {
        ensure!(
            self.managed.iter().any(|name| name == user),
            "unknown managed account"
        );
        let _lock = self.lock()?;
        let value = hash_password(password)?;
        let record = serde_json::to_vec(&serde_json::json!({"phase":"updating", "user":user}))?;
        private_atomic(&self.credentials.join(".transaction.json"), &record)?;
        private_atomic(
            &self.credentials.join(format!("{user}.hash")),
            value.as_bytes(),
        )?;
        failpoint("credential")?;
        self.generate_locked()
    }
}
pub fn locked_template(content: &str, name: &str) -> Result<String> {
    let count = match name {
        "passwd" => 7,
        "group" | "gshadow" => 4,
        "shadow" => 9,
        _ => bail!("invalid account template"),
    };
    let mut result = String::new();
    let mut names = std::collections::BTreeSet::new();
    for line in content.lines() {
        let mut fields: Vec<_> = line.split(':').collect();
        ensure!(
            fields.len() == count && !fields[0].is_empty() && names.insert(fields[0]),
            "invalid account template"
        );
        if name.contains("shadow") {
            fields[1] = "!";
        }
        result.push_str(&fields.join(":"));
        result.push('\n');
    }
    Ok(result)
}
pub fn import_shadow(config: &Config) -> Result<()> {
    let source = Zeroizing::new(fs::read_to_string("/etc/shadow")?);
    for user in ["root", config.accounts.user.name.as_str()] {
        let value = source
            .lines()
            .find(|l| l.split(':').next() == Some(user))
            .and_then(|l| l.split(':').nth(1))
            .context("managed credential missing from bootstrap")?;
        validate_hash(value)?;
        let destination = Path::new(STATE)
            .join("credentials")
            .join(format!("{user}.hash"));
        if destination.exists() {
            ensure!(
                read_private(&destination)?.trim() == value,
                "partial credential import differs; explicit recovery required"
            );
        } else {
            private_atomic(&destination, value.as_bytes())?;
        }
    }
    Ok(())
}
pub fn disable_dumps() -> Result<()> {
    let limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    ensure!(
        unsafe { libc::setrlimit(libc::RLIMIT_CORE, &limit) } == 0
            && unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0) } == 0,
        "cannot disable credential dumps"
    );
    Ok(())
}
