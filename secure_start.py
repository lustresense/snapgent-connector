"""Interactive, source-only Snapgent startup for Windows."""
import json
import os
import re
import socket
import subprocess
import sys
from pathlib import Path
from bridge_security import security_directory, load_or_create_token

ROOT = Path(__file__).resolve().parent


def main():
    for port in (17613, 17614):
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
            if sock.connect_ex(('127.0.0.1', port)) == 0:
                print(f'Port {port} is already occupied. Close the previous bridge manually. No process was terminated.')
                return 1
    directory = security_directory()
    directory.mkdir(parents=True, exist_ok=True)
    origins_file = directory / 'trusted-extension-id'
    extension_id = origins_file.read_text(encoding='ascii').strip() if origins_file.exists() else ''
    if not re.fullmatch(r'[a-p]{32}', extension_id):
        print('Open chrome://extensions, enable Developer mode, and locate the Snapgent extension ID.')
        extension_id = input('Trusted Snapgent extension ID: ').strip()
        if not re.fullmatch(r'[a-p]{32}', extension_id):
            print('Invalid extension ID.')
            return 1
        origins_file.write_text(extension_id, encoding='ascii')
    load_or_create_token()
    print('Trusted extension:', extension_id)
    print('Pair through the extension popup using the credential in:')
    print(directory / 'bridge-token')
    print('Do not share that credential with an AI conversation.')
    print('Default mode asks for local approval before terminal execution.')
    trusted = input('Trust unrestricted terminal execution for this bridge session? [y/N]: ').strip().lower() == 'y'
    config_path = ROOT / 'config.json'
    subprocess.check_call(['powershell.exe', '-NoProfile', '-File', str(ROOT / 'setup-config.ps1')], cwd=ROOT)
    figcraft_helper = ROOT / 'start-figcraft.ps1'
    if figcraft_helper.is_file():
        subprocess.run(['powershell.exe', '-NoProfile', '-File', str(figcraft_helper), '-NoLaunch'], cwd=ROOT, check=False)
    config = json.loads(config_path.read_text(encoding='utf-8-sig'))
    servers = config.setdefault('mcpServers', {})
    for server_id in ('vscode', 'zed'):
        script = ROOT / (server_id + '_mcp.py')
        if not script.is_file():
            raise RuntimeError('Missing MCP source: ' + script.name)
        entry = servers.setdefault(server_id, {})
        entry.update(command=sys.executable, args=[str(script)])
        entry.setdefault('env', {})
    temp = config_path.with_suffix('.secure.tmp')
    temp.write_text(json.dumps(config, indent=2, ensure_ascii=False) + '\n', encoding='utf-8')
    temp.replace(config_path)
    env = os.environ.copy()
    env['ZS_TRUSTED_EXTENSION_ORIGINS'] = 'chrome-extension://' + extension_id
    env['ZS_TRUST_TERMINAL_SESSION'] = '1' if trusted else '0'
    env['ZS_BRIDGE_PORT'] = '17613'
    env['ZS_BRIDGE_STREAM_PORT'] = '17614'
    return subprocess.call([sys.executable, str(ROOT / 'bridge.py')], cwd=ROOT, env=env)


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print('Secure startup failed:', type(error).__name__)
        sys.exit(1)
