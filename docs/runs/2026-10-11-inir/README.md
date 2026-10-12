# iNiR validation

Date: 2026-10-11. All builds, installation, QML checks, and functional tests ran
inside `Looom-feedback-limine`: MAC `52:54:00:10:10:11`, 4 vCPU/4 GiB,
64 GiB VirtIO, UEFI/Limine. The host only edited/transferred files and operated
the VM console. The result was confirmed read-only `inir-desktop-v2`, kernel
`7.2.9-arch1-1`, using 924 signed packages from Arch snapshot `2026-10-07`.

## Inputs

- iNiR 2.33.0, commit `48cdcecb0f6089cfbb7019054602eab28e154b3b`.
- [Portable profile and instructions](../../../examples/inir/README.md).
- [System declaration](base.yaml), [lock](base.lock), [package list](inir-packages.yaml).
- Python 3.12.15 and pinned user dependencies from the example profile.
- [Profile hashes](profile.sha256), [manager hash](binary.sha256), [test hash](test.sha256).

The profile preserves KDE as a fallback and adds Niri/Quickshell. The readonly
patch blocks iNiR host package mutation and host update/theme paths. User settings
and private backups stay in HOME; credentials and backup contents were not exported.

## Graphics and checks

The VM required VirtIO 3D acceleration and SPICE OpenGL. Removing an explicitly
forced `device='virtio-vga'` allowed `virtio-vga-gl` and virgl. A separate Breeze
greeter issue was resolved through a declarative software-rendering setting for
SDDM only. Niri and Quickshell then ran in a real SDDM seat0 session, not a nested
compositor, on `Virtual-1` at 1280×800.

[test-inir-session.py](../../../scripts/test-inir-session.py) checked the real
session/output, adapted QML syntax, host package-action guard, IPC, overview,
settings, clipboard storage, notifications, native terminal, and shell restart.
The service had `NRestarts=0` after acceptance.

[Result](result.json), [acceptance](acceptance.log), [output](outputs.json),
[windows](windows.json), [IPC](ipc.txt), [package guard](package-guard.log),
[service](service.log), [system verification](system-verify.log), [boot status](status.log).
[Preparation](prepare.log) checked package/file input copying, private declaration
permissions, and original input preservation. [Build](rebuild.log),
[user installation](install.log), and [GPU details](gpu.log) retain the relevant logs.

Screenshots were captured inside the guest after actual IPC/UI actions:

![iNiR desktop](desktop.png)
![Application overview](overview.png)
![Shell settings](settings.png)
![Notification](notification.png)
![Native terminal](terminal.png)

## Scope

The main shell worked without optional AUR themes/fonts or Gowall. Bluetooth,
backlight, fingerprint, hardware power modes, AI keys, authenticated integrations,
and browser applications were not tested. Personal shell settings survive system
rollback; restoring HOME is a separate backup operation.

[Public evidence audit](public-audit.log) checked the selected artifacts on the
source VM. Detailed superseded diagnostics remain in Git history.
