use crate::{config::Config, machine::STATE, util::*};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

const BASE: &str = include_str!("../configs/bootstrap/packages.txt");
const DESKTOP: &str = "plasma-desktop plasma-workspace plasma-nm plasma-pa kscreen xdg-desktop-portal-kde sddm dolphin konsole mesa pipewire pipewire-pulse wireplumber ttf-dejavu noto-fonts spice-vdagent";
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub archive: String,
    pub sha256: String,
    pub signature: String,
    pub signature_sha256: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Repository {
    pub database_sha256: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackageLock {
    pub schema_version: u32,
    pub kind: String,
    pub request: Value,
    pub architecture: String,
    pub archive_date: String,
    pub repositories: BTreeMap<String, Repository>,
    pub package_signatures: String,
    pub packages: Vec<Package>,
}
pub fn request(config: &Config) -> Result<Value> {
    Ok(
        json!({"source":config.source,"kernel":config.kernel.package,"desktop":config.desktop.environment,"packages":config.packages,"recipe_sha256":hash_file(&std::env::current_exe()?)?, "engine":"rust-0.2"}),
    )
}
pub fn requested(config: &Config) -> Vec<String> {
    let mut result: BTreeSet<String> = BASE
        .lines()
        .map(str::trim)
        .filter(|p| !p.is_empty() && !p.starts_with('#') && *p != "linux" && *p != "python")
        .map(str::to_owned)
        .collect();
    result.extend(config.packages.iter().cloned());
    result.insert(config.kernel.package.clone());
    result.insert("dbus-broker-units".into());
    if config.desktop.environment == "plasma" {
        result.extend(DESKTOP.split_whitespace().map(str::to_owned));
    }
    result.into_iter().collect()
}
pub fn config(snapshot: &str, database: bool) -> String {
    let date = snapshot.replace('-', "/");
    format!(
        "[options]\nArchitecture = x86_64\nCheckSpace\nParallelDownloads = 5\nSigLevel = Required DatabaseOptional\nLocalFileSigLevel = Required\n{}[core]\nServer = https://archive.archlinux.org/repos/{date}/$repo/os/$arch\n[extra]\nServer = https://archive.archlinux.org/repos/{date}/$repo/os/$arch\n",
        if database {
            "DBPath = /usr/lib/looom/pacman\n"
        } else {
            ""
        }
    )
}
pub fn lock_path(yaml: &Path) -> PathBuf {
    yaml.parent().unwrap_or(Path::new(".")).join("base.lock")
}
pub fn load(yaml: &Path, config: &Config) -> Result<PackageLock> {
    let result: PackageLock = serde_json::from_slice(
        &fs::read(lock_path(yaml)).context("base.lock missing; run looom lock")?,
    )?;
    ensure!(
        result.request == request(config)?,
        "base.lock does not match package request or native recipe; run looom lock explicitly"
    );
    result.validate(config, false)?;
    Ok(result)
}
impl PackageLock {
    pub fn validate(&self, cfg: &Config, archives: bool) -> Result<()> {
        ensure!(
            self.schema_version == 1
                && self.kind == "looom-package-lock"
                && self.architecture == "x86_64"
                && self.archive_date == cfg.source.snapshot.replace('-', "/"),
            "unsupported or inconsistent lock"
        );
        ensure!(
            !self.packages.is_empty() && self.packages.len() < 10000,
            "invalid package closure size"
        );
        ensure!(
            self.repositories
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                == BTreeSet::from(["core", "extra"]),
            "invalid repository set"
        );
        let hexadecimal = |s: &str| {
            s.len() == 64
                && s.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        };
        let filename = |s: &str| {
            !s.is_empty()
                && !s.starts_with('.')
                && Path::new(s).file_name().and_then(|v| v.to_str()) == Some(s)
                && s.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"@._+:-".contains(&c))
        };
        let mut names = BTreeSet::new();
        for p in &self.packages {
            ensure!(
                names.insert(p.name.clone())
                    && filename(&p.name)
                    && filename(&p.archive)
                    && p.signature == p.archive.clone() + ".sig"
                    && hexadecimal(&p.sha256)
                    && hexadecimal(&p.signature_sha256),
                "invalid/duplicate package identity"
            );
            ensure!(
                !p.version.is_empty() && !p.version.chars().any(char::is_whitespace),
                "invalid package version"
            );
            if archives {
                for (name, sha) in [(&p.archive, &p.sha256), (&p.signature, &p.signature_sha256)] {
                    let path = Path::new("/var/cache/pacman/pkg").join(name);
                    ensure!(
                        fs::symlink_metadata(&path)?.is_file() && hash_file(&path)? == *sha,
                        "locked archive/signature hash mismatch: {}",
                        p.name
                    );
                }
            }
        }
        ensure!(
            requested(cfg).iter().all(|p| names.contains(p)),
            "lock does not contain requested packages"
        );
        for (repo, value) in &self.repositories {
            ensure!(
                hexadecimal(&value.database_sha256),
                "invalid repository digest"
            );
            if archives {
                ensure!(
                    hash_file(
                        &Path::new(STATE)
                            .join("repository-cache")
                            .join(&self.archive_date)
                            .join(format!("{repo}.db"))
                    )? == value.database_sha256,
                    "repository database hash mismatch"
                );
            }
        }
        Ok(())
    }
    pub fn inventory(&self) -> BTreeMap<String, String> {
        self.packages
            .iter()
            .map(|p| (p.name.clone(), p.version.clone()))
            .collect()
    }
}
pub fn inventory(root: Option<&Path>) -> Result<BTreeMap<String, String>> {
    let data = if let Some(root) = root {
        chroot_output(root, "pacman", &["-Q"])?
    } else {
        output("pacman", &["-Q"])?
    };
    data.lines()
        .map(|l| {
            l.split_once(' ')
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .context("invalid pacman inventory")
        })
        .collect()
}
pub fn resolve(cfg: &Config, destination: &Path) -> Result<()> {
    root()?;
    mkdir(&Path::new(STATE).join("dev"), 0o700)?;
    let _lock = Lock::acquire(&Path::new(STATE).join("release-control.lock"))?;
    let work = tempfile::Builder::new()
        .prefix("rust-resolve-")
        .tempdir_in(Path::new(STATE).join("dev"))?;
    let db = work.path().join("db");
    mkdir(&db, 0o700)?;
    let conf = work.path().join("pacman.conf");
    fs::write(&conf, config(&cfg.source.snapshot, false))?;
    let log = work.path().join("pacman.log");
    let args = vec![
        "--config",
        string(&conf)?,
        "--dbpath",
        string(&db)?,
        "--cachedir",
        "/var/cache/pacman/pkg",
        "--logfile",
        string(&log)?,
        "--gpgdir",
        "/etc/pacman.d/gnupg",
        "--noconfirm",
    ];
    let mut sync = args.clone();
    sync.push("-Sy");
    command("pacman", &sync)?;
    let requests = requested(cfg);
    let mut print = args.clone();
    print.extend(["-Sp", "--print-format", "%n %v %r %f"]);
    print.extend(requests.iter().map(String::as_str));
    let data = output("pacman", &print)?;
    let mut packages = Vec::new();
    for line in data.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        ensure!(
            fields.len() == 4 && ["core", "extra"].contains(&fields[2]),
            "unexpected package resolution"
        );
        let [name, version, repo, archive] = [fields[0], fields[1], fields[2], fields[3]];
        ensure!(
            Path::new(archive).file_name().and_then(|p| p.to_str()) == Some(archive),
            "invalid archive filename"
        );
        let url = format!(
            "https://archive.archlinux.org/repos/{}/{repo}/os/x86_64/{archive}",
            cfg.source.snapshot.replace('-', "/")
        );
        for (file, url) in [
            (archive.to_owned(), url.clone()),
            (format!("{archive}.sig"), format!("{url}.sig")),
        ] {
            let path = Path::new("/var/cache/pacman/pkg").join(&file);
            if !path.exists() {
                let mut temporary = tempfile::Builder::new()
                    .prefix(".looom-download-")
                    .tempfile_in("/var/cache/pacman/pkg")?;
                command(
                    "curl",
                    &[
                        "--fail",
                        "--location",
                        "--retry",
                        "2",
                        "--output",
                        string(temporary.path())?,
                        &url,
                    ],
                )?;
                temporary.as_file_mut().sync_all()?;
                temporary.persist(&path).map_err(|e| e.error)?;
                sync_dir(path.parent().unwrap())?;
            }
        }
        packages.push(Package {
            name: name.into(),
            version: version.into(),
            archive: archive.into(),
            sha256: hash_file(&Path::new("/var/cache/pacman/pkg").join(archive))?,
            signature: format!("{archive}.sig"),
            signature_sha256: hash_file(
                &Path::new("/var/cache/pacman/pkg").join(format!("{archive}.sig")),
            )?,
        });
    }
    let mut verify = args;
    verify.push("-Sw");
    verify.extend(requests.iter().map(String::as_str));
    command("pacman", &verify)?;
    let mut repositories = BTreeMap::new();
    let cache = Path::new(STATE)
        .join("repository-cache")
        .join(cfg.source.snapshot.replace('-', "/"));
    mkdir(&cache, 0o700)?;
    for repo in ["core", "extra"] {
        let source = db.join("sync").join(format!("{repo}.db"));
        repositories.insert(
            repo.into(),
            Repository {
                database_sha256: hash_file(&source)?,
            },
        );
        atomic(&cache.join(format!("{repo}.db")), &fs::read(source)?, 0o600)?;
    }
    packages.sort_by(|a, b| a.name.cmp(&b.name));
    let lock = PackageLock {
        schema_version: 1,
        kind: "looom-package-lock".into(),
        request: request(cfg)?,
        architecture: "x86_64".into(),
        archive_date: cfg.source.snapshot.replace('-', "/"),
        repositories,
        package_signatures: "Required; pacman verified".into(),
        packages,
    };
    lock.validate(cfg, true)?;
    json(destination, &lock, 0o600)?;
    println!(
        "Locked {} signed packages: {}",
        lock.packages.len(),
        destination.display()
    );
    Ok(())
}
