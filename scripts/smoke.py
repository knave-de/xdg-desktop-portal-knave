#!/usr/bin/env python3
"""Test only in private Wayland/D-Bus/PipeWire sessions; never import host activation."""
import argparse
import json
import os
import signal
from pathlib import Path
import subprocess
import tempfile
import time

REPO = Path(__file__).resolve().parent.parent
ROOT = REPO.parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--release', action='store_true')
parser.add_argument('--native', action='store_true', help='requires manual source selection and confirmation')
parser.add_argument('--render', action='store_true', help='render the native picker without clicking')
parser.add_argument('--installed', action='store_true', help='verify installed D-Bus activation on the private test bus')
parser.add_argument('--bus', action='store_true', help=argparse.SUPPRESS)
args = parser.parse_args()
profile = 'release' if args.release else 'debug'
example = REPO / 'target' / profile / 'examples/knave_smoke'
children = []
runtime = Path(os.environ['XDG_RUNTIME_DIR']) if args.bus else Path(tempfile.mkdtemp(prefix='knave-portal-smoke-'))
runtime.chmod(0o700)
env = os.environ.copy()


def spawn(argv, label, **kwargs):
    log = (runtime / (label + '.log')).open('w')
    child = subprocess.Popen(argv, env=env, stdout=log, stderr=log, **kwargs)
    children.append(child)
    return child


def wait_for(test, child, seconds=10):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if test():
            return
        if child.poll() is not None:
            raise RuntimeError('child exited; inspect ' + str(runtime))
        time.sleep(0.05)
    raise TimeoutError('startup timeout; inspect ' + str(runtime))


def run(argv, timeout=30):
    result = subprocess.run(argv, env=env, timeout=timeout, text=True, capture_output=True)
    if result.returncode:
        raise RuntimeError(result.stdout + result.stderr)
    return result.stdout


def usage(pid):
    directory = Path('/proc') / str(pid)
    fields = (directory / 'stat').read_text().split()
    status = dict(line.split(':', 1) for line in (directory / 'status').read_text().splitlines())
    return {
        'cpu_seconds': (int(fields[13]) + int(fields[14])) / os.sysconf('SC_CLK_TCK'),
        'rss_kib': int(status['VmRSS'].split()[0]),
        'threads': int(status['Threads']),
        'fds': len(list((directory / 'fd').iterdir())),
        'voluntary_switches': sum(int(dict(line.split(':', 1) for line in (task / 'status').read_text().splitlines())['voluntary_ctxt_switches']) for task in (directory / 'task').iterdir()),
    }


def child_pids(pid):
    return {child for task in (Path('/proc') / str(pid) / 'task').iterdir()
            for child in (task / 'children').read_text().split()}


def settings(value):
    path = runtime / 'config/knave/config.toml'
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(value)
    time.sleep(0.3)
    return run(['gdbus', 'call', '--session', '--dest', 'org.freedesktop.impl.portal.desktop.knave',
                '--object-path', '/org/freedesktop/portal/desktop', '--method',
                'org.freedesktop.impl.portal.Settings.Read', 'org.freedesktop.appearance', 'color-scheme'])

try:
    if args.bus:
        def ready():
            return subprocess.run(['busctl', '--address=' + env['DBUS_SESSION_BUS_ADDRESS'],
                'introspect', 'org.freedesktop.impl.portal.desktop.knave', '/org/freedesktop/portal/desktop'],
                env=env, capture_output=True).returncode == 0
        if args.installed:
            run(['busctl', '--address=' + env['DBUS_SESSION_BUS_ADDRESS'], 'introspect',
                 'org.freedesktop.impl.portal.desktop.knave', '/org/freedesktop/portal/desktop'])
            pid = int(run(['busctl', '--address=' + env['DBUS_SESSION_BUS_ADDRESS'], 'call',
                'org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus',
                'GetConnectionUnixProcessID', 's', 'org.freedesktop.impl.portal.desktop.knave']).split()[1])
            installed = Path.home() / '.local/libexec/xdg-desktop-portal-knave'
            assert (Path('/proc') / str(pid) / 'exe').resolve() == installed
            class ActivatedService:
                def __init__(self):
                    self.pid = pid
                def poll(self):
                    try:
                        return None if (Path('/proc') / str(pid) / 'exe').resolve() == installed else 0
                    except OSError:
                        return 0
                def terminate(self):
                    if self.poll() is None:
                        os.kill(pid, signal.SIGTERM)
                def kill(self):
                    if self.poll() is None:
                        os.kill(pid, signal.SIGKILL)
                def wait(self, timeout=5):
                    deadline = time.monotonic() + timeout
                    while self.poll() is None:
                        if time.monotonic() > deadline:
                            raise subprocess.TimeoutExpired('installed backend', timeout)
                        time.sleep(0.05)
            backend = ActivatedService()
            children.append(backend)
            print('installed D-Bus activation: passed', flush=True)
        else:
            backend = spawn([str(REPO / 'target' / profile / 'xdg-desktop-portal-knave')], 'backend')
        wait_for(ready, backend)
        monitor = spawn(['gdbus', 'monitor', '--session', '--dest', 'org.freedesktop.impl.portal.desktop.knave'], 'settings-signals')
        assert 'uint32 1' in settings('schema_version = 2\n[portal]\ncolor_scheme = 1\n')
        assert 'uint32 2' in settings('schema_version = 2\n[portal]\ncolor_scheme = 2\n')
        assert 'uint32 2' in settings('invalid = [')
        assert 'SettingChanged' in (runtime / 'settings-signals.log').read_text()
        print('settings: live updates, signals, invalid-edit retention passed', flush=True)
        (runtime / 'config/knave/config.toml').unlink()
        # Policy-only profile does not open host audio, Bluetooth or camera devices.
        spawn(['wireplumber', '--profile=policy'], 'wireplumber')
        frontend = spawn(['/usr/lib/xdg-desktop-portal', '--verbose'], 'frontend')
        time.sleep(1)
        compositor_pid = int(env['KNAVE_SMOKE_COMPOSITOR_PID'])
        compositor_before = usage(compositor_pid)
        before_fds = {str(p.resolve()) for p in (Path('/proc') / str(backend.pid) / 'fd').iterdir()}
        before = usage(backend.pid)
        started = time.monotonic()
        time.sleep(3)
        idle = usage(backend.pid)
        idle['cpu_percent'] = round(100 * (idle['cpu_seconds'] - before['cpu_seconds']) / (time.monotonic() - started), 2)
        print('backend idle:', json.dumps(idle), flush=True)
        compositor_idle = usage(compositor_pid)
        compositor_idle['cpu_percent'] = round(100 * (compositor_idle['cpu_seconds'] - compositor_before['cpu_seconds']) / (time.monotonic() - started), 2)
        print('compositor idle:', json.dumps(compositor_idle), flush=True)
        env['KNAVE_SMOKE_HOLD'] = '3'
        for attempt in range(3 if not args.native else 1):
            sample_before = usage(backend.pid)
            compositor_before = usage(compositor_pid)
            compositor_peak = compositor_before.copy()
            sample_start = time.monotonic()
            consumer = subprocess.Popen([str(example), 'frontend'], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            children.append(consumer)
            peak = sample_before.copy()
            deadline = sample_start + (180 if args.native else 30)
            while consumer.poll() is None:
                if time.monotonic() >= deadline:
                    raise TimeoutError('frontend test timed out')
                sample = usage(backend.pid)
                compositor_sample = usage(compositor_pid)
                for key in ('rss_kib', 'threads', 'fds'):
                    compositor_peak[key] = max(compositor_peak[key], compositor_sample[key])
                for key in ('rss_kib', 'threads', 'fds'):
                    peak[key] = max(peak[key], sample[key])
                time.sleep(0.2)
            stdout, stderr = consumer.communicate()
            if consumer.returncode:
                raise RuntimeError(stdout + stderr)
            sample_after = usage(backend.pid)
            peak['cpu_percent'] = round(100 * (sample_after['cpu_seconds'] - sample_before['cpu_seconds']) / (time.monotonic() - sample_start), 2)
            peak['wakeups_per_second'] = round((sample_after['voluntary_switches'] - sample_before['voluntary_switches']) / (time.monotonic() - sample_start), 2)
            print(stdout, flush=True)
            print('active workload peak:', json.dumps(peak), flush=True)
            compositor_after = usage(compositor_pid)
            compositor_peak['cpu_percent'] = round(100 * (compositor_after['cpu_seconds'] - compositor_before['cpu_seconds']) / (time.monotonic() - sample_start), 2)
            print('compositor active workload peak:', json.dumps(compositor_peak), flush=True)
            print('after session:', json.dumps(sample_after), flush=True)
        # Failure of the native sharing control must close the frontend session.
        stop_test = subprocess.Popen([str(example), 'stop'], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        children.append(stop_test)
        time.sleep(5)
        child_ids = child_pids(backend.pid)
        stopped = False
        for child_id in child_ids:
            cmdline = (Path('/proc') / child_id / 'cmdline').read_bytes()
            if b'portal-picker' in cmdline:
                os.kill(int(child_id), signal.SIGTERM)
                stopped = True
        assert stopped, 'sharing control missing'
        stdout, stderr = stop_test.communicate(timeout=15)
        if stop_test.returncode:
            raise RuntimeError(stdout + stderr)
        print(stdout, flush=True)
        after = usage(backend.pid)
        print('backend after repeated sessions:', json.dumps(after), flush=True)
        after_fds = {str(p.resolve()) for p in (Path('/proc') / str(backend.pid) / 'fd').iterdir()}
        print('retained fd targets:', json.dumps(sorted(after_fds - before_fds)), flush=True)
        if not args.native and not args.installed:
            sleeper = runtime / 'cancel-test-source'
            sleeper.write_text('#!/usr/bin/env python3\nimport time\ntime.sleep(30)\n')
            sleeper.chmod(0o700)
            # Start a new backend with a blocking selection fixture, then cancel through the frontend.
            backend.terminate()
            backend.wait(timeout=5)
            env['XDP_KNAVE_SOURCE_PICKER'] = str(sleeper)
            backend = spawn([str(REPO / 'target' / profile / 'xdg-desktop-portal-knave')], 'cancel-backend')
            wait_for(ready, backend)
            print(run([str(example), 'cancel']), flush=True)
            time.sleep(0.5)
            assert not any('cancel-test-source' in (Path('/proc') / str(child) / 'cmdline').read_text(errors='ignore')
                for child in child_pids(backend.pid))
            print('cancelled picker child reaped', flush=True)
    else:
        host = Path(env['WAYLAND_DISPLAY'])
        if not host.is_absolute():
            host = Path(env['XDG_RUNTIME_DIR']) / host
        env.update(XDG_RUNTIME_DIR=str(runtime), XDG_CONFIG_HOME=str(runtime / 'config'),
            XDG_DATA_HOME=str(runtime / 'data'), XDG_CACHE_HOME=str(runtime / 'cache'),
            XDG_DATA_DIRS=str(runtime / 'data') + ':/usr/share', WAYLAND_DISPLAY=str(host),
            XDG_CURRENT_DESKTOP='Knave', XDP_KNAVE_SHELL_BINARY=str(ROOT / 'knaveshell/target' / profile / 'knave-shell'))
        # Do not inherit test overrides from a normal desktop.
        env.pop('XDP_KNAVE_SOURCE_PICKER', None)
        env.pop('XDP_GENERIC_SOURCE_PICKER', None)
        pw = spawn(['pipewire'], 'pipewire')
        wait_for(lambda: (runtime / 'pipewire-0').exists(), pw)
        compositor = spawn([str(ROOT / 'villain/target' / profile / 'villain'), '--winit'], 'villain')
        wait_for(lambda: any(runtime.glob('wayland-*')) and any(p.is_socket() for p in runtime.glob('wayland-*')), compositor)
        env['KNAVE_SMOKE_COMPOSITOR_PID'] = str(compositor.pid)
        env['WAYLAND_DISPLAY'] = next(p.name for p in runtime.glob('wayland-*') if p.is_socket())
        env['KNAVE_SMOKE_PNG'] = str(runtime / 'capture.png')
        print('test artifacts:', runtime, flush=True)
        print(run([str(example), 'capture']), flush=True)
        if args.render:
            request = {'version': 1, 'request_id': 'render-test', 'app_id': 'org.knave.Test',
                'operation': 'screenshot', 'multiple': False, 'sources': [{'id': 1, 'name': 'Nested display',
                'description': 'Native monitor selection', 'width': 1904, 'height': 982,
                'preview_png': None}]}
            picker = spawn([env['XDP_KNAVE_SHELL_BINARY'], 'portal-picker'], 'picker', stdin=subprocess.PIPE)
            picker.stdin.write(json.dumps(request).encode())
            picker.stdin.close()
            time.sleep(8)
            assert picker.poll() is None
            initial = usage(picker.pid)
            started = time.monotonic()
            time.sleep(3)
            settled = usage(picker.pid)
            settled['cpu_percent'] = round(100 * (settled['cpu_seconds'] - initial['cpu_seconds']) / (time.monotonic() - started), 2)
            print('native picker idle:', json.dumps(settled), flush=True)
            print(run([str(example), 'capture']), flush=True)
        else:
            descriptor = runtime / 'data/xdg-desktop-portal/portals/knave.portal'
            descriptor.parent.mkdir(parents=True)
            descriptor.write_text((REPO / 'data/knave.portal').read_text())
            routing = runtime / 'config/xdg-desktop-portal/knave-portals.conf'
            routing.parent.mkdir(parents=True)
            routing.write_text((REPO / 'data/knave-portals.conf').read_text())
            if args.installed:
                service = runtime / 'data/dbus-1/services/org.freedesktop.impl.portal.desktop.knave.service'
                service.parent.mkdir(parents=True)
                service.write_text((Path.home() / '.local/share/dbus-1/services/org.freedesktop.impl.portal.desktop.knave.service').read_text())
            if not args.native:
                picker = runtime / 'select-test-source'
                picker.write_text("#!/bin/sh\nawk -F '\\t' 'NR==1 { print $1; exit }'\n")
                picker.chmod(0o700)
                env['XDP_KNAVE_SOURCE_PICKER'] = str(picker)
            command = ['dbus-run-session', '--', 'python3', str(Path(__file__).resolve()), '--bus']
            if args.release:
                command.append('--release')
            if args.native:
                command.append('--native')
            if args.installed:
                command.append('--installed')
            print(run(command, timeout=210 if args.native else 120), flush=True)
finally:
    for child in reversed(children):
        if child.poll() is None:
            child.terminate()
    for child in children:
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()
