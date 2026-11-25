"""Run after cargo build --release: python3 tests/lifecycle.py (Linux/macOS)."""
import json
import http.server
import threading
import time
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

MSET = str(Path(os.environ.get('MSET_BINARY', str(Path(__file__).resolve().parents[1] / 'target/release/mset'))).resolve())


@unittest.skipUnless(os.name == 'posix', 'Unix lifecycle support')
class Lifecycle(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='mset-lifecycle-')
        self.root = Path(self.temp.name)
        self.api_status = 200
        self.api_ready_at = 0
        self.api_invalid_json = False
        self.api_response_delay = 0
        owner = self
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_): pass
            def do_GET(self):
                status = owner.api_status if time.monotonic() >= owner.api_ready_at else 503
                self.send_response(status)
                self.end_headers()
                time.sleep(owner.api_response_delay)
                try:
                    self.wfile.write(b'<html>wrong server</html>' if owner.api_invalid_json else
                                     json.dumps({'mode': 'rule', 'tun': {'enable': False}}).encode())
                except BrokenPipeError:
                    pass
        self.server = http.server.HTTPServer(('127.0.0.1', 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        (self.root / 'config.yaml').write_text(
            f'external-controller: 127.0.0.1:{self.server.server_port}\nmode: rule\n')
        self.mock('echo mock-started\nexec sleep 120')

    def mock(self, body):
        binary = self.root / 'mihomo'
        binary.write_text('#!/bin/sh\nif [ "$1" = "-t" ]; then exit 0; fi\n' + body + '\n')
        binary.chmod(0o700)

    def command(self, action, success=True, timeout=15, startup_timeout=3):
        args = [MSET, action, '--dir', str(self.root)]
        if action in ('start', 'restart'): args += ['--timeout', str(startup_timeout)]
        result = subprocess.run(args,
                                capture_output=True, text=True, timeout=timeout)
        self.assertEqual(result.returncode == 0, success, result.stdout + result.stderr)
        return result

    def record(self):
        return json.loads((self.root / 'mihomo.pid').read_text())

    def tearDown(self):
        subprocess.run([MSET, 'stop', '--dir', str(self.root)],
                       capture_output=True, timeout=15)
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()
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

    def test_restart_waits_for_slow_tun_teardown(self):
        delay = self.root / 'shutdown-delay'
        delay.write_text('12')
        binary = self.root / 'mihomo'
        binary.write_text(f"""#!{sys.executable}
import pathlib, signal, sys, time
if '-t' in sys.argv:
    sys.exit(0)
delay_file = pathlib.Path({str(delay)!r})
def shutdown(*_):
    print('shutdown-started', flush=True)
    time.sleep(float(delay_file.read_text()))
    print('shutdown-finished', flush=True)
    sys.exit(0)
signal.signal(signal.SIGTERM, shutdown)
print('core-started', flush=True)
while True:
    time.sleep(1)
""")
        binary.chmod(0o700)
        self.command('start')
        old_pid = self.record()['pid']
        try:
            self.command('restart', timeout=30)
            self.assertNotEqual(old_pid, self.record()['pid'])
            events = (self.root / 'mihomo.log').read_text().splitlines()
            self.assertEqual(events[:4], ['core-started', 'shutdown-started',
                                         'shutdown-finished', 'core-started'])
        finally:
            delay.write_text('0')

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

    def test_restart_invalid_config_does_not_stop_running_core(self):
        self.command('start')
        pid = self.record()['pid']
        (self.root / 'config.yaml').write_text('tun: [broken\n')
        self.command('restart', success=False)
        self.assertEqual(self.record()['pid'], pid)
        os.kill(pid, 0)

    def test_start_waits_for_api_readiness(self):
        self.api_ready_at = time.monotonic() + 1.5
        before = time.monotonic()
        self.command('start')
        self.assertGreaterEqual(time.monotonic() - before, 1.4)

    def test_startup_api_errors_are_clear_and_process_remains_manageable(self):
        self.api_status = 401
        result = self.command('start', success=False)
        self.assertIn('HTTP 401', result.stderr)
        self.assertIn('MIHOMO_SECRET', result.stderr)
        self.assertTrue((self.root / 'mihomo.pid').exists())
        self.command('stop')
        self.api_status = 503
        result = self.command('start', success=False, startup_timeout=1)
        self.assertIn('timed out', result.stderr)
        self.command('stop')
        self.api_status = 200
        self.api_invalid_json = True
        result = self.command('start', success=False)
        self.assertIn('invalid controller response', result.stderr)
        self.command('stop')
        self.api_invalid_json = False
        self.api_response_delay = 2
        result = self.command('start', success=False, startup_timeout=1)
        self.assertIn('timed out', result.stderr)

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
