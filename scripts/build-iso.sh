#!/usr/bin/env bash
# Run as root inside an Arch Linux VM with archiso installed.
set -Eeuo pipefail
[[ $EUID == 0 ]] || { echo 'Build inside a VM as root.' >&2; exit 1; }
systemd-detect-virt --vm >/dev/null || { echo 'An isolated build VM is required.' >&2; exit 1; }
[[ $# == 3 ]] || { echo "Usage: $0 /absolute/looom /absolute/work /absolute/output" >&2; exit 1; }
manager=$(realpath -- "$1")
work=$2
out=$3
[[ $work == /* && $out == /* && $work != / && $out != / ]] || exit 1
[[ -f $manager && -x $manager ]] || { echo 'Executable looom required.' >&2; exit 1; }
[[ ! -e $work ]] || { echo 'Use a new work directory for each build.' >&2; exit 1; }
command -v mkarchiso >/dev/null
project=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
snapshot=${LOOOM_ISO_SNAPSHOT:-2026-10-07}
version=${LOOOM_ISO_VERSION:-0.2.0}
[[ $snapshot =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ && $version =~ ^[a-zA-Z0-9._-]+$ ]] || exit 1
archive_date=${snapshot//-//}
export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-1791763200}
mkdir -p -- "$work" "$out"
profile=$work/profile
cp -a /usr/share/archiso/configs/releng "$profile"
cp -a "$project/configs/iso/airootfs/." "$profile/airootfs/"
install -Dm755 "$manager" "$profile/airootfs/usr/bin/looom"
install -Dm644 "$project/configs/installer/base.yaml" "$profile/airootfs/usr/share/looom/base.yaml"
install -Dm644 "$project/configs/installer/apps.yaml" "$profile/airootfs/usr/share/looom/apps.yaml"
install -Dm644 "$project/docs/live-iso.md" "$profile/airootfs/usr/share/looom/INSTALLATION.md"
# Do not inherit upstream's optional remote startup script or boot-time mirror selection.
rm -f "$profile/airootfs/root/.automated_script.sh"
printf 'looom-live\n' > "$profile/airootfs/etc/hostname"
cat >> "$profile/profiledef.sh" <<EOF

# looom live installer; the installed system uses Limine + per-release UKIs.
iso_name="looom"
iso_label="LOOOM_020"
iso_publisher="looom <https://github.com/Kexfff/looom>"
iso_application="looom online installer and rescue environment"
iso_version="$version"
bootmodes=('uefi.systemd-boot')
airootfs_image_tool_options=('-comp' 'zstd' '-Xcompression-level' '15' '-b' '1M')
file_permissions["/usr/local/bin/looom-live"]="0:0:755"
file_permissions["/usr/bin/looom"]="0:0:755"
EOF
cat > "$profile/pacman.conf" <<EOF
[options]
Architecture = auto
ParallelDownloads = 5
SigLevel = Required DatabaseOptional
LocalFileSigLevel = Required
[core]
Server = https://archive.archlinux.org/repos/$archive_date/\$repo/os/\$arch
[extra]
Server = https://archive.archlinux.org/repos/$archive_date/\$repo/os/\$arch
EOF
cp "$profile/pacman.conf" "$profile/airootfs/etc/pacman.conf"
printf 'Server = https://archive.archlinux.org/repos/%s/$repo/os/$arch\n' "$archive_date" > "$profile/airootfs/etc/pacman.d/mirrorlist"
for package in gptfdisk dosfstools btrfs-progs arch-install-scripts efibootmgr openssh curl python qemu-guest-agent; do
    grep -qxF "$package" "$profile/packages.x86_64" || printf '%s\n' "$package" >> "$profile/packages.x86_64"
done
for entry in "$profile"/efiboot/loader/entries/*.conf; do
    sed -i 's/Arch Linux install medium/looom installer/g' "$entry"
done
# Serial console is for VM diagnostics; the normal screen retains the welcome menu.
printf '\nkernel_params_x86_64="console=ttyS0,115200 console=tty0"\n' >> "$profile/profiledef.sh"
mkdir -p "$profile/airootfs/etc/systemd/system/serial-getty@ttyS0.service.d"
cat > "$profile/airootfs/etc/systemd/system/serial-getty@ttyS0.service.d/autologin.conf" <<'EOF'
[Service]
ExecStart=
ExecStart=-/sbin/agetty --autologin root --noclear --keep-baud 115200,38400,9600 - $TERM
EOF
manifest=$out/looom-$version-build.txt
{
    printf 'version=%s\nsnapshot=%s\nsource_date_epoch=%s\nsource_revision=%s\n' "$version" "$snapshot" "$SOURCE_DATE_EPOCH" "${LOOOM_SOURCE_REVISION:-unspecified}"
    pacman -Q archiso
    sha256sum "$manager" "$project/scripts/build-iso.sh"
    printf '\nProfile file checksums (relative paths):\n'
    (cd "$profile"; find . -type f -print0 | sort -z | xargs -0 sha256sum)
} > "$manifest"
mkarchiso -v -w "$work/archiso" -o "$out" "$profile"
iso=$out/looom-$version-x86_64.iso
[[ -s $iso ]]
cp "$work/archiso/iso/arch/pkglist.x86_64.txt" "$out/looom-$version-packages.txt"
(cd "$out"; sha256sum "$(basename "$iso")" > "$(basename "$iso").sha256")
printf '\nISO ready: %s\n' "$iso"
