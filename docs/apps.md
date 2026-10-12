# User applications

`base.yaml` controls the read-only system. `apps.yaml` belongs to your regular
user and applies without sudo or a system rebuild. New releases include Flatpak,
KDE portals, Distrobox, rootless Podman, and FUSE support for AppImage.

The installer creates `~/looom/apps.yaml` alongside the system declaration.
On an existing installation, run `looom apps init`; it preserves an existing file.
The new default declares Firefox. Installation starts only when you explicitly
choose it in `looom setup` or run `looom apps apply`.

## Declaration

```yaml
schema: 1
flatpak: [org.mozilla.firefox]
arch:
  image: docker.io/library/archlinux:latest
  packages: [git, base-devel]
  exports: []
appimages: {}
```

JSON is also accepted. The OCI image may be pinned with `@sha256:…` instead of
`latest`. There is one managed container, `looom-arch`, created only when packages,
exports, or `exec`/`run-script` need it. An empty Arch declaration does not pull an image.

```sh
looom apps init
looom apps check
looom apps plan
looom apps apply
looom apps status
looom apps update
looom apps exec -- bash
```

`check`, `plan`, `apply`, `status`, and `update` accept an optional YAML path;
the default is `~/looom/apps.yaml`. Modifying operations require a regular user
session. Do not run `sudo looom apps` or `sudo distrobox`.

The rule is **ensure declared applications are present**. Removing an entry does
not uninstall anything. Manual applications, old AppImages, and data remain.
`apply` does not update already installed applications; `update` updates declared
Flatpaks and the entire Arch container with `pacman -Syu`. If an old rolling-mirror
package disappeared, update the container and retry `apply`.

Application operations are serialized per user. Successful installations remain
when a later step fails; rerun `apply` to continue. The receipt is written only
after success and records Flatpak commits and the actual OCI image ID. It is not
a complete lock or an atomic rollback mechanism for all application sources.

## Flatpak

Applications are installed with `--user` from the standard Flathub remote, using
normal Flatpak signature verification. A preexisting `flathub` remote with a
different URL is rejected, and an application from a different origin is not
silently replaced. Desktop entries use Flatpak's normal export mechanism;
a session restart may be needed after adding the remote for the first time.

## Arch containers and installer scripts

```sh
looom apps exec -- sudo pacman -Syu
looom apps run-script ~/Downloads/vendor/setup argument
looom apps run-script --root ~/Downloads/vendor/root-install.sh
```

`run-script` runs the whole script **inside the Arch container as the regular user**.
The user's HOME is shared, so tools such as `makepkg` or an AUR helper can run
without root. During this child process only, a container PATH shim forwards
bare `pacman` to `sudo -n /usr/bin/pacman`. `sudo pacman` and
`sudo /usr/bin/pacman` also modify the container. An absolute `/usr/bin/pacman`
without sudo remains unprivileged. Host pacman is never globally intercepted.

Use `--root` only when the script explicitly requires root. This is container
root, not host root; files it creates in shared HOME may have subordinate UID/GID
ownership. The script is not added to the declaration or rerun during `apply`.
`exec` keeps the normal Distrobox environment without the pacman shim.

Place the script and its supporting files inside HOME. Its working directory
is the script directory, and arguments are passed individually without shell
concatenation. For graphical container applications, declare their packages
and desktop names in `arch.exports`, then apply:

```yaml
arch:
  image: docker.io/library/archlinux:latest
  packages: [firefox]
  exports: [firefox]
```

Distrobox is an integrated compatibility environment, not a security sandbox for
untrusted code. Scripts that configure the host kernel, bootloader, drivers,
udev, accounts, or system services need explicit host-side configuration instead.
Changing `arch.image` does not replace an existing container; migrate its data and
recreate it explicitly through Distrobox. An unrelated container with the same
name is rejected.

## Import system dependencies

Some graphical-session dependencies must be in the base system. Add a supported
session and an imported package list rather than enumerating every dependency:

```yaml
desktop:
  environment: plasma
  sessions: [niri]
packages_from: [desktop-packages.yaml]
```

`environment: niri` selects Niri alone. Its profile includes Niri, Quickshell,
Xwayland Satellite, SDDM, portals, a polkit agent, audio, a terminal, QML libraries,
and fonts. The iNiR user shell is installed separately.

After installing dependencies inside the container:

```sh
looom apps export-packages ~/looom/desktop-packages.yaml
```

This exports **all explicitly installed container packages**, including earlier
applications and Distrobox tools. Review the list and remove software that should
stay only in the container. Existing output files are never overwritten;
without a path, the document is printed to stdout.

```yaml
schema: 1
packages: [quickshell, jq, cliphist]
```

Repository packages must be available in official Arch core/extra on the selected
system snapshot date. Extra container repositories and their trust configuration
are not imported. AUR/local packages appear in `foreign`; a nonempty `foreign`
list blocks system check/build until you handle them separately and explicitly
remove that field. There is no automatic system AUR builder. Do not copy binary
Qt/QML plugins from a container into a host with different library versions.

Apply the reviewed list through a normal system rebuild. Includes are confined
relative files with no symlinks, traversal, tags/anchors, or nested includes;
the resolved package set is frozen for recovery. See [configuration](configuration.md#package-lists).

For iNiR, use the [pinned integration profile](../examples/inir/README.md).
Third-party installers can separate dependencies, host session setup, and user
configuration into phases. looom does not rewrite arbitrary shell code or turn
host `systemctl`/`usermod` actions into atomic release resources.

## AppImage

Add an HTTPS URL or a local source, plus a required lowercase SHA-256 digest:

```yaml
appimages:
  example:
    source: ./Example.AppImage
    sha256: <64 lowercase hexadecimal characters>
    name: Example
```

Replace the placeholder with the file's actual digest. For a download, replace
`source` with `url: https://publisher.example/Example.AppImage`. Relative local
sources are resolved from the YAML directory.

looom verifies the digest and AppImage format, then atomically stores the executable.
It does not execute downloaded files during installation. A desktop launcher calls
`looom apps launch <id>`, which checks the active digest again before execution:

```sh
looom apps launch example -- --version
```

`update` does not automatically discover new AppImage versions. Change the URL/file
and digest, then apply. Previous versions remain. FUSE2 is included, but individual
AppImage compatibility depends on the application. AppImage itself is not a sandbox.

## Rollback and data

Applications, container storage, launchers, and data live in persistent HOME,
including `.local/share` or a persistent `XDG_DATA_HOME` inside HOME. They survive
system rollback. A very old base release without the infrastructure may be unable
to launch them while preserving their files.

`looom gc` does not delete user applications or container storage. Application
cleanup and backups are separate responsibilities. System rollback does not undo
application upgrades or restore personal desktop settings.
