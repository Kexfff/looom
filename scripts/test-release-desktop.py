#!/usr/bin/env python3
"""Native first-login/apps/lifecycle acceptance on the dedicated 0.2.0 VM."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import pty
import secrets
import subprocess
import sys

STATE = Path('/var/lib/looom/dev/release-0.2.0-test')
HOME = Path('/home/kexfff')
WORK = HOME/'looom/release-test'

def call(*args):
    print('+', ' '.join(args), flush=True)
    subprocess.run(args, check=True)

def user(*args):
    return ['runuser', '-u', 'kexfff', '--', 'env', 'HOME='+str(HOME),
            'XDG_RUNTIME_DIR=/run/user/1000', 'DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus', *args]

def terminal():
    spec = importlib.util.spec_from_file_location('wizard', Path(__file__).with_name('test-installer-limine.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    class Setup(module.Terminal):
        def __init__(self):
            self.pid, self.fd = pty.fork()
            if self.pid == 0:
                os.execv('/usr/sbin/runuser', user('/usr/bin/looom', 'setup'))
            self.output, self.cursor = b'', 0
    return Setup()

def setup():
    # Status and no-TTY calls cannot apply apps or confirm a system.
    marker = HOME/'.local/state/looom/welcome-seen'
    state = Path('/var/lib/looom/releases/initial.json').read_bytes()
    before = marker.read_bytes() if marker.exists() else None
    call(*user('looom', 'setup', '--status'))
    call(*user('looom', 'setup'))
    assert Path('/var/lib/looom/releases/initial.json').read_bytes() == state
    assert (marker.read_bytes() if marker.exists() else None) == before
    menu = terminal()
    menu.expect(b'Choice.*: ', b'2')
    secret = json.loads((STATE/'private/passwords.json').read_text())['user'].encode()
    menu.expect(rb'\[sudo\] password for kexfff: ', secret, True)
    menu.expect(b'Choice.*: ', b'0')
    output = menu.finish(True, b'Confirmed healthy running release', secret)
    (STATE/'setup-confirm.log').write_bytes(output)
    assert json.loads(Path('/var/lib/looom/releases/initial.json').read_text())['phase'] == 'confirmed'
    menu = terminal()
    menu.expect(b'Choice.*: ', b'1')
    menu.expect(rb'Proceed\? Type YES: ', b'YES')
    menu.expect(b'Choice.*: ', b'0', timeout=1800)
    output = menu.finish(True, b'org.mozilla.firefox')
    assert b'Operation failed' not in output
    (STATE/'setup-apps.log').write_bytes(output)
    call(*user('flatpak', 'info', '--user', 'org.mozilla.firefox'))
    print('PASS: status/no-TTY do not mutate or confirm; explicit menu confirmation authenticates with sudo; Firefox installs as user', flush=True)

def apps():
    WORK.mkdir(exist_ok=True)
    subprocess.run(['chown', 'kexfff:kexfff', str(WORK)], check=True)
    image = WORK/'appimagetool.AppImage'
    call(*user('curl', '--fail', '--location', '--retry', '3', '-o', str(image),
         'https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage'))
    digest = 'ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0'
    assert hashlib.sha256(image.read_bytes()).hexdigest() == digest
    cfg = HOME/'looom/apps.yaml'
    cfg.write_text(json.dumps({'schema': 1, 'flatpak': ['org.mozilla.firefox'],
        'arch': {'image': 'docker.io/library/archlinux:latest', 'packages': ['tree'], 'exports': []},
        'appimages': {'tool': {'source': './release-test/appimagetool.AppImage', 'sha256': digest}}}, indent=2)+'\n')
    call(*user('looom', 'apps', 'apply'))
    call(*user('looom', 'apps', 'launch', 'tool', '--', '--version'))
    script = WORK/'installer.sh'
    script.write_text('''#!/bin/bash
set -euo pipefail
test "$EUID" = 0
test -e /run/.containerenv
test ! -e /etc/looom/release-id
pacman -S --needed --noconfirm jq
sudo /usr/bin/pacman -S --needed --noconfirm bc
printf '%s\\n' "$2" > "$1"
''')
    call('chown', 'kexfff:kexfff', str(script))
    literal = 'literal spaces; $(touch /tmp/looom-should-not-exist)'
    call(*user('looom', 'apps', 'run-script', '--root', str(script), str(WORK/'installer-result.txt'), literal))
    assert (WORK/'installer-result.txt').read_text() == literal+'\n'
    assert not Path('/tmp/looom-should-not-exist').exists()
    call(*user('looom', 'apps', 'exec', '--', 'tree', '--version'))
    call(*user('looom', 'apps', 'exec', '--', 'jq', '--version'))
    call(*user('looom', 'apps', 'exec', '--', 'bc', '--version'))
    assert 'ro' in subprocess.check_output(['findmnt', '-nro', 'OPTIONS', '/'], text=True).split(',')
    assert not Path('/usr/bin/tree').exists() and not Path('/usr/bin/bc').exists()
    print('PASS: declared AppImage, rootless Arch container and real pacman installer script; arguments remain literal; host stays read-only', flush=True)

def persistent_state():
    assert Path('/run/user/1000/bus').exists(), 'log in as the desktop user before checking rootless applications'
    paths = [HOME/'looom/apps.yaml', WORK/'installer-result.txt', HOME/'.local/state/looom/welcome-seen',
             HOME/'.local/share/applications/looom-tool.desktop', HOME/'.local/share/looom/apps/appimages/tool/active.json',
             HOME/'.var/app/org.mozilla.firefox/release-proof.txt', Path('/etc/looom-local/release-proof.txt')]
    result = {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in paths}
    result['container'] = subprocess.check_output(user('podman', 'inspect', '--format', '{{.Id}}', 'looom-arch'), text=True).strip()
    result['firefox'] = subprocess.check_output(user('flatpak', 'info', '--user', '--show-commit', 'org.mozilla.firefox'), text=True).strip()
    # Credentials are compared locally only, never included in public evidence.
    result['credentials'] = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in Path('/var/lib/looom/credentials').rglob('*') if p.is_file()}
    return result

def snapshot():
    proof = HOME/'.var/app/org.mozilla.firefox/release-proof.txt'
    proof.write_text('user application state survives system rollback\n')
    call('chown', 'kexfff:kexfff', str(proof))
    Path('/etc/looom-local/release-proof.txt').write_text('explicit local settings survive rollback\n')
    baseline = STATE/'private/before-rollback.json'
    if baseline.exists():
        assert json.loads(baseline.read_text()) == persistent_state(), 'original user state changed'
    else:
        baseline.write_text(json.dumps(persistent_state()))
        baseline.chmod(0o600)
    cfg = WORK/'base.yaml'
    # The wizard exports its YAML-compatible declaration as formatted JSON.
    declaration = json.loads((HOME/'looom/base.yaml').read_text())
    declaration['files']['/etc/looom-release-proof.conf'] = {'source': 'proof.conf', 'mode': '0644'}
    cfg.write_text(json.dumps(declaration, indent=2)+'\n')
    (WORK/'proof.conf').write_text('declarative file exists only in desktop-changed\n')
    call('chown', '-R', 'kexfff:kexfff', str(WORK))
    call('looom', 'check', str(cfg))
    print('PASS: same-VM private state snapshot and update declaration prepared', flush=True)

def verify():
    assert persistent_state() == json.loads((STATE/'private/before-rollback.json').read_text())
    running = Path('/etc/looom/release-id').read_text().strip()
    if running == 'desktop-changed':
        assert Path('/etc/looom-release-proof.conf').read_text() == 'declarative file exists only in desktop-changed\n'
    else:
        assert running in ('initial', 'desktop-next') and not Path('/etc/looom-release-proof.conf').exists()
    call('looom', 'verify')
    call('looom', 'accounts', 'check')
    call(*user('looom', 'apps', 'launch', 'tool', '--', '--version'))
    call(*user('looom', 'apps', 'exec', '--', 'jq', '--version'))
    print('PASS: '+running+' keeps passwords, container, Flatpak commit, AppImage, home and local settings; declarative file matches release', flush=True)

def password():
    fixture = STATE/'private/passwords.json'
    passwords = json.loads(fixture.read_text())
    (STATE/'private/passwords.before-change.json').write_bytes(fixture.read_bytes())
    fresh = secrets.token_hex(16)
    menu = terminal()
    menu.expect(b'Choice.*: ', b'6')
    menu.expect(rb'\[sudo\] password for kexfff: ', passwords['user'].encode(), True)
    menu.expect(b'New password: ', fresh.encode(), True)
    menu.expect(b'Repeat password: ', fresh.encode(), True)
    menu.expect(b'Choice.*: ', b'0')
    output = menu.finish(True, b'Persistent password and runtime credentials synchronized', fresh.encode())
    assert passwords['user'].encode() not in output
    passwords['user'] = fresh
    fixture.write_text(json.dumps(passwords))
    fixture.chmod(0o600)
    (STATE/'setup-password.log').write_bytes(output)
    call('looom', 'accounts', 'check')
    print('PASS: setup changes the managed user password with hidden sudo/new/repeat prompts; fixture retained only on this VM', flush=True)

def finish():
    current = persistent_state()
    before = json.loads((STATE/'private/before-rollback.json').read_text())
    assert current.pop('credentials') != before.pop('credentials'), 'password change was not persistent'
    assert current == before, 'noncredential user state changed'
    assert Path('/etc/looom/release-id').read_text().strip() == 'desktop-changed'
    assert subprocess.check_output(['pgrep', '-u', '1000', 'plasmashell'], text=True).strip()
    marker = HOME/'.local/state/looom/welcome-seen'
    assert marker.stat().st_uid == 1000 and marker.stat().st_mode & 0o777 == 0o600
    call(*user('looom', 'setup', '--first-login'))
    call('looom', 'verify')
    call('looom', 'accounts', 'check')
    # is-active with several units succeeds if ANY is active; assert every line.
    assert subprocess.check_output(['systemctl', 'is-active', 'sddm', 'power-profiles-daemon'], text=True).splitlines() == ['active']*2
    call('systemctl', 'is-enabled', 'bluetooth')  # No adapter in this VM: hardware activation is not asserted.
    states = subprocess.check_output(user('systemctl', '--user', 'is-active', 'pipewire', 'pipewire-pulse', 'wireplumber',
         'plasma-powerdevil', 'xdg-desktop-portal', 'plasma-xdg-desktop-portal-kde'), text=True)
    assert states.splitlines() == ['active']*6
    print('PASS: every required desktop/audio/portal unit active; Bluetooth enabled (no virtual adapter)', flush=True)
    menu = terminal()
    menu.expect(b'Choice.*: ', b'5')
    menu.expect(rb'Proceed\? Type YES: ', b'YES')
    menu.expect(b'Choice.*: ', b'0', timeout=1800)
    output = menu.finish(True, b'Applications applied')
    (STATE/'setup-update.log').write_bytes(output)
    print('PASS: confirmed read-only KDE, new password login after changing root, persistent apps/data, one-time welcome and explicit application update', flush=True)

if __name__ == '__main__':
    assert os.geteuid() == 0
    assert Path('/sys/class/net/enp1s0/address').read_text().strip() == '52:54:00:20:02:00'
    os.umask(0o077)
    os.chdir(HOME)
    {'setup': setup, 'apps': apps, 'snapshot': snapshot, 'verify': verify,
     'password': password, 'finish': finish}[sys.argv[1]]()
