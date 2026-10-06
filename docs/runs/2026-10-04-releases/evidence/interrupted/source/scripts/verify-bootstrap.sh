#!/usr/bin/env bash
# Read-only acceptance checks, run as root on the installed bootstrap.
set -Eeuo pipefail
[[ -d /sys/firmware/efi ]] || { echo 'FAIL: not booted in UEFI'; exit 1; }
[[ $(findmnt -rn -o FSTYPE /) == btrfs ]]
[[ $(findmnt -rn -o FSROOT /) == /@bootstrap ]]
[[ $(findmnt -rn -o FSROOT /home) == /@home ]]
[[ $(findmnt -rn -o FSROOT /var) == /@var ]]
[[ $(findmnt -rn -o FSROOT /var/lib/looom) == /@state ]]
[[ $(findmnt -rn -o FSTYPE /efi) == vfat ]]
[[ $(readlink /var/lib/pacman) == /usr/lib/looom/pacman ]]
[[ $(pacman-conf DBPath) == /usr/lib/looom/pacman/ || $(pacman-conf DBPath) == /usr/lib/looom/pacman ]]
[[ $(id -u codex) == 1000 && $(id -g codex) == 1000 ]]
visudo -cf /etc/sudoers
sshd -t
systemctl is-active NetworkManager sshd qemu-guest-agent systemd-timesyncd
[[ -z $(systemctl --failed --no-legend --plain) ]]
pacman -Dk
findmnt --verify --verbose
grub-script-check /efi/looom/grub/grub.cfg
[[ -s /efi/EFI/Linux/looom-bootstrap.efi && -s /efi/EFI/BOOT/BOOTX64.EFI ]]
printf '\nInstalled bootstrap:\n'
hostnamectl
uname -r
findmnt -t btrfs,vfat
ip -brief address
pacman -Q linux grub btrfs-progs pacman openssh networkmanager
grub-editenv /efi/looom/grub/grubenv list
bootctl status --no-pager
printf '\nPASS: installed UEFI bootstrap checks\n'
