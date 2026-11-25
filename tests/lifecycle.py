"""Run after cargo build --release: python3 tests/lifecycle.py (Linux/macOS)."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

MSET = str(Path(os.environ.get('MSET_BINARY', str(Path(__file__).resolve().parents[1] / 'target/release/mset'))).resolve())


@unittest.skipUnless(os.name == 'posix', 'Unix lifecycle support')
class Lifecycle(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='mset-lifecycle-')
        self.root = Path(self.temp.name)
        (self.root / 'config.yaml').write_text('mode: rule\n')
        self.mock('echo mock-started\nexec sleep 120')

    def mock(self, body):
        binary = self.root / 'mihomo'
        binary.write_text('#!/bin/sh\nif [ "$1" = "-t" ]; then exit 0; fi\n' + body + '\n')
        binary.chmod(0o700)

    def command(self, action, success=True):
        result = subprocess.run([MSET, action, '--dir', str(self.root)],
                                capture_output=True, text=True, timeout=15)
        self.assertEqual(result.returncode == 0, success, result.stdout + result.stderr)
        return result

    def record(self):
        return json.loads((self.root / 'mihomo.pid').read_text())

    def tearDown(self):
        subprocess.run([MSET, 'stop', '--dir', str(self.root)],
                       capture_output=True, timeout=15)
        self.temp.cleanup()

    def test_background_restart_and_idempotence(self):
        self.command('start')  # capture_output would hang if child inherited the terminal pipes
        first = self.record()['pid']
        os.kill(first, 0)
        self.assertIn('mock-started', (self.root / 'mihomo.log').read_text())
        self.command('start')
        self.assertEqual(first, self.record()['pid'])
        self.command('restart')
        self.assertNotEqual(first, self.record()['pid'])
        self.command('stop')
        self.assertFalse((self.root / 'mihomo.pid').exists())
        self.command('stop')

    def test_stale_pid_does_not_signal_unrelated_process(self):
        other = subprocess.Popen(['sleep', '120'])
        try:
            (self.root / 'mihomo.pid').write_text(json.dumps({'pid': other.pid, 'identity': 'stale'}))
            self.command('stop')
            self.assertIsNone(other.poll())
            self.assertFalse((self.root / 'mihomo.pid').exists())
        finally:
            other.terminate()
            other.wait()

    def test_startup_failure_and_invalid_config(self):
        self.mock('exit 9')
        self.command('start', success=False)
        self.assertFalse((self.root / 'mihomo.pid').exists())
        (self.root / 'mihomo').write_text('#!/bin/sh\nexit 2\n')
        self.command('start', success=False)
        self.assertFalse((self.root / 'mihomo.pid').exists())

    def test_simultaneous_start(self):
        args = [MSET, 'start', '--dir', str(self.root)]
        first = subprocess.Popen(args, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        second = subprocess.Popen(args, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        first.communicate(timeout=15)
        second.communicate(timeout=15)
        self.assertIn(0, [first.returncode, second.returncode])
        self.assertTrue((self.root / 'mihomo.pid').exists())
        self.command('stop')


if __name__ == '__main__':
    unittest.main()
