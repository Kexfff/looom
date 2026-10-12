# Boot and recovery

New installations use Limine and a separate Unified Kernel Image (UKI) for each
release. The bootloader comes from the same pinned Arch snapshot as the system.
looom requires support for the UEFI Boot Loader Interface `LoaderEntryOneShot`.
Snapshot `2026-10-07` supplies Limine `12.9.3-1`.

## Trial boots and rollback

```sh
sudo looom rebuild ~/looom/base.yaml desktop-next
sudo reboot
# Check the new desktop before confirming.
sudo looom verify
sudo looom confirm

sudo looom rollback initial
sudo reboot
```

Build/publication leave the permanent default unchanged. `try` selects one next
boot through the UEFI BLI variable; Limine consumes it before starting the kernel.
Without confirmation, another ordinary boot uses the previous default.
A hang needs an explicit restart; there is no automatic watchdog or boot counting.
The first installation uses writable recovery as its default until `initial`
is confirmed. looom rejects an unknown one-shot request from another bootloader.

## Boot files

Paths below are relative to the ESP mounted at `/efi`:

| Path | Purpose |
| --- | --- |
| `EFI/looom/liminex64.efi` | Pinned bootloader |
| `EFI/looom/limine.conf` | Generated menu and permanent default |
| `EFI/BOOT/BOOTX64.EFI`, `EFI/BOOT/limine.conf` | New installation's fallback loader/menu |
| `EFI/Linux/looom-<id>.efi` | Published release UKI |
| `EFI/Linux/looom-bootstrap.efi` | Stable writable recovery UKI |

`/var/lib/looom/bootloader.json` selects the backend. Its absence indicates a
legacy GRUB machine. `machine.json` retains its schema so older account services
can still read it after rollback. Do not edit these files to force a migration.

## Damaged menus and interrupted operations

Installation and `looom boot-recovery` register a firmware entry named
`looom-recovery-<root-UUID-prefix>`. It boots the recovery UKI directly, without
the Limine configuration. Select it in the firmware menu, then run:

```sh
sudo looom recover
sudo looom status
sudo looom try <healthy-release-id>
sudo reboot
# After checking the healthy release:
sudo looom verify
sudo looom confirm
```

If the permanent selection is missing or invalid, menu recovery uses the recovery
system as default until you explicitly choose and confirm a healthy release.
On a migrated machine with an old recovery manager, use
`sudo /var/lib/looom/boot-manager recover` instead.

`recover` reconciles journaled operations. Publication uses synchronized temporary
UKI files and metadata, but FAT writes cannot guarantee survival of every power
loss. A damaged unselected validated candidate can be republished; published,
confirmed, or selected images are not silently replaced. A damaged recovery UKI,
ESP, or firmware boot path requires a live environment.

## GRUB migration

No disk formatting is needed. Obtain and verify a current manager binary; below,
`./looom-new` means that binary, not the old installed manager. First prepare a
release containing the new manager and Limine:

```sh
sudo ./looom-new lock ~/looom/base.yaml
sudo ./looom-new build ~/looom/base.yaml limine-ready
sudo ./looom-new publish limine-ready
sudo ./looom-new try limine-ready
sudo reboot
# This boot is still through GRUB.
sudo looom verify
sudo looom confirm
sudo looom boot-migrate
sudo looom boot-recovery
sudo reboot
sudo looom verify
sudo looom status
```

Migration requires a confirmed permanent release and no pending GRUB trial.
It keeps the previous GRUB EFI image, menu, and fallback, installs Limine,
retains the default release, and registers a new firmware entry. Repeating it
on an already migrated machine does nothing.

The current manager is also retained at `/var/lib/looom/boot-manager`. After
rolling back to an old release or recovery, use this binary through sudo to
manage Limine. Older `/usr/bin/looom` versions only know their GRUB backend.
Update recovery through the `bootstrap-update` → `bootstrap-try` → reboot →
`bootstrap-confirm` workflow when ready; see [system commands](native-rust.md#recovery-generations).
