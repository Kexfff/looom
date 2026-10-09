set -Eeuo pipefail
base=/var/lib/looom/dev/installer-20261009
work=/root/install-work
hostwork="$base/live-root$work"
mkdir -p "$base/live-iso" "$base/live-lower" "$base/live-root"
mount -o loop,ro "$base/archlinux-live.iso" "$base/live-iso"
mount -o loop,ro "$base/live-iso/arch/x86_64/airootfs.sfs" "$base/live-lower"
mount -t overlay -o "lowerdir=$base/live-lower,upperdir=$base/live-upper,workdir=$base/live-work" live-install "$base/live-root"
cp "$base/looom" "$base/live-root/root/looom"
cp "$base/base.yaml" "$base/live-root/root/base.yaml"
cp "$base/management.pub" "$base/live-root/root/management.pub"
before_order=$(efibootmgr | sed -n 's/^BootOrder: //p')
label=''
restore() {
  efibootmgr --bootorder "$before_order" >/dev/null
  if test -n "$label"; then
    id=$(efibootmgr | awk -v label="$label" '$2==label {print substr($1,5,4)}')
    if test -n "$id"; then
      efibootmgr -v | grep -F "$label" | grep -qi "$esp_guid"
      efibootmgr --bootnum "$id" --delete-bootnum >/dev/null
    fi
  fi
}
trap restore EXIT
arch-chroot "$base/live-root" pacman-key --init
arch-chroot "$base/live-root" pacman-key --populate archlinux
if ! test -f "$hostwork/plan.json"; then
  arch-chroot "$base/live-root" /root/looom install plan /dev/vdb /root/base.yaml "$work" --ssh-key /root/management.pub
fi
confirmation=$(python -c 'import json;p=json.load(open("/var/lib/looom/dev/installer-20261009/live-root/root/install-work/plan.json"));d=p["disk"];print("ERASE",d["path"],d["serial"],d["size"])')
label=$(python -c 'import json;p=json.load(open("/var/lib/looom/dev/installer-20261009/live-root/root/install-work/plan.json"));print("looom-"+p["root_uuid"][:8])')
esp_guid=$(python -c 'import json;p=json.load(open("/var/lib/looom/dev/installer-20261009/live-root/root/install-work/plan.json"));print(p["esp_partuuid"])')
mkdir -p -m 700 "$hostwork/cache"
cp --reflink=auto "$base/install/cache/"* "$hostwork/cache/"
if test -f "$hostwork/journal.json"; then
  arch-chroot "$base/live-root" /root/looom install resume "$work" --passwords-stdin < "$base/private/passwords.secret"
else
  arch-chroot "$base/live-root" /root/looom install apply "$work" --confirm "$confirmation" --passwords-stdin < "$base/private/passwords.secret"
fi
first=$(efibootmgr | sed -n 's/^BootOrder: //p' | cut -d, -f1)
efibootmgr -v | grep -F "Boot${first}" | grep -F "$label" | grep -qi "$esp_guid"
printf 'PASS: complete install from official ISO userspace, default UEFI entry GUID/path and first BootOrder validated\n'
arch-chroot "$base/live-root" /root/looom install resume "$work"
restore
trap - EXIT
test "$(efibootmgr | sed -n 's/^BootOrder: //p')" = "$before_order"
printf 'PASS: test UEFI entry removed; development VM BootOrder restored\n'
