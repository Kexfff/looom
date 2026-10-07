//! Acceptance entry point restricted to the original disposable VM.
#[path = "support/acceptance.rs"]
mod acceptance;
use anyhow::{Result, ensure};
use looom::{credentials, util::*};
fn run() -> Result<()> {
    root()?;
    credentials::disable_dumps()?;
    ensure!(
        matches!(
            output("systemd-detect-virt", &["--vm"])?.as_str(),
            "qemu" | "kvm"
        ),
        "VM harness only"
    );
    ensure!(
        std::fs::read_to_string("/sys/class/net/enp1s0/address")?.trim() == "52:54:00:7b:23:63",
        "unexpected VM identity"
    );
    acceptance::run()
}
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("VM acceptance: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
