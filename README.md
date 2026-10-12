# looom

looom is a Rust system manager for an atomic Arch Linux desktop. It builds
complete system releases from a declaration, boots them with a read-only Btrfs
root, and lets you return to a previous confirmed release. User applications
and personal data live separately from the base system.

**[Download looom 0.2.0](https://github.com/Kexfff/looom/releases/tag/v0.2.0)** —
installation ISO, native binary, build manifest, and SHA-256 checksums.

## Install

Boot the ISO in UEFI mode, connect to the internet, and select **Install looom**.
Choose the target disk by number, confirm `YES`, and set your own passwords.
The installer erases the entire selected disk and installs KDE Plasma, Limine,
and the first read-only release. No manual partitioning or compilation is needed.

After signing in to KDE, `looom setup` opens once. Check the desktop, network,
and sound, then confirm the system and install your declared applications.
The default application declaration contains Firefox as a user Flatpak.

Supported: official Arch, x86_64/UEFI, a whole disk, Btrfs, and one managed user.
The ISO is a console-based online installer. Secure Boot, disk encryption,
dual boot, CachyOS, and Calamares are not supported yet. For a VM, use 4 vCPU,
8 GiB RAM, and a 64 GiB disk.

[Installation and upgrades](docs/live-iso.md) ·
[Advanced installer usage](docs/installer.md)

## Everyday use

Your editable declarations are `~/looom/base.yaml` for the system and
`~/looom/apps.yaml` for applications. Keep the account identity and persistent
mounts from the installed system when editing `base.yaml`.

```sh
# Build and select a trial system release; use a new release name each time.
sudo looom rebuild ~/looom/base.yaml desktop-next
sudo reboot

# After checking the new desktop:
sudo looom verify
sudo looom confirm

# Apply or update user applications without rebuilding the system.
looom apps apply
looom apps update

# Return to a previously confirmed release.
sudo looom rollback initial
sudo reboot

# Preview cleanup before explicitly applying it.
sudo looom gc --keep 3
sudo looom gc --keep 3 --apply
```

`rebuild` runs check → lock → build → publish → try and stops on the first error.
Rebooting and confirming remain explicit. An unconfirmed trial returns to the
previous default on the next ordinary boot; a hung machine still needs a restart.

System rollback preserves passwords, HOME, applications, `/var`, and explicitly
allowed local settings. It does not undo application updates or personal desktop
configuration. Keep independent backups of your data.

## Documentation

- [System configuration](docs/configuration.md): supported YAML fields and ownership rules.
- [Applications](docs/apps.md): Flatpak, Distrobox, AppImage, and installer scripts.
- [Command reference](docs/native-rust.md): system releases, recovery, passwords, and GC.
- [Boot and recovery](docs/limine.md): Limine, recovery entries, and GRUB migration.
- [Architecture](docs/architecture.md): system boundaries and persistent state.
- [Credential security](docs/credentials-security.md): password storage and limitations.
- [Development](docs/development.md): VM-only builds, tests, and ISO generation.
- [Optional Niri/iNiR profile](examples/inir/README.md): install after the KDE system is ready.
- [Release notes](docs/releases/0.2.0.md) and [validation evidence](docs/runs/2026-10-12-release-0.2.0/README.md).

Version 0.2.0 passed VM acceptance covering ISO installation, KDE and Firefox,
container scripts, AppImage, trial fallback, rollback, password persistence, and GC.
Hardware coverage remains limited; see the validation report for its scope.
