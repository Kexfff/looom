#!/usr/bin/env python3
"""Trusted VM adapter for the first Rust CLI; package locks are inputs, not reports."""
import contextlib
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
from urllib.parse import urlparse

PROJECT = Path(__file__).resolve().parents[1]
STATE = Path('/var/lib/looom')
CACHE = Path('/var/cache/pacman/pkg')
DESKTOP = 'plasma-desktop plasma-workspace plasma-nm plasma-pa kscreen xdg-desktop-portal-kde sddm dolphin konsole mesa pipewire pipewire-pulse wireplumber ttf-dejavu noto-fonts spice-vdagent'.split()


def run(*args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            h.update(chunk)
    return h.hexdigest()


def atomic(path, value, mode=0o600):
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, name = tempfile.mkstemp(prefix='.' + path.name, dir=path.parent)
    try:
        os.fchmod(fd, mode)
        with os.fdopen(fd, 'w') as f:
            json.dump(value, f, indent=2, sort_keys=True)
            f.write('\n')
            f.flush()
            os.fsync(f.fileno())
        os.replace(name, path)
        fd = os.open(path.parent, os.O_DIRECTORY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)
    finally:
        if os.path.exists(name):
            os.unlink(name)


def guard():
    if os.geteuid() != 0:
        raise RuntimeError('VM mutations require root')
    if subprocess.check_output(['systemd-detect-virt'], text=True).strip() not in ('qemu', 'kvm'):
        raise RuntimeError('VM backend only')
    if Path('/sys/class/net/enp1s0/address').read_text().strip() != '52:54:00:7b:23:63':
        raise RuntimeError('Unexpected VM identity')


def requested(config):
    base = [p.strip() for p in (PROJECT / 'configs/bootstrap/packages.txt').read_text().splitlines()
            if p.strip() and not p.startswith('#') and p.strip() != 'linux']
    return sorted(set(base + ['dbus-broker-units', config['kernel']['package']] + config['packages'] +
                      (DESKTOP if config['desktop']['environment'] == 'plasma' else [])))


def resolve(bundle, destination):
    guard()
    with tempfile.TemporaryDirectory(prefix='resolve-', dir=STATE / 'dev') as work:
        root = Path(work)
        (root / 'db').mkdir()
        date = bundle['config']['source']['snapshot'].replace('-', '/')
        conf = root / 'pacman.conf'
        conf.write_text('[options]\nArchitecture = x86_64\nSigLevel = Required DatabaseOptional\nLocalFileSigLevel = Required\n' +
                        ''.join(f'[{repo}]\nServer = https://archive.archlinux.org/repos/{date}/$repo/os/$arch\n' for repo in ('core', 'extra')))
        common = ['pacman', '--config', str(conf), '--dbpath', str(root / 'db'),
                  '--cachedir', str(CACHE), '--logfile', str(root / 'pacman.log'),
                  '--gpgdir', '/etc/pacman.d/gnupg', '--noconfirm']
        run(*common, '-Sy')
        # An empty installation DB resolves the whole dependency closure.
        output = subprocess.check_output(common + ['-Sp', '--print-format', '%n %v %r %f'] + requested(bundle['config']), text=True)
        packages = []
        for line in output.splitlines():
            name, version, repository, archive = line.split()
            if repository not in ('core', 'extra') or Path(archive).name != archive:
                raise RuntimeError('Unexpected package source')
            url = f'https://archive.archlinux.org/repos/{date}/{repository}/os/x86_64/{archive}'
            for filename, location in ((archive, url), (archive + '.sig', url + '.sig')):
                path = CACHE / filename
                if not path.exists():
                    pending = path.with_name(path.name + '.looom-pending')
                    run('curl', '--fail', '--location', '--retry', '2', '--output', str(pending), location)
                    os.replace(pending, path)
            packages.append(dict(name=name, version=version, archive=archive,
                                 sha256=digest(CACHE / archive), signature=archive + '.sig',
                                 signature_sha256=digest(CACHE / (archive + '.sig'))))
        # Download-only transaction verifies signatures through pacman before lock publication.
        run(*common, '-Sw', *requested(bundle['config']))
        if len({p['name'] for p in packages}) != len(packages):
            raise RuntimeError('Duplicate package in closure')
        lock = dict(schema_version=1, kind='looom-package-lock', request=bundle['request'],
                    architecture='x86_64', archive_date=date,
                    repositories={r: dict(database_sha256=digest(root / 'db/sync' / (r + '.db'))) for r in ('core', 'extra')},
                    package_signatures='Required; pacman verified', packages=sorted(packages, key=lambda p: p['name']))
        repository_cache = STATE / 'repository-cache' / date
        repository_cache.mkdir(parents=True, exist_ok=True)
        for repo in ('core', 'extra'):
            pending = repository_cache / (repo + '.db.pending')
            shutil.copy2(root / 'db/sync' / (repo + '.db'), pending)
            with pending.open('rb') as f:
                os.fsync(f.fileno())
            os.replace(pending, repository_cache / (repo + '.db'))
        atomic(destination, lock)
        print(f'Locked {len(packages)} signed packages: {destination}')


def validate_lock(bundle):
    lock = bundle['lock']
    if lock.get('request') != bundle['request'] or lock.get('kind') != 'looom-package-lock':
        raise RuntimeError('Lock request mismatch')
    packages = lock['packages']
    if len({p['name'] for p in packages}) != len(packages):
        raise RuntimeError('Duplicate locked package')
    for p in packages:
        for key, hash_key in (('archive', 'sha256'), ('signature', 'signature_sha256')):
            filename = p[key]
            if Path(filename).name != filename or not re.fullmatch(r'[a-zA-Z0-9@._+:-]+', filename):
                raise RuntimeError('Invalid archive filename')
            if digest(CACHE / filename) != p[hash_key]:
                raise RuntimeError('Locked archive/signature hash mismatch: ' + p['name'])
    return packages


def check_state(config):
    # First VM migration-free subset. Additional mounts need a journaled state registry.
    expected = {
        '/etc/NetworkManager/system-connections': {'id': 'network-connections', 'owner': 'root', 'group': 'root', 'mode': '0700'},
        '/etc/looom-local': {'id': 'local-settings', 'owner': 'root', 'group': 'root', 'mode': '0700'},
    }
    if config['persistent']['directories'] != expected:
        raise RuntimeError('MVP VM requires the existing two persistent directory contracts; migrations are deferred')
    for user in ('root', 'codex'):
        # Full no-follow/permissions/hash checks use the new credential component.
        import importlib.util
        spec = importlib.util.spec_from_file_location('accounts_check', PROJECT / 'scripts/looom-accounts.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        module.validate_hash(module.read_private(STATE / 'credentials' / (user + '.hash')).strip())


def plan(bundle):
    packages = bundle['lock']['packages']
    current = dict(line.split(maxsplit=1) for line in subprocess.check_output(['pacman', '-Q'], text=True).splitlines())
    target = {p['name']: p['version'] for p in packages}
    changes = dict(add={k:v for k,v in target.items() if k not in current},
                   remove={k:v for k,v in current.items() if k not in target},
                   upgrade={k:dict(old=current[k], new=v) for k,v in target.items() if k in current and current[k] != v})
    previous = Path('/usr/lib/looom/declaration.json')
    previous = json.loads(previous.read_text()) if previous.exists() else {'config': {'units': {}, 'files': {}}, 'files': {}}
    old_files = {k: hashlib.sha256(bytes(v)).hexdigest() for k, v in previous['files'].items()}
    new_files = {k: hashlib.sha256(bytes(v)).hexdigest() for k, v in bundle['files'].items()}
    file_changes = dict(add=sorted(new_files.keys()-old_files.keys()), remove=sorted(old_files.keys()-new_files.keys()),
                        change=sorted(k for k in old_files.keys()&new_files.keys() if old_files[k]!=new_files[k]
                            or any(previous['config']['files'][k][field]!=bundle['config']['files'][k][field] for field in ('mode','owner','group'))))
    old_units = previous['config']['units']; new_units = bundle['config']['units']
    unit_changes = {k: dict(old=old_units.get(k), new=new_units.get(k)) for k in old_units.keys()|new_units.keys() if old_units.get(k)!=new_units.get(k)}
    print(json.dumps(dict(explicit_requests=requested(bundle['config']), packages=changes,
                         file_changes=file_changes, unit_changes=unit_changes,
                         target_packages=len(target), system=bundle['config']['system'],
                         kernel=bundle['config']['kernel'], desktop=bundle['config']['desktop'],
                         units=bundle['config']['units'], health=bundle['config']['health'],
                         files={k: dict(sha256=hashlib.sha256(bytes(v)).hexdigest(), **bundle['config']['files'][k]) for k,v in bundle['files'].items()},
                         persistent=bundle['config']['persistent']), indent=2, sort_keys=True))


def build(bundle):
    guard()
    rid = bundle['release']
    if not re.fullmatch(r'[a-z][a-z0-9-]{0,63}', rid):
        raise RuntimeError('Invalid release ID')
    check_state(bundle['config'])
    validate_lock(bundle)
    inputs = STATE / 'inputs' / rid
    inputs.mkdir(parents=True, mode=0o700, exist_ok=False)
    atomic(inputs / 'bundle.json', bundle)
    frozen = inputs / 'source'
    frozen.mkdir(mode=0o700)
    for folder in ('scripts', 'configs'):
        shutil.copytree(PROJECT / folder, frozen / folder, ignore=shutil.ignore_patterns('__pycache__'))
    shutil.copytree(PROJECT / 'src', frozen / 'src')
    shutil.copy2(PROJECT / 'Cargo.toml', frozen / 'Cargo.toml')
    evidence = frozen / 'docs/runs/2026-10-04-bootstrap/evidence'
    evidence.mkdir(parents=True)
    shutil.copy2(PROJECT / 'docs/runs/2026-10-04-bootstrap/evidence/mkinitcpio.conf', evidence / 'mkinitcpio.conf')
    shutil.copy2(PROJECT / 'Cargo.lock', frozen / 'Cargo.lock')
    (frozen / 'target/release').mkdir(parents=True)
    binary = PROJECT / 'target/release/looom'
    if not binary.is_file():
        binary = Path('/usr/bin/looom')
    shutil.copy2(binary, frozen / 'target/release/looom')
    # The old trusted shell builder is retained during the staged Rust migration.
    # All generated assignments are shell-quoted; user YAML is never sourced.
    cfg = bundle['config']
    profile = frozen / 'configs/releases' / (rid + '.env')
    if profile.exists():
        raise RuntimeError('Refusing to overwrite profile')
    values = dict(RELEASE_ID=rid, KERNEL_PACKAGE=cfg['kernel']['package'],
                  DECLARATIVE_VALUE=rid, DESKTOP='yes' if cfg['desktop']['environment']=='plasma' else 'no', BROKEN_BOOT='no')
    profile.write_text(''.join(k+'='+shlex.quote(v)+'\n' for k,v in values.items())+
                       'EXTRA_PACKAGES=('+ ' '.join(shlex.quote(p) for p in cfg['packages'] + (DESKTOP if values['DESKTOP']=='yes' else []))+')\n')
    environment = os.environ.copy()
    environment['LOOOM_MANIFEST'] = str(inputs / 'bundle.json')
    run('bash', str(frozen / 'scripts/build-release.sh'), rid, env=environment)


def recover_builds():
    for operation in sorted((STATE / 'operations').glob('*.json')):
        record = json.loads(operation.read_text())
        rid = record['id']
        if record['phase'] != 'configured' or not re.fullmatch(r'[a-z][a-z0-9-]{0,63}', rid):
            continue
        frozen = STATE / 'inputs' / rid / 'source'
        if not frozen.is_dir():
            continue
        metadata = STATE / 'releases' / (rid + '.json')
        if metadata.exists():
            atomic(operation, dict(id=rid, phase=json.loads(metadata.read_text())['phase']))
            continue
        environment = os.environ.copy()
        environment.pop('LOOOM_FAIL_AFTER', None)
        environment['LOOOM_MANIFEST'] = str(STATE / 'inputs' / rid / 'bundle.json')
        run('bash', str(frozen / 'scripts/finalize-release.sh'), rid, env=environment)


def main():
    op = sys.argv[1]
    if op in ('status', 'publish', 'try', 'confirm', 'rollback', 'recover'):
        guard()
        if op == 'recover':
            with (STATE / 'mvp-input.lock').open('a') as lock:
                fcntl.flock(lock, fcntl.LOCK_EX)
                recover_builds()
        args = [sys.executable, str(PROJECT / 'scripts/looom-release.py'), op]
        if op == 'confirm' and len(sys.argv) == 2:
            args.append(Path('/etc/looom/release-id').read_text().strip())
        else:
            args.extend(sys.argv[2:])
        run(*args)
        return
    bundle = json.load(sys.stdin)
    if op == 'plan':
        plan(bundle)
        return
    guard()
    (STATE / 'dev').mkdir(mode=0o700, exist_ok=True)
    with (STATE / 'mvp-input.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if op == 'lock':
            resolve(bundle, Path(sys.argv[2]))
        elif op == 'build':
            build(bundle)
        else:
            raise RuntimeError('Unknown backend operation')


if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, OSError, ValueError, subprocess.CalledProcessError) as e:
        print('looom backend: ' + str(e), file=sys.stderr)
        raise SystemExit(1)
