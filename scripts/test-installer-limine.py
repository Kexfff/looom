#!/usr/bin/env python3
"""VM-only installer regression: numbered selection and an unlabelled 64 GiB disk."""
import hashlib
import json
import os
from pathlib import Path
import pty
import re
import select
import subprocess
import sys
import termios
import time

BASE = Path('/var/lib/looom/dev/feedback-limine-20261010')
BINARY = BASE / 'looom'
DISK = '/dev/vdb'


def guard():
    assert os.geteuid() == 0
    assert Path('/sys/class/net/enp1s0/address').read_text().strip() == '52:54:00:7b:23:63'
    assert subprocess.check_output(['systemd-detect-virt', '--vm'], text=True).strip() in ('qemu', 'kvm')
    assert subprocess.check_output(['blockdev', '--getsize64', DISK], text=True).strip() == '68719476736'
    assert subprocess.check_output(['lsblk', '-dnro', 'SERIAL,WWN', DISK], text=True).strip() == ''


def digest_disk():
    with open(DISK, 'rb', buffering=0) as f:
        first = f.read(1048576)
        f.seek(-1048576, 2)
        last = f.read()
    return hashlib.sha256(first + last).hexdigest()


def call(args, expected=True, message=None):
    result = subprocess.run([str(BINARY), *args], text=True, capture_output=True)
    assert (result.returncode == 0) == expected, 'unexpected installer exit'
    if message:
        assert message in result.stderr, 'unexpected failure reason'
    return result


def profile():
    cfg = json.loads(Path('/var/lib/looom/dev/installer-20261009/base.yaml').read_text())
    cfg['system']['hostname'] = 'looom-limine'
    cfg['accounts']['user']['name'] = 'kexfff'
    cfg['accounts']['user']['password_secret'] = 'login-kexfff'
    p = BASE / 'base.yaml'
    p.write_text(json.dumps(cfg, indent=2))
    os.chmod(p, 0o600)
    return p


class Terminal:
    def __init__(self):
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.execv(BINARY, [str(BINARY), 'install'])
        self.output = b''
        self.cursor = 0

    def expect(self, pattern, answer, hidden=False):
        deadline = time.monotonic() + 90
        while True:
            match = re.search(pattern, self.output[self.cursor:])
            if match:
                self.cursor += match.end()
                if hidden:
                    # rpassword flushes its prompt before switching the TTY mode.
                    # Wait for that switch rather than racing the child process.
                    echo_deadline = time.monotonic() + 2
                    while termios.tcgetattr(self.fd)[3] & termios.ECHO and time.monotonic() < echo_deadline:
                        time.sleep(0.005)
                    assert not termios.tcgetattr(self.fd)[3] & termios.ECHO, 'password echo enabled'
                os.write(self.fd, (answer() if callable(answer) else answer) + b'\n')
                return
            assert time.monotonic() < deadline, 'prompt timeout'
            if select.select([self.fd], [], [], 1)[0]:
                self.output += os.read(self.fd, 65536)

    def finish(self, success, message=None, secret=None):
        try:
            while True:
                data = os.read(self.fd, 65536)
                if not data:
                    break
                self.output += data
        except OSError:
            pass
        _, status = os.waitpid(self.pid, 0)
        os.close(self.fd)
        assert (status == 0) == success, 'unexpected wizard exit'
        if message:
            assert message in self.output, 'unexpected wizard failure'
        if secret:
            assert secret not in self.output, 'password echoed'
        return self.output

    def disk_number(self):
        return re.search(rb'(\d+)\) /dev/vdb \|', self.output).group(1)

    def configure(self, key=False):
        self.expect(b'Computer name.*: ', b'looom-limine')
        self.expect(b'User name.*: ', b'kexfff')
        for label in [b'Timezone', b'Locale', b'Console keymap']:
            self.expect(label + b'.*: ', b'')
        self.expect(b'SSH public key file.*: ', b'/var/lib/looom/dev/installer-20261009/management.pub' if key else b'')


def negatives():
    before = digest_disk()
    terminal = Terminal()
    terminal.expect(b'Disk number to erase.*: ', b'0')
    output = terminal.finish(False, b'installation cancelled')
    assert b'/dev/loop' not in output and b'/dev/sr' not in output
    assert b'1) /dev/vda' in output and b'2) /dev/vdb' in output
    assert b'Unavailable: selected disk is mounted at /' in output
    assert b'Available (virtual disk without serial/WWN' in output
    assert b'WARNING: existing partitions/data will be erased' in output
    print('PASS: whole disks numbered; optical/loop devices excluded; occupied root unavailable; zero cancels')

    terminal = Terminal()
    for answer in [b'abc', b'999', b'1']:
        terminal.expect(b'Disk number to erase.*: ', answer)
    terminal.expect(b'Disk number to erase.*: ', terminal.disk_number)
    terminal.configure()
    terminal.expect(b'Type YES to erase.*: ', b'CANCEL')
    output = terminal.finish(False, b'installation cancelled')
    assert b'Enter a disk number from 1 to 2' in output and b'Disk unavailable:' in output
    assert b'Confirmation: YES' in output
    print('PASS: invalid number and occupied disk retry; chosen number resolves to vdb; simple YES confirmation shown')

    for mode in ['root', 'user']:
        terminal = Terminal()
        terminal.expect(b'Disk number to erase.*: ', terminal.disk_number)
        terminal.configure()
        terminal.expect(b'Type YES to erase.*: ', lambda: b'YES')
        secret = os.urandom(24).hex().encode()
        terminal.expect(b'root password: ', secret, True)
        terminal.expect(b'Repeat password: ', secret + b'-wrong' if mode == 'root' else secret, True)
        if mode == 'user':
            terminal.expect(b'user password: ', secret, True)
            terminal.expect(b'Repeat password: ', secret + b'-wrong', True)
        terminal.finish(False, b'passwords differ', secret)
        print('PASS: hidden ' + mode + ' password input; mismatch rejects before disk writes')

    work = BASE / 'identity-plan'
    call(['install', 'plan', DISK, str(profile()), str(work), '--no-nvram'])
    p = work / 'plan.json'
    original = p.read_bytes()
    plan = json.loads(original)
    assert plan['disk']['serial'] == plan['disk']['wwn'] == ''
    assert plan['disk']['session_identity']['diskseq'] > 0
    call(['install', 'show', str(work)])
    for field, value, error in [('boot_id', '00000000-0000-0000-0000-000000000000', 'previous live boot'), ('diskseq', plan['disk']['session_identity']['diskseq'] + 1, 'disk identity changed')]:
        changed = json.loads(original)
        changed['disk']['session_identity'][field] = value
        p.write_text(json.dumps(changed))
        call(['install', 'show', str(work)], False, error)
    p.write_bytes(original)
    assert digest_disk() == before
    print('PASS: unlabelled disk plan works; changed boot/diskseq reject; disk remains unchanged')


def replugged():
    call(['install', 'show', str(BASE / 'identity-plan')], False, 'disk identity changed')
    print('PASS: real detach/re-attach of same vdb and image invalidates old disk plan')


def install():
    terminal = Terminal()
    terminal.expect(b'Disk number to erase.*: ', terminal.disk_number)
    terminal.configure(key=True)
    def confirm():
        workspace = Path(re.search(rb'Workspace: ([^\r\n]+)', terminal.output).group(1).decode())
        (BASE / 'wizard-workspace.txt').write_text(str(workspace))
        cache = workspace / 'cache'
        cache.mkdir(mode=0o700)
        subprocess.run(['cp', '--reflink=auto', '-a', '/var/lib/looom/dev/installer-20261009/install/cache/.', str(cache)], check=True)
        return b'YES'
    terminal.expect(b'Type YES to erase.*: ', confirm)
    passwords = json.loads(Path('/var/lib/looom/dev/installer-20261009/private/passwords.secret').read_text())
    for name in ['root', 'user']:
        secret = passwords[name].encode()
        terminal.expect(name.encode() + b' password: ', secret, True)
        terminal.expect(b'Repeat password: ', secret, True)
    # A long install has no interactive prompts. Keep secret bytes out of exported output.
    output = terminal.finish(True, secret=secret)
    for password in passwords.values():
        assert password.encode() not in output, 'secret in installation protocol'
    assert b'Installation complete.' in output
    (BASE / 'public/full-wizard-install.log').write_bytes(output)
    print('PASS: full numbered Limine wizard replaced a nonempty 64 GiB virtual disk without serial/WWN')


if __name__ == '__main__':
    guard()
    os.umask(0o077)
    {'negatives': negatives, 'replugged': replugged, 'install': install}[sys.argv[1]]()
