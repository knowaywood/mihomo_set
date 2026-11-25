"""Mock-controller tests: python3 tests/configuration.py (no TUN privileges required)."""
import http.server
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import threading
import unittest

MSET = str(Path(os.environ.get('MSET_BINARY', str(Path(__file__).resolve().parents[1] / 'target/release/mset'))).resolve())


class Configuration(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='mset-config-api-')
        self.root = Path(self.temp.name)
        self.config = self.root / 'config.yaml'
        self.runtime = {'mode': 'rule', 'tun': {'enable': False, 'device': 'mset-nonexistent-test'}}
        self.calls = []
        self.fail_patch = False
        self.get_status = 200
        self.secret = 'a"b\\c#d'
        owner = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def reply(self, code, data):
                self.send_response(code)
                self.end_headers()
                self.wfile.write(json.dumps(data).encode())

            def authorized(self):
                if self.headers.get('Authorization') != f'Bearer {owner.secret}':
                    self.reply(401, {'message': 'bad secret'})
                    return False
                return True

            def do_GET(self):
                if not self.authorized():
                    return
                owner.calls.append(('GET', self.path))
                if owner.get_status != 200:
                    self.reply(owner.get_status, {'message': 'mock controller failure'})
                    return
                if self.path == '/configs':
                    self.reply(200, owner.runtime)
                else:
                    self.reply(200, {'proxies': {}, 'providers': {}})

            def do_PATCH(self):
                if not self.authorized():
                    return
                patch = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                owner.calls.append(('PATCH', patch))
                if owner.fail_patch:
                    self.reply(500, {'message': 'mock failure'})
                    return
                for key, value in patch.items():
                    if isinstance(value, dict):
                        owner.runtime.setdefault(key, {}).update(value)
                    else:
                        owner.runtime[key] = value
                self.reply(204, {})

            def do_PUT(self):
                if not self.authorized():
                    return
                self.rfile.read(int(self.headers['Content-Length']))
                owner.runtime['mode'] = re.search(r'^mode: (\w+)', owner.config.read_text(), re.M)[1]
                self.reply(204, {})

        self.server = http.server.HTTPServer(('127.0.0.1', 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.endpoint = f'http://127.0.0.1:{self.server.server_port}'
        self.original = (f'external-controller: "{self.endpoint}" # controller comment\n'
                         f'secret: {json.dumps(self.secret)} # escaped secret\n'
                         'mode: rule\ntun: {enable: false, device: mset-nonexistent-test, mtu: 1400}\n'
                         'dns: {enable: true}\nproxies: []\n')
        self.config.write_text(self.original)
        self.env = {k: v for k, v in os.environ.items() if not k.startswith('MIHOMO_')}
        self.env.update(HOME=str(self.root), MIHOMO_CONFIG=str(self.config))

    def run_cli(self, *args, success=True):
        result = subprocess.run([MSET, *args], cwd=self.root, env=self.env,
                                input='', capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode == 0, success, result.stdout + result.stderr)
        return result

    def tearDown(self):
        if (self.root / 'mihomo.pid').exists():
            self.run_cli('stop', '--dir', str(self.root))
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()
        self.temp.cleanup()

    def test_yaml_secret_and_mode_persist_across_reload_and_restart(self):
        self.run_cli('mode')  # Authentication verifies escaped and commented YAML.
        core = self.root / 'mihomo'
        core.write_text(f'''#!{sys.executable}
import pathlib, re, sys, time
if '-t' in sys.argv: sys.exit(0)
text = pathlib.Path({str(self.config)!r}).read_text()
print('started-mode=' + re.search(r'^mode: (\\w+)', text, re.M)[1], flush=True)
while True: time.sleep(1)
''')
        core.chmod(0o700)
        self.run_cli('start', '--dir', str(self.root))
        self.run_cli('mode', 'global')
        self.assertIn('mode: global', self.config.read_text())
        self.assertEqual(self.config.with_suffix('.yaml.bak').read_text(), self.original)
        self.runtime['mode'] = 'direct'  # Simulate a temporary external change.
        self.run_cli('reload')
        self.assertEqual(self.runtime['mode'], 'global')
        self.run_cli('restart', '--dir', str(self.root))
        self.assertIn('started-mode=global', (self.root / 'mihomo.log').read_text())
        self.run_cli('set', 'mixed-port', '7891')
        self.assertIn('mixed-port: 7891', self.config.read_text())

    def test_api_failure_does_not_commit_configuration(self):
        self.fail_patch = True
        self.run_cli('mode', 'global', success=False)
        self.assertEqual(self.config.read_text(), self.original)
        self.assertFalse(list(self.root.glob('*.tmp')))

    @unittest.skipUnless(sys.platform.startswith('linux'), 'Linux device inspection')
    def test_configured_on_with_missing_device_is_repaired_then_reports_failure(self):
        self.runtime['tun']['enable'] = True
        result = self.run_cli('tun', 'on', '--api', self.endpoint, success=False)
        patches = [body['tun']['enable'] for method, body in self.calls if method == 'PATCH']
        self.assertEqual(patches, [False, True, False])
        self.assertIn('no matching local TUN device is UP', result.stderr)
        self.assertFalse(self.runtime['tun']['enable'])
        self.assertEqual(self.config.read_text(), self.original)

    def test_tun_off_is_persisted_and_keeps_other_tun_fields(self):
        self.runtime['tun']['enable'] = True
        self.run_cli('tun', 'off')
        text = self.config.read_text()
        self.assertIn('enable: false', text)
        self.assertIn('mtu: 1400', text)
        self.assertIn('dns:', text)

    def test_tun_authentication_failure_does_not_try_restart_or_grant(self):
        result = self.run_cli('tun', 'on', '--dir', str(self.root), '--secret', 'wrong', success=False)
        self.assertIn('HTTP 401', result.stderr)
        self.assertIn('MIHOMO_SECRET', result.stderr)
        self.assertNotIn('sudo', result.stdout)
        self.assertFalse((self.root / 'mihomo.pid').exists())
        self.assertEqual(self.config.read_text(), self.original)

    def test_http_errors_keep_status_and_give_specific_guidance(self):
        for code, hint in [(403, 'access denied'), (404, 'not found'), (500, 'mihomo.log')]:
            with self.subTest(code=code):
                self.get_status = code
                result = self.run_cli('mode', success=False)
                self.assertIn(f'HTTP {code}', result.stderr)
                self.assertIn(hint, result.stderr)
                self.assertNotIn('run mset start', result.stderr)

    def test_discovery_skips_legacy_minimal_home_config(self):
        legacy = self.root / '.config/mihomo/config.yaml'
        legacy.parent.mkdir(parents=True)
        legacy.write_text('mixed-port: 7890\n')
        active = self.root / '.config/clash/config.yaml'
        active.parent.mkdir(parents=True)
        active.write_text(self.original)
        work = self.root / 'work'
        work.mkdir()
        env = dict(self.env)
        env.pop('MIHOMO_CONFIG')
        result = subprocess.run([MSET, 'mode'], cwd=work, env=env, input='',
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('rule', result.stdout)
        self.assertEqual(legacy.read_text(), 'mixed-port: 7890\n')
        if sys.platform.startswith('linux'):
            result = subprocess.run([MSET, 'tun', 'on'], cwd=work, env=env, input='',
                                    capture_output=True, text=True, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('no matching local TUN device is UP', result.stderr)
            self.assertNotIn('external-controller address', result.stderr)
        env['MIHOMO_CONFIG'] = str(legacy)
        result = subprocess.run([MSET, 'mode'], cwd=work, env=env, input='',
                                capture_output=True, text=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('external-controller', result.stderr)

    def test_invalid_yaml_reports_error_before_network(self):
        self.config.write_text('external-controller: [broken\n')
        result = self.run_cli('mode', success=False)
        self.assertIn('invalid YAML', result.stderr)
        self.assertEqual(self.calls, [])


if __name__ == '__main__':
    unittest.main()
