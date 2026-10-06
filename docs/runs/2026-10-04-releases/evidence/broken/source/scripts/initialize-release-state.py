#!/usr/bin/env python3
"""One-time migration from writable bootstrap; never prints credential contents."""
import os
from pathlib import Path
import shutil
import subprocess

state = Path('/var/lib/looom')
assert os.geteuid() == 0
assert subprocess.check_output(['findmnt', '-nro', 'FSROOT', '/'], text=True).strip() == '/@bootstrap'
marker = state / 'release-state-initialized'
if marker.exists():
    print('Release state already initialized')
    raise SystemExit(0)
for name in ('credentials', 'ssh', 'account-registry', 'local-etc/NetworkManager/system-connections', 'local-etc/looom-local'):
    path = state / name
    path.mkdir(parents=True, exist_ok=True, mode=0o700)
    os.chmod(path, 0o700)
for line in Path('/etc/shadow').read_text().splitlines():
    fields = line.split(':')
    if fields[0] in ('root', 'codex'):
        assert fields[1].startswith('$')
        path = state / 'credentials' / (fields[0] + '.hash')
        with path.open('x') as stream:
            os.chmod(path, 0o600)
            stream.write(fields[1] + '\n')
for name in ('passwd', 'group', 'shadow', 'gshadow'):
    lines = []
    for line in (Path('/etc') / name).read_text().splitlines():
        fields = line.split(':')
        if name in ('shadow', 'gshadow'):
            fields[1] = '!'
        lines.append(':'.join(fields))
    path = state / 'account-registry' / name
    path.write_text('\n'.join(lines) + '\n')
    os.chmod(path, 0o600 if name in ('shadow', 'gshadow') else 0o644)
for path in Path('/etc/ssh').glob('ssh_host_*'):
    shutil.copy2(path, state / 'ssh' / path.name)
shutil.copytree('/etc/NetworkManager/system-connections',
                state / 'local-etc/NetworkManager/system-connections', dirs_exist_ok=True)
root_ssh = state / 'root-ssh'
shutil.copytree('/root/.ssh', root_ssh, dirs_exist_ok=True)
os.chmod(root_ssh, 0o700)
shutil.copy2('/etc/machine-id', state / 'machine-id')
marker.write_text('Initialized from authorized VM bootstrap; no credentials in releases.\n')
print('Persistent identity, credentials and explicit local paths initialized')
