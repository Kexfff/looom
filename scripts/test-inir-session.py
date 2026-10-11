#!/usr/bin/env python3
"""Functional iNiR acceptance; run as the user inside the dedicated desktop VM."""
import json
import os
from pathlib import Path
import subprocess
import time

assert os.geteuid() == 1000 and Path.home() == Path('/home/kexfff')
assert Path('/sys/class/net/enp1s0/address').read_text().strip() == '52:54:00:10:10:11'
assert Path('/etc/looom/release-id').read_text().strip().startswith('inir-')
work = Path.home() / 'looom/inir-20261011'
public = work / 'public'
public.mkdir(exist_ok=True)
shell = Path.home() / '.config/quickshell/inir'
env = dict(os.environ, XDG_RUNTIME_DIR='/run/user/1000',
           DBUS_SESSION_BUS_ADDRESS='unix:path=/run/user/1000/bus')
for line in subprocess.check_output(['systemctl', '--user', 'show-environment'], text=True).splitlines():
    key, _, value = line.partition('=')
    if key in {'WAYLAND_DISPLAY', 'DISPLAY', 'NIRI_SOCKET', 'XDG_CURRENT_DESKTOP', 'XDG_SESSION_TYPE'}:
        env[key] = value

def run(*args, **kwargs):
    result = subprocess.run(args, env=env, text=True, capture_output=True, timeout=30, **kwargs)
    assert result.returncode == 0, (args, result.stdout, result.stderr)
    return result.stdout

def ipc(target, method):
    return run('qs', '-p', str(shell), 'ipc', 'call', target, method)

def screenshot(name):
    time.sleep(2)
    run('grim', str(public / name))

outputs = json.loads(run('niri', 'msg', '--json', 'outputs'))
assert outputs and any(output.get('current_mode') is not None for output in outputs.values()), outputs
(public / 'outputs.json').write_text(json.dumps(outputs, indent=2) + '\n')
for _ in range(60):
    if run('systemctl', '--user', 'is-active', 'niri.service', 'inir.service').splitlines() == ['active', 'active']:
        break
    time.sleep(.2)
else:
    raise AssertionError('Niri/iNiR did not finish starting')
seat_sessions = run('loginctl', 'list-sessions', '--no-legend')
assert any('kexfff' in line and 'seat0' in line and 'tty' in line for line in seat_sessions.splitlines())
for relative in ['services/Updates.qml', 'services/deferred/PackageSearch.qml']:
    run('/usr/lib/qt6/bin/qmlformat', str(shell / relative))
# Execute the actual prefix/suffix used by the package menu against a harmless sentinel.
line = next(line for line in (shell / 'services/deferred/PackageSearch.qml').read_text().splitlines()
            if 'const looomGuard =' in line)
prefix, suffix = line.split('=', 1)[1].strip().split(' + script + ')
sentinel = work / 'unexpected-host-package-action'
assert not sentinel.exists()
guard_output = run('bash', '-c', json.loads(prefix) + 'touch "$1"' + json.loads(suffix), 'bash', str(sentinel))
assert not sentinel.exists() and 'managed by looom' in guard_output
(public / 'package-guard.log').write_text(guard_output)
(public / 'ipc.txt').write_text(run('qs', '-p', str(shell), 'ipc', 'show'))
screenshot('desktop.png')
ipc('overview', 'open')
screenshot('overview.png')
ipc('overview', 'close')
ipc('settings', 'openOverlay')
screenshot('settings.png')
ipc('settings', 'toggle')
# wl-copy forks its clipboard owner; captured pipes would remain open in that child.
subprocess.run(['wl-copy', 'looom iNiR clipboard acceptance'], env=env,
               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30, check=True)
assert run('wl-paste', '--no-newline') == 'looom iNiR clipboard acceptance'
for _ in range(30):
    if 'looom iNiR clipboard acceptance' in run('cliphist', 'list'):
        break
    time.sleep(.2)
else:
    raise AssertionError('Clipboard watcher did not save the test entry')
run('notify-send', '-a', 'looom acceptance', 'iNiR acceptance', 'Notifications work inside Niri')
screenshot('notification.png')
run('niri', 'msg', 'action', 'spawn', '--', str(Path.home() / '.local/bin/inir'), 'terminal')
for _ in range(50):
    windows = json.loads(run('niri', 'msg', '--json', 'windows'))
    kitty = next((window for window in windows if window.get('app_id') == 'kitty'), None)
    if kitty:
        break
    time.sleep(.2)
else:
    raise AssertionError('Native terminal did not launch')
(public / 'windows.json').write_text(json.dumps(windows, indent=2) + '\n')
screenshot('terminal.png')
run('niri', 'msg', 'action', 'close-window', '--id', str(kitty['id']))
before = run('systemctl', '--user', 'show', 'inir', '-p', 'MainPID', '--value').strip()
run('systemctl', '--user', 'restart', 'inir')
after = run('systemctl', '--user', 'show', 'inir', '-p', 'MainPID', '--value').strip()
assert before != after and after != '0'
ipc('overview', 'open')
ipc('overview', 'close')
assert run('systemctl', '--user', 'is-active', 'inir').strip() == 'active'
result = {
    'release': Path('/etc/looom/release-id').read_text().strip(),
    'source_revision': run('git', '-C', str(work / 'source'), 'rev-parse', 'HEAD').strip(),
    'outputs': list(outputs),
    'checks': ['SDDM seat session', 'native Niri output', 'QML package integration syntax',
               'host package action guard', 'iNiR IPC', 'overview', 'settings',
               'clipboard watcher', 'notification', 'native terminal', 'shell restart'],
    'shell_restart_pid_changed': before != after,
}
(public / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
print('PASS: actual Niri session, iNiR panels, settings, clipboard, terminal and restart')
