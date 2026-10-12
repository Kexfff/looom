# Advanced installer

`looom install` runs the native terminal wizard, including over SSH. The ready-made
[ISO](live-iso.md) is the simplest path. The same release binary can be copied to
an official Arch live environment, checked against `SHA256SUMS`, made executable,
and run as root:

```sh
chmod 755 ./looom
./looom install
```

The wizard installs KDE on a whole disk. It allows existing partitions with an
explicit erase warning, excludes mounted/unsupported disks, and uses numbered
selection and `YES` confirmation. Passwords are entered twice with hidden input.
For the complete user workflow, see [installation](live-iso.md).

## SSH keys

An optional public key file enables key-based login to the installed system.
Private client keys stay on your client. Password-based SSH login is disabled
in read-only releases; your user password is for KDE, local consoles, and sudo.

Server host keys are generated automatically and persist in `/var/lib/looom/ssh`
across release changes. These keys identify the server; they are separate from
user access keys in `authorized_keys`. Their presence on a clean installation
is expected even when you supplied no public key.

## Plan and apply

For automation or a workspace on a separate persistent disk:

```sh
./looom install plan /dev/nvme0n1 ./base.yaml /run/looom-session \
  --ssh-key /root/management.pub
./looom install show /run/looom-session
./looom install apply /run/looom-session --confirm YES
./looom install resume /run/looom-session
```

`plan` writes a root-only workspace without changing the selected disk. Use an
absolute path with trusted parents and no symlinks, on a different disk from
the installation target. To resume after reboot, keep it on persistent storage;
`/run` belongs to the live session and disappears.

The wizard uses `/run/looom-installer/<uuid>` and prints the path. With an empty,
unmounted parent, it creates a dedicated tmpfs capped at 50% RAM. This is why
8 GiB RAM is recommended for Plasma. Smaller machines should use `plan/apply`
with a workspace on another persistent device. The tmpfs limit does not reserve
all that memory upfront.

After preparing packages, the installer stores a copy of its plan, lock, and
journal in the installed State. This is a record, not an automatic reconstruction
of a lost workspace. `installation/result.json` records installer completion;
`looom status` records whether the system was later booted and confirmed.

## Disk identity and boot registration

The installer checks disk identity, size, partitions, filesystem UUIDs, mounts,
swap/holders, and frozen inputs before writing. Physical disks need a serial or WWN.
An identified QEMU/KVM disk can instead use the current boot ID, sysfs path,
and kernel `diskseq`. Such a plan is valid only during that live boot and stops
working after reattachment. Set a virtual serial/WWN beforehand if you need
resume across live-system restarts; retain the workspace too.

`plan --no-nvram` leaves the current firmware and BLI variables untouched while
writing the target ESP's fallback loader. The first boot then enters recovery;
run `looom try initial` there and reboot. Normal installation registers its own
Limine entry first in BootOrder plus a direct recovery UKI entry. Other disks'
EFI loaders are not overwritten.

## Password automation

`apply` and `resume` accept `--passwords-stdin`: bounded JSON with `root` and
`user` string values from a private input source. Keep secrets out of arguments,
YAML, history, and public logs. The workspace contains root-owned `0600` password
hash files and private installation data. Public evidence must be selected and
audited before export. See [credential security](credentials-security.md).

## Resume and interruptions

The journal tracks frozen inputs and pending/completed stages: verified packages,
partitioning, formatting, subvolumes, package installation, configuration,
bootstrap, initial build, publication, and boot registration.

- Package closure, signatures, repository hashes, and manager SHA are prepared
  **before disk erasure**. Disk identity is checked again before writing.
- `resume` skips completed partition/format stages. If `partition`, `esp-format`,
  or `root-format` is pending, automatic repetition is refused. Preserve the
  journal and inspect the disk; a fresh installation needs a fresh plan and
  another explicit erase confirmation.
- Ordinary interrupted stages can resume with matching inputs. Configured builds
  finish through recovery. Earlier incomplete initial builds can restart only
  when inputs match and nothing has been published; old inputs remain protected.
- Mounted partial roots must be explicitly unmounted before cleanup; the installer
  does not recursively delete mounted data.
- A completed resume does not reset passwords or boot selection.

The installer never reboots or confirms the first release automatically. Confirm
only after its real boot and usability checks.

The current contract excludes custom file resources during installation,
nonempty extra kernel arguments, manual partition layouts, OS import, offline
installation, encryption, and dual boot. After installation, supported file
resources can be added through [system configuration](configuration.md).
