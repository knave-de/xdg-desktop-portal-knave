#!/usr/bin/env python3
"""Install the portal backend and activation metadata without administrator access."""
import argparse
import os
from pathlib import Path
import subprocess
import shutil
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--user', action='store_true')
parser.add_argument('--prefix', type=Path)
parser.add_argument('--destdir', type=Path, default=Path('/'))
parser.add_argument('--no-build', action='store_true')
parser.add_argument('--uninstall', action='store_true')
args = parser.parse_args()
root = Path(__file__).resolve().parent.parent
prefix = (args.prefix or Path.home() / '.local').resolve()
config = Path(os.environ.get('XDG_CONFIG_HOME', Path.home() / '.config'))
unit_dir = config / 'systemd/user' if args.prefix is None else prefix / 'lib/systemd/user'
binary = prefix / 'libexec/xdg-desktop-portal-knave'
# D-Bus activation is mediated by systemd, but Exec is also valid without systemd.
quoted = '"' + str(binary).replace('\\', '\\\\').replace('"', '\\"') + '"'
unit_exec = quoted.replace('%', '%%')
files = {
    binary: None,
    prefix / 'share/xdg-desktop-portal/portals/knave.portal': (root / 'data/knave.portal').read_text(),
    prefix / 'share/xdg-desktop-portal/knave-portals.conf': (root / 'data/knave-portals.conf').read_text(),
    prefix / 'share/dbus-1/services/org.freedesktop.impl.portal.desktop.knave.service':
        '[D-BUS Service]\nName=org.freedesktop.impl.portal.desktop.knave\n'
        f'Exec={quoted}\nSystemdService=xdg-desktop-portal-knave.service\n',
    unit_dir / 'xdg-desktop-portal-knave.service':
        '[Unit]\nDescription=Knave XDG Desktop Portal Backend\n'
        'PartOf=graphical-session.target\nStartLimitIntervalSec=30\nStartLimitBurst=3\n\n[Service]\nType=dbus\n'
        'BusName=org.freedesktop.impl.portal.desktop.knave\n'
        f'ExecStart={unit_exec}\nRestart=on-failure\nRestartSec=2\n',
}
if not args.uninstall and not args.no_build:
    subprocess.run(['cargo', 'build', '--release', '--locked', '--manifest-path', str(root / 'Cargo.toml')], check=True)
for path, contents in files.items():
    staged = args.destdir / path.relative_to('/')
    if args.uninstall:
        staged.unlink(missing_ok=True)
    else:
        staged.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile(dir=staged.parent, delete=False) as temporary:
            temporary_path = Path(temporary.name)
        try:
            if contents is None:
                shutil.copyfile(root / 'target/release/xdg-desktop-portal-knave', temporary_path)
                temporary_path.chmod(0o755)
            else:
                temporary_path.write_text(contents)
                temporary_path.chmod(0o644)
            temporary_path.replace(staged)
        finally:
            temporary_path.unlink(missing_ok=True)
    print(staged)
if args.prefix is None and args.destdir == Path('/'):
    subprocess.run(['systemctl', '--user', 'daemon-reload'], check=True)
