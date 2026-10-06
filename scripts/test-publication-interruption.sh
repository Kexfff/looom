#!/usr/bin/env bash
# Run on VM before initial publication of a validated candidate.
set -Eeuo pipefail
rid=${1:?Release ID}
manager=/var/lib/looom/release-source/scripts/looom-release.py
before_menu=$(sha256sum /efi/looom/grub/grub.cfg)
before_env=$(grub-editenv /efi/looom/grub/grubenv list)
if LOOOM_FAIL_AFTER=uki python "$manager" publish "$rid"; then
    echo 'FAIL: injected interruption did not interrupt publication'; exit 1
fi
[[ $(sha256sum /efi/looom/grub/grub.cfg) == "$before_menu" ]]
[[ $(grub-editenv /efi/looom/grub/grubenv list) == "$before_env" ]]
! grep -q -- "--id looom-$rid {" /efi/looom/grub/grub.cfg
python "$manager" recover
! grep -q -- "--id looom-$rid {" /efi/looom/grub/grub.cfg
python "$manager" publish "$rid"
grep -q -- "--id looom-$rid {" /efi/looom/grub/grub.cfg
[[ $(grub-editenv /efi/looom/grub/grubenv list) == "$before_env" ]]
echo 'PASS: interrupted publication preserves menu and confirmed choice; retry succeeds'
