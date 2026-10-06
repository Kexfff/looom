#!/usr/bin/env python3
"""Root-only credentials, serialized changes and recoverable runtime shadow."""
import argparse
from contextlib import contextmanager
import ctypes as C
import fcntl
import getpass
import json
import os
from pathlib import Path
import re
import secrets
import stat

TEMPLATES = Path('/usr/lib/looom/accounts')
RUNTIME = Path('/run/looom/accounts')
CREDENTIALS = Path('/var/lib/looom/credentials')
MANAGED = ('root', 'codex')


def checked(fd, directory=False, private=False):
    info = os.fstat(fd)
    expected = stat.S_ISDIR if directory else stat.S_ISREG
    if not expected(info.st_mode) or info.st_uid != 0:
        raise RuntimeError('Credential object must be root-owned and have the expected type')
    forbidden = 0o077 if private or not directory else 0o022
    if info.st_mode & forbidden:
        raise RuntimeError('Unsafe credential object permissions')
    if 'system.posix_acl_access' in os.listxattr(fd):
        raise RuntimeError('Credential access ACLs are unsupported')


def directory_fd(path, private=False):
    path = Path(path)
    if not path.is_absolute() or '..' in path.parts:
        raise RuntimeError('Expected an absolute trusted directory')
    fd = os.open('/', os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        for part in path.parts[1:]:
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC,
                            dir_fd=fd)
            os.close(fd)
            fd = child
            checked(fd, directory=True)
        checked(fd, directory=True, private=private)
        return fd
    except BaseException:
        os.close(fd)
        raise


def read_private(path):
    parent = directory_fd(path.parent)
    try:
        fd = os.open(path.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC,
                     dir_fd=parent)
        try:
            checked(fd)
            with os.fdopen(fd, 'r', closefd=False) as stream:
                data = stream.read(16385)
            if len(data) > 16384:
                raise RuntimeError('Credential object too large')
            return data
        finally:
            os.close(fd)
    finally:
        os.close(parent)


def atomic_write(path, data, mode=0o600):
    if mode != 0o600:
        raise ValueError('Credential writes require mode 0600')
    parent = directory_fd(path.parent)
    temporary = '.' + path.name + '.' + secrets.token_hex(12)
    try:
        try:
            existing = os.open(path.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=parent)
        except FileNotFoundError:
            pass
        else:
            try:
                checked(existing)
            finally:
                os.close(existing)
        fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                     mode, dir_fd=parent)
        with os.fdopen(fd, 'w') as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.rename(temporary, path.name, src_dir_fd=parent, dst_dir_fd=parent)
        os.fsync(parent)
    finally:
        try:
            os.unlink(temporary, dir_fd=parent)
        except FileNotFoundError:
            pass
        os.close(parent)


@contextmanager
def credential_lock():
    parent = directory_fd(CREDENTIALS, private=True)
    try:
        fd = os.open('.lock', os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK,
                     0o600, dir_fd=parent)
        try:
            checked(fd)
            fcntl.flock(fd, fcntl.LOCK_EX)
            yield
        finally:
            os.close(fd)
    finally:
        os.close(parent)


def crypt_library():
    lib = C.CDLL('libcrypt.so.2', use_errno=True)
    lib.crypt_gensalt_rn.argtypes = [C.c_char_p, C.c_ulong, C.c_char_p, C.c_int,
                                    C.c_char_p, C.c_int]
    lib.crypt_gensalt_rn.restype = C.c_void_p
    lib.crypt.argtypes = [C.c_char_p, C.c_char_p]
    lib.crypt.restype = C.c_char_p
    return lib


def validate_hash(value):
    sha = re.fullmatch(r'\$6\$(?:rounds=([0-9]+)\$)?[./A-Za-z0-9]{1,16}\$[./A-Za-z0-9]{86}', value)
    yes = re.fullmatch(r'\$y\$(j[0-9A-Za-z./]{1,7})\$[./A-Za-z0-9]{16,86}\$[./A-Za-z0-9]{43}', value)
    if sha and (sha[1] is None or 1000 <= int(sha[1]) <= 1000000):
        return
    if yes and yes[1] == 'j9T':
        return
    raise RuntimeError('Unsupported or malformed password hash')


def hash_password(password):
    if not password or any(c in password for c in ('\n', '\r', '\x00')):
        raise ValueError('Expected a nonempty single-line password')
    lib = crypt_library()
    setting = C.create_string_buffer(192)
    if not lib.crypt_gensalt_rn(b'$y$', 5, None, 0, setting, len(setting)):
        raise RuntimeError('yescrypt salt generation failed')
    value = lib.crypt(password.encode(), setting.value)
    if not value or value.startswith(b'*'):
        raise RuntimeError('yescrypt hashing failed')
    value = value.decode('ascii')
    validate_hash(value)
    return value


def _generate():
    lines = []
    seen = set()
    for line in (TEMPLATES / 'shadow').read_text().splitlines():
        fields = line.split(':')
        if len(fields) != 9 or fields[1] != '!':
            raise RuntimeError('Expected a locked release shadow template')
        if fields[0] in MANAGED:
            value = read_private(CREDENTIALS / (fields[0] + '.hash')).rstrip('\n')
            validate_hash(value)
            fields[1] = value
            seen.add(fields[0])
        lines.append(':'.join(fields))
    if seen != set(MANAGED):
        raise RuntimeError('Managed account missing from release template')
    RUNTIME.mkdir(parents=True, exist_ok=True, mode=0o755)
    fd = directory_fd(RUNTIME)
    os.close(fd)
    atomic_write(RUNTIME / 'gshadow', (TEMPLATES / 'gshadow').read_text())
    atomic_write(RUNTIME / 'shadow', '\n'.join(lines) + '\n')
    atomic_write(CREDENTIALS / '.transaction.json', json.dumps({'phase': 'synchronized'}) + '\n')


def generate():
    with credential_lock():
        _generate()


def set_password(user, password, *, fail_after_commit=False):
    if user not in MANAGED:
        raise ValueError('Unknown managed account')
    with credential_lock():
        value = hash_password(password)
        atomic_write(CREDENTIALS / '.transaction.json', json.dumps({'phase': 'updating', 'user': user}) + '\n')
        atomic_write(CREDENTIALS / (user + '.hash'), value + '\n')
        if fail_after_commit:
            raise RuntimeError('Injected interruption after credential commit')
        _generate()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=('generate', 'password'))
    parser.add_argument('user', nargs='?', choices=MANAGED)
    args = parser.parse_args()
    if os.geteuid() != 0:
        parser.error('Run as root')
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
