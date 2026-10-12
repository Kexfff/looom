# Development

Build and run looom in a dedicated Arch VM. Run integration and destructive tests
only in the explicitly authorized VM fixtures, never on the everyday host.
Local source editing, file transfer, and VM orchestration do not require running
the system manager on the host.

## Build the manager

Use a writable Arch build environment with Rust, Cargo, and the locked dependencies:

```sh
git clone https://github.com/Kexfff/looom.git
cd looom
cargo build --locked --release --bin looom
```

The result is `target/release/looom`. Version 0.2.0 was built using
Rust/Cargo 1.99.0 for `x86_64-unknown-linux-gnu`.

For a read-only development system, keep the writable toolchain/build root on a
separate Btrfs subvolume and enter it with `arch-chroot`; bind the source checkout
at `/workspace`. Do not install the toolchain into the active read-only release.

## Checks and VM fixtures

Inside the prepared development VM:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
LOOOM_TEST_VM=1 cargo test --locked -- --nocapture --test-threads=1
cargo build --locked --release --bin looom
```

The full suite expects root, QEMU/KVM, a prepared Btrfs environment, and the
fixture identity `52:54:00:7b:23:63` on `enp1s0`. Integration fixtures use
`/workspace/.native-fixtures` and real mounts/subvolumes. Review the setup and
identity guards before using a different VM; do not disable them merely to run
the suite on an unrelated machine.

Acceptance helpers under `scripts/test-*.py` and Rust `src/bin/looom-vm-*` have
their own guards and setup requirements. They are test tools, not general
installers. Release acceptance combines real ISO installation, firmware boot,
graphical login, user applications, trial fallback, rollback, and cleanup.
See [0.2.0 validation](runs/2026-10-12-release-0.2.0/README.md).

The older Python/Bash prototype helpers remain for compatibility with historical
experiments. Current system management uses the Rust binary. Their reusable
initramfs input is [configs/bootstrap/mkinitcpio.conf](../configs/bootstrap/mkinitcpio.conf),
not a documentation artifact.

## Build the ISO

Use a dedicated Arch VM with root access and `archiso 91-1` from the pinned
`2026-10-07` archive. First build and validate the native manager in the VM:

```sh
LOOOM_SOURCE_REVISION=$(git rev-parse HEAD) \
  bash scripts/build-iso.sh /absolute/target/release/looom /absolute/new-work /absolute/output
```

Each build requires a fresh work directory. The script uses the installed Archiso
releng profile, overlays `configs/iso`, and embeds the verified binary,
installer declarations, and `docs/live-iso.md` as the installation guide.
Defaults are version `0.2.0`, snapshot `2026-10-07`, and
`SOURCE_DATE_EPOCH=1791763200`. `LOOOM_ISO_VERSION`, `LOOOM_ISO_SNAPSHOT`,
and `SOURCE_DATE_EPOCH` can be set explicitly for a new build.

Outputs are the ISO, its `.sha256`, a build-input manifest, and a live-package list.
The build script requires a VM and root; it does not copy the builder's HOME,
SSH keys, or installed machine State. Audit the actual payload before publishing.
Input manifests support reproduction of composition and process; byte-identical
ISO/UKI output is not promised.

## Evidence and publication

Keep source revision, toolchain version, declarations, lock/input hashes, package
inventories, boot checks, and relevant screenshots. Audit selected evidence on
its source VM against local credentials before export. Never publish State backups,
installer password fixtures, private keys, or whole user directories.

Release validation and the optional iNiR report are retained under `docs/runs`.
Superseded prototype plans and older run archives are available in Git history
and previous tags. Published ISO/binary assets remain tied to their original
source revision; editing documentation does not rewrite existing release files.
