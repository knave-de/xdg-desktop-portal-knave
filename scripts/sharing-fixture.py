#!/usr/bin/env python3
"""Own private portal services attached to a nested desktop-harness run."""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import time

ROOT = Path(__file__).resolve().parent.parent.parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--run', type=Path, required=True)
parser.add_argument('--installed', action='store_true')
parser.add_argument('--exec', dest='command', nargs=argparse.REMAINDER)
args = parser.parse_args()
run = args.run.resolve()
manifest = json.loads((run / 'manifest.json').read_text())
if 'nested Winit' not in manifest.get('rendering', ''):
    raise SystemExit('requires a nested desktop-harness manifest')
runtime = Path(manifest['runtime'])
if (not runtime.is_dir() or runtime.stat().st_uid != os.getuid()
        or not runtime.name.startswith('knave-test-')):
    raise SystemExit('requires the owned desktop-harness runtime')
directory = run / 'portal-fixture'
if args.command:
    environment = json.loads((directory / 'environment.json').read_text())
    raise SystemExit(subprocess.run(args.command, env=dict(os.environ, **environment)).returncode)
if (runtime / 'pipewire-0').exists():
    raise SystemExit('close the existing private PipeWire fixture first')
wayland = [p.name for p in runtime.glob('wayland-*') if p.is_socket()]
if len(wayland) != 1:
    raise SystemExit('requires one live nested Wayland socket')
backend = (Path.home() / '.local/libexec/xdg-desktop-portal-knave' if args.installed
           else ROOT / 'xdg-desktop-portal-knave/target/release/xdg-desktop-portal-knave')
shell = (Path.home() / '.local/bin/knave-shell' if args.installed
         else ROOT / 'knaveshell/target/release/knave-shell')
if not backend.is_file() or not shell.is_file():
    raise SystemExit('build or install the matching backend and Shell first')
os.umask(0o077)
directory.mkdir(mode=0o700)
env = dict(os.environ, XDG_RUNTIME_DIR=str(runtime), WAYLAND_DISPLAY=wayland[0],
           DISPLAY=manifest['display'], XAUTHORITY=str(run / 'Xauthority'),
           XDG_CURRENT_DESKTOP='Knave', XDG_SESSION_TYPE='wayland',
           LIBGL_ALWAYS_SOFTWARE='1', LP_NUM_THREADS='4', WGPU_BACKEND='gl',
           QT_QPA_PLATFORM='wayland', GDK_BACKEND='wayland',
           XDP_KNAVE_SHELL_BINARY=str(shell), RUST_LOG='xdg_desktop_portal_knave=debug')
for key, folder in [('XDG_CONFIG_HOME', 'config'), ('XDG_DATA_HOME', 'data'),
                    ('XDG_CACHE_HOME', 'cache')]:
    path = directory / folder
    path.mkdir()
    env[key] = str(path)
env['XDG_DATA_DIRS'] = env['XDG_DATA_HOME'] + ':/usr/share'
env['KNAVE_CONFIG'] = str(directory / 'portal.toml')
Path(env['KNAVE_CONFIG']).write_text('[portal]\nenabled=true\n')
for key in ['DBUS_SESSION_BUS_ADDRESS', 'KNAVE_HARNESS_READY_DIR',
            'XDP_KNAVE_SOURCE_PICKER', 'XDP_GENERIC_SOURCE_PICKER']:
    env.pop(key, None)
for key, relative, source in [
    ('XDG_DATA_HOME', 'xdg-desktop-portal/portals/knave.portal', 'knave.portal'),
    ('XDG_CONFIG_HOME', 'xdg-desktop-portal/knave-portals.conf', 'knave-portals.conf'),
]:
    path = Path(env[key]) / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text((ROOT / 'xdg-desktop-portal-knave/data' / source).read_text())
children = []
logs = []
stopping = False


def stop(*_):
    global stopping
    stopping = True


signal.signal(signal.SIGTERM, stop)
signal.signal(signal.SIGINT, stop)


def spawn(command, name):
    log = (directory / (name + '.log')).open('w')
    logs.append(log)
    child = subprocess.Popen(command, env=env, stdin=subprocess.DEVNULL,
                             stdout=log, stderr=log, start_new_session=True)
    children.append(child)
    return child


def wait_ready(predicate):
    for _ in range(200):
        if stopping or any(child.poll() is not None for child in children):
            raise RuntimeError('private service exited; inspect ' + str(directory))
        if predicate():
            return
        time.sleep(.1)
    raise TimeoutError('private service startup; inspect ' + str(directory))


try:
    bus_log = (directory / 'bus.log').open('w')
    logs.append(bus_log)
    bus = subprocess.Popen(['dbus-daemon', '--session', '--nofork', '--print-address=1'],
                           env=env, stdout=subprocess.PIPE, stderr=bus_log,
                           text=True, start_new_session=True)
    children.append(bus)
    env['DBUS_SESSION_BUS_ADDRESS'] = bus.stdout.readline().strip()
    spawn(['pipewire'], 'pipewire')
    wait_ready(lambda: (runtime / 'pipewire-0').exists())
    spawn(['wireplumber', '--profile=policy'], 'wireplumber')
    spawn([str(backend)], 'backend')
    wait_ready(lambda: subprocess.run([
        'busctl', '--auto-start=no', '--address=' + env['DBUS_SESSION_BUS_ADDRESS'],
        'introspect', 'org.freedesktop.impl.portal.desktop.knave',
        '/org/freedesktop/portal/desktop',
    ], env=env, capture_output=True).returncode == 0)
    subprocess.run(['busctl', '--address=' + env['DBUS_SESSION_BUS_ADDRESS'],
                    'introspect', 'org.freedesktop.portal.Desktop',
                    '/org/freedesktop/portal/desktop'], env=env, capture_output=True, check=True)
    keys = ['XDG_RUNTIME_DIR', 'WAYLAND_DISPLAY', 'DISPLAY', 'XAUTHORITY',
            'DBUS_SESSION_BUS_ADDRESS', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME',
            'XDG_CACHE_HOME', 'XDG_DATA_DIRS', 'XDG_CURRENT_DESKTOP', 'XDG_SESSION_TYPE',
            'LIBGL_ALWAYS_SOFTWARE', 'LP_NUM_THREADS', 'WGPU_BACKEND',
            'QT_QPA_PLATFORM', 'GDK_BACKEND', 'XDP_KNAVE_SHELL_BINARY', 'KNAVE_CONFIG']
    (directory / 'environment.json').write_text(json.dumps({k: env[k] for k in keys}, indent=2))
    (directory / 'fixture.pid').write_text(str(os.getpid()))
    print('Private portal ready; environment: ' + str(directory / 'environment.json'), flush=True)
    while not stopping:
        if any(child.poll() is not None for child in children):
            raise RuntimeError('private service exited; inspect ' + str(directory))
        time.sleep(.5)
finally:
    for child in reversed(children):
        try:
            os.killpg(child.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
    for child in children:
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.wait()
    for log in logs:
        log.close()
