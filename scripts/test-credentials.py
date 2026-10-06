#!/usr/bin/env python3
"""Root-only VM tests; real credentials are never used as fixtures."""
import concurrent.futures
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SOURCE = Path(__file__).with_name('looom-accounts.py')


def load(root):
    spec = importlib.util.spec_from_file_location('accounts_under_test', SOURCE)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.CREDENTIALS = root / 'credentials'
    module.RUNTIME = root / 'runtime'
    module.TEMPLATES = root / 'templates'
    return module


class Credentials(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='credentials-', dir='/var/lib/looom/dev')
        self.root = Path(self.temp.name)
        for name in ('credentials', 'runtime', 'templates'):
            (self.root / name).mkdir(mode=0o700)
        self.m = load(self.root)
        self.runtime_temp = tempfile.TemporaryDirectory(prefix='looom-credentials-test-', dir='/run')
        self.m.RUNTIME = Path(self.runtime_temp.name)
        self.m.RUNTIME.chmod(0o755)
        (self.m.TEMPLATES / 'shadow').write_text('root:!:20000:0:99999:7:::\ncodex:!:20000:0:99999:7:::\n')
        (self.m.TEMPLATES / 'gshadow').write_text('root:!::\ncodex:!::\n')
        for user in self.m.MANAGED:
            self.m.atomic_write(self.m.CREDENTIALS / (user + '.hash'), self.m.hash_password('test-only-in-memory') + '\n')
        self.m.generate()

    def tearDown(self):
        self.runtime_temp.cleanup()
        self.temp.cleanup()

    def test_unprivileged_read(self):
        for path in (self.m.CREDENTIALS / 'codex.hash', self.m.RUNTIME / 'shadow'):
            result = subprocess.run(['runuser', '-u', 'codex', '--', 'test', '-r', str(path)])
            self.assertNotEqual(result.returncode, 0)

    def test_invalid_sources_preserve_runtime(self):
        path = self.m.CREDENTIALS / 'codex.hash'
        before = (self.m.RUNTIME / 'shadow').read_bytes()
        original = path.read_bytes()
        for bad in ('', '$garbage', '$6$invalid', '\n', '!:'):
            path.write_text(bad)
            with self.assertRaises(RuntimeError):
                self.m.generate()
            self.assertEqual(before, (self.m.RUNTIME / 'shadow').read_bytes())
        path.write_bytes(original)
        for mode in (0o644, 0o660):
            path.chmod(mode)
            with self.assertRaises(RuntimeError):
                self.m.generate()
        path.chmod(0o600)
        os.chown(path, 1000, 1000)
        with self.assertRaises(RuntimeError):
            self.m.generate()
        os.chown(path, 0, 0)
        path.unlink()
        path.symlink_to(self.m.CREDENTIALS / 'root.hash')
        with self.assertRaises(OSError):
            self.m.generate()
        path.unlink()
        with self.assertRaises(FileNotFoundError):
            self.m.generate()

    def test_acl_rejected(self):
        path = self.m.CREDENTIALS / 'codex.hash'
        subprocess.run(['/var/lib/looom/dev/root/usr/bin/setfacl', '-m', 'u:1000:r', str(path)], check=True)
        with self.assertRaises(RuntimeError):
            self.m.generate()

    def test_interrupted_change_recovers(self):
        old = (self.m.RUNTIME / 'shadow').read_bytes()
        with self.assertRaises(RuntimeError):
            self.m.set_password('codex', 'new-test-only-password', fail_after_commit=True)
        self.assertEqual(old, (self.m.RUNTIME / 'shadow').read_bytes())
        self.assertEqual('updating', json.loads((self.m.CREDENTIALS / '.transaction.json').read_text())['phase'])
        self.m.generate()
        value = self.m.read_private(self.m.CREDENTIALS / 'codex.hash').strip()
        self.assertEqual(value, (self.m.RUNTIME / 'shadow').read_text().splitlines()[1].split(':')[1])
        self.assertEqual('synchronized', json.loads((self.m.CREDENTIALS / '.transaction.json').read_text())['phase'])

    def test_concurrent_changes(self):
        with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
            list(pool.map(lambda i: self.m.set_password('codex', 'test-only-' + str(i)), range(8)))
        value = self.m.read_private(self.m.CREDENTIALS / 'codex.hash').strip()
        self.assertEqual(value, (self.m.RUNTIME / 'shadow').read_text().splitlines()[1].split(':')[1])


if __name__ == '__main__':
    if os.geteuid() != 0 or subprocess.check_output(['systemd-detect-virt'], text=True).strip() not in ('qemu', 'kvm'):
        raise SystemExit('These tests require the authorized VM as root')
    unittest.main()
