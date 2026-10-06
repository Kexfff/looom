#!/usr/bin/env python3
"""Real FAT ENOSPC and publication recovery, isolated from the actual VM ESP."""
import errno
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def run(*args):
    subprocess.run(args, check=True)


def main():
    if os.geteuid()!=0 or subprocess.check_output(['systemd-detect-virt'],text=True).strip() not in ('qemu','kvm'):
        raise RuntimeError('VM only')
    with tempfile.TemporaryDirectory(prefix='fat-fault-', dir='/var/lib/looom/dev') as tmp:
        root=Path(tmp)
        image=root/'esp.img'
        with image.open('wb') as f:
            f.truncate(128*1024*1024)
        run('mkfs.fat','-F','32',str(image))
        esp=root/'esp'; esp.mkdir()
        run('mount','-o','loop,umask=0077',str(image),str(esp))
        try:
            spec=importlib.util.spec_from_file_location('release',Path(__file__).with_name('looom-release.py'))
            module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
            module.STATE=root/'state'; module.STATE.mkdir()
            module.RELEASES=module.STATE/'releases'; module.RELEASES.mkdir()
            (module.STATE/'operations').mkdir()
            module.GRUB=esp/'looom/grub'; module.GRUB.mkdir(parents=True)
            module.EFI=esp/'EFI/Linux'; module.EFI.mkdir(parents=True)
            metadata=json.loads(Path('/var/lib/looom/releases/v2.json').read_text())
            metadata['phase']='validated'
            module.atomic_write(module.RELEASES/'v2.json',json.dumps(metadata))
            module.atomic_write(module.STATE/'operations/v2.json',json.dumps({'id':'v2','phase':'validated'}))
            run('grub-editenv',str(module.GRUB/'grubenv'),'create')
            run('grub-editenv',str(module.GRUB/'grubenv'),'set','saved_entry=looom-bootstrap')
            original='set timeout=5\nmenuentry "recovery" { true; }\n'
            module.atomic_write(module.GRUB/'grub.cfg',original)
            env=module.environment()
            def operation(op):
                previous=sys.argv
                try:
                    sys.argv=['looom-release',op]+([] if op=='recover' else ['v2'])
                    module.main()
                finally:
                    sys.argv=previous
            # Leave 1 MiB free: UKI copy must fail part-way through on actual FAT.
            size=os.statvfs(esp)
            with (esp/'filler').open('wb') as f:
                left=size.f_bavail*size.f_frsize-1024*1024
                chunk=b'\0'*(1024*1024)
                while left>0:
                    amount=min(left,len(chunk)); f.write(chunk[:amount]); left-=amount
                f.flush(); os.fsync(f.fileno())
            try:
                operation('publish')
                raise AssertionError('Expected actual FAT ENOSPC')
            except OSError as e:
                assert e.errno==errno.ENOSPC,e
            assert (module.GRUB/'grub.cfg').read_text()==original
            assert module.environment()==env
            assert not (module.EFI/'looom-v2.efi').exists()
            assert json.loads((module.RELEASES/'v2.json').read_text())['phase']=='validated'
            print('PASS: FAT ENOSPC preserves boot menu, saved choice and unpublished metadata')
            (esp/'filler').unlink()
            operation('recover')
            assert '--id looom-v2 {' not in (module.GRUB/'grub.cfg').read_text()
            os.environ['LOOOM_FAIL_AFTER']='uki'
            try:
                operation('publish')
                raise AssertionError('Expected publication interruption')
            except RuntimeError as e:
                assert 'Injected interruption' in str(e)
            finally:
                del os.environ['LOOOM_FAIL_AFTER']
            operation('recover')
            assert '--id looom-v2 {' not in (module.GRUB/'grub.cfg').read_text()
            assert module.environment()==env
            print('PASS: recovery excludes durable but unpublished UKI')
            operation('publish')
            module.validate(metadata,published=True)
            assert json.loads((module.RELEASES/'v2.json').read_text())['phase']=='published'
            assert module.environment()==env
            print('PASS: publication resumes after both faults without changing saved choice')
        finally:
            run('umount',str(esp))


if __name__=='__main__':
    main()
