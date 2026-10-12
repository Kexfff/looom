//! First login and a small user-facing entry point to the existing managers.
use crate::{apps, builder::write, config};
use anyhow::{Context, Result, ensure};
use std::{
    env,
    fs::{self, OpenOptions},
    io::{self, BufRead, Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::Command,
};

const GUIDE: &str = "looom 0.2.0 — everyday use\n\nYour system configuration: ~/looom/base.yaml\nYour applications: ~/looom/apps.yaml\n\nRun looom setup from a regular user terminal for first login and maintenance.\nFirefox is a user Flatpak; installing it needs internet and may take several minutes.\nEdit apps.yaml before applying it if you prefer other applications. Existing declarations are preserved.\n\nAfter checking your desktop, network and sound, confirm the trial system.\nUnconfirmed trials return to the previous system or recovery on the next ordinary boot.\n\nSystem change: sudo looom rebuild ~/looom/base.yaml <new-release-name>\nThen reboot, check the desktop, and sudo looom confirm.\nPreview: sudo looom lock ~/looom/base.yaml; sudo looom plan ~/looom/base.yaml\nRollback: sudo looom rollback <previous-confirmed-release>; sudo reboot\nCleanup preview: sudo looom gc --keep 3\nApply cleanup: sudo looom gc --keep 3 --apply\n\nApplications: looom apps apply | status | update\nArch container: looom apps exec -- bash\nInstaller script: looom apps run-script ~/Downloads/setup.sh\nHost pacman does not install into the read-only system.\n\nPassword: sudo looom password <your-user-name>\nPersonal files, app data and passwords survive system rollback.\nUser desktop settings are not reverted by a system rollback; back them up separately.\n\nSupport: https://github.com/Kexfff/looom\n";

pub(crate) fn install(root: &Path, plasma: bool) -> Result<()> {
    write(root, "usr/share/looom/GETTING-STARTED.txt", GUIDE, 0o644)?;
    write(
        root,
        "usr/share/applications/looom-setup.desktop",
        "[Desktop Entry]\nType=Application\nName=looom System Setup\nName[ru]=Настройка looom\nComment=Applications, system confirmation, passwords and maintenance\nExec=/usr/bin/looom setup\nIcon=preferences-system\nTerminal=true\nCategories=System;Settings;\n",
        0o644,
    )?;
    if plasma {
        write(
            root,
            "etc/xdg/autostart/looom-first-login.desktop",
            "[Desktop Entry]\nType=Application\nName=looom First Login\nExec=/usr/bin/looom setup --first-login\nIcon=preferences-system\nTerminal=false\nOnlyShowIn=KDE;\nX-KDE-autostart-after=panel\n",
            0o644,
        )?;
    }
    Ok(())
}

fn choice(label: &str) -> Result<String> {
    print!("{label}: ");
    io::stdout().flush()?;
    let mut answer = String::new();
    ensure!(
        io::stdin().lock().take(4097).read_line(&mut answer)? > 0 && answer.len() <= 4096,
        "input closed or too long"
    );
    Ok(answer.trim().to_owned())
}

fn privileged(args: &[&str]) -> Result<()> {
    // The installed root-owned manager is always used, never a user-supplied executable.
    let status = Command::new("/usr/bin/sudo")
        .arg("--")
        .arg("/usr/bin/looom")
        .args(args)
        .status()?;
    ensure!(
        status.success(),
        "system operation failed; the error above explains why"
    );
    Ok(())
}

fn application_config(home: &Path) -> Result<PathBuf> {
    let path = home.join("looom/apps.yaml");
    if !path.try_exists()? {
        apps::dispatch(&["init".into()])?;
    }
    apps::load(&path)?;
    Ok(path)
}

fn summary(home: &Path) -> Result<()> {
    println!("looom {} — system setup", crate::VERSION);
    println!(
        "Running system: {}",
        fs::read_to_string("/etc/looom/release-id")?.trim()
    );
    println!(
        "System declaration: {}",
        home.join("looom/base.yaml").display()
    );
    println!("Applications: {}", home.join("looom/apps.yaml").display());
    println!("Guide: /usr/share/looom/GETTING-STARTED.txt\n");
    Ok(())
}

pub fn dispatch(args: &[String]) -> Result<()> {
    if matches!(args, [op] if ["--help", "-h"].contains(&op.as_str())) {
        println!(
            "looom setup [--status|--first-login]\nRun as your regular desktop user. All modifying actions are selected explicitly.\n--status prints paths and the running release without installing or confirming anything."
        );
        return Ok(());
    }
    ensure!(
        args.is_empty()
            || matches!(args, [op] if ["--status", "--first-login"].contains(&op.as_str())),
        "setup [--status|--first-login]"
    );
    ensure!(
        unsafe { libc::geteuid() } != 0 && env::var_os("SUDO_USER").is_none(),
        "run looom setup as your regular user, without sudo"
    );
    ensure!(
        Path::new("/etc/looom/release-id").is_file(),
        "setup requires an installed looom system"
    );
    let home = PathBuf::from(env::var_os("HOME").context("HOME missing")?).canonicalize()?;
    let first = args.first().map(String::as_str) == Some("--first-login");
    let marker = home.join(".local/state/looom/welcome-seen");
    if first && marker.try_exists()? {
        return Ok(());
    }
    let terminal = unsafe { libc::isatty(libc::STDIN_FILENO) == 1 };
    if first && !terminal {
        ensure!(
            env::var_os("WAYLAND_DISPLAY").is_some() || env::var_os("DISPLAY").is_some(),
            "first login requires a desktop session; run looom setup in a terminal instead"
        );
        let status = Command::new("/usr/bin/konsole")
            .args([
                "--separate",
                "-e",
                "/usr/bin/looom",
                "setup",
                "--first-login",
            ])
            .status()?;
        ensure!(
            status.success(),
            "could not open setup; run looom setup in a terminal"
        );
        return Ok(());
    }
    summary(&home)?;
    if args.first().map(String::as_str) == Some("--status") || !terminal {
        println!("Run looom setup in a terminal for the interactive menu.");
        return Ok(());
    }
    if first {
        fs::create_dir_all(marker.parent().context("marker parent")?)?;
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&marker)
        {
            Ok(mut file) => {
                writeln!(file, "{}", crate::VERSION)?;
                file.sync_all()?;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        println!(
            "Welcome. First check your desktop, network and sound.\nConfirm only when they work. Install your user applications separately.\n"
        );
    }
    loop {
        println!(
            "1) Install declared applications (default: Firefox)\n2) Check and confirm the running system\n3) System and release status\n4) Application status\n5) Update declared applications\n6) Change my password\n7) Rebuild from a system declaration\n8) Preview old release cleanup\n9) Getting started guide\n0) Close"
        );
        let answer = match choice("Choice [0]") {
            Ok(answer) => answer,
            Err(_) => return Ok(()),
        };
        let result = (|| -> Result<()> {
            match answer.as_str() {
                "" | "0" => return Ok(()),
                "1" | "5" => {
                    let path = application_config(&home)?;
                    let cfg = apps::load(&path)?;
                    println!("Declared Flatpaks: {}", cfg.flatpak.join(", "));
                    println!(
                        "Arch packages: {}; AppImages: {}. Internet may be required.",
                        cfg.arch.packages.len(),
                        cfg.appimages.len()
                    );
                    if choice("Proceed? Type YES")? == "YES" {
                        apps::dispatch(&[if answer == "1" { "apply" } else { "update" }.into()])?;
                    }
                }
                "2" => {
                    privileged(&["verify"])?;
                    privileged(&["confirm"])?;
                }
                "3" => privileged(&["status"])?,
                "4" => {
                    application_config(&home)?;
                    apps::dispatch(&["status".into()])?;
                }
                "6" => {
                    let declaration: serde_json::Value =
                        serde_json::from_slice(&fs::read("/usr/lib/looom/declaration.json")?)?;
                    let user = declaration["config"]["accounts"]["user"]["name"]
                        .as_str()
                        .context("managed user")?;
                    privileged(&["password", user])?;
                }
                "7" => {
                    let input = choice(&format!(
                        "System YAML [{}]",
                        home.join("looom/base.yaml").display()
                    ))?;
                    let path = if input.is_empty() {
                        home.join("looom/base.yaml")
                    } else {
                        PathBuf::from(input)
                    };
                    let path = path.canonicalize()?;
                    config::load(&path)?;
                    let id = choice("New release name")?;
                    ensure!(config::identifier(&id), "invalid release name");
                    let path = path.to_str().context("UTF-8 declaration path")?;
                    privileged(&["rebuild", path, &id])?;
                    println!(
                        "Trial selected. Reboot when ready, check the new desktop and confirm it."
                    );
                }
                "8" => privileged(&["gc", "--keep", "3"])?,
                "9" => println!("{GUIDE}"),
                _ => println!("Choose a number from the menu."),
            }
            Ok(())
        })();
        if answer.is_empty() || answer == "0" {
            return Ok(());
        }
        if let Err(error) = result {
            eprintln!("{error:#}");
        }
        println!();
    }
}
