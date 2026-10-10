#!/usr/bin/env python3
"""Session program for virtual KWin, run only in the desktop acceptance VM."""
import json
import os
from pathlib import Path
import subprocess
import time

assert os.geteuid() == 1000 and Path.home() == Path('/home/kexfff')
assert Path('/sys/class/net/enp1s0/address').read_text().strip() == '52:54:00:10:10:11'
assert os.environ.get('WAYLAND_DISPLAY')
assert Path('/etc/looom/release-id').read_text().strip() == 'compat-niri'
work = Path.home()/'looom/compat-acceptance'
runtime = Path(os.environ['XDG_RUNTIME_DIR'])
shell = work/'quickshell'
shell.mkdir(exist_ok=True)
(shell/'shell.qml').write_text('''import QtQuick
import Quickshell
import org.kde.kirigami as Kirigami

ShellRoot {
    FloatingWindow {
        visible: true
        title: "looom Niri Quickshell acceptance"
        implicitWidth: 480
        implicitHeight: 160
        color: "#162331"
        Kirigami.Heading {
            anchors.centerIn: parent
            text: "Niri + Quickshell"
            color: "white"
        }
    }
}
''')
config = work/'niri-test.kdl'
config.write_text('''input {
    keyboard {
        xkb {
            layout "us"
        }
    }
}
prefer-no-csd
''')
niri = None
with (work/'niri-session.log').open('w') as log:
    try:
        # A nested compositor uses the virtual parent, without needing a physical
        # GPU/seat or changing the logged-in user's desktop configuration.
        niri = subprocess.Popen(['niri', '-c', str(config)], stdout=log, stderr=log)
        socket = None
        for _ in range(100):
            assert niri.poll() is None, (work/'niri-session.log').read_text()
            matches = list(runtime.glob(f'niri.*.{niri.pid}.sock'))
            if matches:
                socket = matches[0]
                break
            time.sleep(0.1)
        assert socket, (work/'niri-session.log').read_text()
        env = dict(os.environ, NIRI_SOCKET=str(socket))
        result = subprocess.run(['niri', 'msg', 'action', 'spawn', '--', 'qs', '-p', str(shell)],
                                env=env, text=True, capture_output=True)
        assert result.returncode == 0, result.stderr
        for _ in range(100):
            result = subprocess.run(['niri', 'msg', '--json', 'windows'], env=env,
                                    text=True, capture_output=True)
            if result.returncode == 0:
                windows = json.loads(result.stdout)
                if any(w.get('title') == 'looom Niri Quickshell acceptance' for w in windows):
                    print(result.stdout, flush=True)
                    (work/'niri-window.json').write_text(result.stdout)
                    print('PASS: native Quickshell window with Kirigami under nested native Niri', flush=True)
                    break
            time.sleep(0.1)
        else:
            raise RuntimeError('Quickshell window missing: '+(work/'niri-session.log').read_text())
        subprocess.run(['niri', 'msg', 'action', 'close-window'], env=env, check=True)
    finally:
        if niri and niri.poll() is None:
            niri.terminate()
            try:
                niri.wait(timeout=8)
            except subprocess.TimeoutExpired:
                niri.kill()
                niri.wait()
