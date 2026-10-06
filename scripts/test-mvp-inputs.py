#!/usr/bin/env python3
"""Reject stale locks/corrupt cached identities before creating any release state."""
import json
import os
from pathlib import Path
import subprocess
import tempfile


def main():
    if subprocess.check_output(['systemd-detect-virt'], text=True).strip() not in ('qemu','kvm'):
        raise RuntimeError('VM tests only')
    project=Path(__file__).resolve().parents[1]
    environment=os.environ.copy(); environment['LOOOM_BACKEND_ROOT']=str(project)
    reference=project/'configs/mvp/b'
    with tempfile.TemporaryDirectory(prefix='input-fault-',dir='/var/lib/looom/dev') as tmp:
        root=Path(tmp); cfg=root/'base.yaml'; lock=root/'base.lock'
        original=(reference/'base.yaml').read_text()
        valid=json.loads((reference/'base.lock').read_text())
        cfg.write_text(original.replace('2026-10-03','2026-10-04'))
        lock.write_text(json.dumps(valid))
        result=subprocess.run(['looom','plan',str(cfg)],env=environment,capture_output=True,text=True)
        assert result.returncode!=0 and 'does not match' in result.stderr
        print('PASS: stale lock rejected without resolving new versions')
        cfg.write_text(original)
        valid['packages'][0]['sha256']='0'*64
        lock.write_text(json.dumps(valid))
        rid='mvp-invalid-input'
        assert not (Path('/var/lib/looom/inputs')/rid).exists()
        result=subprocess.run(['looom','build',str(cfg),rid],env=environment,capture_output=True,text=True)
        assert result.returncode!=0 and 'hash mismatch' in result.stderr
        assert not (Path('/var/lib/looom/inputs')/rid).exists()
        assert not (Path('/var/lib/looom/releases')/(rid+'.json')).exists()
        print('PASS: corrupt archive identity rejected before creating build/metadata')


if __name__=='__main__':
    main()
