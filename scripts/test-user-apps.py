#!/usr/bin/env python3
"""Run inside the disposable apps acceptance VM as kexfff, never on the host.
Phases: install, verify, negative, parallel, snapshot. Source fixtures remain in ~/looom/acceptance.
"""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time

HOME = Path.home()
WORK = HOME / 'looom/acceptance'
CFG = HOME / 'looom/apps.yaml'
DATA = HOME / '.local/share/looom/apps'
URL = 'https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage'
# Official release 1.9.1 fetched over HTTPS and independently hashed on this VM.
DIGEST = 'ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0'

def command(*args, expected=0):
    print('+', ' '.join(args), flush=True)
    value = subprocess.Popen(args, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    lines = []
    for line in value.stdout:
        lines.append(line)
        print(line, end='', flush=True)
    value.wait()
    if expected == 0:
        assert value.returncode == 0, (args, value.returncode)
    else:
        assert value.returncode != 0, f'Unexpected success: {args}'
    return "".join(lines)


def guard():
    assert os.geteuid() == 1000 and HOME == Path('/home/kexfff')
    assert Path('/sys/class/net/enp1s0/address').read_text().strip() == '52:54:00:10:10:11'
    assert command('systemd-detect-virt', '--vm').strip() in ('qemu', 'kvm')
    assert Path('/etc/looom/release-id').read_text().strip().startswith('apps-')
    assert 'ro' in command('findmnt', '-no', 'OPTIONS', '/').strip().split(',')
    assert Path('/etc/subuid').read_text() == 'kexfff:100000:65536\n'
    assert Path('/etc/subgid').read_text() == 'kexfff:100000:65536\n'
    assert os.environ.get('XDG_RUNTIME_DIR') == '/run/user/1000'
    os.environ['DBUS_SESSION_BUS_ADDRESS'] = 'unix:path=/run/user/1000/bus'
    for exe in ('flatpak', 'distrobox', 'podman', 'fusermount'):
        command('which', exe)


def install():
    WORK.mkdir(parents=True, exist_ok=True)
    command('looom', 'apps', 'init')
    assert CFG.stat().st_uid == 1000
    assert hashlib.sha256((WORK/'appimagetool.AppImage').read_bytes()).hexdigest() == DIGEST
    cfg = {'schema': 1, 'flatpak': ['org.gnome.Calculator'],
           'arch': {'image': 'docker.io/library/archlinux:latest', 'packages': ['tree', 'xterm'], 'exports': ['xterm']},
           'appimages': {'tool': {'url': URL, 'sha256': DIGEST, 'name': 'Looom AppImage acceptance'},
                         'tool-local': {'source': './acceptance/appimagetool.AppImage', 'sha256': DIGEST}}}
    CFG.write_text(json.dumps(cfg, indent=2)+'\n')  # JSON is a valid YAML input.
    (WORK/'original-apps.json').write_text(json.dumps(cfg, indent=2)+'\n')
    command('looom', 'apps', 'check')
    command('looom', 'apps', 'plan')
    command('looom', 'apps', 'apply')
    verify()
    applied = (DATA/'applied.json').read_bytes()
    command('looom', 'apps', 'apply')
    assert (DATA/'applied.json').read_bytes() == applied
    # Installer script: bare pacman, sudo and absolute pacman, root check, cwd/assets,
    # and literal argument forwarding. No wrapper intercepts host pacman.
    (WORK/'support.txt').write_text('support\n')
    (WORK/'installer.sh').write_text('''#!/bin/bash
set -euo pipefail
test "$EUID" = 0
test -e /run/.containerenv
test ! -e /etc/looom/release-id
test "$(cat support.txt)" = support
pacman -S --needed --noconfirm jq
sudo /usr/bin/pacman -S --needed --noconfirm bc
printf '%s\\n' "$2" > "$1"
''')
    literal = 'literal spaces; $(touch /tmp/looom-should-not-exist)'
    command('looom', 'apps', 'run-script', str(WORK/'installer.sh'), str(WORK/'installer-result.txt'), literal)
    assert (WORK/'installer-result.txt').read_text() == literal+'\n'
    command('looom', 'apps', 'exec', '--', 'jq', '--version')
    command('looom', 'apps', 'exec', '--', 'bc', '--version')
    assert not Path('/tmp/looom-should-not-exist').exists()
    # Ensure applying a smaller declaration preserves previous/manual applications.
    CFG.write_text(json.dumps({'schema':1}))
    command('looom', 'apps', 'apply')
    command('flatpak', 'info', '--user', 'org.gnome.Calculator')
    command('looom', 'apps', 'exec', '--', 'jq', '--version')
    command('looom', 'apps', 'launch', 'tool', '--', '--version')
    CFG.write_bytes((WORK/'original-apps.json').read_bytes())
    command('looom', 'apps', 'apply')
    negative()
    snapshot()


def verify():
    value = json.loads(command('looom', 'apps', 'status'))
    assert value['arch_container'] and value['last_apply']
    assert value['flatpak']['org.gnome.Calculator']
    assert value['appimages'] == {'tool':True, 'tool-local':True}
    command('flatpak', 'run', 'org.gnome.Calculator', '--version')
    command('looom', 'apps', 'exec', '--', 'tree', '--version')
    command('looom', 'apps', 'exec', '--', 'xterm', '-version')
    command('looom', 'apps', 'launch', 'tool', '--', '--version')
    command('looom', 'apps', 'launch', 'tool-local', '--', '--version')
    exports = list((HOME/'.local/share/applications').glob('*xterm*.desktop'))
    assert exports and any('distrobox-enter' in p.read_text() for p in exports)
    desktop = HOME/'.local/share/applications/looom-tool.desktop'
    assert 'Exec=/usr/bin/looom apps launch tool -- %U' in desktop.read_text()
    if (WORK/'installer-result.txt').exists():
        command('looom', 'apps', 'exec', '--', 'jq', '--version')
        command('looom', 'apps', 'exec', '--', 'bc', '--version')
        assert (WORK/'installer-result.txt').read_text().startswith('literal spaces;')
    print('PASS: declared applications execute in a read-only system', flush=True)


def negative():
    original = CFG.read_bytes()
    receipt = (DATA/'applied.json').read_bytes()
    active = (DATA/'appimages/tool/active.json').read_bytes()
    cfg = json.loads(original)
    cfg['appimages']['tool']['sha256'] = '0'*64
    cfg['appimages']['tool'].pop('url')
    cfg['appimages']['tool']['source'] = './acceptance/appimagetool.AppImage'
    CFG.write_text(json.dumps(cfg))
    assert 'SHA256 mismatch' in command('looom', 'apps', 'apply', expected=1)
    assert (DATA/'applied.json').read_bytes() == receipt
    assert (DATA/'appimages/tool/active.json').read_bytes() == active
    CFG.write_bytes(original)
    cfg = json.loads(original)
    cfg['arch']['image'] = 'docker.io/library/archlinux:base'
    CFG.write_text(json.dumps(cfg))
    assert 'unmanaged or uses a different image' in command('looom', 'apps', 'apply', expected=1)
    CFG.write_bytes(original)
    # Refuse modified executable content before launch.
    image = DATA / f'appimages/tool/{DIGEST}.AppImage'
    size = image.stat().st_size
    with image.open('ab') as f:
        f.write(b'corrupted')
    try:
        assert 'active AppImage was modified' in command('looom', 'apps', 'launch', 'tool', '--', '--version', expected=1)
    finally:
        with image.open('r+b') as f:
            f.truncate(size)
    # A matching digest cannot turn a shell script into an AppImage.
    bad = WORK/'not-an-appimage'
    bad.write_text('#!/bin/bash\necho unexpected\n')
    cfg = json.loads(original)
    cfg['appimages']['bad'] = {'source':str(bad), 'sha256':hashlib.sha256(bad.read_bytes()).hexdigest()}
    CFG.write_text(json.dumps(cfg))
    try:
        assert 'not a supported AppImage' in command('looom', 'apps', 'apply', expected=1)
        assert not (DATA/'appimages/bad/active.json').exists()
    finally:
        CFG.write_bytes(original)
    command('flatpak', 'remote-modify', '--user', '--url=https://example.org/untrusted/', 'flathub')
    try:
        assert 'different URL' in command('looom', 'apps', 'apply', expected=1)
    finally:
        command('flatpak', 'remote-modify', '--user', '--url=https://dl.flathub.org/repo/', 'flathub')
    command('looom', 'apps', 'run-script', '/etc/hostname', expected=1)
    print('PASS: failed downloads/declarations preserve previous activation', flush=True)


def snapshot():
    paths = [CFG, DATA/'applied.json', DATA/'appimages/tool/active.json',
             DATA/'appimages/tool-local/active.json', WORK/'installer-result.txt',
             HOME/'.local/share/applications/looom-tool.desktop']
    state = {str(p.relative_to(HOME)): hashlib.sha256(p.read_bytes()).hexdigest() for p in paths}
    state['container_id'] = command('podman', 'inspect', '--format', '{{.Id}}', 'looom-arch').strip()
    state['flatpak_commit'] = command('flatpak', 'info', '--user', '--show-commit', 'org.gnome.Calculator').strip()
    state['flatpak_setting_show_thousands'] = command('flatpak', 'run', '--command=gsettings', 'org.gnome.Calculator', 'get', 'org.gnome.calculator', 'show-thousands').strip()
    path = WORK/'before-rollback.json'
    if path.exists():
        assert json.loads(path.read_text()) == state, 'User apps/data changed across system rollback'
    else:
        path.write_text(json.dumps(state, indent=2)+'\n')
    print(json.dumps(state, indent=2), flush=True)
    print('PASS: stable application IDs, content and user state', flush=True)

def parallel():
    # A long-lived container shell must not block launching other applications.
    process = subprocess.Popen(['looom', 'apps', 'exec', '--', 'sleep', '8'])
    try:
        time.sleep(2)
        assert process.poll() is None
        command('looom', 'apps', 'status')
        command('looom', 'apps', 'launch', 'tool', '--', '--version')
        assert process.wait() == 0
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait()
    print('PASS: container session does not hold the application management lock', flush=True)

if __name__ == '__main__':
    guard()
    {'install': install, 'verify': verify, 'negative': negative, 'snapshot': snapshot, 'parallel':parallel}[sys.argv[1]]()
