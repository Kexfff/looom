#!/usr/bin/env python3
"""Run only as root on the dedicated development VM; uses real signed builds."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess


def command(*args, fail=False, env=None):
    print('+', ' '.join(map(str, args)), flush=True)
    result = subprocess.run(args, env=env, text=True, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT)
    print(result.stdout, end='', flush=True)
    assert (result.returncode != 0) if fail else (result.returncode == 0), result.returncode
    return result.stdout


assert os.geteuid() == 0
assert Path('/sys/class/net/enp1s0/address').read_text().strip() == '52:54:00:7b:23:63'
assert command('systemd-detect-virt', '--vm').strip() in ('qemu', 'kvm')
assert 'ro' in command('findmnt', '-nro', 'OPTIONS', '/').strip().split(',')
state = Path('/var/lib/looom')
work = state / 'dev/rebuild-20261011'
public = work / 'public'
public.mkdir(parents=True, exist_ok=True)
binary = state / 'dev/src/target/release/looom'
boot_env = Path('/efi/looom/grub/grubenv')
assert boot_env.is_file(), 'This controller covers the development VM GRUB backend'


def environment():
    return dict(line.split('=', 1) for line in command('grub-editenv', str(boot_env), 'list').splitlines())


def stages(output):
    return re.findall(r'^\[\d/5\] (\w+):', output, re.MULTILINE)


before = environment()
running = Path('/etc/looom/release-id').read_text().strip()
boot_id = Path('/proc/sys/kernel/random/boot_id').read_text()
assert before.get('saved_entry') == 'looom-' + running
source = (state / 'dev/src/configs/native/upgrade/base.yaml').read_text()


def declaration(name, text):
    folder = work / name
    folder.mkdir(mode=0o700)
    path = folder / 'base.yaml'
    path.write_text(text)
    return path


invalid = declaration('invalid', 'schema: 999\n')
output = command(str(binary), 'rebuild', str(invalid), 'rebuild-invalid-20261011', fail=True)
assert stages(output) == ['check'] and 'rebuild stopped at check' in output
assert not (invalid.parent / 'base.lock').exists()
assert environment() == before

for args in [(), (str(invalid),), (str(invalid), 'x', 'extra'), (str(invalid), '../escape')]:
    output = command(str(binary), 'rebuild', *args, fail=True)
    assert not stages(output)
assert environment() == before

duplicate = declaration('duplicate', source)
output = command(str(binary), 'rebuild', str(duplicate), running, fail=True)
assert stages(output) == ['check'] and 'existing build/release' in output
assert not (duplicate.parent / 'base.lock').exists()
assert environment() == before

missing = declaration('missing-package', source.replace('packages: []',
    'packages: [looom-rebuild-intentionally-missing-package]'))
output = command(str(binary), 'rebuild', str(missing), 'rebuild-missing-20261011', fail=True)
assert stages(output) == ['check', 'lock'] and 'rebuild stopped at lock' in output
assert 'target not found' in output and 'looom-rebuild-intentionally-missing-package' in output
assert not (missing.parent / 'base.lock').exists()
assert not (state / 'inputs/rebuild-missing-20261011').exists()
assert environment() == before
print('PASS: malformed declaration, invalid arguments/ID, duplicate release and failed resolution stop before build and preserve boot selection', flush=True)

# Interrupt after real package installation. The private incomplete build is
# retained for diagnosis, and neither publication nor trial selection runs.
failed_id = 'rebuild-interrupted-20261011'
failed = declaration('interrupted-build', source.replace('environment: plasma', 'environment: none'))
output = command(str(binary), 'rebuild', str(failed), failed_id, fail=True,
                 env={**os.environ, 'LOOOM_FAIL_AFTER': 'build-packages'})
assert stages(output) == ['check', 'lock', 'build'] and 'rebuild stopped at build' in output
assert 'Injected interruption after build-packages' in output
assert json.loads((state / f'operations/{failed_id}.json').read_text())['phase'] == 'building'
assert not (state / f'releases/{failed_id}.json').exists()
assert not Path(f'/efi/EFI/Linux/looom-{failed_id}.efi').exists()
assert environment() == before
print('PASS: real interrupted build remains private; publish/try did not run; saved and next boot unchanged', flush=True)

release_id = 'rebuild-desktop-20261011'
valid = declaration('desktop declaration', source.replace('files: {}', '''files:
  /etc/looom-rebuild-proof.conf:
    source: files/proof.conf
    mode: "0644"'''))
(valid.parent / 'files').mkdir()
(valid.parent / 'files/proof.conf').write_text('rebuild includes the checked file contents\n')
try:
    output = command(str(binary), 'rebuild', str(valid), release_id)
    assert stages(output) == ['check', 'lock', 'build', 'publish', 'try']
    assert 'Rebuild complete:' in output and 'Unable to load host key:' not in output
    after = environment()
    assert after.get('saved_entry') == before.get('saved_entry')
    assert after.get('next_entry') == 'looom-' + release_id
    metadata = json.loads((state / f'releases/{release_id}.json').read_text())
    assert metadata['phase'] == 'published'
    root = Path('/run/looom-top') / f'@root-{release_id}'
    assert command('btrfs', 'property', 'get', '-ts', str(root), 'ro').strip() == 'ro=true'
    assert (root / 'etc/looom-rebuild-proof.conf').read_text() == (valid.parent / 'files/proof.conf').read_text()
    lock = json.loads((valid.parent / 'base.lock').read_text())
    bundle = json.loads((state / f'inputs/{release_id}/bundle.json').read_text())
    assert bundle['lock'] == lock
    assert {'plasma-desktop', 'sddm'} <= {p['name'] for p in lock['packages']}
    digest = hashlib.sha256(binary.read_bytes()).hexdigest()
    assert bundle['request']['recipe_sha256'] == digest
    assert hashlib.sha256((root / 'usr/bin/looom').read_bytes()).hexdigest() == digest
    assert hashlib.sha256(Path(f'/efi/EFI/Linux/looom-{release_id}.efi').read_bytes()).hexdigest() == metadata['uki_sha256']
    assert Path('/proc/sys/kernel/random/boot_id').read_text() == boot_id
    assert Path('/etc/looom/release-id').read_text().strip() == running
    assert not Path(f'/run/looom-build-{release_id}').exists()
    assert not any(p.name.startswith('.looom-validation-') for p in (root / 'etc/ssh').iterdir())
    (public / 'rebuild-result.json').write_text(json.dumps({
        'release': release_id, 'phase': metadata['phase'], 'root_read_only': True,
        'package_count': len(lock['packages']), 'recipe_sha256': digest,
        'saved_entry': after.get('saved_entry'), 'next_entry': after.get('next_entry'),
        'running_release_unchanged': True, 'automatic_reboot': False,
    }, indent=2) + '\n')
    print('PASS: actual Plasma rebuild, exact frozen lock/files/binary, read-only root, published UKI, trial selected, saved boot unchanged, no automatic reboot', flush=True)
finally:
    # Only the test selection is undone; keep the published candidate available.
    command('grub-editenv', str(boot_env), 'set', 'next_entry=' + before.get('next_entry', ''))
    assert environment() == before
print('PASS: development VM boot choice restored; published test candidate retained', flush=True)
