set -Eeuo pipefail
base=/var/lib/looom/dev/installer-20261009
work=/root/install-work
hostwork="$base/live-root$work"
mount -o loop,ro "$base/archlinux-live.iso" "$base/live-iso"
mount -o loop,ro "$base/live-iso/arch/x86_64/airootfs.sfs" "$base/live-lower"
mount -t overlay -o "lowerdir=$base/live-lower,upperdir=$base/live-upper,workdir=$base/live-work" live-replay "$base/live-root"
before_order=$(efibootmgr | sed -n 's/^BootOrder: //p')
label=$(python -c 'import json;p=json.load(open("/var/lib/looom/dev/installer-20261009/live-root/root/install-work/plan.json"));print("looom-"+p["root_uuid"][:8])')
esp_guid=$(python -c 'import json;p=json.load(open("/var/lib/looom/dev/installer-20261009/live-root/root/install-work/plan.json"));print(p["esp_partuuid"])')
restore() {
  efibootmgr --bootorder "$before_order" >/dev/null
  id=$(efibootmgr | awk -v label="$label" '$2==label {print substr($1,5,4)}')
  if test -n "$id"; then
    efibootmgr -v | grep -F "$label" | grep -qi "$esp_guid"
    efibootmgr --bootnum "$id" --delete-bootnum >/dev/null
  fi
}
trap restore EXIT
python - <<'PY'
import json,os
from pathlib import Path
p=Path('/var/lib/looom/dev/installer-20261009/live-root/root/install-work/journal.json')
j=json.loads(p.read_text());assert j['completed']==12 and j['pending'] is None
j['completed']=10;j['pending']='boot'
p.write_text(json.dumps(j,indent=2));os.chmod(p,0o600)
print('VM-only fixture: replay completed boot action as a pending boot checkpoint')
PY
if LOOOM_FAIL_AFTER=install-action-boot arch-chroot "$base/live-root" /root/looom install resume "$work" 2> "$base/live-boot-error.txt"; then exit 1; fi
grep -q 'Injected interruption after install-action-boot' "$base/live-boot-error.txt"
cat "$base/live-boot-error.txt"
entry=$(efibootmgr -v | grep -F "$label")
printf '%s\n' "$entry"
printf '%s\n' "$entry" | grep -qi "$esp_guid"
printf '%s\n' "$entry" | grep -Fqi '\EFI\looom\grubx64.efi'
first=$(efibootmgr | sed -n 's/^BootOrder: //p' | cut -d, -f1)
printf '%s\n' "$entry" | grep -F "Boot${first}"
arch-chroot "$base/live-root" /root/looom install resume "$work"
test "$(efibootmgr -v | grep -F "$label")" = "$entry"
test "$(efibootmgr | awk -v label="$label" '$2==label {n++} END {print n}')" = 1
restore
trap - EXIT
test "$(efibootmgr | sed -n 's/^BootOrder: //p')" = "$before_order"
printf 'PASS: interrupted boot action resumes with same single UEFI entry, expected GUID/path; original BootOrder restored\n'
