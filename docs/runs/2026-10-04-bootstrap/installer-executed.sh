#!/usr/bin/env bash
# Run inside the Arch ISO, with this repository copied into /run/looom-bootstrap.
# This intentionally erases ONLY the configured disposable VM disk.
set -Eeuo pipefail
umask 022
SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
PROJECT_DIR=$(cd -- "$SCRIPT_DIR/.." && pwd)
source "$PROJECT_DIR/configs/bootstrap/install.env"
TARGET=/mnt/looom-target
TOP=/mnt/looom-top
EVIDENCE="$PROJECT_DIR/evidence"
PUBLIC_KEY=${PUBLIC_KEY:-/run/looom-management.pub}
fail() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
trap 'printf "Installation failed at line %s; mounted target is retained for diagnosis.\n" "$LINENO" >&2' ERR

[[ ${1:-} == "--erase-disk=$DISK" ]] || fail "Pass --erase-disk=$DISK explicitly"
[[ $EUID -eq 0 && -d /run/archiso ]] || fail 'Run as root in the Arch ISO'
[[ $DISK == /dev/vda ]] || fail 'This prototype script only supports /dev/vda'
[[ $(systemd-detect-virt) =~ ^(kvm|qemu)$ ]] || fail 'Expected QEMU/KVM VM'
[[ $(blockdev --getsize64 "$DISK") == "$EXPECTED_DISK_BYTES" ]] || fail 'Unexpected disk size'
[[ $(cat "/sys/class/net/$VM_INTERFACE/address") == "$VM_MAC" ]] || fail 'Unexpected VM MAC'
[[ -s $PUBLIC_KEY ]] || fail 'Supply the management PUBLIC key file'
mountpoints=$(lsblk -nrpo MOUNTPOINTS "$DISK")
[[ -z ${mountpoints//[[:space:]]/} ]] || fail 'The target disk or one of its partitions is mounted'
for tool in sgdisk mkfs.fat mkfs.btrfs btrfs pacstrap arch-chroot curl; do
    command -v "$tool" >/dev/null || fail "Missing live tool: $tool"
done
mkdir -p "$EVIDENCE" "$TARGET" "$TOP"
lsblk -o NAME,SIZE,TYPE,FSTYPE,MOUNTPOINTS > "$EVIDENCE/disks-before.txt"
printf 'Formatting authorized VM disk %s; archive %s\n' "$DISK" "$ARCHIVE_DATE"
curl -fsSI --max-time 30 "https://archive.archlinux.org/repos/$ARCHIVE_DATE/core/os/x86_64/core.db" \
    > "$EVIDENCE/archive-response.txt"

# Retain no secret in the script, logs or package manifests. Reuse the supplied
# live root credential for initial local console login only; SSH uses a key.
bootstrap_hash=$(getent shadow root | cut -d: -f2)
[[ $bootstrap_hash == \$* ]] || fail 'Live root must have an initialized password'

sgdisk --zap-all "$DISK"
sgdisk --new=1:0:+2G --typecode=1:ef00 --change-name=1:LOOOM-ESP \
    --new=2:0:0 --typecode=2:8300 --change-name=2:LOOOM-ROOT "$DISK"
partprobe "$DISK"
udevadm settle
mkfs.fat -F 32 -n LOOOM_EFI "${DISK}1"
mkfs.btrfs -f -L LOOOM_ROOT "${DISK}2"
mount -o subvolid=5,compress=zstd:3 "${DISK}2" "$TOP"
for subvol in "$ROOT_SUBVOL" "$HOME_SUBVOL" "$VAR_SUBVOL" "$STATE_SUBVOL"; do
    btrfs subvolume create "$TOP/$subvol"
done
mount -o "subvol=$ROOT_SUBVOL,compress=zstd:3,noatime" "${DISK}2" "$TARGET"
mkdir -p "$TARGET/efi"
mount "${DISK}1" "$TARGET/efi"

cat > "$EVIDENCE/pacman-install.conf" <<EOF
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
mapfile -t packages < <(sed '/^[[:space:]]*#/d; /^[[:space:]]*$/d' "$PROJECT_DIR/configs/bootstrap/packages.txt")
pacstrap -K -M -C "$EVIDENCE/pacman-install.conf" "$TARGET" "${packages[@]}"

# Move the installed package database BEFORE sharing /var. The compatibility
# symlink in shared /var resolves into whichever release is currently mounted.
mkdir -p "$TARGET/usr/lib/looom"
mv "$TARGET/var/lib/pacman" "$TARGET/usr/lib/looom/pacman"
ln -s /usr/lib/looom/pacman "$TARGET/var/lib/pacman"
cp "$EVIDENCE/pacman-install.conf" "$TARGET/etc/pacman.conf"
sed -i '/^\[options\]/a DBPath = /usr/lib/looom/pacman' "$TARGET/etc/pacman.conf"
mv "$TARGET/var" "$TARGET/var.bootstrap-seed"
mkdir -p "$TARGET/var" "$TARGET/home"
mount -o "subvol=$VAR_SUBVOL,compress=zstd:3,noatime" "${DISK}2" "$TARGET/var"
cp -a "$TARGET/var.bootstrap-seed/." "$TARGET/var/"
rm -rf -- "$TARGET/var.bootstrap-seed"
mount -o "subvol=$HOME_SUBVOL,compress=zstd:3,noatime" "${DISK}2" "$TARGET/home"
mkdir -p "$TARGET/var/lib/looom"
mount -o "subvol=$STATE_SUBVOL,compress=zstd:3,noatime" "${DISK}2" "$TARGET/var/lib/looom"
chmod 700 "$TARGET/var/lib/looom"

root_uuid=$(blkid -s UUID -o value "${DISK}2")
esp_uuid=$(blkid -s UUID -o value "${DISK}1")
cat > "$TARGET/etc/fstab" <<EOF
UUID=$root_uuid / btrfs rw,noatime,compress=zstd:3,subvol=$ROOT_SUBVOL 0 0
UUID=$root_uuid /home btrfs rw,noatime,compress=zstd:3,subvol=$HOME_SUBVOL 0 0
UUID=$root_uuid /var btrfs rw,noatime,compress=zstd:3,subvol=$VAR_SUBVOL 0 0
UUID=$root_uuid /var/lib/looom btrfs rw,noatime,compress=zstd:3,subvol=$STATE_SUBVOL 0 0
UUID=$esp_uuid /efi vfat rw,umask=0077 0 2
tmpfs /tmp tmpfs rw,nosuid,nodev,mode=1777 0 0
EOF
ln -sf "/usr/share/zoneinfo/$VM_TIMEZONE" "$TARGET/etc/localtime"
printf '%s\n' "$VM_HOSTNAME" > "$TARGET/etc/hostname"
printf '127.0.0.1 localhost\n::1 localhost\n127.0.1.1 %s\n' "$VM_HOSTNAME" > "$TARGET/etc/hosts"
printf 'en_US.UTF-8 UTF-8\nru_RU.UTF-8 UTF-8\n' > "$TARGET/etc/locale.gen"
printf 'LANG=en_US.UTF-8\n' > "$TARGET/etc/locale.conf"
printf 'KEYMAP=us\n' > "$TARGET/etc/vconsole.conf"
arch-chroot "$TARGET" locale-gen
arch-chroot "$TARGET" systemd-machine-id-setup
arch-chroot "$TARGET" groupadd -g "$VM_GID" "$VM_USER"
arch-chroot "$TARGET" useradd -m -u "$VM_UID" -g "$VM_GID" -G wheel -s /bin/bash "$VM_USER"
printf 'root:%s\n%s:%s\n' "$bootstrap_hash" "$VM_USER" "$bootstrap_hash" | arch-chroot "$TARGET" chpasswd -e
unset bootstrap_hash
for home in "$TARGET/root" "$TARGET/home/$VM_USER"; do
    install -d -m 700 "$home/.ssh"
    install -m 600 "$PUBLIC_KEY" "$home/.ssh/authorized_keys"
done
chown -R "$VM_UID:$VM_GID" "$TARGET/home/$VM_USER/.ssh"
printf '%s ALL=(ALL:ALL) NOPASSWD: ALL\n' "$VM_USER" > "$TARGET/etc/sudoers.d/10-looom-vm"
chmod 440 "$TARGET/etc/sudoers.d/10-looom-vm"
arch-chroot "$TARGET" visudo -cf /etc/sudoers
mkdir -p "$TARGET/etc/ssh/sshd_config.d"
cat > "$TARGET/etc/ssh/sshd_config.d/10-looom-vm.conf" <<EOF
PubkeyAuthentication yes
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin prohibit-password
AllowUsers root $VM_USER
EOF
arch-chroot "$TARGET" ssh-keygen -A
arch-chroot "$TARGET" sshd -t

install -d -m 700 "$TARGET/etc/NetworkManager/system-connections"
cat > "$TARGET/etc/NetworkManager/system-connections/looom-vm.nmconnection" <<EOF
[connection]
id=looom-vm
type=ethernet
interface-name=$VM_INTERFACE
autoconnect=true
[ethernet]
mac-address=$VM_MAC
[ipv4]
method=auto
dhcp-client-id=mac
[ipv6]
method=auto
EOF
chmod 600 "$TARGET/etc/NetworkManager/system-connections/looom-vm.nmconnection"
systemctl --root="$TARGET" enable NetworkManager sshd systemd-timesyncd qemu-guest-agent serial-getty@ttyS0.service

mkdir -p "$TARGET/etc/kernel" "$TARGET/efi/EFI/Linux" "$TARGET/efi/looom/grub"
printf 'root=UUID=%s rootflags=subvol=%s rw console=tty0 console=ttyS0,115200n8\n' \
    "$root_uuid" "$ROOT_SUBVOL" > "$TARGET/etc/kernel/cmdline"
cat > "$TARGET/etc/mkinitcpio.conf" <<'EOF'
MODULES=(virtio_pci virtio_blk virtio_net btrfs)
BINARIES=()
FILES=()
HOOKS=(base systemd autodetect microcode modconf kms keyboard sd-vconsole block filesystems fsck)
COMPRESSION="zstd"
EOF
kernel_version=$(find "$TARGET/usr/lib/modules" -mindepth 1 -maxdepth 1 -type d -printf '%f\n')
[[ $kernel_version != *$'\n'* && -n $kernel_version ]] || fail 'Expected exactly one installed kernel'
if [[ -f "$TARGET/usr/lib/modules/$kernel_version/vmlinuz" ]]; then
    install -m 644 "$TARGET/usr/lib/modules/$kernel_version/vmlinuz" "$TARGET/boot/vmlinuz-linux"
fi
cat > "$TARGET/etc/mkinitcpio.d/linux.preset" <<'EOF'
ALL_config="/etc/mkinitcpio.conf"
ALL_kver="/boot/vmlinuz-linux"
PRESETS=('default')
default_uki="/efi/EFI/Linux/looom-bootstrap.efi"
default_options="--cmdline /etc/kernel/cmdline"
EOF
arch-chroot "$TARGET" mkinitcpio -P
arch-chroot "$TARGET" grub-install --target=x86_64-efi --efi-directory=/efi \
    --boot-directory=/efi/looom --bootloader-id=looom --no-nvram
# Freshly erased ESP: install the standard UEFI fallback path as well. This
# permits provisioning from a BIOS-booted ISO followed by a cold UEFI start.
arch-chroot "$TARGET" grub-install --target=x86_64-efi --efi-directory=/efi \
    --boot-directory=/efi/looom --removable --no-nvram
cat > "$TARGET/efi/looom/grub/grub.cfg" <<EOF
set timeout=5
set timeout_style=menu
serial --unit=0 --speed=115200 --word=8 --parity=no --stop=1
terminal_input console serial
terminal_output console serial
insmod part_gpt
insmod fat
insmod chain
search --no-floppy --fs-uuid --set=esp $esp_uuid
if [ -s (\$esp)/looom/grub/grubenv ]; then
    load_env -f (\$esp)/looom/grub/grubenv
fi
if [ "\$next_entry" ]; then
    set default="\$next_entry"
    set next_entry=
    save_env -f (\$esp)/looom/grub/grubenv next_entry
elif [ "\$saved_entry" ]; then
    set default="\$saved_entry"
else
    set default=looom-bootstrap
fi
menuentry 'looom bootstrap — Arch $ARCHIVE_DATE' --id looom-bootstrap {
    chainloader (\$esp)/EFI/Linux/looom-bootstrap.efi
}
EOF
arch-chroot "$TARGET" grub-editenv /efi/looom/grub/grubenv create
arch-chroot "$TARGET" grub-editenv /efi/looom/grub/grubenv set saved_entry=looom-bootstrap
arch-chroot "$TARGET" grub-script-check /efi/looom/grub/grub.cfg
printf 'GRUB_DEFAULT=saved\nGRUB_SAVEDEFAULT=false\nGRUB_TIMEOUT=5\n' > "$TARGET/etc/default/grub"

arch-chroot "$TARGET" pacman -Q > "$EVIDENCE/packages.txt"
arch-chroot "$TARGET" pacman -Qqe > "$EVIDENCE/explicit-packages.txt"
arch-chroot "$TARGET" pacman -Qkk > "$EVIDENCE/package-file-check.txt" 2>&1 || true
cp "$TARGET/etc/fstab" "$EVIDENCE/fstab"
cp "$TARGET/etc/kernel/cmdline" "$EVIDENCE/kernel-cmdline"
cp "$TARGET/efi/looom/grub/grub.cfg" "$EVIDENCE/grub.cfg"
cp "$TARGET/etc/pacman.conf" "$EVIDENCE/pacman.conf"
cp "$TARGET/etc/mkinitcpio.conf" "$EVIDENCE/mkinitcpio.conf"
cp "$TARGET/etc/mkinitcpio.d/linux.preset" "$EVIDENCE/linux.preset"
arch-chroot "$TARGET" bash -c 'find /var/cache/pacman/pkg -type f -name "*.pkg.tar.*" -exec sha256sum {} +' \
    > "$EVIDENCE/package-archives.sha256"
arch-chroot "$TARGET" bash -c 'sha256sum /usr/lib/looom/pacman/sync/*.db /efi/EFI/Linux/looom-bootstrap.efi' \
    > "$EVIDENCE/build-artifacts.sha256"
for key in "$TARGET"/etc/ssh/ssh_host_*_key.pub; do
    ssh-keygen -lf "$key" >> "$EVIDENCE/ssh-host-fingerprints.txt"
done
cp "$TARGET/etc/ssh/ssh_host_ed25519_key.pub" "$EVIDENCE/ssh-host-ed25519.pub"
sgdisk -p "$DISK" > "$EVIDENCE/partition-table.txt"
btrfs subvolume list "$TOP" > "$EVIDENCE/subvolumes.txt"
install -d -m 700 "$TARGET/var/lib/looom/bootstrap-evidence"
cp -a "$EVIDENCE/." "$TARGET/var/lib/looom/bootstrap-evidence/"
printf 'archive=%s\nroot=%s\nuser=%s\nuid=%s\ngid=%s\n' \
    "$ARCHIVE_DATE" "$ROOT_SUBVOL" "$VM_USER" "$VM_UID" "$VM_GID" \
    > "$TARGET/var/lib/looom/bootstrap-evidence/install-summary.txt"
sync
printf 'Bootstrap prepared. Fetch evidence and configure UEFI before poweroff.\n'
