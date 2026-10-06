#!/usr/bin/env python3
"""Generate runtime NSS/PAM files; persist only explicitly managed password hashes."""
import argparse
import getpass
import os
from pathlib import Path
import subprocess
import tempfile

TEMPLATES = Path('/usr/lib/looom/accounts')
RUNTIME = Path('/run/looom/accounts')
CREDENTIALS = Path('/var/lib/looom/credentials')
MANAGED = ('root', 'codex')


def atomic_write(path, data, mode):
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd, temporary = tempfile.mkstemp(prefix='.' + path.name, dir=path.parent)
    try:
        os.fchmod(fd, mode)
        with os.fdopen(fd, 'w') as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        directory = os.open(path.parent, os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def generate():
    RUNTIME.mkdir(parents=True, exist_ok=True, mode=0o755)
    os.chmod(RUNTIME, 0o755)
    for name in ('gshadow',):
        atomic_write(RUNTIME / name, (TEMPLATES / name).read_text(),
                     0o600)
    lines = []
    for line in (TEMPLATES / 'shadow').read_text().splitlines():
        fields = line.split(':')
        if fields[0] in MANAGED:
            credential = CREDENTIALS / (fields[0] + '.hash')
            if credential.stat().st_mode & 0o077:
                raise RuntimeError('Credential permissions must be 0600 or stricter')
            value = credential.read_text().strip()
            if not value.startswith('$') or ':' in value or '\n' in value:
                raise RuntimeError('Invalid credential format')
            fields[1] = value
        lines.append(':'.join(fields))
    atomic_write(RUNTIME / 'shadow', '\n'.join(lines) + '\n', 0o600)


def set_password(user, password):
    if user not in MANAGED or not password or '\n' in password:
        raise ValueError('Expected a nonempty password for root or codex')
    hashed = subprocess.run(['openssl', 'passwd', '-6', '-stdin'],
                            input=password + '\n', text=True, capture_output=True,
                            check=True).stdout
    atomic_write(CREDENTIALS / (user + '.hash'), hashed, 0o600)
    generate()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=('generate', 'password'))
    parser.add_argument('user', nargs='?', choices=MANAGED)
    args = parser.parse_args()
    if os.geteuid() != 0:
        parser.error('Run as root (sudo looom-password codex)')
    if args.operation == 'generate':
        generate()
    else:
        if not args.user:
            parser.error('Specify root or codex')
        password = getpass.getpass('New password: ')
        if password != getpass.getpass('Repeat password: '):
            parser.error('Passwords differ')
        set_password(args.user, password)


if __name__ == '__main__':
    main()
