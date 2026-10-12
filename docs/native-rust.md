# System command reference

looom 0.2.0 manages the base system in Rust. It calls standard Arch tools to
resolve packages and construct releases; it does not install packages into the
active read-only root. User applications use [separate commands](apps.md).

## Everyday workflow

```sh
# Regular user; also available in the KDE application menu.
looom setup
looom setup --status

# System administration.
sudo looom status
sudo looom rebuild ~/looom/base.yaml desktop-next
sudo reboot
# After checking the new desktop:
sudo looom verify
sudo looom confirm

sudo looom rollback initial
sudo reboot
```

`setup` offers confirmation, application installation/update, password changes,
rebuilding, status, and a cleanup preview. System actions use the installed manager
through sudo; application actions run as the regular user. Run setup without sudo.
`--status` only prints paths and the current release. KDE opens `--first-login`
once per HOME; its marker records that the menu was shown, not that the system
was confirmed or applications were installed. The offline guide is
`/usr/share/looom/GETTING-STARTED.txt`.

## Release commands

| Command | Behavior |
| --- | --- |
| `looom check <base.yaml>` | Validate schema and file inputs; no build or boot selection |
| `sudo looom lock <base.yaml>` | Resolve and verify the pinned package closure; write adjacent `base.lock` |
| `sudo looom plan <base.yaml>` | Compare packages, files, and units with the current system; do not refresh the lock |
| `sudo looom build <base.yaml> <id>` | Build and validate a private root, UKI, and read-only snapshot |
| `sudo looom publish <id>` | Publish the validated release; leave the permanent boot choice unchanged |
| `sudo looom try <id>` | Select one next boot |
| `sudo looom rebuild <base.yaml> <id>` | Run check → lock → build → publish → try; stop on error |
| `sudo looom verify` | Check the running root, kernel, mounts, accounts, inventory, manager, and required units |
| `sudo looom confirm [id]` | Make the healthy, actually running release the permanent choice |
| `sudo looom rollback <id>` | Select a previously confirmed release; reboot separately |
| `sudo looom reject <id>` | Reject an unused unconfirmed candidate; retain it for diagnosis/GC |
| `sudo looom status` | Show the running release, boot selections, and release records |
| `sudo looom recover` | Reconcile interrupted builds, publication, menus, recovery updates, and removals |

`rebuild` reads the declaration and file inputs once. An existing build/release ID
is rejected before resolution; failures retain private state for diagnosis.
There is no automatic reboot or confirmation. An unconfirmed trial returns to
the old default on the next ordinary boot. Restart a hung machine yourself.

Frozen build inputs are under `/var/lib/looom/inputs/<id>`. The installed recipe
is under `/usr/lib/looom/source`; each release's package database is under
`/usr/lib/looom/pacman`. The request includes the exact manager SHA-256.
See [configuration](configuration.md) for schema and lock rules.

## Passwords and accounts

```sh
sudo looom password <user-name>
sudo looom accounts check
```

Password prompts are hidden. `password <user-name> --stdin` is available for
controlled private automation; never place passwords in command arguments.
`accounts generate` regenerates runtime account files and is normally invoked
by the boot service. Direct `passwd`/GUI password changes are unsupported.
[Credential security](credentials-security.md) explains storage and limitations.

## Garbage collection

```sh
sudo looom gc --keep 3
sudo looom gc --keep 3 --apply
```

The default is a preview; `--apply` is required for removal. `--keep` defaults to 2
and must be at least 2. GC retains the latest required confirmed releases and
always protects the running, saved, next, mounted, and active recovery resources.
A published candidate must first be explicitly rejected before it can be removed.

Only owned roots, UKIs, and release metadata are removed. HOME, shared `/var`,
credentials, frozen inputs, operation journals, and evidence are retained.
Interrupted removals are resumed by `recover` with protection checks repeated.
Keeping these inputs means GC does not reclaim every byte associated with a build.

## Recovery generations

```sh
sudo looom bootstrap-list
sudo looom bootstrap-update <confirmed-running-release> [generation-id]
sudo looom bootstrap-try
sudo reboot
# In the actually booted recovery candidate:
sudo looom bootstrap-confirm
```

This creates a writable recovery root from the healthy confirmed running release.
The recovery candidate boots into `multi-user.target`; KDE packages remain present.
Confirmation replaces the stable recovery UKI while leaving the saved system
release unchanged. To repeat an update from the same source release, supply a
new generation ID.

`bootstrap-recover` reconciles the update journal across FAT and Btrfs; `recover`
also performs this step. Stable and previous recovery roots and EFI copies are
kept. `bootstrap-list` shows history; GC can remove obsolete completed generations.
Older managers that only understand the previous singleton journal should not
manage the new history; use the current recovery manager instead.

## Attach a prepared Arch installation

The ISO installer is the supported route for a new whole-disk installation.
Advanced attachment uses a prepared writable Arch root, FAT ESP mounted at `/efi`,
one unencrypted Btrfs filesystem, and separate writable `/home`, `/var`, and
`/var/lib/looom` subvolumes. The declared user must already exist with the same identity.
Additional mounts and persistent-resource migrations are rejected.

```sh
sudo looom bootstrap /path/to/base.yaml
sudo looom boot-entry
```

`bootstrap` does not partition the disk. It registers the machine, imports protected
identity/credentials, creates the recovery UKI, and initializes boot infrastructure.
It journals its work; retries require matching declaration, kernel, and mounts.
A completed retry does not replace passwords or boot selection. `init <base.yaml>`
registers or validates infrastructure that has already been prepared.

`boot-entry` registers the firmware entry separately. `boot-recovery` creates a
direct firmware recovery entry. `boot-migrate` migrates an eligible legacy GRUB
installation to Limine; use the [migration guide](limine.md#grub-migration).
Do not modify `machine.json` or boot metadata manually to bypass their guards.

[Boot and recovery](limine.md) · [Advanced installer](installer.md) ·
[Development and tests](development.md)
