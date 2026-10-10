#!/usr/bin/env python3
"""Full VM-only installer regression, reusing the historical terminal driver.
Requires the explicitly prepared primary VM and its disposable unlabelled vdb.
Passwords remain in the existing private fixture on that same VM.
"""
import hashlib
import importlib.util
import os
from pathlib import Path
import subprocess

spec = importlib.util.spec_from_file_location('wizard', Path(__file__).with_name('test-installer-limine.py'))
wizard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(wizard)
wizard.BASE = Path('/var/lib/looom/dev/apps-installer-20261010')
wizard.BINARY = Path('/var/lib/looom/dev/src/target/release/looom')
wizard.guard()
os.umask(0o077)
def inspect():
    top = Path('/run/looom-apps-installer-inspect')
    top.mkdir(exist_ok=True)
    subprocess.run(['mount', '-o', 'subvolid=5', '/dev/vdb2', str(top)], check=True)
    try:
        template = Path(__file__).parents[1]/'configs/installer/apps.yaml'
        apps = top/'@home/kexfff/looom/apps.yaml'
        assert apps.read_bytes() == template.read_bytes()
        assert apps.stat().st_uid == apps.stat().st_gid == 1000
        assert apps.stat().st_mode & 0o777 == 0o644
        root = top/'@root-initial'
        for filename in ('etc/subuid', 'etc/subgid'):
            assert (root/filename).read_text() == 'kexfff:100000:65536\n'
        for filename in ('usr/bin/flatpak', 'usr/bin/distrobox', 'usr/bin/podman', 'usr/bin/fusermount'):
            assert (root/filename).exists()
        assert hashlib.sha256((root/'usr/bin/looom').read_bytes()).digest() == hashlib.sha256(wizard.BINARY.read_bytes()).digest()
        assert subprocess.check_output(['btrfs', 'property', 'get', '-ts', str(root), 'ro'], text=True).strip() == 'ro=true'
        print('PASS: fresh read-only installation includes apps infrastructure, stable UID maps, owner-writable apps.yaml and exact native manager', flush=True)
    finally:
        subprocess.run(['umount', str(top)], check=True)


def cleanup():
    import array
    import fcntl
    import re
    partition = subprocess.check_output(['blkid', '-s', 'PARTUUID', '-o', 'value', '/dev/vdb1'], text=True).strip()
    firmware = subprocess.check_output(['efibootmgr', '-v'], text=True)
    current = re.search(r'BootCurrent: ([0-9A-Fa-f]{4})', firmware).group(1)
    own = [(match.group(1), line) for line in firmware.splitlines()
           if (match := re.match(r'Boot([0-9A-Fa-f]{4})\*? looom', line)) and partition in line]
    assert len(own) == 2 and all(number != current for number, line in own)
    selection = Path('/sys/firmware/efi/efivars/LoaderEntryOneShot-4a67b082-0a4c-41cf-b6c7-440b29bb8c4f')
    if selection.exists():
        assert selection.read_bytes()[4:].decode('utf-16-le').rstrip('\0') == 'looom-initial'
        with selection.open('rb') as file:
            flags = array.array('l', [0])
            fcntl.ioctl(file.fileno(), 0x80086601, flags)
            if flags[0] & 0x10:
                flags[0] &= ~0x10
                fcntl.ioctl(file.fileno(), 0x40086602, flags)
        selection.unlink()
    for number, line in own:
        subprocess.run(['efibootmgr', '-b', number, '-B'], check=True)
    subprocess.run(['efibootmgr', '-v'], check=True)
    print('PASS: only scratch-disk EFI entries and its one-shot request removed; primary boot entries preserved', flush=True)

if __name__ == '__main__':
    import sys
    phase = sys.argv[1] if len(sys.argv) > 1 else 'install'
    if phase == 'install':
        (wizard.BASE/'public').mkdir(parents=True, exist_ok=True)
        wizard.install()
        inspect()
    elif phase == 'inspect':
        inspect()
    elif phase == 'cleanup':
        cleanup()
    else:
        raise SystemExit('install | inspect | cleanup')
