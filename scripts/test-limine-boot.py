#!/usr/bin/env python3
"""Run ONLY inside the explicitly allocated Limine acceptance VM."""
import json
import os
from pathlib import Path
import subprocess
import sys

STATE = Path('/var/lib/looom')
PUBLIC = STATE / 'dev/limine-acceptance/public'
BLI = Path('/sys/firmware/efi/efivars')
GUID = '4a67b082-0a4c-41cf-b6c7-440b29bb8c4f'
EXPECTED = '9b3602801ac3b06587f3639b30f84b43cb71b74c43c8d468d220ac00ff41057b'


def run(*args):
    return subprocess.check_output(args, text=True).strip()


def guard(direct=False):
    assert os.geteuid() == 0
    assert Path('/sys/class/net/enp1s0/address').read_text().strip() == '52:54:00:10:10:11'
    assert run('systemd-detect-virt', '--vm') in ('qemu', 'kvm')
    assert json.loads((STATE / 'bootloader.json').read_text()) == 'limine'
    if direct:
        firmware = run('efibootmgr', '-v')
        current = firmware.split('BootCurrent: ')[1].splitlines()[0]
        line = next(line for line in firmware.splitlines() if line.startswith('Boot' + current))
        assert 'looom-recovery-' in line and '\\EFI\\Linux\\looom-bootstrap.efi' in line
    else:
        assert (BLI / f'LoaderInfo-{GUID}').read_bytes()[4:].decode('utf-16-le').rstrip('\0') == 'Limine 12.9.3'
    import hashlib
    assert hashlib.sha256(Path('/usr/bin/looom').read_bytes()).hexdigest() == EXPECTED
    PUBLIC.mkdir(parents=True, exist_ok=True)


def inspect(expected, direct=False):
    assert not (BLI / f'LoaderEntryOneShot-{GUID}').exists(), 'one-shot not consumed'
    current = Path('/etc/looom/release-id')
    if expected == 'recovery':
        assert not current.exists()
        assert run('findmnt', '-nro', 'FSROOT', '/') == '/@bootstrap'
    else:
        assert current.read_text().strip() == expected
        print(run('looom', 'verify'))
    for unit in ['sshd', 'NetworkManager', 'dbus-broker', 'sddm']:
        assert run('systemctl', 'is-active', unit) == 'active'
    assert run('systemctl', '--failed', '--no-legend', '--plain') == ''
    if direct:
        result = subprocess.run(['looom', 'status'], text=True, capture_output=True)
        assert result.returncode != 0 and 'Limine default missing' in result.stderr
        print('PASS: status rejects the deliberately empty menu before repair')
    else:
        print(run('looom', 'status'))
    cfg = Path('/home/kexfff/looom/base.yaml')
    lock = cfg.with_name('base.lock')
    for path in [cfg, lock]:
        assert path.stat().st_uid == path.stat().st_gid == 1000
        subprocess.run(['runuser', '-u', 'kexfff', '--', 'test', '-r', str(path)], check=True)
        subprocess.run(['runuser', '-u', 'kexfff', '--', 'test', '-w', str(path)], check=True)
    before = cfg.read_bytes()
    subprocess.run(['runuser', '-u', 'kexfff', '--', '/bin/sh', '-c', 'printf "\\n" >> "$HOME/looom/base.yaml"'], check=True)
    assert cfg.read_bytes() == before + b'\n'
    cfg.write_bytes(before)
    print('PASS: ' + expected + '; UEFI boot identity, consumed trial, required services, user-owned editable YAML/lock')


def damage():
    # Direct UEFI recovery bypasses both damaged Limine configuration files.
    print(run('looom', 'boot-recovery'))
    machine = json.loads((STATE / 'machine.json').read_text())
    label = 'looom-recovery-' + machine['root_uuid'][:8]
    entry = next(line[4:8] for line in run('efibootmgr', '-v').splitlines() if line.split()[1:2] == [label])
    for path in [Path('/efi/EFI/looom/limine.conf'), Path('/efi/EFI/BOOT/limine.conf')]:
        path.write_bytes(b'')
    run('sync', '-f', '/efi')
    print(run('efibootmgr', '--bootnext', entry))
    print('PASS: both menus deliberately truncated; direct recovery selected for next boot')


guard(direct=sys.argv[1] == 'inspect-direct')
if sys.argv[1] in ('inspect', 'inspect-direct'):
    inspect(sys.argv[2], direct=sys.argv[1] == 'inspect-direct')
elif sys.argv[1] == 'damage':
    damage()
else:
    raise SystemExit('inspect <release|recovery> | inspect-direct recovery | damage')
