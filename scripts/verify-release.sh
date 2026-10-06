#!/usr/bin/env bash
# Acceptance checks against a genuinely booted release, never a build chroot.
set -Eeuo pipefail
rid=${1:?Expected release ID}
[[ $(cat /etc/looom/release-id) == "$rid" ]]
[[ -d /sys/firmware/efi ]]
[[ $(findmnt -nro FSROOT /) == "/@root-$rid" ]]
[[ ,$(findmnt -nro OPTIONS /), == *,ro,* ]]
[[ $(btrfs property get -ts / ro) == ro=true ]]
for path in /home /var /var/lib/looom /etc/looom-local /etc/NetworkManager/system-connections; do
    [[ ,$(findmnt -nro OPTIONS "$path"), == *,rw,* ]]
done
[[ $(id -u codex) == 1000 && $(id -g codex) == 1000 ]]
[[ $(readlink /etc/shadow) == /run/looom/accounts/shadow ]]
[[ ! -L /etc/passwd && ! -L /etc/group ]]
[[ $(stat -c %a /run/looom/accounts/shadow) == 600 ]]
[[ $(readlink /etc/resolv.conf) == /run/NetworkManager/resolv.conf ]]
[[ $(pacman-conf DBPath) == /usr/lib/looom/pacman/ || $(pacman-conf DBPath) == /usr/lib/looom/pacman ]]
for unit in looom-accounts NetworkManager sshd qemu-guest-agent systemd-timesyncd; do
    [[ $(systemctl is-active "$unit") == active ]]
    printf '%s: active\n' "$unit"
done
[[ -z $(systemctl --failed --no-legend --plain) ]]
pacman -Dk
visudo -cf /etc/sudoers
sshd -t
findmnt --verify --verbose
if touch /etc/looom/write-must-fail 2>/run/looom-readonly-error; then
    echo 'FAIL: /etc is writable'; exit 1
fi
if touch /usr/lib/looom/write-must-fail 2>/run/looom-readonly-error; then
    echo 'FAIL: /usr is writable'; exit 1
fi
grep -q 'Read-only file system' /run/looom-readonly-error
[[ -z $(grub-editenv /efi/looom/grub/grubenv list | sed -n 's/^next_entry=//p') ]]
python - <<'PY'
from pathlib import Path
for name in ('shadow', 'gshadow'):
    assert all(line.split(':')[1] == '!'
               for line in (Path('/usr/lib/looom/accounts') / name).read_text().splitlines())
print('Release account templates contain no password hashes')
PY
printf '\nBooted release %s\n' "$rid"
cat /proc/sys/kernel/random/boot_id
uname -r
cat /proc/cmdline /etc/looom/declarative-value
findmnt -t btrfs,vfat
grub-editenv /efi/looom/grub/grubenv list
printf '\nPASS: read-only release %s\n' "$rid"
