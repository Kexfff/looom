#!/usr/bin/env python3
"""Audit selected public evidence on its source VM before exporting it."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys

assert os.geteuid() == 0
assert subprocess.check_output(['systemd-detect-virt', '--vm'], text=True).strip() in ('qemu', 'kvm')
assert Path('/sys/class/net/enp1s0/address').read_text().strip() in ('52:54:00:7b:23:63', '52:54:00:20:02:00')
folder = Path(sys.argv[1]).resolve(strict=True)
assert folder.is_dir() and folder != Path('/')
needles = [p.read_bytes().strip() for p in Path('/var/lib/looom/credentials').rglob('*')
           if p.is_file() and len(p.read_bytes().strip()) > 20]
for fixture in Path('/var/lib/looom/dev/release-0.2.0-test/private').glob('passwords*.json'):
    needles.extend(value.encode() for value in json.loads(fixture.read_text()).values())
count = 0
for file in folder.rglob('*'):
    assert not file.is_symlink(), 'public evidence contains a symlink'
    if file.is_file():
        data = file.read_bytes()
        assert not re.search(rb'\x24(?:y|6|5|2[aby]|argon2id)\x24', data), 'credential hash marker in '+file.name
        assert not any(b'BEGIN '+kind+b'PRIVATE KEY' in data for kind in (b'', b'OPENSSH ', b'RSA ', b'EC ')), file.name
        assert all(secret not in data for secret in needles), 'local credential in '+file.name
        count += 1
print(f'PASS: {count} selected public files audited on their source VM against local credentials and login fixture; no private data exported')
