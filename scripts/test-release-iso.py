#!/usr/bin/env python3
"""0.2.0 release acceptance, inside the dedicated new VM only."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import secrets
import subprocess
import sys

BASE = Path('/run/looom-release-test')
STATE = Path('/var/lib/looom/dev/release-0.2.0-test')
MAC = '52:54:00:20:02:00'

def call(*args):
    return subprocess.check_output(args, text=True).strip()

def guard():
    assert os.geteuid() == 0
    assert Path('/sys/class/net/enp1s0/address').read_text().strip() == MAC
    assert call('systemd-detect-virt', '--vm') in ('qemu', 'kvm')
    assert Path('/sys/firmware/efi').is_dir()
    assert call('blockdev', '--getsize64', '/dev/vda') == '68719476736'
    expected = (BASE/'expected-manager.sha256').read_text().split()[0] if BASE.exists() else (STATE/'expected-manager.sha256').read_text().split()[0]
    assert re.fullmatch('[0-9a-f]{64}', expected)
    assert hashlib.sha256(Path('/usr/bin/looom').read_bytes()).hexdigest() == expected

def driver():
    path = Path(__file__).with_name('test-installer-limine.py')
    spec = importlib.util.spec_from_file_location('wizard', path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.BINARY = Path('/usr/bin/looom')
    return module

def inspect():
    assert 'archisobasedir=arch' in Path('/proc/cmdline').read_text()
    assert not Path('/var/lib/looom/machine.json').exists()
    subprocess.run(['looom', 'check', '/usr/share/looom/base.yaml'], check=True)
    subprocess.run(['looom', 'setup', '--help'], check=True)
    subprocess.run(['looom', 'apps', 'check', '/usr/share/looom/apps.yaml'], check=True)
    assert 'org.mozilla.firefox' in Path('/usr/share/looom/apps.yaml').read_text()
    before = hashlib.sha256(Path('/dev/vda').open('rb').read(1048576)).digest()
    wizard = driver()
    terminal = wizard.Terminal()
    terminal.expect(b'Disk number to erase.*: ', b'0')
    output = terminal.finish(False, b'installation cancelled')
    assert b'1) /dev/vda' in output and b'/dev/sr' not in output and b'/dev/loop' not in output
    wizard.BINARY = Path('/usr/local/bin/looom-live')
    terminal = wizard.Terminal()
    terminal.expect(b'Choice.*: ', b'1')
    terminal.expect(b'Disk number to erase.*: ', b'0')
    terminal.expect(b'Choice.*: ', b'0')
    terminal.finish(True, b'Return to this menu')
    assert before == hashlib.sha256(Path('/dev/vda').open('rb').read(1048576)).digest()
    print('PASS: real UEFI ISO, exact manager, default Firefox declaration, numbered disk, cancellation and welcome menu')

def install():
    assert not Path('/var/lib/looom/machine.json').exists()
    private = BASE/'private'
    private.mkdir(mode=0o700)
    passwords = {name: secrets.token_hex(16) for name in ('root', 'user')}
    (private/'passwords.json').write_text(json.dumps(passwords))
    (private/'passwords.json').chmod(0o600)
    wizard = driver()
    terminal = wizard.Terminal()
    terminal.expect(b'Disk number to erase.*: ', b'1')
    terminal.expect(b'Computer name.*: ', b'looom-020')
    terminal.expect(b'User name.*: ', b'kexfff')
    for label in (b'Timezone', b'Locale', b'Console keymap'):
        terminal.expect(label+b'.*: ', b'')
    terminal.expect(b'SSH public key file.*: ', str(BASE/'management.pub').encode())
    def confirm():
        # Optional verified public warm cache. Never copy credentials or machine state.
        cache = Path('/run/looom-installer/public-package-cache')
        if cache.exists():
            workspace = Path(re.search(rb'Workspace: ([^\r\n]+)', terminal.output).group(1).decode())
            cache.rename(workspace/'cache')
            print('Using explicitly prepared public package cache', flush=True)
        return b'YES'
    terminal.expect(b'Type YES to erase.*: ', confirm)
    for name in ('root', 'user'):
        secret = passwords[name].encode()
        terminal.expect(name.encode()+b' password: ', secret, True)
        terminal.expect(b'Repeat password: ', secret, True)
    output = terminal.finish(True, b'Installation complete')
    assert all(password.encode() not in output for password in passwords.values())
    (BASE/'install.log').write_bytes(output)
    # Keep private login fixture on this same VM, never transfer it to another machine.
    mount = BASE/'installed-state'
    mount.mkdir()
    subprocess.run(['mount', '-o', 'subvol=@state', '/dev/vda2', str(mount)], check=True)
    try:
        destination = mount/'dev/release-0.2.0-test'
        destination.mkdir(parents=True, mode=0o700)
        (destination/'private').mkdir(mode=0o700)
        (destination/'private/passwords.json').write_text(json.dumps(passwords))
        (destination/'private/passwords.json').chmod(0o600)
        (destination/'expected-manager.sha256').write_bytes((BASE/'expected-manager.sha256').read_bytes())
        (destination/'install.log').write_bytes(output)
    finally:
        subprocess.run(['umount', str(mount)], check=True)
    print('PASS: actual ISO whole-disk install, KDE-only wizard, hidden passwords, same-VM private fixture retained')

def boot():
    assert call('findmnt', '-nro', 'FSROOT', '/') == '/@root-initial'
    assert 'ro' in call('findmnt', '-nro', 'OPTIONS', '/').split(',')
    assert call('systemctl', 'is-active', 'sddm', 'power-profiles-daemon').splitlines() == ['active', 'active']
    for name in ('base.yaml', 'base.lock', 'apps.yaml'):
        path = Path('/home/kexfff/looom')/name
        assert path.is_file() and path.stat().st_uid == 1000
    for name in ('powerdevil', 'bluedevil', 'polkit-kde-agent', 'ark', 'kate', 'spectacle', 'pipewire-alsa'):
        subprocess.run(['pacman', '-Q', name], check=True)
    assert Path('/usr/share/applications/looom-setup.desktop').is_file()
    assert Path('/etc/xdg/autostart/looom-first-login.desktop').is_file()
    assert Path('/usr/share/looom/GETTING-STARTED.txt').is_file()
    assert Path('/usr/lib/looom/source/src/setup.rs').is_file()
    subprocess.run(['looom', 'verify'], check=True)
    subprocess.run(['looom', 'accounts', 'check'], check=True)
    print('PASS: installed read-only initial, desktop services and utilities, native setup assets, editable declarations, accounts and exact manager')

if __name__ == '__main__':
    guard()
    os.umask(0o077)
    {'inspect': inspect, 'install': install, 'boot': boot}[sys.argv[1]]()
