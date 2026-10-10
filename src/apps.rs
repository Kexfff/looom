//! Persistent per-user applications. Host package operations never enter this layer.
use crate::{
    config,
    util::{hash, hash_file, string, sync_dir},
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub const TEMPLATE: &str = include_str!("../configs/installer/apps.yaml");
const CONTAINER: &str = "looom-arch";
const FLATHUB: &str = "https://dl.flathub.org/repo/";
#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Apps {
    pub schema: u32,
    #[serde(default)]
    pub flatpak: Vec<String>,
    #[serde(default)]
    pub arch: Arch,
    #[serde(default)]
    pub appimages: BTreeMap<String, AppImage>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Arch {
    #[serde(default = "arch_image")]
    pub image: String,
    #[serde(default)]
    pub packages: Vec<String>,
    #[serde(default)]
    pub exports: Vec<String>,
}
fn arch_image() -> String {
    "docker.io/library/archlinux:latest".into()
}
impl Default for Arch {
    fn default() -> Self {
        Self {
            image: arch_image(),
            packages: vec![],
            exports: vec![],
        }
    }
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AppImage {
    pub url: Option<String>,
    pub source: Option<String>,
    pub sha256: String,
    pub name: Option<String>,
}
fn token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 256
        && !s.starts_with('-')
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._+-".contains(&c))
}
fn unique(v: &[String]) -> bool {
    v.iter().collect::<BTreeSet<_>>().len() == v.len()
}
pub fn load(path: &Path) -> Result<Apps> {
    let cfg: Apps = config::yaml(path)?;
    ensure!(cfg.schema == 1, "unsupported apps schema");
    ensure!(
        unique(&cfg.flatpak)
            && cfg.flatpak.iter().all(|s| token(s)
                && s.split('.').count() >= 3
                && s.split('.').all(|s| !s.is_empty())),
        "invalid/duplicate Flatpak IDs"
    );
    ensure!(
        unique(&cfg.arch.packages) && cfg.arch.packages.iter().all(|s| token(s)),
        "invalid/duplicate Arch packages"
    );
    ensure!(
        unique(&cfg.arch.exports) && cfg.arch.exports.iter().all(|s| token(s)),
        "invalid/duplicate desktop exports"
    );
    // Fully qualified image avoids implicit registry searches; digest references are supported.
    ensure!(
        cfg.arch.image.len() <= 512
            && cfg.arch.image.contains('/')
            && !cfg.arch.image.starts_with('-')
            && cfg
                .arch
                .image
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"/._:@-".contains(&c)),
        "invalid qualified Arch image reference"
    );
    for (id, app) in &cfg.appimages {
        ensure!(config::identifier(id), "invalid AppImage ID");
        ensure!(
            app.sha256.len() == 64
                && app
                    .sha256
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
            "AppImage {id}: lowercase SHA256 required"
        );
        ensure!(
            app.name.as_ref().is_none_or(|n| !n.is_empty()
                && n.len() <= 256
                && !n.chars().any(char::is_control)),
            "invalid AppImage display name"
        );
        match (&app.url, &app.source) {
            (Some(url), None) => ensure!(
                url.starts_with("https://")
                    && url.len() <= 4096
                    && !url.chars().any(|c| c.is_control() || c.is_whitespace()),
                "AppImage requires HTTPS"
            ),
            (None, Some(source)) => ensure!(
                !source.is_empty() && !source.chars().any(char::is_control),
                "invalid AppImage source"
            ),
            _ => bail!("AppImage {id}: exactly one url/source required"),
        }
    }
    Ok(cfg)
}
/// One managed personal account; the fixed subordinate range survives every release.
pub(crate) fn configure_userns(root: &Path, user: &str) -> Result<()> {
    ensure!(config::identifier(user), "invalid user namespace account");
    for name in ["subuid", "subgid"] {
        crate::util::atomic(
            &root.join("etc").join(name),
            format!("{user}:100000:65536\n").as_bytes(),
            0o644,
        )?;
    }
    Ok(())
}
struct User {
    home: PathBuf,
    data: PathBuf,
    desktop: PathBuf,
}
impl User {
    fn load() -> Result<Self> {
        ensure!(
            unsafe { libc::geteuid() } != 0,
            "run looom apps as your regular user, without sudo"
        );
        ensure!(
            env::var_os("SUDO_USER").is_none(),
            "open a regular user session instead of sudo"
        );
        let home = PathBuf::from(env::var_os("HOME").context("HOME missing")?).canonicalize()?;
        let data = env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        // Require persistent home-backed storage rather than transient runtime paths.
        ensure!(
            data.is_absolute() && data.starts_with(&home),
            "XDG_DATA_HOME must be inside HOME"
        );
        Ok(Self {
            home,
            data: data.join("looom/apps"),
            desktop: data.join("applications"),
        })
    }
    fn config(&self) -> PathBuf {
        self.home.join("looom/apps.yaml")
    }
    fn image(&self, id: &str, app: &AppImage) -> PathBuf {
        self.data
            .join("appimages")
            .join(id)
            .join(format!("{}.AppImage", app.sha256))
    }
    fn lock(&self) -> Result<File> {
        fs::create_dir_all(&self.data)?;
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.data.join("operation.lock"))?;
        ensure!(
            unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
            "another apps operation is running"
        );
        Ok(f)
    }
}
fn save(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let parent = path.parent().context("missing parent")?;
    fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.as_file()
        .set_permissions(fs::Permissions::from_mode(mode))?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    sync_dir(parent)
}
fn cmd(program: &str) -> Command {
    let mut command = Command::new(program);
    command
        .env("DBX_CONTAINER_MANAGER", "podman")
        .env("DBX_NON_INTERACTIVE", "1")
        .env_remove("CONTAINER_HOST")
        .env_remove("CONTAINER_CONNECTION");
    command
}
fn run(mut command: Command) -> Result<()> {
    let status = command.status().context("start application tool")?;
    ensure!(
        status.success(),
        "application tool failed ({status}); rerun apply after resolving the error"
    );
    Ok(())
}
fn capture(program: &str, args: &[&str]) -> Result<String> {
    let out = cmd(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("start {program}"))?;
    ensure!(
        out.status.success(),
        "{program}: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8(out.stdout)?.trim().into())
}
fn container_exists() -> Result<bool> {
    let status = cmd("podman")
        .args(["container", "exists", CONTAINER])
        .stdin(Stdio::null())
        .status()?;
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!("cannot inspect rootless Podman; check your user session and subuid/subgid"),
    }
}
fn enter(args: &[String]) -> Command {
    let mut command = cmd("distrobox");
    command.args(["enter", "--name", CONTAINER]);
    if !unsafe { libc::isatty(libc::STDIN_FILENO) == 1 } {
        command.arg("--no-tty");
    }
    command.arg("--").args(args);
    command
}
fn ensure_arch(cfg: &Arch) -> Result<()> {
    let label = hash(cfg.image.as_bytes());
    if !container_exists()? {
        println!("Creating rootless {CONTAINER} ({})", cfg.image);
        let mut command = cmd("distrobox");
        command.args([
            "create",
            "--yes",
            "--name",
            CONTAINER,
            "--image",
            &cfg.image,
            "--additional-flags",
            &format!("--label=io.looom.image={label}"),
        ]);
        run(command)?;
    }
    let labels: Value = serde_json::from_str(&capture(
        "podman",
        &["inspect", "--format", "{{json .Config.Labels}}", CONTAINER],
    )?)?;
    ensure!(
        labels["io.looom.image"].as_str() == Some(label.as_str()),
        "looom-arch is unmanaged or uses a different image; preserve/export its data before explicitly replacing it"
    );
    let os = capture_enter(&["cat", "/etc/os-release"])?;
    ensure!(
        os.lines().any(|l| l == "ID=arch" || l == "ID=\"arch\""),
        "installer compatibility requires an Arch container"
    );
    Ok(())
}
fn capture_enter(args: &[&str]) -> Result<String> {
    let args: Vec<String> = args.iter().map(|s| (*s).into()).collect();
    let out = enter(&args).stdin(Stdio::null()).output()?;
    ensure!(
        out.status.success(),
        "container: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8(out.stdout)?)
}
fn script_command(script: &Path, arguments: &[String], root: bool) -> Result<Command> {
    let mut args = if root {
        vec!["sudo".into(), "-n".into(), "bash".into(), "--".into()]
    } else {
        // The shim only exists in the managed container. Absolute pacman paths
        // remain native and require sudo for writes; AUR builds stay unprivileged.
        run(enter(&[
            "sudo".into(), "-n".into(), "sh".into(), "-c".into(),
            "set -eu; install -d -m 755 /usr/local/lib/looom/compat; looom_shim=$(mktemp /usr/local/lib/looom/compat/.pacman.XXXXXX); trap 'rm -f -- \"$looom_shim\"' EXIT; printf '%s\\n' '#!/bin/sh' 'exec sudo -n /usr/bin/pacman \"$@\"' > \"$looom_shim\"; chmod 755 \"$looom_shim\"; mv -f -- \"$looom_shim\" /usr/local/lib/looom/compat/pacman".into(),
        ]))?;
        vec![
            "bash".into(),
            "-c".into(),
            "export PATH=/usr/local/lib/looom/compat:$PATH; exec bash -- \"$@\"".into(),
            "looom-script".into(),
        ]
    };
    args.push(string(script)?.into());
    args.extend(arguments.iter().cloned());
    let mut command = enter(&args);
    command.current_dir(script.parent().context("script directory")?);
    Ok(command)
}
fn export_packages(path: Option<&Path>) -> Result<()> {
    ensure!(
        container_exists()?,
        "no Arch container; run an installer or apps apply first"
    );
    let explicit: BTreeSet<String> = capture_enter(&["pacman", "-Qqe"])?
        .lines()
        .map(str::to_owned)
        .collect();
    let native: BTreeSet<String> = capture_enter(&["pacman", "-Qqn"])?
        .lines()
        .map(str::to_owned)
        .collect();
    ensure!(
        explicit.iter().all(|p| token(p)) && explicit.len() < 10000,
        "invalid container package inventory"
    );
    let set = config::PackageSet {
        schema: 1,
        packages: explicit.intersection(&native).cloned().collect(),
        foreign: explicit.difference(&native).cloned().collect(),
    };
    // JSON is strict YAML too. Foreign packages are visible and block base
    // inclusion until the user explicitly resolves them; never silently drop AUR.
    let mut bytes = serde_json::to_vec_pretty(&set)?;
    bytes.push(b'\n');
    if let Some(path) = path {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(path)
            .context("package set already exists or cannot be created")?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        sync_dir(
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?;
        println!(
            "Exported {} repository requests and {} foreign packages to {}",
            set.packages.len(),
            set.foreign.len(),
            path.display()
        );
    } else {
        std::io::stdout().write_all(&bytes)?;
    }
    Ok(())
}
fn flathub() -> Result<()> {
    let remotes = capture("flatpak", &["remotes", "--user", "--columns=name,url"])?;
    if let Some(line) = remotes
        .lines()
        .find(|l| l.split_whitespace().next() == Some("flathub"))
    {
        ensure!(
            line.split_whitespace().nth(1) == Some(FLATHUB),
            "existing flathub remote has a different URL"
        );
    } else {
        let mut command = cmd("flatpak");
        command.args([
            "remote-add",
            "--user",
            "--if-not-exists",
            "flathub",
            "https://dl.flathub.org/repo/flathub.flatpakrepo",
        ]);
        run(command)?;
    }
    Ok(())
}
fn flatpak_installed(id: &str) -> Result<bool> {
    let ids = capture(
        "flatpak",
        &["list", "--user", "--app", "--columns=application"],
    )?;
    Ok(ids.lines().any(|s| s == id))
}
fn install_image(user: &User, path: &Path, id: &str, app: &AppImage) -> Result<()> {
    let target = user.image(id, app);
    let parent = target.parent().context("image parent")?;
    fs::create_dir_all(parent)?;
    if target.try_exists()? {
        ensure!(
            hash_file(&target)? == app.sha256,
            "stored AppImage {id} was modified"
        );
    } else {
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        if let Some(url) = &app.url {
            let mut command = cmd("curl");
            command.args([
                "--fail",
                "--location",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--connect-timeout",
                "20",
                "--max-time",
                "900",
                "--max-filesize",
                "2147483648",
                "--output",
                string(temp.path())?,
                url,
            ]);
            run(command)?;
        } else {
            let source = PathBuf::from(app.source.as_ref().context("source")?);
            let source = if source.is_absolute() {
                source
            } else {
                path.parent().unwrap_or(Path::new(".")).join(source)
            };
            let mut input = File::open(&source)
                .with_context(|| format!("AppImage source {}", source.display()))?
                .take(2147483649);
            let length = std::io::copy(&mut input, &mut temp)?;
            ensure!(length <= 2147483648, "AppImage exceeds 2 GiB");
        }
        ensure!(
            hash_file(temp.path())? == app.sha256,
            "AppImage {id}: SHA256 mismatch; nothing activated"
        );
        let mut magic = [0u8; 11];
        File::open(temp.path())?.read_exact(&mut magic)?;
        ensure!(
            &magic[..4] == b"\x7fELF" && &magic[8..10] == b"AI" && [1, 2].contains(&magic[10]),
            "not a supported AppImage"
        );
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o755))?;
        temp.as_file().sync_all()?;
        temp.persist(&target).map_err(|e| e.error)?;
        sync_dir(parent)?;
    }
    // A native launcher avoids embedding arbitrary filesystem paths in Desktop Exec.
    save(
        &parent.join("active.json"),
        &serde_json::to_vec(app)?,
        0o600,
    )?;
    let name = app.name.as_deref().unwrap_or(id).replace('\\', "\\\\");
    save(&user.desktop.join(format!("looom-{id}.desktop")), format!("[Desktop Entry]\nType=Application\nName={name}\nExec=/usr/bin/looom apps launch {id} -- %U\nTerminal=false\nCategories=Utility;\nX-Looom-AppImage=true\n").as_bytes(), 0o644)?;
    Ok(())
}
fn apply(user: &User, path: &Path, cfg: &Apps, update: bool) -> Result<()> {
    flathub()?;
    for id in &cfg.flatpak {
        if flatpak_installed(id)? {
            ensure!(
                capture("flatpak", &["info", "--user", "--show-origin", id])? == "flathub",
                "{id} is installed from another origin"
            );
            if update {
                let mut command = cmd("flatpak");
                command.args(["update", "--user", "--noninteractive", id]);
                run(command)?;
            }
        } else {
            let mut command = cmd("flatpak");
            command.args(["install", "--user", "--noninteractive", "flathub", id]);
            run(command)?;
        }
    }
    let use_arch =
        !cfg.arch.packages.is_empty() || !cfg.arch.exports.is_empty() || container_exists()?;
    if use_arch {
        ensure_arch(&cfg.arch)?;
    }
    if use_arch && (update || !cfg.arch.packages.is_empty()) {
        let mut args = vec![
            "sudo".into(),
            "-n".into(),
            "pacman".into(),
            if update { "-Syu" } else { "-S" }.into(),
            "--needed".into(),
            "--noconfirm".into(),
        ];
        args.extend(cfg.arch.packages.iter().cloned());
        run(enter(&args))?;
    }
    for app in &cfg.arch.exports {
        run(enter(&[
            "distrobox-export".into(),
            "--app".into(),
            app.clone(),
        ]))?;
    }
    for (id, app) in &cfg.appimages {
        install_image(user, path, id, app)?;
    }
    let mut commits = BTreeMap::new();
    for id in &cfg.flatpak {
        commits.insert(
            id,
            capture("flatpak", &["info", "--user", "--show-commit", id])?,
        );
    }
    let container_image = if use_arch {
        Some(capture(
            "podman",
            &["inspect", "--format", "{{.Image}}", CONTAINER],
        )?)
    } else {
        None
    };
    save(
        &user.data.join("applied.json"),
        &serde_json::to_vec_pretty(
            &json!({"schema":1,"declaration":cfg,"flatpak_commits":commits,"container_image":container_image}),
        )?,
        0o600,
    )?;
    println!("Applications applied; manual applications and previous AppImages retained");
    Ok(())
}
fn status(user: &User, cfg: &Apps) -> Result<()> {
    let mut apps = BTreeMap::new();
    for id in &cfg.flatpak {
        apps.insert(id, flatpak_installed(id)?);
    }
    let mut images = BTreeMap::new();
    for (id, app) in &cfg.appimages {
        let p = user.image(id, app);
        images.insert(id, p.try_exists()? && hash_file(&p)? == app.sha256);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"flatpak":apps,"arch_container":container_exists()?,"appimages":images,"last_apply":user.data.join("applied.json").is_file()})
        )?
    );
    Ok(())
}
pub fn dispatch(args: &[String]) -> Result<()> {
    let op = args.first().map(String::as_str).unwrap_or("--help");
    if ["--help", "-h"].contains(&op) {
        println!(
            "looom apps init|check|plan|apply|status|update [apps.yaml]\nlooom apps exec -- <command...>\nlooom apps run-script [--root] <file.sh> [arguments...]\nlooom apps export-packages [new-file.yaml]\n\nDefault declaration: ~/looom/apps.yaml. Run as your regular user.\nApply ensures presence; update explicitly updates declared Flatpaks and Arch.\nRun-script executes as the container user; bare pacman uses container sudo. --root is explicit. Scripts must live inside shared HOME.\nExport-packages creates a reviewable packages_from input for base.yaml; it does not change host packages.\nSystem rollback preserves apps and their data; apply does not remove manual apps."
        );
        return Ok(());
    }
    // Offline check/plan can also be used by the builder/administrator.
    if ["check", "plan"].contains(&op) && args.len() == 2 {
        let path = Path::new(&args[1]);
        let cfg = load(path)?;
        if op == "plan" {
            println!("{}", serde_json::to_string_pretty(&cfg)?);
        } else {
            println!("Valid apps schema 1");
        }
        return Ok(());
    }
    let user = User::load()?;
    let _lock = user.lock()?;
    if op == "init" {
        ensure!(args.len() <= 2, "apps init [apps.yaml]");
        let path = args
            .get(1)
            .map(PathBuf::from)
            .unwrap_or_else(|| user.config());
        fs::create_dir_all(
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?;
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&path)
            .context("declaration already exists or cannot be created")?;
        f.write_all(TEMPLATE.as_bytes())?;
        f.sync_all()?;
        println!("Created {}", path.display());
        return Ok(());
    }
    if op == "launch" {
        ensure!(
            args.len() >= 2 && config::identifier(&args[1]),
            "apps launch <AppImage-id> [-- arguments...]"
        );
        let id = &args[1];
        let app: AppImage = serde_json::from_slice(&fs::read(
            user.data.join("appimages").join(id).join("active.json"),
        )?)?;
        ensure!(
            app.sha256.len() == 64 && app.sha256.bytes().all(|c| c.is_ascii_hexdigit()),
            "invalid active AppImage digest"
        );
        let path = user.image(id, &app);
        ensure!(
            hash_file(&path)? == app.sha256,
            "active AppImage was modified"
        );
        let mut command = Command::new(path);
        let tail = &args[2..];
        command.args(if tail.first().map(String::as_str) == Some("--") {
            &tail[1..]
        } else {
            tail
        });
        // Release management lock before a long-lived GUI process starts.
        drop(_lock);
        return run(command);
    }
    if ["exec", "run-script", "export-packages"].contains(&op) {
        let path = user.config();
        let cfg = if path.try_exists()? {
            load(&path)?
        } else {
            Apps {
                schema: 1,
                ..Default::default()
            }
        };
        if op == "export-packages" {
            ensure!(args.len() <= 2, "apps export-packages [new-file.yaml]");
            ensure!(container_exists()?, "no Arch container to export");
            ensure_arch(&cfg.arch)?;
            return export_packages(args.get(1).map(Path::new));
        }
        if op == "exec" {
            ensure!(
                args.len() >= 3 && args[1] == "--",
                "apps exec -- <command...>"
            );
            ensure_arch(&cfg.arch)?;
            drop(_lock);
            return run(enter(&args[2..]));
        }
        let root = args.get(1).map(String::as_str) == Some("--root");
        let index = if root { 2 } else { 1 };
        let path = args
            .get(index)
            .context("apps run-script [--root] <file.sh> [arguments...]")?;
        ensure!(
            !path.starts_with('-'),
            "script path cannot start with an option"
        );
        let script = Path::new(path).canonicalize()?;
        ensure!(
            script.starts_with(&user.home) && script.is_file(),
            "put the script and its supporting files inside HOME first"
        );
        ensure_arch(&cfg.arch)?;
        let command = script_command(&script, &args[index + 1..], root)?;
        drop(_lock);
        return run(command);
    }
    ensure!(
        ["check", "plan", "apply", "status", "update"].contains(&op) && args.len() <= 2,
        "unknown apps operation; see looom apps --help"
    );
    let path = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| user.config());
    let cfg = load(&path)?;
    match op {
        "check" => println!("Valid apps schema 1"),
        "plan" => println!("{}", serde_json::to_string_pretty(&cfg)?),
        "status" => status(&user, &cfg)?,
        _ => apply(&user, &path, &cfg, op == "update")?,
    }
    Ok(())
}
