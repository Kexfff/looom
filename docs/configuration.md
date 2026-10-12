# System configuration

`~/looom/base.yaml` describes the next system release. Start from the file created
by your installation: it contains the account identity and persistent resources
bound to that machine. Editing it does not change the running system until you
build, boot, and confirm a new release.

YAML and its JSON subset are accepted. The installer writes formatted JSON;
you can keep that format or use ordinary YAML. Schema 1 is typed and rejects
unknown fields, duplicate keys, YAML tags, anchors/aliases, and merge keys.
It is a declaration, not a programming language or shell script.

## Example

This matches the installer profile. Use your installed user name, UID/GID,
credential reference, and persistent resource IDs rather than copying a new identity.

```yaml
schema: 1
source:
  profile: arch
  snapshot: "2026-10-07"
system:
  hostname: looom
  timezone: Europe/Moscow
  locale: en_US.UTF-8
  console_keymap: us
kernel:
  package: linux
desktop:
  environment: plasma
packages: [intel-ucode, amd-ucode]
accounts:
  user:
    name: owner
    uid: 1000
    gid: 1000
    groups: [wheel]
    shell: /bin/bash
    password_secret: login-owner
  root:
    password_secret: login-root
units:
  sshd.service: enabled
files: {}
persistent:
  directories:
    /etc/NetworkManager/system-connections:
      id: network-connections
      owner: root
      group: root
      mode: "0700"
    /etc/looom-local:
      id: local-settings
      owner: root
      group: root
      mode: "0700"
health:
  required_units: [sshd.service]
```

## Supported fields

| Field | Contract |
| --- | --- |
| `schema` | `1` |
| `source.profile` | `arch`; official core/extra repositories |
| `source.snapshot` | Quoted `YYYY-MM-DD` Arch Linux Archive date; package resolution uses the whole selected snapshot |
| `system` | Hostname, timezone, locale, console keymap |
| `kernel.package` | `linux` or `linux-lts` |
| `kernel.extra_command_line` | Only an empty list is supported |
| `desktop.environment` | `plasma`, `niri`, or `none` |
| `desktop.sessions` | Additional `plasma`/`niri` sessions, without duplicates or the primary environment |
| `packages` | Additional package names; transitive dependencies are resolved automatically |
| `packages_from` | Up to 16 confined relative package-list files |
| `accounts` | One stable user, UID/GID 1000–65533, exactly `[wheel]`, `/bin/bash`; credential names `login-<user>` and `login-root` |
| `units` | Unit name → `enabled`, `disabled`, or `masked`; core runtime services are protected |
| `files` | Regular root-owned files under `/etc`, subject to protected-path and conflict checks |
| `persistent.directories` | The two fixed directories shown above, root:root/`0700`; migrations are not supported |
| `health.required_units` | Additional units checked before confirming a release |

The interactive installer always selects KDE. Niri and console profiles are
available for subsequent releases. Use [the iNiR profile](../examples/inir/README.md)
for a complete optional Niri shell rather than assuming the bare session includes it.

## Declarative files

Use exactly one of `content` or `source`. Quote octal modes. Files are root-owned;
non-root owners/groups and arbitrary file resources outside `/etc` are unsupported.

```yaml
files:
  /etc/sysctl.d/90-local.conf:
    content: |
      vm.swappiness = 10
    mode: "0644"
```

For a separate file, replace `content` with `source: files/90-local.conf` and keep
that file inside the declaration directory. Paths cannot escape through `..`
or symlinks. YAML and each source file are limited to 1 MiB.

Replacing a package-owned file requires `replace_package_file: true` and a path
that is otherwise permitted. A declarative file cannot overlap a persistent
resource or another file's parent/child path.

looom generates protected account files, `fstab`, machine identity, package-manager
configuration, kernel/boot configuration, and its runtime service resources.
They cannot be overridden through `files`. General `fstab` customization and
additional mounts require future typed handlers. Keep local edits inside the
explicit writable directories; other `/etc` changes belong in the declaration.

## Package lists

```yaml
packages_from: [desktop-packages.yaml]
```

A package-list file contains:

```yaml
schema: 1
packages: [jq, cliphist]
```

Lists are relative to `base.yaml`, limited to 1 MiB each, with no nested includes,
symlinks, or parent traversal. Duplicate names across separate lists are merged;
duplicates within one list are rejected. A nonempty `foreign` list blocks system
builds: AUR/local packages must remain in the container or be handled separately.
See [dependency import](apps.md#import-system-dependencies).

## Apply and reproduce

```sh
looom check ~/looom/base.yaml
sudo looom lock ~/looom/base.yaml
sudo looom plan ~/looom/base.yaml

# Use a new release ID; do not reuse an existing build/release name.
sudo looom rebuild ~/looom/base.yaml desktop-next
sudo reboot
# Check the desktop, network, and sound before confirming.
sudo looom verify
sudo looom confirm
```

Release IDs start with a lowercase letter and contain only lowercase letters,
digits, and hyphens, up to 64 characters.

`base.lock` records the complete package closure, archives, signatures,
repository hashes, and the manager digest. Replacing the manager requires a
fresh lock even if package versions are unchanged. Keep the declaration, its
file inputs, the lock, and verified package/repository inputs for reproduction.
A lock alone cannot recreate packages that are unavailable locally or remotely.
Passwords and private machine state are not reproducibility inputs to publish.

[Command reference](native-rust.md) · [Architecture](architecture.md)
