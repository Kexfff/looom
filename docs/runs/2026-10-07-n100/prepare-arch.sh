#!/usr/bin/env bash
# Executed on the authorized physical pilot from Arch ISO, not a general installer.
# Credentials and SSH private keys never leave the target.
set -Eeuo pipefail
[[ ${1:-} == --erase-authorized-n100 || ${1:-} == --resume-packages ]] || exit 2
[[ $EUID == 0 && -d /run/archiso && -d /sys/firmware/efi/efivars ]]
[[ $(cat /sys/class/dmi/id/product_name) == 'MINI S' ]]
[[ $(lsblk -ndo MODEL /dev/nvme0n1 | xargs) == '512GB SSD' ]]
[[ $(lsblk -ndo SIZE /dev/nvme0n1) == 476.9G ]]
[[ $(lsblk -ndo TRAN /dev/nvme0n1) == nvme ]]
[[ -z $(swapon --noheadings --show=NAME) ]]
grep -q 'Intel(R) N100' /proc/cpuinfo
[[ $(cat /sys/class/net/enp1s0/address) == e8:ff:1e:d0:be:47 ]]
test -f /root/.ssh/authorized_keys
test -x /tmp/looom-native
curl --fail --head --max-time 30 https://archive.archlinux.org/repos/2026/10/03/core/os/x86_64/core.db
for secureboot in /sys/firmware/efi/efivars/SecureBoot-*; do
    [[ $(od -An -j4 -N1 -t u1 "$secureboot" | xargs) == 0 ]]
done
if [[ $1 == --erase-authorized-n100 ]]; then
[[ -z $(lsblk -nro MOUNTPOINTS /dev/nvme0n1 | tr -d '[:space:]') ]]
echo 'AUTHORIZED TARGET VERIFIED: internal NVMe; USB excluded'
sgdisk --zap-all /dev/nvme0n1
sgdisk --new=1:0:+2G --typecode=1:ef00 --change-name=1:LOOOM-ESP \
       --new=2:0:0 --typecode=2:8300 --change-name=2:LOOOM-ROOT /dev/nvme0n1
partprobe /dev/nvme0n1
udevadm settle
mkfs.fat -F32 -n LOOOM_EFI /dev/nvme0n1p1
mkfs.btrfs -f -L LOOOM_ROOT /dev/nvme0n1p2
mkdir -p /mnt/looom-top
mount -o subvolid=5,compress=zstd:3 /dev/nvme0n1p2 /mnt/looom-top
for volume in @bootstrap @home @var @state; do
    btrfs subvolume create "/mnt/looom-top/$volume"
done
umount /mnt/looom-top
mount -o subvol=@bootstrap,noatime,compress=zstd:3 /dev/nvme0n1p2 /mnt
mkdir -p /mnt/home /mnt/var /mnt/efi
mount -o subvol=@home,noatime,compress=zstd:3 /dev/nvme0n1p2 /mnt/home
mount -o subvol=@var,noatime,compress=zstd:3 /dev/nvme0n1p2 /mnt/var
mkdir -p /mnt/var/lib/looom
mount -o subvol=@state,noatime,compress=zstd:3 /dev/nvme0n1p2 /mnt/var/lib/looom
chmod 0700 /mnt/var/lib/looom
mount -o umask=0077 /dev/nvme0n1p1 /mnt/efi
else
[[ $(findmnt -nro FSROOT /mnt) == /@bootstrap ]]
[[ $(findmnt -nro SOURCE /mnt) == '/dev/nvme0n1p2[/@bootstrap]' ]]
[[ $(findmnt -nro FSROOT /mnt/var/lib/looom) == /@state ]]
[[ $(findmnt -nro SOURCE /mnt/efi) == /dev/nvme0n1p1 ]]
[[ ! -e /mnt/var/lib/looom/machine.json ]]
[[ ! -L /mnt/var/lib/pacman ]]
echo 'RESUMING PACKAGE STAGE ONLY; no repartitioning'
fi
cat > /tmp/looom-pacman.conf <<'EOF'
[options]
Architecture = auto
CheckSpace
ParallelDownloads = 5
SigLevel = Required DatabaseOptional
LocalFileSigLevel = Required
[core]
Server = https://archive.archlinux.org/repos/2026/10/03/$repo/os/$arch
[extra]
Server = https://archive.archlinux.org/repos/2026/10/03/$repo/os/$arch
EOF
pacstrap -K -M -C /tmp/looom-pacman.conf /mnt \
    base linux linux-firmware intel-ucode btrfs-progs dosfstools gptfdisk grub \
    efibootmgr mkinitcpio systemd-ukify arch-install-scripts openssh sudo \
    networkmanager curl git vim pciutils rust base-devel
cp /tmp/looom-pacman.conf /mnt/etc/pacman.conf
genfstab -U /mnt > /mnt/etc/fstab
install -m 0755 /tmp/looom-native /mnt/usr/bin/looom
# Keep the already pinned host identity. No host private key is exported.
install -d -m 0700 /mnt/root/.ssh
install -m 0600 /root/.ssh/authorized_keys /mnt/root/.ssh/authorized_keys
cp -p /etc/ssh/ssh_host_* /mnt/etc/ssh/
# Reuse the authorized live root credential as an initial credential internally.
# Do not echo it, include it in arguments, or enable shell tracing.
awk -F: '$1=="root" {print "root:" $2; print "owner:" $2}' /etc/shadow > /mnt/root/initial-credentials
chmod 0600 /mnt/root/initial-credentials
arch-chroot /mnt /bin/bash -s <<'CHROOT'
set -Eeuo pipefail
ln -sf /usr/share/zoneinfo/Europe/Moscow /etc/localtime
printf 'en_US.UTF-8 UTF-8\nru_RU.UTF-8 UTF-8\n' > /etc/locale.gen
locale-gen
printf 'LANG=en_US.UTF-8\n' > /etc/locale.conf
printf 'KEYMAP=us\n' > /etc/vconsole.conf
printf 'looom-pc\n' > /etc/hostname
printf '127.0.0.1 localhost\n::1 localhost\n127.0.1.1 looom-pc\n' > /etc/hosts
systemd-machine-id-setup
getent group owner >/dev/null || groupadd -g 1000 owner
id -u owner >/dev/null 2>&1 || useradd -m -u 1000 -g 1000 -G wheel -s /bin/bash owner
chpasswd -e < /root/initial-credentials
rm /root/initial-credentials
printf '%%wheel ALL=(ALL:ALL) ALL\n' > /etc/sudoers.d/10-wheel
chmod 0440 /etc/sudoers.d/10-wheel
visudo -cf /etc/sudoers
install -d -m 0700 /etc/looom-local /etc/NetworkManager/system-connections
install -d -m 0700 -o owner -g owner /home/owner/.ssh
install -m 0600 -o owner -g owner /root/.ssh/authorized_keys /home/owner/.ssh/authorized_keys
cat > /etc/NetworkManager/system-connections/looom-wired.nmconnection <<'EOF'
[connection]
id=looom-wired
uuid=5314f047-309d-47b7-a76d-16cb2201d4b2
type=ethernet
interface-name=enp1s0
autoconnect=true
[ethernet]
mac-address=e8:ff:1e:d0:be:47
[ipv4]
method=manual
address1=192.168.1.200/24,192.168.1.1
dns=192.168.1.1;
[ipv6]
method=auto
EOF
chmod 0600 /etc/NetworkManager/system-connections/looom-wired.nmconnection
cat > /etc/ssh/sshd_config.d/10-looom-management.conf <<'EOF'
PermitRootLogin prohibit-password
PasswordAuthentication no
KbdInteractiveAuthentication no
AllowUsers root owner
EOF
sshd -t
systemctl enable NetworkManager systemd-timesyncd sshd
install -d /usr/lib/looom
mv /var/lib/pacman /usr/lib/looom/pacman
ln -s /usr/lib/looom/pacman /var/lib/pacman
sed -i '/^\[options\]/a DBPath = /usr/lib/looom/pacman' /etc/pacman.conf
pacman -Dk
looom --version
CHROOT
install -d -m 0700 /mnt/var/lib/looom/config/desktop-a
install -m 0600 /tmp/physical-base.yaml /mnt/var/lib/looom/config/desktop-a/base.yaml
arch-chroot /mnt looom bootstrap /var/lib/looom/config/desktop-a/base.yaml
arch-chroot /mnt looom boot-entry
arch-chroot /mnt looom status
efibootmgr -v
cp /tmp/looom-install.log /mnt/var/lib/looom/physical-install.log
# arch-chroot bind-mounts resolv.conf; replace it only after all chroot calls.
rm /mnt/etc/resolv.conf
ln -s /run/NetworkManager/resolv.conf /mnt/etc/resolv.conf
echo 'INSTALLATION PREPARED; reboot is a separate, verified step'
