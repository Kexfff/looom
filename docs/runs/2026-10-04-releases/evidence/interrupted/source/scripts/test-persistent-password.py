#!/usr/bin/env python3
"""VM-only PAM test. Temporary password remains in protected guest state."""
import argparse
import ctypes as C
import importlib.util
import os
from pathlib import Path
import secrets

TEST = Path('/var/lib/looom/private-password-test')
HASH = Path('/var/lib/looom/credentials/codex.hash')


def accounts_module():
    spec = importlib.util.spec_from_file_location('looom_accounts', '/usr/lib/looom/looom-accounts.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def authenticate(password, service):
    class Message(C.Structure):
        _fields_ = [('style', C.c_int), ('message', C.c_char_p)]

    class Response(C.Structure):
        _fields_ = [('response', C.c_void_p), ('code', C.c_int)]

    callback_type = C.CFUNCTYPE(C.c_int, C.c_int, C.POINTER(C.POINTER(Message)),
                              C.POINTER(C.POINTER(Response)), C.c_void_p)
    libc = C.CDLL('libc.so.6')
    libc.calloc.argtypes = [C.c_size_t, C.c_size_t]
    libc.calloc.restype = C.c_void_p
    libc.strdup.argtypes = [C.c_char_p]
    libc.strdup.restype = C.c_void_p

    @callback_type
    def conversation(count, messages, responses, data):
        allocation = libc.calloc(count, C.sizeof(Response))
        array = C.cast(allocation, C.POINTER(Response))
        for i in range(count):
            style = messages[i].contents.style
            if style in (1, 2):
                answer = password if style == 1 else 'codex'
                array[i].response = libc.strdup(answer.encode())
        responses[0] = array
        return 0

    class Conversation(C.Structure):
        _fields_ = [('function', callback_type), ('data', C.c_void_p)]

    pam = C.CDLL('libpam.so.0')
    pam.pam_start.argtypes = [C.c_char_p, C.c_char_p, C.POINTER(Conversation), C.POINTER(C.c_void_p)]
    pam.pam_authenticate.argtypes = [C.c_void_p, C.c_int]
    pam.pam_acct_mgmt.argtypes = [C.c_void_p, C.c_int]
    pam.pam_end.argtypes = [C.c_void_p, C.c_int]
    handle = C.c_void_p()
    conv = Conversation(conversation, None)
    result = pam.pam_start(service.encode(), b'codex', C.byref(conv), C.byref(handle))
    assert result == 0, f'pam_start status {result}'
    try:
        result = pam.pam_authenticate(handle, 0)
        if result == 0:
            result = pam.pam_acct_mgmt(handle, 0)
        return result
    finally:
        pam.pam_end(handle, result)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=('prepare', 'verify', 'restore'))
    parser.add_argument('--service', default='login')
    args = parser.parse_args()
    assert os.geteuid() == 0
    module = accounts_module()
    if args.operation == 'prepare':
        assert not TEST.exists(), 'An existing password test must be restored first'
        TEST.mkdir(mode=0o700)
        module.atomic_write(TEST / 'original.hash', HASH.read_text(), 0o600)
        password = secrets.token_urlsafe(24)
        module.atomic_write(TEST / 'password.secret', password, 0o600)
        module.set_password('codex', password)
        print('Temporary password changed through the persistent credential API')
    elif args.operation == 'restore':
        module.atomic_write(HASH, (TEST / 'original.hash').read_text(), 0o600)
        module.generate()
        for path in TEST.iterdir():
            path.unlink()
        TEST.rmdir()
        print('Original user password restored; private test state removed')
        return
    password = (TEST / 'password.secret').read_text()
    assert authenticate(password, args.service) == 0, 'Correct password rejected'
    assert authenticate(password + '-wrong', args.service) != 0, 'Incorrect password accepted'
    print(f'PASS: {args.service} PAM accepts changed password and rejects incorrect password')


if __name__ == '__main__':
    main()
