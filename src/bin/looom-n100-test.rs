//! Acceptance entry point for the explicitly authorized physical N100 pilot.
//! Never runs the destructive VM runtime fixtures and never enters a release.
#[path = "support/acceptance.rs"]
mod acceptance;
use anyhow::{Result, ensure};
use looom::{credentials, machine::Machine, util::*};
use std::fs;
fn run() -> Result<()> {
    root()?;
    credentials::disable_dumps()?;
    ensure!(
        !succeeds("systemd-detect-virt", &["--vm"]),
        "physical hardware required"
    );
    ensure!(
        fs::read_to_string("/sys/class/dmi/id/product_name")?.trim() == "MINI S",
        "unexpected board"
    );
    ensure!(
        fs::read_to_string("/proc/cpuinfo")?.contains("Intel(R) N100"),
        "unexpected CPU"
    );
    ensure!(
        fs::read_to_string("/sys/class/net/enp1s0/address")?.trim() == "e8:ff:1e:d0:be:47",
        "unexpected authorized NIC"
    );
    ensure!(
        output("lsblk", &["-ndo", "MODEL", "/dev/nvme0n1"])? == "512GB SSD",
        "unexpected internal disk"
    );
    let machine = Machine::load()?;
    machine.guard()?;
    ensure!(
        machine.user == "owner"
            && !machine.guest_agent
            && !machine.serial_console
            && !machine.passwordless_sudo,
        "unexpected physical profile"
    );
    acceptance::run()
}
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("N100 acceptance: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
