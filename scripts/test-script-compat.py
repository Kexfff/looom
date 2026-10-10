#!/usr/bin/env python3
"""Execute only in the dedicated desktop VM as its managed user."""
import hashlib
import json
import os
from pathlib import Path
import subprocess


def command(*args, expected=0, cwd=None):
    print('+', ' '.join(map(str, args)), flush=True)
    result = subprocess.run(args, cwd=cwd, text=True, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT)
    print(result.stdout, end='', flush=True)
    if expected == 0:
        assert result.returncode == 0, (args, result.returncode)
    else:
        assert result.returncode == expected, (args, result.returncode)
    return result.stdout


assert os.geteuid() == 1000 and Path.home() == Path('/home/kexfff')
assert Path('/sys/class/net/enp1s0/address').read_text().strip() == '52:54:00:10:10:11'
assert command('systemd-detect-virt', '--vm').strip() in ('qemu', 'kvm')
assert 'ro' in command('findmnt', '-no', 'OPTIONS', '/').strip().split(',')
os.environ['DBUS_SESSION_BUS_ADDRESS'] = 'unix:path=/run/user/1000/bus'
looom = os.environ.get('LOOOM_COMPAT_BINARY', '/usr/bin/looom')
work = Path.home() / 'looom/compat-acceptance'
work.mkdir(exist_ok=True)
apps = Path.home() / 'looom/apps.yaml'
apps_hash = hashlib.sha256(apps.read_bytes()).hexdigest()
host_inventory = command('pacman', '-Q')
host_path = os.environ['PATH']
(work/'support.txt').write_text('support\n')
(work/'user installer.sh').write_text('''#!/bin/bash
set -euo pipefail
test "$EUID" = 1000
test "$(whoami)" = kexfff
test "$HOME" = /home/kexfff
test -e /run/.containerenv
test ! -e /etc/looom/release-id
test "$(cat support.txt)" = support
pacman -S --needed --noconfirm figlet base-devel
sudo /usr/bin/pacman -S --needed --noconfirm bc
printf '%s\\n' "$2" > "$1"
''')
literal = 'literal spaces; $(touch /tmp/looom-compat-injection)'
result_file = work/'user-result.txt'
command(looom, 'apps', 'run-script', str(work/'user installer.sh'), str(result_file), literal)
assert result_file.read_text() == literal+'\n' and result_file.stat().st_uid == 1000
assert not Path('/tmp/looom-compat-injection').exists()

# makepkg must remain an ordinary user; install a local foreign package to test
# the same classification used for AUR, without depending on an AUR server.
(work/'PKGBUILD').write_text('''pkgname=looom-compat-fixture-local
pkgver=1
pkgrel=1
arch=(any)
depends=(bc)
package() {
    install -d "$pkgdir/usr/share/looom-compat-fixture"
    printf '%s\\n' fixture > "$pkgdir/usr/share/looom-compat-fixture/local"
}
''')
(work/'build.sh').write_text('''#!/bin/bash
set -euo pipefail
test "$EUID" = 1000
makepkg --force --noconfirm
sudo /usr/bin/pacman -U --noconfirm looom-compat-fixture-local-1-1-any.pkg.tar.zst
''')
command(looom, 'apps', 'run-script', str(work/'build.sh'))
(work/'root.sh').write_text('''#!/bin/bash
set -eu
test "$(id -u)" = 0
test -e /run/.containerenv
test ! -e /etc/looom/release-id
/usr/bin/pacman -Q bc
''')
command(looom, 'apps', 'run-script', str(work/'root.sh'), expected=1)
command(looom, 'apps', 'run-script', '--root', str(work/'root.sh'))

export = work/'exported.yaml'
if export.exists():
    export.unlink()
command(looom, 'apps', 'export-packages', str(export))
parsed = json.loads(export.read_text())
assert parsed['schema'] == 1
assert {'figlet', 'bc', 'base-devel'} <= set(parsed['packages'])
assert 'looom-compat-fixture-local' in parsed['foreign']
assert export.stat().st_uid == 1000
unchanged = export.read_bytes()
command(looom, 'apps', 'export-packages', str(export), expected=1)
assert export.read_bytes() == unchanged
assert json.loads(command(looom, 'apps', 'export-packages')) == parsed

# Explicitly keep the local foreign package in the container, and freeze the
# whole official request list for a real system build. No individual dependency
# names are copied by hand.
official = work/'official.yaml'
official.write_text(json.dumps({'schema': 1, 'packages': parsed['packages']}, indent=2)+'\n')
assert apps_hash == hashlib.sha256(apps.read_bytes()).hexdigest()
assert host_inventory == command('pacman', '-Q')
assert os.environ['PATH'] == host_path
print('PASS: user identity/HOME, container pacman, makepkg, explicit root, literal arguments, package export and unchanged host')
