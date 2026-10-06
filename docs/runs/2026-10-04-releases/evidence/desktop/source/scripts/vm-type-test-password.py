#!/usr/bin/env python3
"""Type a guest-generated temporary test password into the focused VM field.

Use only after visually checking SDDM's password field. No secret is printed,
stored on the host, or passed as a plaintext command argument.
"""
from pathlib import Path
import subprocess
import time

project = Path(__file__).resolve().parent.parent
password = subprocess.run([str(project / 'scripts/vm-ssh.sh'),
                           'cat /var/lib/looom/private-password-test/password.secret'],
                          text=True, capture_output=True, check=True).stdout
assert password and '\n' not in password
letters = {
    'q':16, 'w':17, 'e':18, 'r':19, 't':20, 'y':21, 'u':22, 'i':23, 'o':24, 'p':25,
    'a':30, 's':31, 'd':32, 'f':33, 'g':34, 'h':35, 'j':36, 'k':37, 'l':38,
    'z':44, 'x':45, 'c':46, 'v':47, 'b':48, 'n':49, 'm':50,
}
digits = {str(number): number + 1 for number in range(1, 10)} | {'0':11}


def key(*codes):
    subprocess.run(['virsh', '-c', 'qemu:///system', 'send-key', 'Cachyos',
                    '--codeset', 'linux', '--holdtime', '35', *map(str, codes)],
                   check=True, capture_output=True)
    time.sleep(0.025)


# Clear only the focused password field, then type the temporary value.
key(29, 30)  # Ctrl+A
key(14)      # Backspace
for character in password:
    if character.lower() in letters:
        key(*((42,) if character.isupper() else ()), letters[character.lower()])
    elif character in digits:
        key(digits[character])
    elif character == '-':
        key(12)
    elif character == '_':
        key(42, 12)
    else:
        raise ValueError('Unsupported temporary password character')
key(28)  # Enter
print('Guest temporary password entered; verify the session before confirmation')
