use anyhow::{Context, Result, bail, ensure};
use looom::{
    builder::{self, Bundle},
    config,
    credentials::{self, Accounts},
    machine::{self, Machine},
    packages,
    releases::Manager,
    util::*,
};
use std::{env, fs, io::Read, path::Path, process::ExitCode};
use zeroize::Zeroizing;

fn run() -> Result<()> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    let basename = env::args().next().and_then(|v| {
        Path::new(&v)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
    });
    if basename.as_deref() == Some("looom-password") {
        args.insert(0, "password".into());
    }
    let op=args.first().map(String::as_str).context("Usage: looom check|init|lock|plan <base.yaml>; build <base.yaml> <id>; status|recover|publish|try|confirm|rollback|reject [id]; password <user> [--stdin]; accounts generate|check; gc [--keep N] [--apply]")?;
    if op == "--version" {
        println!("looom {} (native Rust)", looom::VERSION);
        return Ok(());
    }
    if op == "--help" || op == "-h" {
        println!(
            "looom {} — native Rust system releases\n\ncheck|init|bootstrap|lock|plan <base.yaml>\nboot-entry | boot-recovery\nbuild <base.yaml> <release-id>\nstatus | verify | recover\npublish|try|rollback|reject <release-id>\nconfirm [release-id]\npassword <user> [--stdin]\naccounts generate|check\ngc [--keep N] [--apply]\n\nBuild never changes the boot choice. Reboot separately after try/rollback.\nGC previews candidates unless --apply is given.",
            looom::VERSION
        );
        return Ok(());
    }
    if op == "boot-recovery" {
        ensure!(args.len() == 1, "boot-recovery takes no arguments");
        return looom::bootstrap::boot_recovery();
    }
    if op == "accounts" {
        root()?;
        ensure!(args.len() == 2, "accounts generate|check");
        if args[1] == "check" {
            ensure!(
                succeeds(
                    "pwck",
                    &["-qr", "/etc/passwd", "/run/looom/accounts/shadow"]
                ) && succeeds(
                    "grpck",
                    &["-r", "/etc/group", "/run/looom/accounts/gshadow"]
                ),
                "runtime account integrity check failed"
            );
            return Ok(());
        }
        ensure!(args[1] == "generate", "unknown accounts operation");
        credentials::disable_dumps()?;
        Accounts::installed(&Machine::load()?).generate()?;
        return Ok(());
    }
    if op == "password" {
        root()?;
        credentials::disable_dumps()?;
        ensure!(
            args.len() == 2 || args.len() == 3 && args[2] == "--stdin",
            "password <user> [--stdin]"
        );
        let accounts = Accounts::installed(&Machine::load()?);
        let password = if args.len() == 3 {
            let mut value = Zeroizing::new(String::new());
            std::io::stdin().take(4098).read_to_string(&mut value)?;
            ensure!(value.len() <= 4097, "password input too long");
            if value.ends_with('\n') {
                value.pop();
            }
            value
        } else {
            let value = Zeroizing::new(rpassword::prompt_password("New password: ")?);
            let repeat = Zeroizing::new(rpassword::prompt_password("Repeat password: ")?);
            ensure!(*value == *repeat, "passwords differ");
            value
        };
        accounts.set_password(&args[1], &password)?;
        println!("Persistent password and runtime credentials synchronized");
        return Ok(());
    }
    if op == "boot-entry" {
        ensure!(args.len() == 1, "boot-entry takes no arguments");
        return looom::bootstrap::boot_entry();
    }
    if ["check", "init", "bootstrap", "lock", "plan", "build"].contains(&op) {
        ensure!(
            args.len() == if op == "build" { 3 } else { 2 },
            "invalid argument count"
        );
        let path = Path::new(&args[1]);
        let (cfg, files) = config::load(path)?;
        match op {
            "check" => {
                println!(
                    "Valid schema 1; {} packages, {} files, {} persistent directories",
                    cfg.packages.len(),
                    files.len(),
                    cfg.persistent.directories.len()
                );
            }
            "init" => machine::initialize(&cfg)?,
            "bootstrap" => looom::bootstrap::prepare(&cfg)?,
            "lock" => {
                let m = Machine::load()?;
                m.guard()?;
                m.config_contract(&cfg)?;
                packages::resolve(&cfg, &packages::lock_path(path))?;
            }
            "plan" => {
                let lock = packages::load(path, &cfg)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&builder::plan(&cfg, &files, &lock)?)?
                );
            }
            "build" => {
                let lock = packages::load(path, &cfg)?;
                let request = packages::request(&cfg)?;
                let manager = Manager::installed(Machine::load()?)?;
                let _lock = manager.lock()?;
                builder::build(
                    &manager,
                    Bundle {
                        schema: 2,
                        config: cfg,
                        files,
                        request,
                        lock,
                        release: args[2].clone(),
                    },
                )?;
            }
            _ => unreachable!(),
        }
        return Ok(());
    }
    if op == "internal-finalize" {
        ensure!(args.len() == 2, "frozen input directory required");
        let manager = Manager::installed(Machine::load()?)?;
        let _lock = manager.lock()?;
        return builder::finalize(&manager, Path::new(&args[1]));
    }
    let manager = Manager::installed(Machine::load()?)?;
    if op == "verify" {
        ensure!(args.len() == 1, "verify takes no arguments");
        let rid = fs::read_to_string("/etc/looom/release-id")?;
        manager.health(rid.trim())?;
        let bundle: Bundle = serde_json::from_slice(&fs::read("/usr/lib/looom/declaration.json")?)?;
        ensure!(
            packages::inventory(None)? == bundle.lock.inventory(),
            "running package inventory differs from frozen lock"
        );
        if manager.load(rid.trim())?.engine.as_deref() == Some("rust-0.2") {
            ensure!(
                hash_file(Path::new("/usr/bin/looom"))?
                    == bundle.request["recipe_sha256"].as_str().unwrap_or(""),
                "running manager differs from frozen input"
            );
            ensure!(
                !Path::new("/usr/lib/looom/source/scripts").exists(),
                "legacy project backend installed"
            );
        }
        command("pacman", &["-Dk"])?;
        ensure!(
            succeeds(
                "pwck",
                &["-qr", "/etc/passwd", "/run/looom/accounts/shadow"]
            ) && succeeds(
                "grpck",
                &["-r", "/etc/group", "/run/looom/accounts/gshadow"]
            ),
            "account integrity check failed"
        );
        println!(
            "PASS: booted read-only release {}, kernel, persistent mounts, units, accounts, exact inventory and native manager",
            rid.trim()
        );
        return Ok(());
    }
    if op == "status" {
        ensure!(args.len() == 1, "status takes no arguments");
        return manager.status();
    }
    if op == "recover" {
        ensure!(args.len() == 1, "recover takes no arguments");
        builder::recover_builds(&manager)?;
        let _lock = manager.lock()?;
        manager.recover_publications()?;
        for metadata in manager.list()? {
            if metadata.phase == "removing" {
                manager.finish_removal(&metadata)?;
            }
        }
        manager.write_menu(&manager.menu(None)?)?;
        println!("Published menu restored; incomplete candidates remain unselected");
        return Ok(());
    }
    let _lock = manager.lock()?;
    if op == "gc" {
        let mut keep = 2;
        let mut apply = false;
        let mut index = 1;
        while index < args.len() {
            match args[index].as_str() {
                "--apply" => apply = true,
                "--keep" => {
                    index += 1;
                    keep = args.get(index).context("missing keep count")?.parse()?;
                }
                _ => bail!("gc accepts --keep N and --apply"),
            };
            index += 1;
        }
        return manager.gc(keep, apply);
    }
    let inferred = if op == "confirm" && args.len() == 1 {
        Some(
            fs::read_to_string("/etc/looom/release-id")?
                .trim()
                .to_owned(),
        )
    } else {
        None
    };
    let rid = if let Some(id) = inferred.as_deref() {
        id
    } else {
        ensure!(args.len() == 2, "release ID required");
        &args[1]
    };
    match op {
        "publish" => manager.publish(rid),
        "try" | "confirm" | "rollback" => manager.select(op, rid),
        "reject" => manager.reject(rid),
        _ => bail!("unknown operation {op}"),
    }
}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("looom: {e:#}");
            ExitCode::FAILURE
        }
    }
}
