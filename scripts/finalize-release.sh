#!/usr/bin/env bash
# Finalize a fully configured private build, including after a safe interruption.
set -Eeuo pipefail
SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
PROJECT_DIR=$(cd -- "$SCRIPT_DIR/.." && pwd)
source "$PROJECT_DIR/configs/bootstrap/install.env"
profile=${1:?Profile name}
[[ $profile =~ ^[a-z0-9-]+$ ]]
source "$PROJECT_DIR/configs/releases/$profile.env"
[[ $EUID == 0 && $(systemd-detect-virt) =~ ^(kvm|qemu)$ ]]
[[ $(cat "/sys/class/net/$VM_INTERFACE/address") == "$VM_MAC" ]]
STATE=/var/lib/looom
TOP=/run/looom-top
if [[ ${LOOOM_BUILD_LOCKED:-} != "$RELEASE_ID" ]]; then
    exec flock --close --exclusive "$STATE/release-control.lock" \
        env LOOOM_BUILD_LOCKED="$RELEASE_ID" bash "$0" "$profile"
fi
BUILD="/run/looom-build-$RELEASE_ID"
BUILD_SUBVOL="$TOP/@build-$RELEASE_ID"
ROOT="$TOP/@root-$RELEASE_ID"
mountpoint -q "$TOP" || { mkdir -p "$TOP"; mount -o subvolid=5,rw "${DISK}2" "$TOP"; }
[[ -d $BUILD_SUBVOL && ! -e $STATE/releases/$RELEASE_ID.json ]]
mkdir -p "$BUILD"
mountpoint -q "$BUILD" || mount -o "subvol=@build-$RELEASE_ID,rw" "${DISK}2" "$BUILD"
[[ $(cat "$BUILD/etc/looom/release-id") == "$RELEASE_ID" ]]
[[ $(cat "$BUILD/etc/looom/declarative-value") == "$DECLARATIVE_VALUE" ]]
[[ $(stat -c %a "$BUILD/etc") == 755 ]]
[[ $(readlink "$BUILD/etc/shadow") == /run/looom/accounts/shadow ]]
[[ ! -e $BUILD/etc/pacman.d/gnupg/private-keys-v1.d ]]
[[ -z $(find "$BUILD/etc" "$BUILD/root" "$BUILD/var" -xdev -type f \
    \( -name 'ssh_host_*_key' -o -name '*.hash' \) -print -quit) ]]
arch-chroot "$BUILD" pacman -Dk
arch-chroot "$BUILD" runuser -u dbus -- test -r /etc/machine-id
EVIDENCE="$STATE/release-evidence/$RELEASE_ID"
arch-chroot "$BUILD" pacman -Q > "$EVIDENCE/final-packages.txt"
cmp "$EVIDENCE/packages.txt" "$EVIDENCE/final-packages.txt"
python - "$BUILD" "$EVIDENCE" "$KERNEL_PACKAGE" <<'PY'
import hashlib, json, sys
from pathlib import Path
build, evidence = map(Path, sys.argv[1:3])
package = sys.argv[3]
lock = json.loads((evidence / 'lock.json').read_text())
installed = {line.split()[0]: line.split()[1] for line in (evidence / 'packages.txt').read_text().splitlines()}
assert installed == {pkg['name']: pkg['version'] for pkg in lock['packages']}
assert package in installed and not ({'linux', 'linux-lts'} - {package}) & installed.keys()
for name in ('shadow', 'gshadow'):
    assert all(line.split(':')[1] == '!'
               for line in (build / 'usr/lib/looom/accounts' / name).read_text().splitlines())
rid = (build / 'etc/looom/release-id').read_text().strip()
uki = build / 'boot' / ('looom-' + rid + '.efi')
expected = [line.split()[0] for line in (evidence / 'build-artifacts.sha256').read_text().splitlines()
            if line.split()[-1].endswith(uki.name)]
assert expected == [hashlib.sha256(uki.read_bytes()).hexdigest()]
print('Finalization: exact inventory, locked credentials and unchanged UKI verified')
PY
sha256sum "$SCRIPT_DIR/finalize-release.sh" > "$EVIDENCE/finalization-source.sha256"
sync -f "$BUILD"
python - "$STATE/operations/$RELEASE_ID.json" "$RELEASE_ID" <<'PY'
import json, os, sys, tempfile
from pathlib import Path
path = Path(sys.argv[1])
fd, temporary = tempfile.mkstemp(dir=path.parent, prefix='.' + sys.argv[2])
with os.fdopen(fd, 'w') as stream:
    json.dump(dict(id=sys.argv[2], phase='configured'), stream)
    stream.flush(); os.fsync(stream.fileno())
os.replace(temporary, path)
fd = os.open(path.parent, os.O_DIRECTORY); os.fsync(fd); os.close(fd)
PY
if [[ ${LOOOM_FAIL_AFTER:-} == build ]]; then
    echo 'Injected interruption after validated private build' >&2; exit 90
fi
if [[ -e $ROOT ]]; then
    [[ $(btrfs property get -ts "$ROOT" ro) == ro=true ]]
    cmp "$ROOT/etc/looom/release-id" "$BUILD/etc/looom/release-id"
    cmp "$ROOT/boot/looom-$RELEASE_ID.efi" "$BUILD/boot/looom-$RELEASE_ID.efi"
else
    btrfs subvolume snapshot -r "$BUILD" "$ROOT"
fi
sync -f "$TOP"
kernel_version=$(find "$ROOT/usr/lib/modules" -mindepth 1 -maxdepth 1 -type d -printf '%f\n')
[[ -n $kernel_version && $kernel_version != *$'\n'* ]]
root_uuid=$(blkid -s UUID -o value "${DISK}2")
esp_uuid=$(blkid -s UUID -o value "${DISK}1")
python - "$RELEASE_ID" "$KERNEL_PACKAGE" "$kernel_version" "$root_uuid" "$esp_uuid" "$STATE" <<'PY'
import hashlib, json, os, sys, tempfile
from pathlib import Path
rid, package, kernel, root_uuid, esp_uuid, state = sys.argv[1:]
root = Path('/run/looom-top') / ('@root-' + rid)
uki = root / 'boot' / ('looom-' + rid + '.efi')
metadata = dict(schema_version=1, id=rid, phase='validated', root_subvolume='@root-' + rid,
                kernel_package=package, kernel_version=kernel, root_uuid=root_uuid,
                esp_uuid=esp_uuid, uki_sha256=hashlib.sha256(uki.read_bytes()).hexdigest(),
                declarative_value=(root / 'etc/looom/declarative-value').read_text().strip())
for folder, data in (('releases', metadata), ('operations', dict(id=rid, phase='validated'))):
    path = Path(state) / folder / (rid + '.json')
    fd, temporary = tempfile.mkstemp(dir=path.parent, prefix='.' + rid)
    with os.fdopen(fd, 'w') as f:
        json.dump(data, f, indent=2); f.write('\n'); f.flush(); os.fsync(f.fileno())
    os.replace(temporary, path)
    directory = os.open(path.parent, os.O_DIRECTORY); os.fsync(directory); os.close(directory)
PY
umount "$BUILD"
rmdir "$BUILD"
btrfs subvolume delete --recursive --commit-after "$BUILD_SUBVOL"
printf 'Validated read-only release %s; not yet selectable in GRUB\n' "$RELEASE_ID"
