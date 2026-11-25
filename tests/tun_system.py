"""Real-kernel test. MUST run inside a private user/network/mount namespace.

unshare -Urnm --propagation private sh -eu -c \
  'mount -t sysfs sysfs /sys; MSET_ISOLATED_TEST=1 python3 tests/tun_system.py'

No subscription, external network, sudo, or host network changes are required.
"""
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
MSET = str(Path(os.environ.get('MSET_BINARY', ROOT / 'target/release/mset')).resolve())
CORE = os.environ.get('MIHOMO_TEST_BINARY', '/usr/local/bin/mihomo')


def main():
    if os.environ.get('MSET_ISOLATED_TEST') != '1' or os.geteuid() != 0:
        raise SystemExit('Refusing to run: use the private namespace command in the module header.')
    # A fresh namespace has only loopback: refuse to touch an existing network.
    def ip(*args):
        return subprocess.check_output(['ip', *args], text=True)
    assert [v['ifname'] for v in json.loads(ip('-j', 'link'))] == ['lo']
    assert Path('/dev/net/tun').exists(), '/dev/net/tun is required'
    assert Path(CORE).is_file(), 'MIHOMO_TEST_BINARY must point to a real core'
    ip('link', 'set', 'lo', 'up')
    ip('link', 'add', 'uplink', 'type', 'dummy')
    ip('addr', 'add', '192.0.2.2/24', 'dev', 'uplink')
    ip('link', 'set', 'uplink', 'up')
    ip('route', 'add', 'default', 'via', '192.0.2.1', 'dev', 'uplink')

    def snapshot():
        commands = [('link',), ('route', 'show', 'table', 'all'), ('rule', 'show')]
        result = []
        for family in ['-4', '-6']:
            for args in commands:
                values = json.loads(ip(family, '-j', *args))
                # Link counters/flags change during startup; identity alone is stable.
                if args == ('link',): values = [(v['ifindex'], v['ifname']) for v in values]
                result.append(json.dumps(values, sort_keys=True))
        return result

    before = snapshot()
    with tempfile.TemporaryDirectory(prefix='mset-real-tun-') as temp:
        directory = Path(temp)
        shutil.copy2(CORE, directory / 'mihomo')
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        config = directory / 'config.yaml'
        config.write_text(f'''mixed-port: 0
external-controller: 127.0.0.1:{port}
secret: isolated-test
mode: rule
log-level: info
proxies: []
proxy-groups:
  - name: PROXY
    type: select
    proxies: [DIRECT, REJECT]
rules: ["MATCH,PROXY"]
tun:
  enable: false
  device: mset-test
  stack: mixed
  auto-route: true
  auto-detect-interface: true
''')
        env = {k: v for k, v in os.environ.items() if not k.startswith('MIHOMO_')}
        env.update(HOME=temp, MIHOMO_CONFIG=str(config))
        def run(*args):
            result = subprocess.run([MSET, *args], env=env, cwd=temp,
                                    capture_output=True, text=True, timeout=90)
            if result.returncode:
                logs = (directory / 'mihomo.log').read_text() if (directory / 'mihomo.log').exists() else ''
                raise AssertionError(f'{args}: {result.stdout}\n{result.stderr}\n{logs}')
            return result.stdout
        try:
            run('start', '--dir', temp)
            run('use', 'REJECT', '-g', 'PROXY')  # Upgrades existing config to native selection cache.
            run('restart', '--dir', temp)
            assert 'REJECT' in run('status')
            # Remove explicit config selection so auto-permission inspection can validate the managed core.
            env.pop('MIHOMO_CONFIG')
            run('tun', 'on', '--dir', temp)
            assert 'Linux TUN, UP' in run('--api', f'http://127.0.0.1:{port}', '--secret', 'isolated-test', 'status')
            assert snapshot() != before, 'TUN did not change links/routes/rules'
            route = json.loads(ip('-j', 'route', 'get', '198.51.100.1'))
            assert any(v.get('dev') == 'mset-test' for v in route), route
            env['MIHOMO_CONFIG'] = str(config)
            run('reload')
            assert 'Linux TUN, UP' in run('status')
            run('restart', '--dir', temp)
            assert 'Linux TUN, UP' in run('status')
            assert 'REJECT' in run('status')
            run('tun', 'off')
            assert snapshot() == before, 'network resources were not restored after tun off'
            run('tun', 'on', '--dir', temp)
            run('stop', '--dir', temp)
            assert snapshot() == before, 'network resources were not restored after stop'
            run('start', '--dir', temp)
            assert 'Linux TUN, UP' in run('status')
            assert 'REJECT' in run('status'), 'node selection was not restored'
            run('stop', '--dir', temp)
            assert snapshot() == before, 'network resources were not restored after restart/stop'
            print('PASS: real Mihomo API readiness, native node cache, TUN device/routing, reload, restart, and complete link/route/rule cleanup in isolated namespace.')
        finally:
            subprocess.run([MSET, 'stop', '--dir', temp], env=env, capture_output=True, timeout=90)


if __name__ == '__main__':
    main()
