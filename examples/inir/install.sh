#!/usr/bin/env bash
# Pinned iNiR integration for looom. Run as the desktop user, never with sudo.
set -Eeuo pipefail
here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
revision=$(cat "$here/upstream-revision.txt")
work=${LOOOM_INIR_DIR:-"$HOME/looom/inir"}
source_dir="$work/source"
mode=${1:-help}
[[ $EUID != 0 ]] || { echo 'Run this script as your desktop user, without sudo.' >&2; exit 1; }
[[ -f /etc/looom/release-id ]] || { echo 'This integration requires an installed looom system.' >&2; exit 1; }

prepare_source() {
    mkdir -p "$work"
    if [[ ! -d "$source_dir" ]]; then
        git clone https://github.com/snowarch/iNiR.git "$source_dir"
        git -C "$source_dir" checkout --detach "$revision"
    fi
    [[ $(git -C "$source_dir" rev-parse HEAD) == "$revision" ]] || {
        echo "Existing source must be at $revision; use a separate LOOOM_INIR_DIR." >&2; exit 1;
    }
    if git -C "$source_dir" apply --check "$here/looom-readonly.patch" 2>/dev/null; then
        git -C "$source_dir" apply "$here/looom-readonly.patch"
    else
        git -C "$source_dir" apply --reverse --check "$here/looom-readonly.patch"
    fi
    # Upstream ranges otherwise change the user runtime independently of its revision.
    cp -- "$here/python-requirements.lock" "$source_dir/sdata/uv/requirements.txt"
}

case "$mode" in
    prepare)
        base=${2:-"$HOME/looom/base.yaml"}
        greeter=${3:-}
        [[ -z $greeter || $greeter == --software-greeter ]] || { echo 'Unknown prepare option' >&2; exit 1; }
        prepare_source
        # A dedicated directory also keeps its base.lock separate from the original.
        python3 - "$base" "$here/packages.yaml" "$work/declaration" "$greeter" <<'PY'
import json
from pathlib import Path
import sys
import shutil
base, packages, directory = map(Path, sys.argv[1:4])
try:
    config = json.loads(base.read_text())
except json.JSONDecodeError:
    raise SystemExit('Use JSON-formatted YAML (as produced by looom install), or add desktop.sessions: [niri] and packages_from: [inir-packages.yaml] manually.')
output = directory / 'base.yaml'
package_output = directory / 'inir-packages.yaml'
if directory.exists():
    raise SystemExit(f'{directory} already exists; review it instead of overwriting.')
includes = config.get('packages_from', [])
sources = []
file_sources = [spec['source'] for spec in config.get('files', {}).values() if spec.get('source') is not None]
for name in includes + file_sources:
    rel = Path(name)
    source = base.parent / rel
    if rel.is_absolute() or '..' in rel.parts or not source.is_file() or any(p.is_symlink() for p in [source, *source.parents]) or name == 'inir-packages.yaml':
        raise SystemExit(f'Unsafe or conflicting package include: {name}')
    sources.append((source, directory / rel))
directory.mkdir(mode=0o700, parents=True)
for source, target in sources:
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, target)
desktop = config.setdefault('desktop', {'environment': 'plasma'})
desktop['sessions'] = sorted(set(desktop.get('sessions', [])) | {'niri'})
units = config.setdefault('units', {})
units.setdefault('bluetooth.service', 'enabled')
units.setdefault('power-profiles-daemon.service', 'enabled')
config['packages_from'] = list(dict.fromkeys(includes + ['inir-packages.yaml']))
if sys.argv[4] == '--software-greeter':
    files = config.setdefault('files', {})
    key = '/etc/sddm.conf.d/90-inir-software-greeter.conf'
    if key in files:
        raise SystemExit(f'{key} already declared; review it manually.')
    files[key] = {'content': '[General]\nGreeterEnvironment=QT_QUICK_BACKEND=software\n', 'mode': '0644'}
package_output.write_text(packages.read_text())
output.write_text(json.dumps(config, indent=2) + '\n')
print(f'Prepared {output}; rebuild it into a new release and reboot before install.')
PY
        ;;
    install)
        prepare_source
        mapfile -t packages < <(python3 -c 'import json,sys; print("\n".join(json.load(open(sys.argv[1]))["packages"]))' "$here/packages.yaml")
        if ! pacman -T "${packages[@]}"; then
            echo 'Missing host packages. Rebuild and boot the prepared system declaration first.' >&2; exit 1
        fi
        backup="$work/backups/$(date -u +%Y%m%dT%H%M%S)-$$"
        mkdir -p -m 700 "$backup"
        # Contains private user files: keep locally, never commit/publish this backup.
        for rel in .config .bashrc .bash_profile .profile .zshrc .local/bin/inir .local/share/konsole .local/state/dolphinstaterc; do
            if [[ -e "$HOME/$rel" ]]; then
                mkdir -p "$backup/$(dirname "$rel")"
                cp -a --reflink=auto -- "$HOME/$rel" "$backup/$rel"
            fi
        done
        export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-"/run/user/$UID"}
        export DBUS_SESSION_BUS_ADDRESS=${DBUS_SESSION_BUS_ADDRESS:-"unix:path=$XDG_RUNTIME_DIR/bus"}
        export INIR_LOOOM_SKIP_SYSTEM_THEME=1
        venv="${XDG_STATE_HOME:-$HOME/.local/state}/quickshell/.venv"
        if [[ -d "$venv" ]]; then
            [[ $("$venv/bin/python" --version) == 'Python 3.12.15' ]] || {
                echo "Existing $venv uses another Python; back it up and move it before installation." >&2; exit 1;
            }
        else
            uv venv --prompt inir "$venv" -p 3.12.15
        fi
        cd "$source_dir"
        ./setup install -y --skip-deps --skip-setups
        python3 - "$here/config-overrides.json" <<'PY'
import json
from pathlib import Path
import sys
path = Path.home() / '.config/inir/config.json'
config = json.loads(path.read_text())
def merge(target, overrides):
    for key, value in overrides.items():
        if isinstance(value, dict):
            merge(target.setdefault(key, {}), value)
        else:
            target[key] = value
merge(config, json.loads(Path(sys.argv[1]).read_text()))
path.write_text(json.dumps(config, indent=2) + '\n')
# The tested profile uses official Arch styles rather than the optional AUR Darkly plugin.
niri_env = Path.home() / '.config/niri/config.d/40-environment.kdl'
if niri_env.is_file() and not Path('/usr/lib/qt6/plugins/styles/darkly6.so').exists():
    niri_env.write_text(niri_env.read_text().replace('QT_STYLE_OVERRIDE "Darkly"', 'QT_STYLE_OVERRIDE "Breeze"'))
PY
        "$HOME/.local/bin/inir" service enable
        niri validate
        echo "Installed pinned iNiR. Config: $HOME/.config/inir/config.json"
        echo "Local backup: $backup. Select Niri in SDDM on your next login."
        ;;
    *)
        echo "Usage: bash $0 prepare [base.yaml] [--software-greeter] | install"
        [[ $mode == help ]] || exit 1
        ;;
esac
