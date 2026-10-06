#!/usr/bin/env python3
"""Verify yescrypt in retained release libraries without actual user secrets."""
import subprocess
from pathlib import Path

CODE = '''
import ctypes as C
lib=C.CDLL('libcrypt.so.2')
lib.crypt_gensalt_rn.argtypes=[C.c_char_p,C.c_ulong,C.c_char_p,C.c_int,C.c_char_p,C.c_int]
lib.crypt_gensalt_rn.restype=C.c_void_p
lib.crypt.argtypes=[C.c_char_p,C.c_char_p]; lib.crypt.restype=C.c_char_p
setting=C.create_string_buffer(192)
assert lib.crypt_gensalt_rn(b'$y$',5,None,0,setting,len(setting))
value=lib.crypt(b'not-a-real-user-password',setting.value)
assert value and value.startswith(b'$y$j9T$')
assert lib.crypt(b'not-a-real-user-password',value)==value
assert lib.crypt(b'incorrect-test-value',value)!=value
print('PASS: retained libxcrypt accepts yescrypt and rejects incorrect input')
'''

if __name__=='__main__':
    assert subprocess.check_output(['systemd-detect-virt'],text=True).strip() in ('qemu','kvm')
    for rid in ('v1r3','v2','desktop','mvp-a4','mvp-b'):
        root=Path('/run/looom-top')/('@root-'+rid)
        assert root.is_dir()
        subprocess.run(['arch-chroot',str(root),'python','-c',CODE],check=True)
        print('Verified release:',rid)
