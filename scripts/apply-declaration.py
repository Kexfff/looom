#!/usr/bin/env python3
"""Apply frozen non-secret resources only inside the private package root."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys


def run(*args):
    subprocess.run(args, check=True)


def destination(root, name):
    path = root / name.lstrip('/')
    current = root
    for part in Path(name).parts[1:]:
        current = current / part
        if current.is_symlink():
            raise RuntimeError('Target symlink rejected: ' + name)
    return path


def main():
    root, manifest = map(Path, sys.argv[1:])
    if not root.name.startswith('looom-build-') or os.geteuid()!=0:
        raise RuntimeError('Private build root only')
    bundle = json.loads(manifest.read_text())
    cfg = bundle['config']
    spec = importlib.util.spec_from_file_location('backend', Path(__file__).with_name('mvp-backend.py'))
    backend = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(backend)
    expected = {p['name']: p['version'] for p in backend.validate_lock(bundle)}
    installed = dict(line.split(maxsplit=1) for line in subprocess.check_output(['arch-chroot', str(root), 'pacman', '-Q'], text=True).splitlines())
    if installed != expected:
        raise RuntimeError('Installed closure differs from input lock')
    for target, resource in cfg['files'].items():
        path = destination(root, target)
        if path.exists() and not resource['replace_package_file']:
            raise RuntimeError('Explicit replace_package_file required: ' + target)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(bytes(bundle['files'][target]))
        path.chmod(int(resource['mode'], 8))
        os.chown(path, 0, 0)
    mandatory = ['looom-accounts.service', 'NetworkManager.service', 'sshd.service',
                 'systemd-timesyncd.service', 'serial-getty@ttyS0.service']
    if cfg['desktop']['environment']=='plasma':
        mandatory.append('sddm.service')
    # Package presets are not an additional source of desired service enablement.
    for base in ('etc/systemd/system',):
        for directory in (root/base).glob('*.wants'):
            if directory.is_dir() and not directory.is_symlink():
                for path in directory.iterdir():
                    if path.is_symlink():
                        path.unlink()
    # Vendor boot dependencies shipped by systemd/dbus remain part of the core
    # recipe. Optional packages may not smuggle additional wanted units into it.
    vendor_dependencies = []
    for directory in (root/'usr/lib/systemd/system').glob('*.wants'):
        if directory.is_dir() and not directory.is_symlink():
            for path in directory.iterdir():
                if path.is_symlink():
                    logical = '/' + str(path.relative_to(root))
                    owner = subprocess.check_output(['arch-chroot', str(root), 'pacman', '-Qqo', logical], text=True).strip()
                    if owner not in ('systemd', 'dbus', 'dbus-broker-units', 'filesystem') and logical != '/usr/lib/systemd/system/timers.target.wants/shadow.timer':
                        path.unlink()
                    else:
                        vendor_dependencies.append(logical)
    units = {u:'enabled' for u in mandatory}
    units.update(cfg['units'])
    for name, state in units.items():
        if state=='enabled':
            run('systemctl', '--root='+str(root), 'enable', name)
            enabled = subprocess.check_output(['systemctl', '--root='+str(root), 'is-enabled', name], text=True).strip()
            if enabled not in ('enabled', 'enabled-runtime', 'alias'):
                raise RuntimeError('Unit does not support explicit enablement: ' + name)
        elif state=='disabled':
            run('systemctl', '--root='+str(root), 'disable', name)
        elif state=='masked':
            run('systemctl', '--root='+str(root), 'mask', name)
    # The early account generator is a required dependency, not only a wanted unit.
    run('systemctl', '--root='+str(root), 'enable', 'looom-accounts.service')
    for name in cfg['health']['required_units']:
        candidates = [root / 'etc/systemd/system' / name, root / 'usr/lib/systemd/system' / name]
        if not any(p.exists() for p in candidates):
            raise RuntimeError('Missing required health unit: ' + name)
    run('arch-chroot', str(root), 'ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-f', '/etc/ssh/looom-validation.key')
    try:
        run('arch-chroot', str(root), '/usr/bin/sshd', '-t', '-o', 'HostKey=/etc/ssh/looom-validation.key')
    finally:
        (root/'etc/ssh/looom-validation.key').unlink(missing_ok=True)
        (root/'etc/ssh/looom-validation.key.pub').unlink(missing_ok=True)
    run('arch-chroot', str(root), 'visudo', '-cf', '/etc/sudoers')
    target = root/'usr/lib/looom/declaration.json'
    target.write_text(json.dumps(bundle, indent=2, sort_keys=True)+'\n')
    target.chmod(0o644)
    (root/'usr/lib/looom/unit-plan.json').write_text(json.dumps(dict(units=units, core_vendor_dependencies=vendor_dependencies,
        device_activated_required_units=['qemu-guest-agent.service']), indent=2)+'\n')
    # Freeze the actual CLI + dependency lock with this root. Toolchain stays outside it.
    project = Path(__file__).resolve().parents[1]
    run('install', '-m', '755', str(project/'target/release/looom'), str(root/'usr/bin/looom'))
    source = root/'usr/lib/looom/source'
    source.mkdir()
    import shutil
    for folder in ('scripts', 'configs'):
        shutil.copytree(project/folder, source/folder, ignore=shutil.ignore_patterns('__pycache__'))
    evidence = source/'docs/runs/2026-10-04-bootstrap/evidence'
    evidence.mkdir(parents=True)
    shutil.copy2(project/'docs/runs/2026-10-04-bootstrap/evidence/mkinitcpio.conf', evidence/'mkinitcpio.conf')
    shutil.copy2(project/'Cargo.lock', source/'Cargo.lock')
    if (project/'src').is_dir():
        shutil.copytree(project/'src', source/'src')
        shutil.copy2(project/'Cargo.toml', source/'Cargo.toml')
    print('Frozen declaration, files, services and exact package closure applied')


if __name__ == '__main__':
    main()
