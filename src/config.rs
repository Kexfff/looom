use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path},
};
use std::{
    ffi::CString,
    fs::File as SourceFile,
    io::Read,
    os::fd::{AsRawFd, FromRawFd},
    os::unix::fs::OpenOptionsExt,
};
use yaml_rust2::scanner::{Scanner, Token, TokenType};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema: u32,
    pub source: Source,
    pub system: System,
    pub kernel: Kernel,
    pub desktop: Desktop,
    #[serde(default)]
    pub packages: Vec<String>,
    /// Resolved into packages before freezing the installer/build configuration.
    #[serde(default, skip_serializing)]
    pub packages_from: Vec<String>,
    pub accounts: Accounts,
    #[serde(default)]
    pub units: BTreeMap<String, UnitState>,
    #[serde(default)]
    pub files: BTreeMap<String, File>,
    #[serde(default)]
    pub persistent: Persistent,
    #[serde(default)]
    pub health: Health,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub profile: String,
    pub snapshot: String,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct System {
    pub hostname: String,
    pub timezone: String,
    pub locale: String,
    pub console_keymap: String,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Kernel {
    pub package: String,
    #[serde(default)]
    pub extra_command_line: Vec<String>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Desktop {
    pub environment: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<String>,
}
impl Desktop {
    pub fn includes(&self, name: &str) -> bool {
        self.environment == name || self.sessions.iter().any(|s| s == name)
    }
    pub fn graphical(&self) -> bool {
        self.environment != "none" || !self.sessions.is_empty()
    }
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackageSet {
    pub schema: u32,
    pub packages: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub foreign: Vec<String>,
}
impl PackageSet {
    pub fn validate(&self) -> Result<()> {
        if self.schema != 1 || !unique(&self.packages) || !self.packages.iter().all(|s| package(s))
        {
            bail!("invalid package set");
        }
        if !self.foreign.is_empty() {
            bail!(
                "package set contains foreign/AUR packages: {}; keep them in the container or package them separately, then explicitly remove the foreign field",
                self.foreign.join(", ")
            );
        }
        Ok(())
    }
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Accounts {
    pub user: User,
    pub root: Root,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct User {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub groups: Vec<String>,
    pub shell: String,
    pub password_secret: String,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Root {
    pub password_secret: String,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UnitState {
    Enabled,
    Disabled,
    Masked,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct File {
    pub source: Option<String>,
    pub content: Option<String>,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default = "root")]
    pub owner: String,
    #[serde(default = "root")]
    pub group: String,
    #[serde(default)]
    pub replace_package_file: bool,
}
fn root() -> String {
    "root".into()
}
fn default_mode() -> String {
    "0644".into()
}
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Persistent {
    #[serde(default)]
    pub directories: BTreeMap<String, Directory>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Directory {
    pub id: String,
    pub owner: String,
    pub group: String,
    pub mode: String,
}
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Health {
    #[serde(default)]
    pub required_units: Vec<String>,
}

pub fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.as_bytes()[0].is_ascii_lowercase()
        && s.bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}
fn package(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && !s.starts_with('-')
        && s.bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"@._+-".contains(&c))
}
fn mode(s: &str) -> bool {
    s.len() == 4 && s.starts_with('0') && s.bytes().all(|c| (b'0'..=b'7').contains(&c))
}
fn unit(s: &str) -> bool {
    !s.starts_with('-')
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"@._:-".contains(&c))
        && [".service", ".socket", ".timer", ".target"]
            .iter()
            .any(|x| s.ends_with(x))
}
fn target(s: &str) -> bool {
    s.starts_with("/etc/")
        && !s.contains("//")
        && !s.ends_with('/')
        && !s.chars().any(char::is_whitespace)
        && Path::new(s)
            .components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
        && !s.split('/').any(|c| c == "." || c == "..")
}
fn reserved(s: &str) -> bool {
    [
        "/etc/passwd",
        "/etc/group",
        "/etc/shadow",
        "/etc/gshadow",
        "/etc/subuid",
        "/etc/subgid",
        "/etc/fstab",
        "/etc/machine-id",
        "/etc/resolv.conf",
        "/etc/hostname",
        "/etc/hosts",
        "/etc/localtime",
        "/etc/locale.conf",
        "/etc/locale.gen",
        "/etc/vconsole.conf",
        "/etc/pacman.conf",
        "/etc/sudoers",
    ]
    .contains(&s)
        || [
            "/etc/looom",
            "/etc/kernel",
            "/etc/mkinitcpio.d",
            "/etc/pacman.d",
        ]
        .iter()
        .any(|p| s == *p || s.starts_with(&format!("{p}/")))
        || s == "/etc/mkinitcpio.conf"
        || s.contains(".wants/")
        || s.contains(".requires/")
        || s.starts_with("/etc/systemd/system/looom-")
        || [
            "NetworkManager",
            "sshd",
            "sddm",
            "shadow",
            "systemd-sysusers",
        ]
        .iter()
        .any(|u| s.starts_with(&format!("/etc/systemd/system/{u}.service")))
        || [
            "/etc/ssh/sshd_config.d/10-looom-vm.conf",
            "/etc/sudoers.d/10-looom-vm",
            "/etc/NetworkManager/conf.d/10-looom-readonly.conf",
            "/etc/sddm.conf.d/10-looom.conf",
        ]
        .contains(&s)
}
fn unique(list: &[String]) -> bool {
    list.iter().collect::<BTreeSet<_>>().len() == list.len()
}
fn date(s: &str) -> bool {
    if !s.is_ascii() || s.len() != 10 || &s[4..5] != "-" || &s[7..8] != "-" {
        return false;
    }
    let (Ok(y), Ok(m), Ok(d)) = (
        s[0..4].parse::<u32>(),
        s[5..7].parse::<u32>(),
        s[8..10].parse::<u32>(),
    ) else {
        return false;
    };
    let max = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
                29
            } else {
                28
            }
        }
        _ => 0,
    };
    y >= 2000 && d >= 1 && d <= max
}
pub(crate) fn yaml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let input = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .with_context(|| format!("read {}", path.display()))?;
    ensure_regular(&input)?;
    let mut raw = String::new();
    input.take(1024 * 1024 + 1).read_to_string(&mut raw)?;
    if raw.len() > 1024 * 1024 {
        bail!("configuration exceeds 1 MiB");
    }
    yaml_text(&raw).with_context(|| format!("schema {}", path.display()))
}
fn yaml_text<T: serde::de::DeserializeOwned>(raw: &str) -> Result<T> {
    let mut after_key = false;
    for Token(mark, kind) in Scanner::new(raw.chars()) {
        match &kind {
            TokenType::Anchor(_)
            | TokenType::Alias(_)
            | TokenType::Tag(_, _)
            | TokenType::TagDirective(_, _) => bail!(
                "{}:{}: YAML anchors, aliases and tags are unsupported",
                mark.line() + 1,
                mark.col() + 1
            ),
            TokenType::Scalar(_, s) if after_key && s == "<<" => {
                bail!("YAML merge keys are unsupported")
            }
            _ => {}
        }
        after_key = matches!(kind, TokenType::Key);
    }
    let value: serde_json::Value = serde_saphyr::from_str(raw)?;
    Ok(serde_json::from_value(value)?)
}
pub fn load(path: &Path) -> Result<(Config, BTreeMap<String, Vec<u8>>)> {
    let mut cfg: Config = yaml(path)?;
    cfg.validate()?;
    let directory = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()?;
    let mut packages: BTreeSet<String> = cfg.packages.iter().cloned().collect();
    for source in &cfg.packages_from {
        let raw = read_source(&directory, Path::new(source))
            .with_context(|| format!("confined package set: {source}"))?;
        let set: PackageSet = yaml_text(std::str::from_utf8(&raw)?)
            .with_context(|| format!("package set: {source}"))?;
        set.validate()?;
        packages.extend(set.packages);
    }
    if !cfg.packages_from.is_empty() {
        cfg.packages = packages.into_iter().collect();
    }
    if cfg.packages.len() > 10000 {
        bail!("too many package requests");
    }
    let mut bytes = BTreeMap::new();
    for (dest, file) in &cfg.files {
        let data = match (&file.source, &file.content) {
            (Some(source), None) => {
                let p = Path::new(source);
                if p.is_absolute() || p.components().any(|c| !matches!(c, Component::Normal(_))) {
                    bail!("file source must be a confined relative path: {dest}");
                }
                read_source(&directory, p)
                    .with_context(|| format!("confined file source: {dest}"))?
            }
            (None, Some(content)) => {
                if content.len() > 1024 * 1024 {
                    bail!("file content too large");
                }
                content.as_bytes().to_vec()
            }
            _ => bail!("exactly one source/content required: {dest}"),
        };
        bytes.insert(dest.clone(), data);
    }
    Ok((cfg, bytes))
}
fn read_source(directory: &Path, relative: &Path) -> Result<Vec<u8>> {
    let mut descriptor = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open("/")?;
    for part in directory.components().skip(1) {
        let name = CString::new(part.as_os_str().as_encoded_bytes())?;
        let fd = unsafe {
            libc::openat(
                descriptor.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        descriptor = unsafe { SourceFile::from_raw_fd(fd) };
    }
    let parts: Vec<_> = relative.components().collect();
    for (index, part) in parts.iter().enumerate() {
        let name = CString::new(part.as_os_str().as_encoded_bytes())?;
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | libc::O_NONBLOCK
            | if index + 1 < parts.len() {
                libc::O_DIRECTORY
            } else {
                0
            };
        let fd = unsafe { libc::openat(descriptor.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        descriptor = unsafe { SourceFile::from_raw_fd(fd) };
    }
    ensure_regular(&descriptor)?;
    let mut bytes = Vec::new();
    descriptor.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        bail!("source exceeds 1 MiB");
    }
    Ok(bytes)
}
fn ensure_regular(file: &SourceFile) -> Result<()> {
    if !file.metadata()?.is_file() {
        bail!("source must be a regular file");
    }
    Ok(())
}
impl Config {
    fn validate(&self) -> Result<()> {
        if self.schema != 1 || self.source.profile != "arch" || !date(&self.source.snapshot) {
            bail!("unsupported schema/profile or invalid snapshot date");
        }
        if !identifier(&self.system.hostname) || self.system.hostname.len() > 63 {
            bail!("invalid hostname");
        }
        for s in [
            &self.system.timezone,
            &self.system.locale,
            &self.system.console_keymap,
        ] {
            if s.is_empty()
                || s.starts_with('/')
                || s.contains("..")
                || !s
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"/_-.@".contains(&c))
            {
                bail!("invalid system setting");
            }
        }
        if !["linux", "linux-lts"].contains(&self.kernel.package.as_str())
            || !self.kernel.extra_command_line.is_empty()
        {
            bail!("MVP supports linux/linux-lts and no custom kernel arguments yet");
        }
        if !["none", "plasma", "niri"].contains(&self.desktop.environment.as_str())
            || !unique(&self.desktop.sessions)
            || self.desktop.sessions.iter().any(|s| {
                !["plasma", "niri"].contains(&s.as_str()) || *s == self.desktop.environment
            })
        {
            bail!("unsupported desktop");
        }
        if !unique(&self.packages) || !self.packages.iter().all(|s| package(s)) {
            bail!("invalid or duplicate package request");
        }
        if !unique(&self.packages_from)
            || self.packages_from.len() > 16
            || self.packages_from.iter().any(|s| {
                s.is_empty()
                    || s.len() > 1024
                    || Path::new(s).is_absolute()
                    || Path::new(s)
                        .components()
                        .any(|c| !matches!(c, Component::Normal(_)))
                    || s.split('/').any(|c| c == "." || c == ".." || c.is_empty())
            })
        {
            bail!("package sets must be unique confined relative paths");
        }
        // Stable personal identity is bound to the registered machine at build time.
        let u = &self.accounts.user;
        if !identifier(&u.name)
            || u.name == "root"
            || !(1000..65534).contains(&u.uid)
            || !(1000..65534).contains(&u.gid)
            || u.groups != ["wheel"]
            || u.shell != "/bin/bash"
            || u.password_secret != format!("login-{}", u.name)
            || self.accounts.root.password_secret != "login-root"
        {
            bail!(
                "one stable personal user, wheel, bash and login-<user>/login-root references required"
            );
        }
        let mut ids = BTreeSet::new();
        for (p, d) in &self.persistent.directories {
            if !target(p)
                || reserved(p)
                || !identifier(&d.id)
                || !ids.insert(&d.id)
                || !mode(&d.mode)
                || d.owner != "root"
                || d.group != "root"
            {
                bail!("invalid persistent resource: {p}");
            }
        }
        let dirs: Vec<_> = self.persistent.directories.keys().collect();
        for (i, a) in dirs.iter().enumerate() {
            for b in dirs.iter().skip(i + 1) {
                if b.starts_with(&format!("{a}/")) {
                    bail!("nested persistent directories conflict");
                }
            }
        }
        for (p, f) in &self.files {
            if !target(p) || reserved(p) || !mode(&f.mode) || f.owner != "root" || f.group != "root"
            {
                bail!("invalid/protected file resource: {p}");
            }
            if dirs.iter().any(|d| {
                p == *d || p.starts_with(&format!("{d}/")) || d.starts_with(&format!("{p}/"))
            }) {
                bail!("declarative/local ownership conflict: {p}");
            }
            if self
                .files
                .keys()
                .any(|q| q != p && q.starts_with(&format!("{p}/")))
            {
                bail!("file/child ownership conflict: {p}");
            }
        }
        for (name, state) in &self.units {
            if !unit(name) {
                bail!("invalid unit name");
            }
            if [
                "looom-accounts.service",
                "NetworkManager.service",
                "sshd.service",
                "qemu-guest-agent.service",
                "systemd-timesyncd.service",
                "shadow.timer",
            ]
            .contains(&name.as_str())
                && !matches!(state, UnitState::Enabled)
            {
                bail!("required unit cannot be disabled/masked: {name}");
            }
            if name == "sddm.service"
                && self.desktop.graphical()
                && !matches!(state, UnitState::Enabled)
            {
                bail!("graphical sessions require SDDM");
            }
        }
        if !unique(&self.health.required_units)
            || !self.health.required_units.iter().all(|s| unit(s))
        {
            bail!("invalid health units");
        }
        for s in &self.health.required_units {
            if matches!(
                self.units.get(s),
                Some(UnitState::Disabled | UnitState::Masked)
            ) {
                bail!("health unit is disabled/masked: {s}");
            }
        }
        Ok(())
    }
}
