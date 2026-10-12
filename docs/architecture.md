# Architecture

looom manages complete system releases rather than wrapping pacman for the active
root. pacman resolves, verifies, and installs packages into an isolated build.
looom owns declarations, frozen inputs, Btrfs roots, per-release UKIs, boot selection,
credentials, recovery, and cleanup. The manager and application layer are written
in Rust; standard Arch tools retain their own dependencies.

## Two layers

| Layer | Managed by | Update and rollback behavior |
| --- | --- | --- |
| Base system: kernel, drivers, services, desktop, application infrastructure | `base.yaml` and looom releases | Build a new root; trial boot; explicitly confirm or roll back |
| User applications: Flatpak, Arch/Distrobox, AppImage | `apps.yaml` and user commands | Update independently; data and installations survive system rollback |

Application declarations ensure that named applications are present. Removing a
name does not uninstall it. User applications do not form an atomic system release.

## Disk and state layout

A new installation uses a 2 GiB FAT32 ESP and the remaining disk as Btrfs.

| Resource | Mount or path | Purpose |
| --- | --- | --- |
| `@root-<id>` | `/`, read-only | A system release, including its package inventory and configuration |
| `@bootstrap` and recovery generations | `/` when booted into recovery, writable | Repair and management outside the normal read-only root |
| `@home` | `/home`, writable | Personal files, desktop configuration, application installations/data |
| `@var` | `/var`, writable | Service data and package cache |
| `@state` | `/var/lib/looom`, writable, root-only | Machine identity, credentials, journals, release records, frozen inputs |
| ESP | `/efi`, writable, restricted permissions | Limine, generated menus, and release/recovery UKIs |

The registered machine profile binds filesystem UUIDs, persistent subvolumes,
and account identity. Hostname, packages, desktop, files, and units may change
through releases; the managed user identity and persistent mount contract remain stable.

## Configuration ownership

The release owns system settings. `files` declares selected regular files in
`/etc`, while typed settings generate protected resources such as `fstab` and
account templates. Arbitrary replacement of these resources is rejected.

Two explicit local directories remain writable across releases:
`/etc/NetworkManager/system-connections` and `/etc/looom-local`.
Personal desktop settings live in HOME and also survive rollback.
[Configuration rules](configuration.md) describe the supported schema and boundaries.

Passwords are separate from releases. Runtime `shadow`/`gshadow` files are generated
from protected persistent credentials, so a system rollback retains the current
password. Machine ID and SSH host keys also persist. See [credential security](credentials-security.md).

## Release lifecycle

1. **Check:** parse and validate the declaration and its file inputs.
2. **Lock:** resolve a complete package closure from the selected Arch Archive date;
   verify signatures and record package/repository hashes and the manager digest.
3. **Build:** freeze the declaration, manager recipe, binary, and file inputs;
   install exact archives in a private Btrfs root, configure it, build the UKI,
   verify the result, and create a read-only release snapshot.
4. **Publish:** synchronize the UKI and publish boot metadata without changing
   the permanent boot choice.
5. **Try:** select one next boot. Limine consumes the UEFI one-shot request before
   starting the kernel. Without confirmation, the next ordinary boot uses the old default.
6. **Confirm:** verify the actually running release and make it the permanent choice.
   Desktop, network, and sound usability still require a human check.

`rebuild` combines the first five steps. It stops on error and does not reboot
or confirm automatically. Build scripts receive private `/var` and `/boot` state;
the working machine's persistent data and ESP are not exposed for package scripts to write.

## Recovery and cleanup

Journals identify incomplete builds, publications, credential writes, recovery
updates, and removals. Recovery uses frozen inputs and the appropriate manager
recipe rather than reinterpreting changed source files. EFI publication and Btrfs
state are coordinated; they are not a single cross-filesystem transaction.

A direct firmware recovery entry can bypass a damaged Limine menu. Damaged recovery
UKIs or an unusable ESP require a live environment. There is no automatic watchdog
or universal guarantee against storage damage after power loss.

GC previews by default and protects running, saved, next, mounted, and required
confirmed releases, plus active/stable recovery generations. It removes only
owned release resources; credentials, HOME, shared `/var`, frozen inputs, and
operation evidence remain. [Command reference](native-rust.md) and
[boot recovery](limine.md) cover the operational procedures.

## Scope

The current implementation targets official Arch, x86_64/UEFI, one Btrfs filesystem,
one managed user, and a whole-disk installation. CachyOS, encryption, Secure Boot,
dual boot, general mount handlers, and broader hardware support remain future work.
Fixing input packages enables repeatable composition; byte-identical ISO or UKI
rebuilds are not promised.
