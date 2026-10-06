#!/usr/bin/env bash
# Fresh package build. Only the explicitly shared package cache is writable.
set -Eeuo pipefail
umask 022
SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
PROJECT_DIR=$(cd -- "$SCRIPT_DIR/.." && pwd)
source "$PROJECT_DIR/configs/bootstrap/install.env"
profile=${1:?Pass a profile name from configs/releases}
[[ $profile =~ ^[a-z0-9-]+$ && -f $PROJECT_DIR/configs/releases/$profile.env ]]
source "$PROJECT_DIR/configs/releases/$profile.env"
[[ $EUID == 0 && $(systemd-detect-virt) =~ ^(kvm|qemu)$ ]]
[[ $(cat "/sys/class/net/$VM_INTERFACE/address") == "$VM_MAC" ]]
[[ $RELEASE_ID =~ ^[a-z0-9-]+$ && $KERNEL_PACKAGE =~ ^linux(-lts)?$ ]]
STATE=/var/lib/looom
TOP=/run/looom-top
[[ -f $STATE/release-state-initialized ]]
if [[ ${LOOOM_BUILD_LOCKED:-} != "$RELEASE_ID" ]]; then
    # --close prevents pacman-key's background GPG processes from inheriting
    # the lock descriptor and retaining it after the build process exits.
    exec flock --close --exclusive "$STATE/release-control.lock" \
        env LOOOM_BUILD_LOCKED="$RELEASE_ID" bash "$0" "$profile"
fi
mkdir -p "$TOP" "$STATE/releases" "$STATE/operations"
mountpoint -q "$TOP" || mount -o subvolid=5,rw,noatime,compress=zstd:3 "${DISK}2" "$TOP"
BUILD_SUBVOL="$TOP/@build-$RELEASE_ID"
BUILD="/run/looom-build-$RELEASE_ID"
ROOT="$TOP/@root-$RELEASE_ID"
[[ ! -e $BUILD_SUBVOL && ! -e $BUILD && ! -e $ROOT && ! -e $STATE/releases/$RELEASE_ID.json ]] || {
    echo 'Refusing to overwrite an existing build or release' >&2; exit 1;
}
available=$(df -B1 --output=avail "$TOP" | tail -n 1)
required=$((4 * 1024 * 1024 * 1024))
[[ $DESKTOP == no ]] || required=$((10 * 1024 * 1024 * 1024))
((available > required)) || { echo 'Insufficient free space for build' >&2; exit 1; }
printf '{"id":"%s","phase":"building"}\n' "$RELEASE_ID" > "$STATE/operations/$RELEASE_ID.json"
btrfs subvolume create "$BUILD_SUBVOL"
mkdir "$BUILD"
mount -o "subvol=@build-$RELEASE_ID,rw,noatime,compress=zstd:3" "${DISK}2" "$BUILD"
trap 'echo "Build interrupted; private @build-$RELEASE_ID retained, boot menu unchanged" >&2' ERR
mkdir -p "$BUILD/etc"
cp -a "$STATE/account-registry/." "$BUILD/etc/"
# cp -a also copies the registry directory's 0700 mode; /etc itself must be
# searchable by service accounts, while individual shadow files stay private.
chmod 755 "$BUILD/etc"
CONFIG=$(mktemp /run/looom-pacman.XXXXXX)
cat > "$CONFIG" <<EOF
[options]
Architecture = auto
CheckSpace
ParallelDownloads = 5
SigLevel = Required DatabaseOptional
LocalFileSigLevel = Required
[core]
Server = https://archive.archlinux.org/repos/$ARCHIVE_DATE/\$repo/os/\$arch
[extra]
Server = https://archive.archlinux.org/repos/$ARCHIVE_DATE/\$repo/os/\$arch
EOF
mapfile -t packages < <(sed '/^[[:space:]]*#/d; /^[[:space:]]*$/d; /^linux$/d' "$PROJECT_DIR/configs/bootstrap/packages.txt")
pacstrap -c -K -M -C "$CONFIG" "$BUILD" "${packages[@]}" "$KERNEL_PACKAGE" "${EXTRA_PACKAGES[@]}"
mkdir -p "$BUILD/usr/lib/looom"
mv "$BUILD/var/lib/pacman" "$BUILD/usr/lib/looom/pacman"
ln -s /usr/lib/looom/pacman "$BUILD/var/lib/pacman"
cp "$CONFIG" "$BUILD/etc/pacman.conf"
sed -i '/^\[options\]/a DBPath = /usr/lib/looom/pacman' "$BUILD/etc/pacman.conf"
rm "$CONFIG"
root_uuid=$(blkid -s UUID -o value "${DISK}2")
esp_uuid=$(blkid -s UUID -o value "${DISK}1")
mkdir -p "$BUILD/home" "$BUILD/efi" "$BUILD/etc/looom-local" \
    "$BUILD/etc/NetworkManager/system-connections" "$BUILD/var/lib/looom"
cat > "$BUILD/etc/fstab" <<EOF
UUID=$root_uuid / btrfs ro,noatime,compress=zstd:3,subvol=@root-$RELEASE_ID 0 0
UUID=$root_uuid /home btrfs rw,noatime,compress=zstd:3,subvol=$HOME_SUBVOL 0 0
UUID=$root_uuid /var btrfs rw,noatime,compress=zstd:3,subvol=$VAR_SUBVOL 0 0
UUID=$root_uuid /var/lib/looom btrfs rw,noatime,compress=zstd:3,subvol=$STATE_SUBVOL 0 0
UUID=$esp_uuid /efi vfat rw,umask=0077 0 2
tmpfs /tmp tmpfs rw,nosuid,nodev,mode=1777 0 0
/var/lib/looom/local-etc/looom-local /etc/looom-local none bind,x-systemd.requires-mounts-for=/var/lib/looom 0 0
/var/lib/looom/local-etc/NetworkManager/system-connections /etc/NetworkManager/system-connections none bind,x-systemd.requires-mounts-for=/var/lib/looom 0 0
EOF
cp "$STATE/machine-id" "$BUILD/etc/machine-id"
chmod 444 "$BUILD/etc/machine-id"
ln -sf "/usr/share/zoneinfo/$VM_TIMEZONE" "$BUILD/etc/localtime"
printf '%s\n' "$VM_HOSTNAME" > "$BUILD/etc/hostname"
printf '127.0.0.1 localhost\n::1 localhost\n127.0.1.1 %s\n' "$VM_HOSTNAME" > "$BUILD/etc/hosts"
printf 'en_US.UTF-8 UTF-8\nru_RU.UTF-8 UTF-8\n' > "$BUILD/etc/locale.gen"
printf 'LANG=en_US.UTF-8\n' > "$BUILD/etc/locale.conf"
printf 'KEYMAP=us\n' > "$BUILD/etc/vconsole.conf"
arch-chroot "$BUILD" locale-gen
[[ $(arch-chroot "$BUILD" id -u "$VM_USER") == "$VM_UID" ]]
[[ $(arch-chroot "$BUILD" id -g "$VM_USER") == "$VM_GID" ]]
mkdir -p "$BUILD/etc/sudoers.d" "$BUILD/etc/ssh/sshd_config.d" "$BUILD/etc/looom"
printf '%s ALL=(ALL:ALL) NOPASSWD: ALL\n' "$VM_USER" > "$BUILD/etc/sudoers.d/10-looom-vm"
chmod 440 "$BUILD/etc/sudoers.d/10-looom-vm"
cat > "$BUILD/etc/ssh/sshd_config.d/10-looom-vm.conf" <<EOF
PubkeyAuthentication yes
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin prohibit-password
AllowUsers root $VM_USER
HostKey /var/lib/looom/ssh/ssh_host_ed25519_key
HostKey /var/lib/looom/ssh/ssh_host_rsa_key
EOF
rm -rf "$BUILD/root/.ssh"
ln -sT "$STATE/root-ssh" "$BUILD/root/.ssh"
[[ $(readlink "$BUILD/root/.ssh") == "$STATE/root-ssh" ]]
rm -f "$BUILD/etc/resolv.conf"
ln -s /run/NetworkManager/resolv.conf "$BUILD/etc/resolv.conf"
printf '%s\n' "$RELEASE_ID" > "$BUILD/etc/looom/release-id"
printf '%s\n' "$DECLARATIVE_VALUE" > "$BUILD/etc/looom/declarative-value"
install -m 755 "$SCRIPT_DIR/looom-accounts.py" "$BUILD/usr/lib/looom/looom-accounts.py"
install -m 755 "$SCRIPT_DIR/looom-release.py" "$BUILD/usr/bin/looom-release"
install -m 755 "$SCRIPT_DIR/verify-release.sh" "$BUILD/usr/lib/looom/verify-release.sh"
cat > "$BUILD/usr/bin/looom-password" <<'EOF'
#!/bin/sh
exec /usr/lib/looom/looom-accounts.py password "$@"
EOF
chmod 755 "$BUILD/usr/bin/looom-password"
cat > "$BUILD/etc/systemd/system/looom-accounts.service" <<'EOF'
[Unit]
Description=Generate runtime accounts from release templates and persistent credentials
DefaultDependencies=no
RequiresMountsFor=/var/lib/looom
After=local-fs.target
Before=systemd-tmpfiles-setup.service sysinit.target
Conflicts=shutdown.target
Before=shutdown.target

[Service]
Type=oneshot
ExecStart=/usr/lib/looom/looom-accounts.py generate
RemainAfterExit=yes

[Install]
RequiredBy=sysinit.target
EOF
# Every static package account was generated inside the private build. Running
# sysusers on the immutable /etc would attempt to replace files across mounts.
ln -sf /dev/null "$BUILD/etc/systemd/system/systemd-sysusers.service"
systemctl --root="$BUILD" enable looom-accounts NetworkManager sshd systemd-timesyncd \
    qemu-guest-agent serial-getty@ttyS0.service
for service in sshd NetworkManager systemd-timesyncd; do
    mkdir -p "$BUILD/etc/systemd/system/$service.service.d"
    printf '[Unit]\nRequires=looom-accounts.service\nAfter=looom-accounts.service\n' \
        > "$BUILD/etc/systemd/system/$service.service.d/10-looom-accounts.conf"
done
if [[ $DESKTOP == yes ]]; then
    mkdir -p "$BUILD/etc/sddm.conf.d" "$BUILD/etc/systemd/system/sddm.service.d"
    printf '[Theme]\nCurrent=breeze\n[General]\nDisplayServer=x11\n' > "$BUILD/etc/sddm.conf.d/10-looom.conf"
    printf '[Unit]\nRequires=looom-accounts.service\nAfter=looom-accounts.service\n' \
        > "$BUILD/etc/systemd/system/sddm.service.d/10-looom-accounts.conf"
    systemctl --root="$BUILD" enable sddm
    systemctl --root="$BUILD" set-default graphical.target
else
    systemctl --root="$BUILD" set-default multi-user.target
fi
mkdir -p "$BUILD/etc/kernel"
printf 'root=UUID=%s rootflags=subvol=@root-%s ro console=tty0 console=ttyS0,115200n8' \
    "$root_uuid" "$RELEASE_ID" > "$BUILD/etc/kernel/cmdline"
[[ $BROKEN_BOOT == no ]] || printf ' systemd.unit=emergency.target' >> "$BUILD/etc/kernel/cmdline"
printf '\n' >> "$BUILD/etc/kernel/cmdline"
cp "$PROJECT_DIR/docs/runs/2026-10-04-bootstrap/evidence/mkinitcpio.conf" "$BUILD/etc/mkinitcpio.conf"
kernel_version=$(find "$BUILD/usr/lib/modules" -mindepth 1 -maxdepth 1 -type d -printf '%f\n')
[[ -n $kernel_version && $kernel_version != *$'\n'* ]]
install -m 644 "$BUILD/usr/lib/modules/$kernel_version/vmlinuz" "$BUILD/boot/vmlinuz-looom"
rm -f "$BUILD/etc/mkinitcpio.d/"*.preset
cat > "$BUILD/etc/mkinitcpio.d/looom.preset" <<EOF
ALL_config="/etc/mkinitcpio.conf"
ALL_kver="/boot/vmlinuz-looom"
PRESETS=('default')
default_uki="/boot/looom-$RELEASE_ID.efi"
default_options="--cmdline /etc/kernel/cmdline"
EOF
arch-chroot "$BUILD" mkinitcpio -P
arch-chroot "$BUILD" pacman -Dk
arch-chroot "$BUILD" visudo -cf /etc/sudoers
arch-chroot "$BUILD" runuser -u dbus -- test -r /etc/machine-id
EVIDENCE="$STATE/release-evidence/$RELEASE_ID"
mkdir -p "$EVIDENCE"
mkdir "$EVIDENCE/source"
cp -a "$PROJECT_DIR/scripts" "$PROJECT_DIR/configs" "$EVIDENCE/source/"
find "$EVIDENCE/source" -type f ! -path '*/__pycache__/*' -exec sha256sum {} + > "$EVIDENCE/source.sha256"
arch-chroot "$BUILD" pacman -Q > "$EVIDENCE/packages.txt"
arch-chroot "$BUILD" pacman -Qqe > "$EVIDENCE/explicit-packages.txt"
cp "$BUILD/etc/fstab" "$BUILD/etc/kernel/cmdline" "$EVIDENCE/"
cp "$PROJECT_DIR/configs/releases/$profile.env" "$EVIDENCE/profile.env"
find /var/cache/pacman/pkg -type f -name '*.pkg.tar.*' -exec sha256sum {} + > "$EVIDENCE/package-archives.sha256"
sha256sum "$BUILD/usr/lib/looom/pacman/sync/"*.db "$BUILD/boot/looom-$RELEASE_ID.efi" \
    > "$EVIDENCE/build-artifacts.sha256"
python "$SCRIPT_DIR/capture-bootstrap-lock.py" "$EVIDENCE" "$EVIDENCE/lock.json" --archive-date "$ARCHIVE_DATE"
python - "$BUILD" "$STATE" <<'PY'
import sys
from pathlib import Path
build, state = map(Path, sys.argv[1:])
templates = build / 'usr/lib/looom/accounts'
templates.mkdir()
for name in ('passwd', 'group', 'shadow', 'gshadow'):
    source = build / 'etc' / name
    lines = []
    for line in source.read_text().splitlines():
        fields = line.split(':')
        if name in ('shadow', 'gshadow'):
            fields[1] = '!'
        lines.append(':'.join(fields))
    data = '\n'.join(lines) + '\n'
    if name in ('passwd', 'group'):
        old = {x.split(':')[0]: x.split(':')[2:4 if name == 'passwd' else 3]
               for x in (state / 'account-registry' / name).read_text().splitlines()}
        new = {x.split(':')[0]: x.split(':')[2:4 if name == 'passwd' else 3] for x in lines}
        assert all(new.get(user) == ids for user, ids in old.items()), 'UID/GID map changed'
        ids = [x.split(':')[2] for x in lines]
        assert len(ids) == len(set(ids)), 'Duplicate UID/GID'
    (templates / name).write_text(data)
    (templates / name).chmod(0o600 if name in ('shadow', 'gshadow') else 0o644)
    # Append-only stable account allocation map, with all passwords locked.
    (state / 'account-registry' / name).write_text(data)
    # PID 1, early sockets and udev need passwd/group before local-fs. These
    # declarative files stay immutable in /etc. Only authentication secrets
    # are supplied later via runtime shadow/gshadow.
    if name in ('shadow', 'gshadow'):
        source.unlink()
        source.symlink_to('/run/looom/accounts/' + name)
PY
# No bootstrap credentials, generated host keys, account backups or GPG private
# key may be published. Package-owned public verification material is retained.
arch-chroot "$BUILD" gpgconf --homedir /etc/pacman.d/gnupg --kill all
rm -f "$BUILD/etc/"{passwd-,group-,shadow-,gshadow-} "$BUILD/etc/ssh/ssh_host_"*
rm -rf "$BUILD/etc/pacman.d/gnupg/private-keys-v1.d"
find "$BUILD/etc" -maxdepth 1 -name '*.pacnew' -delete
[[ -z $(find "$BUILD" -xdev -type f \( -name '*_key' -o -name '*.hash' \) -print -quit) ]]
sync -f "$BUILD"
if [[ ${LOOOM_FAIL_AFTER:-} == build ]]; then
    echo 'Injected interruption after validated private build' >&2; exit 90
fi
btrfs subvolume snapshot -r "$BUILD" "$ROOT"
sync -f "$TOP"
python - "$RELEASE_ID" "$KERNEL_PACKAGE" "$kernel_version" "$root_uuid" "$esp_uuid" "$STATE" <<'PY'
import hashlib, json, os, sys
from pathlib import Path
rid, package, kernel, root_uuid, esp_uuid, state = sys.argv[1:]
root = Path('/run/looom-top') / ('@root-' + rid)
uki = root / 'boot' / ('looom-' + rid + '.efi')
metadata = dict(schema_version=1, id=rid, phase='validated', root_subvolume='@root-' + rid,
                kernel_package=package, kernel_version=kernel, root_uuid=root_uuid,
                esp_uuid=esp_uuid, uki_sha256=hashlib.sha256(uki.read_bytes()).hexdigest(),
                declarative_value=(root / 'etc/looom/declarative-value').read_text().strip())
path = Path(state) / 'releases' / (rid + '.json')
with path.open('x') as f:
    json.dump(metadata, f, indent=2); f.write('\n'); f.flush(); os.fsync(f.fileno())
(Path(state) / 'operations' / (rid + '.json')).write_text(json.dumps(dict(id=rid, phase='validated')) + '\n')
PY
umount "$BUILD"
rmdir "$BUILD"
btrfs subvolume delete --recursive --commit-after "$BUILD_SUBVOL"
printf 'Validated read-only release %s; not yet selectable in GRUB\n' "$RELEASE_ID"
