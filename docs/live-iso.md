# Installation and upgrades

The [looom 0.2.0 release](https://github.com/Kexfff/looom/releases/tag/v0.2.0)
provides a console installation and rescue ISO with the native manager included.
Installation requires internet access; the live environment is not a KDE desktop.

## Requirements

- x86_64, UEFI, Secure Boot disabled.
- A disk dedicated entirely to looom: at least 32 GiB; 64 GiB recommended.
- Official Arch packages, Btrfs, one managed user, no disk encryption or dual boot.
- For a VM: 4 vCPU and 8 GiB RAM recommended; VirtIO disk and network work.

## Install from the ISO

1. Download `looom-0.2.0-x86_64.iso` and its adjacent `.sha256` file.
   Verify them before writing the ISO to a USB drive or attaching it as a VM CD-ROM:

   ```sh
   sha256sum -c looom-0.2.0-x86_64.iso.sha256
   ```

2. Boot in UEFI mode. Ethernet uses DHCP automatically. For Wi-Fi, choose
   **Network help**, enter the shell, and connect with `iwctl`:

   ```text
   device list
   station <device> scan
   station <device> get-networks
   station <device> connect <SSID>
   exit
   ```

   Run `looom-live` to return to the menu. Live Wi-Fi connections are not
   automatically copied into the installed system.

3. Choose **1 — Install looom**. Select the target disk by its displayed number.
   Existing data is allowed, with a warning; **all data on that disk will be erased**.
   Mounted disks, active swap, and the installer workspace disk cannot be selected.
   Enter `0` or leave the disk choice empty to cancel.
4. Set the computer name, user name, timezone, locale, and keyboard layout.
   Leave the SSH public key field empty for a local installation, or provide
   a file containing your public key for later SSH access.
5. Review the disk, type `YES`, and enter separate root and user passwords.
   Password input is hidden. The ISO contains no preset login passwords or keys.
6. Wait for `Installation complete`. Shut down or restart, remove the installation
   medium, and boot from the installed disk. The installer does not reboot for you.
7. Sign in to KDE and check the desktop, network, and sound. The first-login
   `looom setup` menu offers system confirmation and application installation.
   System operations require sudo authentication. The initial boot is a trial;
   rebooting without confirmation returns to writable recovery.

You can also confirm from a terminal after checking the system:

```sh
sudo looom verify
sudo looom confirm
```

## Your configuration

The installer creates user-owned copies:

```text
~/looom/base.yaml
~/looom/base.lock
~/looom/apps.yaml
```

Edit the existing files rather than replacing the account identity or mount
contract. The default `apps.yaml` declares Firefox; install it with `looom setup`
or `looom apps apply` as your regular user. Existing declarations are preserved.

[System configuration](configuration.md) · [Applications](apps.md) ·
[System commands](native-rust.md)

## Upgrade an existing alpha installation

Reinstallation is not required. Download
`looom-0.2.0-x86_64-unknown-linux-gnu` and `SHA256SUMS` from the release.
Verify the binary against its manifest entry, then:

```sh
sudo install -D -m 0755 ./looom-0.2.0-x86_64-unknown-linux-gnu /var/lib/looom/tools/looom-0.2.0
sudo /var/lib/looom/tools/looom-0.2.0 rebuild ~/looom/base.yaml system-0-2-0
sudo reboot
# Check the new desktop before confirming.
sudo looom verify
sudo looom confirm system-0-2-0
```

For an iNiR installation, rebuild from its complete declaration rather than an
older KDE-only `base.yaml`. Existing HOME, passwords, applications, and declarations
remain in place. An existing empty `apps.yaml` is not replaced; add
`org.mozilla.firefox` yourself if desired.

Older GRUB installations use the separate [migration procedure](limine.md#grub-migration).
The original alpha binary does not include `rebuild`; use the new binary above.

## Rescue and SSH

Open the live shell to inspect an interrupted installation. Keep the workspace
path printed by the installer and use `looom install show <workspace>`.
A workspace in `/run` disappears on reboot. See [installer recovery](installer.md#resume-and-interruptions)
before repeating any partition or format operation.

For SSH into the live environment, add your own public key in the local console,
or set a temporary root password with `passwd`, then start `sshd`:

```sh
systemctl start sshd
```

These live settings are not copied into the installed system. Installed releases
use key-based SSH login; their automatically generated server host keys identify
the computer and do not grant user access. See [SSH keys](installer.md#ssh-keys).

For an installed system with a damaged boot menu, use its firmware recovery entry:
[boot and recovery](limine.md). ISO build instructions are in [development](development.md#build-the-iso).
