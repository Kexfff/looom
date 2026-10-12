# looom 0.2.0 validation

Date: 2026-10-12. Product builds, execution, and tests ran **only in VMs**.
The host was used for editing, public-file transfer, libvirt orchestration,
artifact checksums, and publication.

## Inputs and environment

Build source: `13eabe8244dbb245278b789fd67ef5749e2f50c7`.
Arch Archive `2026-10-07`; Archiso `91-1`; Rust/Cargo 1.99.0;
`SOURCE_DATE_EPOCH=1791763200`. Documentation changes do not alter those assets.

| Artifact | SHA-256 |
| --- | --- |
| Native binary | `d89ecd0dc663df14d52167f3a9c2c39f504f2db97e825ebc2c65ecbed906f17b` |
| ISO | `b4ce836240c108c6854f123bb86684f27bab0b71ef7332a220b14755afc9519d` |

[Checksums](SHA256SUMS), [build manifest](looom-0.2.0-build.txt),
[live packages](looom-0.2.0-packages.txt), [Rust source hashes](source.sha256),
and [installed result](result.json). The ISO is 1,706,964,992 bytes;
the installed base contains 720 signed packages.

Rust/build VM: `Cachyos`, MAC `52:54:00:7b:23:63`, 6 vCPU/8 GiB,
60 GiB VirtIO/Btrfs. Its confirmed running root stayed read-only; builds used
separate writable toolchain and Archiso roots.

Clean acceptance VM: `Looom-release-0.2.0`, MAC `52:54:00:20:02:00`,
4 vCPU/8 GiB, 64 GiB VirtIO, UEFI/OVMF with separate NVRAM, Secure Boot off.
The installer used an explicitly prepared public package cache to speed up
acceptance; this was not a cold-download timing measurement. Final inputs,
hashes, and signatures were still verified by the installer. Private credentials
and keys were not transferred between VMs; a management public key was supplied
explicitly for the test and is absent from the ISO.

## Results

| Check | Evidence |
| --- | --- |
| Clippy without warnings; all 12 Rust tests passed | [rust.log](rust.log) |
| Real ISO installation and boot into read-only `initial`, user-owned declarations, services and inventory | [boot.log](boot.log) |
| Actual KDE login and first-login setup; sudo-authenticated confirmation | [setup-confirm.log](setup-confirm.log), [first-login.png](first-login.png) |
| Default user Flatpak Firefox opened a real window | [firefox.png](firefox.png) |
| Declared AppImage, rootless Arch container, real pacman installer script, literal arguments, host remained read-only | [apps.log](apps.log) |
| Unconfirmed trial returned to `initial` on the next ordinary reboot | [fallback.log](fallback.log) |
| New declaration added a system file; candidate booted and was confirmed | [trial-configured.log](trial-configured.log) |
| Manual rollback removed that file while preserving passwords, apps, HOME and local settings | [rollback.log](rollback.log) |
| GC removed a rejected root/UKI and protected the running and next-selected releases | [gc.log](gc.log) |
| Native setup password change; real login with the new password after changing system roots | [password.log](password.log), [setup-password.log](setup-password.log) |
| Required desktop/audio/portal services active individually; one-time welcome; explicit app update | [final.log](final.log), [status.log](status.log) |

The Rust suite also exercises credential permissions/ACLs/concurrency,
bootstrap recovery, publication, protected roots, and interrupted GC removals.
The acceptance controllers are [test-release-iso.py](../../../scripts/test-release-iso.py)
and [test-release-desktop.py](../../../scripts/test-release-desktop.py).
Early controller mistakes involving pre-login rootless checks and JSON-formatted
declarations were corrected before the final results. Private baselines were
not replaced to bypass mismatches. Older detailed diagnostics remain in Git history.

## Security and scope

The ISO payload was checked for absence of SSH private/host keys, authorized
keys, password hashes, builder history, machine State, and private package-keyring
files. Its machine ID was `uninitialized`. Selected logs and images were audited
on their source VMs against local credentials and old/new private login fixtures
before export: [build audit](public-audit.log), [desktop audit](desktop-public-audit.log).
Private fixtures, State backups, and credentials were not published.

This is VM acceptance, not universal hardware certification. The VM had no
Bluetooth/Wi-Fi adapter; Bluetooth was enabled but radio operation was not tested.
PipeWire services were tested, not physical sound or microphones. Suspend/resume,
NVIDIA, encryption, Secure Boot, dual boot, and a fresh physical installation of
this ISO were outside the run.
