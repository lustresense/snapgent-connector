#!/usr/bin/env python3
"""Snapgent MCP adapter for local Zed workspaces."""
import os
import sqlite3
import sys
import importlib.util
from contextlib import closing
from pathlib import Path
from typing import Optional

_spec = importlib.util.spec_from_file_location('_snapgent_zed_engine', Path(__file__).with_name('vscode_mcp.py'))
engine = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(engine)


def database_paths():
    override = os.environ.get('ZS_ZED_DB')
    if override:
        return [Path(override).expanduser()]
    if sys.platform == 'win32':
        base = Path(os.environ.get('LOCALAPPDATA', Path.home() / 'AppData/Local')) / 'Zed'
    elif sys.platform == 'darwin':
        base = Path.home() / 'Library/Application Support/Zed'
    else:
        base = Path(os.environ.get('XDG_DATA_HOME', Path.home() / '.local/share')) / 'zed'
    return [base / 'db/0-stable/db.sqlite']


def active_folder() -> Optional[Path]:
    override = os.environ.get('ZS_ZED_WORKSPACE')
    if override:
        candidate = Path(override).expanduser().resolve()
        return candidate if candidate.is_dir() else None
    for db in database_paths():
        if not db.is_file():
            continue
        try:
            with closing(sqlite3.connect(db.resolve().as_uri() + '?mode=ro', uri=True, timeout=1)) as conn:
                columns = {row[1] for row in conn.execute('PRAGMA table_info(workspaces)')}
                if not {'paths', 'remote_connection_id', 'timestamp', 'workspace_id'} <= columns:
                    continue
                row = conn.execute('SELECT paths, remote_connection_id FROM workspaces ORDER BY timestamp DESC, workspace_id DESC LIMIT 1').fetchone()
        except (OSError, sqlite3.Error):
            continue
        if not row or row[1] is not None or not isinstance(row[0], str):
            return None
        paths = row[0].splitlines()
        if len(paths) != 1 or not paths[0].strip():
            return None
        candidate = Path(paths[0]).expanduser()
        if not candidate.is_absolute() or not candidate.is_dir():
            return None
        return candidate.resolve()
    return None


def get_root() -> Path:
    root = active_folder()
    if root is None:
        raise RuntimeError('Open one local project folder in Zed or configure ZS_ZED_WORKSPACE explicitly.')
    return engine.lock_workspace(root)


engine._vscode_active_folder = active_folder
engine.get_root = get_root
for tool in engine.TOOLS:
    tool['description'] = tool['description'].replace('VS Code', 'Zed')


def main():
    for line in sys.stdin:
        try:
            request = engine.json.loads(line)
        except ValueError:
            continue
        if not isinstance(request, dict):
            continue
        if request.get('method') == 'initialize':
            engine.send_message({'jsonrpc': '2.0', 'id': request.get('id'), 'result': {'protocolVersion': '2024-11-05', 'capabilities': {'tools': {'listChanged': False}}, 'serverInfo': {'name': 'zed-mcp', 'version': '1.0.0'}}})
        else:
            engine.handle_request(request)


if __name__ == '__main__':
    main()
