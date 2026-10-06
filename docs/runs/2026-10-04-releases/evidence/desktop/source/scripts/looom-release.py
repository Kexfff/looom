#!/usr/bin/env python3
"""Small VM release manager: ordered publication, explicit trials and confirmation."""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

STATE = Path('/var/lib/looom')
TOP = Path('/run/looom-top')
GRUB = Path('/efi/looom/grub')
RELEASES = STATE / 'releases'


def run(*args):
    return subprocess.check_output(args, text=True).strip()


def sync_directory(path):
    fd = os.open(path, os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def atomic_write(path, data):
    fd, temporary = tempfile.mkstemp(prefix='.' + path.name, dir=path.parent)
    try:
        with os.fdopen(fd, 'w') as stream:
            stream.write(data); stream.flush(); os.fsync(stream.fileno())
        os.replace(temporary, path)
        sync_directory(path.parent)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def load(rid):
    if not re.fullmatch('[a-z0-9-]+', rid):
        raise ValueError('Invalid release ID')
    metadata = json.loads((RELEASES / (rid + '.json')).read_text())
    assert metadata['id'] == rid and metadata['root_subvolume'] == '@root-' + rid
    return metadata


def ensure_top():
    TOP.mkdir(exist_ok=True)
    if subprocess.call(['mountpoint', '-q', str(TOP)]) != 0:
        subprocess.run(['mount', '-o', 'subvolid=5,rw,noatime,compress=zstd:3',
                        '/dev/vda2', str(TOP)], check=True)


def validate(metadata, published=False):
    ensure_top()
    root = TOP / metadata['root_subvolume']
    assert run('btrfs', 'property', 'get', '-ts', str(root), 'ro') == 'ro=true'
    assert (root / 'etc/looom/release-id').read_text().strip() == metadata['id']
    assert (root / 'usr/lib/modules' / metadata['kernel_version']).is_dir()
    uki = root / 'boot' / ('looom-' + metadata['id'] + '.efi')
    assert hashlib.sha256(uki.read_bytes()).hexdigest() == metadata['uki_sha256']
    assert f"rootflags=subvol={metadata['root_subvolume']}" in (root / 'etc/kernel/cmdline').read_text()
    for name in ('shadow', 'gshadow'):
        assert all(line.split(':')[1] == '!'
                   for line in (root / 'usr/lib/looom/accounts' / name).read_text().splitlines())
    if published:
        esp_uki = Path('/efi/EFI/Linux') / uki.name
        assert hashlib.sha256(esp_uki.read_bytes()).hexdigest() == metadata['uki_sha256']
        assert '--id looom-' + metadata['id'] + ' {' in (GRUB / 'grub.cfg').read_text()


def menu(include=None):
    entries = [json.loads(path.read_text()) for path in sorted(RELEASES.glob('*.json'))]
    entries = [entry for entry in entries if entry['phase'] in ('published', 'confirmed')
               or entry['id'] == include]
    esp_uuid = run('findmnt', '-nro', 'UUID', '/efi')
    text = f'''set timeout=5
set timeout_style=menu
serial --unit=0 --speed=115200 --word=8 --parity=no --stop=1
terminal_input console serial
terminal_output console serial
insmod part_gpt
insmod fat
insmod chain
search --no-floppy --fs-uuid --set=esp {esp_uuid}
if [ -s ($esp)/looom/grub/grubenv ]; then
    load_env -f ($esp)/looom/grub/grubenv
fi
if [ "$next_entry" ]; then
    set default="$next_entry"
    set next_entry=
    save_env -f ($esp)/looom/grub/grubenv next_entry
elif [ "$saved_entry" ]; then
    set default="$saved_entry"
else
    set default=looom-bootstrap
fi
menuentry 'looom bootstrap (recovery)' --id looom-bootstrap {{
    chainloader ($esp)/EFI/Linux/looom-bootstrap.efi
}}
'''
    for entry in entries:
        validate(entry)
        rid = entry['id']
        assert hashlib.sha256((Path('/efi/EFI/Linux') / ('looom-' + rid + '.efi')).read_bytes()).hexdigest() == entry['uki_sha256']
        text += (f"menuentry 'looom {rid} ({entry['kernel_version']})' --id looom-{rid} {{\n"
                 f"    chainloader ($esp)/EFI/Linux/looom-{rid}.efi\n}}\n")
    return text


def write_menu(text):
    temporary = GRUB / 'grub.cfg.pending'
    atomic_write(temporary, text)
    subprocess.run(['grub-script-check', str(temporary)], check=True)
    if (GRUB / 'grub.cfg').exists():
        atomic_write(GRUB / 'grub.cfg.previous', (GRUB / 'grub.cfg').read_text())
    os.replace(temporary, GRUB / 'grub.cfg')
    sync_directory(GRUB)


def environment():
    output = run('grub-editenv', str(GRUB / 'grubenv'), 'list')
    return dict(line.split('=', 1) for line in output.splitlines() if '=' in line)


def health(rid):
    assert Path('/etc/looom/release-id').read_text().strip() == rid, 'Running release differs'
    assert run('findmnt', '-nro', 'FSROOT', '/') == '/@root-' + rid
    assert 'ro' in run('findmnt', '-nro', 'OPTIONS', '/').split(',')
    for path, subvol in (('/home', '@home'), ('/var', '@var'), ('/var/lib/looom', '@state')):
        assert run('findmnt', '-nro', 'FSROOT', path) == '/' + subvol
        assert 'rw' in run('findmnt', '-nro', 'OPTIONS', path).split(',')
    assert run('id', '-u', 'codex') == '1000' and run('id', '-g', 'codex') == '1000'
    for service in ('looom-accounts', 'sshd', 'NetworkManager', 'qemu-guest-agent'):
        assert run('systemctl', 'is-active', service) == 'active'
    assert not run('systemctl', '--failed', '--no-legend', '--plain')
    metadata = load(rid)
    assert run('uname', '-r') == metadata['kernel_version']
    validate(metadata, published=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=('status', 'publish', 'try', 'confirm', 'rollback', 'reject', 'recover'))
    parser.add_argument('id', nargs='?')
    args = parser.parse_args()
    if os.geteuid() != 0:
        parser.error('Run as root')
    # Build holds the mutation lock for minutes. Status is a read-only snapshot
    # of atomic metadata files and must remain available during that time.
    if args.operation == 'status':
        current = Path('/etc/looom/release-id')
        print('Running:', current.read_text().strip() if current.exists() else 'bootstrap')
        print('GRUB:', json.dumps(environment(), sort_keys=True))
        for path in sorted(RELEASES.glob('*.json')):
            info = json.loads(path.read_text())
            print(info['id'], info['phase'], info['kernel_version'], info['root_subvolume'])
        return
    with (STATE / 'release-control.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if args.operation == 'recover':
            for path in sorted((STATE / 'operations').glob('*.json')):
                info = json.loads(path.read_text())
                if info['phase'] not in ('published', 'confirmed'):
                    print('Retained incomplete operation:', info['id'], info['phase'])
            write_menu(menu())
            print('Published menu regenerated; incomplete roots are not added')
            return
        if not args.id:
            parser.error('Specify a release ID')
        metadata = load(args.id)
        validate(metadata)
        if args.operation == 'reject':
            current = Path('/etc/looom/release-id')
            assert not current.exists() or current.read_text().strip() != args.id, 'Cannot reject running release'
            assert metadata['phase'] != 'confirmed', 'Cannot reject confirmed release'
            assert 'looom-' + args.id not in environment().values(), 'Cannot reject a selected release'
            metadata['phase'] = 'rejected'
            atomic_write(RELEASES / (args.id + '.json'), json.dumps(metadata, indent=2) + '\n')
            write_menu(menu())
            print('Rejected candidate retained for diagnosis:', args.id)
            return
        if args.operation == 'publish':
            source = TOP / metadata['root_subvolume'] / 'boot' / ('looom-' + args.id + '.efi')
            destination = Path('/efi/EFI/Linux') / source.name
            if not destination.exists():
                temporary = destination.with_suffix('.pending')
                # The unpublished staging file may be left by an interrupted
                # copy; rewriting it never changes a published UKI.
                with source.open('rb') as src, temporary.open('wb') as dst:
                    shutil.copyfileobj(src, dst); dst.flush(); os.fsync(dst.fileno())
                os.replace(temporary, destination)
                sync_directory(destination.parent)
            assert hashlib.sha256(destination.read_bytes()).hexdigest() == metadata['uki_sha256']
            if os.environ.get('LOOOM_FAIL_AFTER') == 'uki':
                raise RuntimeError('Injected interruption after durable UKI, before boot menu')
            write_menu(menu(include=args.id))
            if metadata['phase'] != 'confirmed':
                metadata['phase'] = 'published'
            atomic_write(RELEASES / (args.id + '.json'), json.dumps(metadata, indent=2) + '\n')
            atomic_write(STATE / 'operations' / (args.id + '.json'), json.dumps({'id': args.id, 'phase': metadata['phase']}) + '\n')
            print('Published:', args.id, '(permanent boot choice unchanged)')
            return
        assert metadata['phase'] in ('published', 'confirmed'), 'Release must be published'
        validate(metadata, published=True)
        if args.operation == 'try':
            subprocess.run(['grub-reboot', '--boot-directory=/efi/looom', 'looom-' + args.id], check=True)
            print('Next boot only:', args.id)
        elif args.operation == 'confirm':
            health(args.id)
            subprocess.run(['grub-set-default', '--boot-directory=/efi/looom', 'looom-' + args.id], check=True)
            metadata['phase'] = 'confirmed'
            atomic_write(RELEASES / (args.id + '.json'), json.dumps(metadata, indent=2) + '\n')
            print('Confirmed running healthy release:', args.id)
        elif args.operation == 'rollback':
            assert metadata['phase'] == 'confirmed', 'Rollback requires previously confirmed release'
            subprocess.run(['grub-editenv', str(GRUB / 'grubenv'), 'set', 'saved_entry=looom-' + args.id, 'next_entry=looom-' + args.id], check=True)
            print('Rollback selected; reboot required:', args.id)
        run('sync', '-f', '/efi')
        print('GRUB:', json.dumps(environment(), sort_keys=True))


if __name__ == '__main__':
    main()
