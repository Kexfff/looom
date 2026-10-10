#!/usr/bin/env python3
"""Run as the session program of virtual KWin inside the apps VM."""
import os
from pathlib import Path
import subprocess
import time

assert os.geteuid() == 1000
assert Path('/sys/class/net/enp1s0/address').read_text().strip() == '52:54:00:10:10:11'
assert os.environ.get('DISPLAY') and os.environ.get('WAYLAND_DISPLAY')
work = Path.home()/'looom/acceptance'
children = []
try:
    exported = next(p for p in (Path.home()/'.local/share/applications').glob('*xterm.desktop') if p.stem.endswith('-xterm'))
    with (work/'gui-xterm.log').open('w') as terminal, (work/'gui-flatpak.log').open('w') as flatpak:
        children.append(subprocess.Popen(['gio', 'launch', str(exported)], stdout=terminal, stderr=subprocess.STDOUT))
        children.append(subprocess.Popen(['flatpak', 'run', '--socket=x11', '--nosocket=wayland', '--env=GDK_BACKEND=x11', 'org.gnome.Calculator'], stdout=flatpak, stderr=subprocess.STDOUT))
        for attempt in range(30):
            result = subprocess.run(['looom', 'apps', 'exec', '--', 'xwininfo', '-root', '-tree'], text=True, capture_output=True)
            if result.returncode == 0 and '("xterm" "XTerm")' in result.stdout and 'Calculator' in result.stdout:
                assert children[0].poll() in (None, 0) and children[1].poll() is None
                print(result.stdout, flush=True)
                print('PASS: actual Flatpak Calculator and exported Arch xterm windows coexist under virtual KWin/Xwayland', flush=True)
                (work/'gui-pass').write_text('PASS\n')
                break
            time.sleep(0.5)
        else:
            print(result.stdout, result.stderr, flush=True)
            for name in ('gui-flatpak.log','gui-xterm.log'):
                print((work/name).read_text(), flush=True)
            raise RuntimeError('Both graphical application windows did not appear')
finally:
    for child in children:
        if child.poll() is None:
            child.terminate()
    for child in children:
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()
