#!/usr/bin/env python3
"""Run only inside the dedicated ISO acceptance VM, never on the host.

inspect: real live boot and terminal menu/cancellation, without disk writes.
install: full numbered wizard; private passwords must already exist in this VM.
boot: installed read-only initial release, declarations and apps infrastructure.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys

EXPECTED = 'd76cdb361bde3f7d9469734fb8f23e98b0b812fd68cb72b5cf2ddd9ce361e9db'
BASE = Path('/run/looom-iso-test')


def guard():
    assert os.geteuid() == 0
    assert subprocess.check_output(['systemd-detect-virt', '--vm'], text=True).strip() in ('kvm', 'qemu')
    assert Path('/sys/class/net/enp1s0/address').read_text().strip() == '52:54:00:11:10:11'
    assert Path('/sys/firmware/efi').is_dir()
    assert subprocess.check_output(['blockdev', '--getsize64', '/dev/vda'], text=True).strip() == '68719476736'
    assert hashlib.sha256(Path('/usr/bin/looom').read_bytes()).hexdigest() == EXPECTED
    assert os.access('/usr/bin/looom', os.X_OK), 'looom must retain executable permissions in the ISO'


def driver():
    spec = importlib.util.spec_from_file_location('wizard', Path(__file__).with_name('test-installer-limine.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.BINARY = Path('/usr/bin/looom')
    return module


def inspect():
    assert not Path('/var/lib/looom/machine.json').exists()
    assert 'archisobasedir=arch' in Path('/proc/cmdline').read_text()
    assert Path('/usr/share/looom/INSTALLATION.md').is_file()
    subprocess.run(['looom', 'check', '/usr/share/looom/base.yaml'], check=True)
    subprocess.run(['looom', 'install', '--help'], check=True)
    for tool in ('sgdisk', 'mkfs.fat', 'mkfs.btrfs', 'pacstrap', 'arch-chroot', 'efibootmgr', 'curl', 'iwctl'):
        subprocess.run(['bash', '-c', 'command -v "$1"', 'check', tool], check=True)
    wizard = driver()
    before = wizard.hashlib.sha256(Path('/dev/vda').open('rb').read(1048576)).digest()
    terminal = wizard.Terminal()
    terminal.expect(b'Disk number to erase.*: ', b'0')
    output = terminal.finish(False, b'installation cancelled')
    assert b'1) /dev/vda' in output
    assert b'Available (virtual disk without serial/WWN' in output
    assert b'/dev/sr' not in output and b'/dev/loop' not in output
    assert before == wizard.hashlib.sha256(Path('/dev/vda').open('rb').read(1048576)).digest()
    # Same PTY driver, but start the actual welcome menu instead of the wizard.
    wizard.BINARY = Path('/usr/local/bin/looom-live')
    terminal = wizard.Terminal()
    terminal.expect(b'Choice.*: ', b'1')
    terminal.expect(b'Disk number to erase.*: ', b'0')
    terminal.expect(b'Choice.*: ', b'0')
    terminal.finish(True, b'Return to this menu with: looom-live')
    assert before == wizard.hashlib.sha256(Path('/dev/vda').open('rb').read(1048576)).digest()
    print('PASS: actual UEFI ISO boot, exact native manager, tools and config; menu launches installer, cancellation returns to menu and preserves disk')


def install():
    assert not Path('/var/lib/looom/machine.json').exists()
    passwords = json.loads((BASE/'private/passwords.secret').read_text())
    wizard = driver()
    terminal = wizard.Terminal()
    terminal.expect(b'Disk number to erase.*: ', b'1')
    terminal.expect(b'Computer name.*: ', b'looom-iso-test')
    terminal.expect(b'User name.*: ', b'kexfff')
    for label in (b'Timezone', b'Locale', b'Console keymap'):
        terminal.expect(label+b'.*: ', b'')
    terminal.expect(b'SSH public key file.*: ', str(BASE/'management.pub').encode())
    terminal.expect(b'Type YES to erase.*: ', b'YES')
    for name in ('root', 'user'):
        secret = passwords[name].encode()
        terminal.expect(name.encode()+b' password: ', secret, True)
        terminal.expect(b'Repeat password: ', secret, True)
    output = terminal.finish(True, b'Installation complete', secret)
    for password in passwords.values():
        assert password.encode() not in output
    (BASE/'install.log').write_bytes(output)
    print('PASS: real ISO kernel/userspace completed the numbered whole-disk wizard; passwords not echoed; no automatic reboot')


def boot():
    assert subprocess.check_output(['findmnt', '-nro', 'FSROOT', '/'], text=True).strip() == '/@root-initial'
    assert 'ro' in subprocess.check_output(['findmnt', '-nro', 'OPTIONS', '/'], text=True).strip().split(',')
    for name in ('base.yaml', 'base.lock', 'apps.yaml'):
        file = Path('/home/kexfff/looom')/name
        assert file.is_file() and file.stat().st_uid == 1000 and file.stat().st_gid == 1000
    for name in ('flatpak', 'distrobox', 'podman', 'fusermount'):
        assert (Path('/usr/bin')/name).is_file()
    assert subprocess.check_output(['systemctl', 'is-active', 'sddm'], text=True).strip() == 'active'
    subprocess.run(['looom', 'verify'], check=True)
    subprocess.run(['looom', 'accounts', 'check'], check=True)
    subprocess.run(['looom', 'status'], check=True)
    print('PASS: real installed read-only initial boot, KDE display manager, owner-accessible declarations and user-apps infrastructure')


if __name__ == '__main__':
    guard()
    os.umask(0o077)
    {'inspect': inspect, 'install': install, 'boot': boot}[sys.argv[1]]()
