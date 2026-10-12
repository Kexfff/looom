# iNiR on looom

This optional profile installs the [iNiR](https://github.com/snowarch/iNiR) 2.33.0
user shell on Niri. Upstream is pinned in `upstream-revision.txt`; Python is
3.12.15 and its dependencies are pinned in `python-requirements.lock`.
System packages come from the official Arch snapshot in your declaration.
The validated snapshot is `2026-10-07`. Keep KDE as a fallback session if desired.

## Install

Start with an installed looom system that supports `rebuild` and has internet
access. Run the user steps as your regular account.

For a VM, enable **VirtIO 3D acceleration** and **SPICE OpenGL**. If its XML
forces `device='virtio-vga'`, remove that attribute so the effective device is
`virtio-vga-gl`. The validated Niri session requires working VM 3D acceleration.

```sh
git clone https://github.com/Kexfff/looom.git ~/looom-src
cd ~/looom-src
bash examples/inir/install.sh prepare ~/looom/base.yaml
sudo looom rebuild ~/looom/inir/declaration/base.yaml inir-desktop
sudo reboot
```

If the VM's Breeze login screen renders only its background, prepare with:

```sh
bash examples/inir/install.sh prepare ~/looom/base.yaml --software-greeter
```

This adds software rendering for the **SDDM greeter only**; Niri still uses 3D.
Choose this before building. `prepare` will not overwrite an existing declaration
directory. For a separate experiment, set `LOOOM_INIR_DIR=~/looom/inir-other`
for both script calls.

After booting the new release, sign in through KDE, SSH, or a TTY as your user:

```sh
cd ~/looom-src
bash examples/inir/install.sh install
sudo looom verify
sudo looom confirm inir-desktop
```

Sign out and select **Niri** in SDDM. Future system rebuilds should use
`~/looom/inir/declaration/base.yaml` with a new release ID; its lock lives next
to that declaration. Preparation preserves the original account identity,
credential names, package-list inputs, and file sources.

The preparation script reads the installer's JSON-formatted YAML. For ordinary
YAML, copy the declaration and its sources into a separate directory, include
this profile's `packages.yaml` through `packages_from`, add
`desktop.sessions: [niri]`, and enable `bluetooth.service` and
`power-profiles-daemon.service`. Then build and run the same user-file installation.

## Configuration

| Path | Purpose |
| --- | --- |
| `~/.config/inir/config.json` | Shell preferences |
| `~/.config/niri/config.kdl`, `config.d/` | Compositor and shortcuts |
| `~/.config/quickshell/inir/` | Installed shell scripts and QML |
| `~/.config/systemd/user/inir.service` | Autostart in the Niri session |
| `~/looom/inir/backups/` | Private local pre-installation backups |
| `config-overrides.json` in this example | Portable profile defaults without personal data |

Defaults use Noto Sans, Papirus, reduced effects, and Breeze instead of the
optional Darkly AUR plugin. Edit `config-overrides.json` before installation or
use `inir settings`. Repeated installation reapplies these overrides.
The script also changes user terminal, GTK/KDE, and shell-profile settings after
creating a private backup. Do not publish that backup; it may contain personal data.

Shortcuts: **Super+Space** overview, **Super+,** settings, **Super+V** clipboard,
**Super+Return** terminal, **Super+/** help. Diagnose with:

```sh
systemctl --user status inir
journalctl --user -u inir
inir settings
inir restart
```

## Compatibility and limits

`looom-readonly.patch` disables host SDDM theme installation, host pacman update
checks, and package-changing menu actions. Update the base through looom and
applications through Flatpak/Distrobox/AppImage. iNiR self-updates are disabled
to preserve the pinned source and adaptation. Do not run the entire upstream
installer with host sudo or expect a container to install a host desktop session.

`packages.yaml` contains reviewed upstream Arch package groups and extra runtime/
build dependencies; pacman resolves the transitive closure. Optional AUR themes,
fonts, and Gowall are not installed automatically. `inir doctor` may suggest them;
do not apply its host package repairs to the read-only system.

A real SDDM/Niri session, panel, overview, settings, clipboard watcher, notifications,
terminal, and shell restart were validated in a VM. Bluetooth, brightness,
fingerprint, hardware power profiles, AI keys, and authenticated network integrations
were not validated. User shell settings survive system rollback and need separate
HOME backups. [Validation report](../../docs/runs/2026-10-11-inir/README.md).
