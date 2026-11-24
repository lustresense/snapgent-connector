"""Local authentication primitives for Snapgent's source runtime."""
import base64
import hmac
import json
import sys
import os
import re
import secrets
from pathlib import Path


def security_directory():
    base = Path(os.environ.get('LOCALAPPDATA') or os.environ.get('XDG_STATE_HOME') or (Path.home() / '.local/state'))
    return base / 'SnapgentSecurity'


def load_or_create_token():
    directory = security_directory()
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    path = directory / 'bridge-token'
    try:
        fd = os.open(str(path), os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError:
        pass
    else:
        with os.fdopen(fd, 'w', encoding='ascii') as handle:
            handle.write(secrets.token_hex(32))
    token = path.read_text(encoding='ascii').strip()
    if not re.fullmatch(r'[0-9a-f]{64}', token):
        raise RuntimeError('Invalid local bridge credential; repair it manually.')
    return token


def token_matches(expected, supplied):
    return isinstance(expected, str) and isinstance(supplied, str) and supplied.isascii() and hmac.compare_digest(expected, supplied)


def fresh_nonce():
    return secrets.token_hex(32)


def valid_nonce(value):
    return isinstance(value, str) and re.fullmatch(r'[0-9a-f]{64}', value) is not None


def authentication_proof(token, role, client_nonce, server_nonce):
    if role not in ('client', 'server') or not valid_nonce(client_nonce) or not valid_nonce(server_nonce):
        raise ValueError('Invalid authentication challenge')
    message = ('snapgent-auth-v1:' + role + ':' + client_nonce + ':' + server_nonce).encode('ascii')
    return hmac.digest(bytes.fromhex(token), message, 'sha256').hex()


def trusted_extension(origin, allowed):
    return isinstance(origin, str) and origin in allowed


def approve_local_action(action, details):
    """Require an explicit local human decision for privileged operations."""
    message = action + '\n\n' + json.dumps(details, ensure_ascii=True, indent=2) + '\n\nAllow this operation?'
    if len(message) > 12000:
        return False
    try:
        if os.name == 'nt':
            import ctypes
            flags = 0x00000004 | 0x00000030 | 0x00000100 | 0x00010000
            return ctypes.windll.user32.MessageBoxW(None, message, 'Snapgent - Security approval', flags) == 6
        if not sys.stdin.isatty():
            return False
        return input(message + ' [y/N]: ').strip().lower() == 'y'
    except (OSError, EOFError, RuntimeError):
        return False


def authorized_http(header, expected):
    if not isinstance(header, str):
        return False
    scheme, _, value = header.partition(' ')
    if scheme.lower() == 'bearer':
        return token_matches(expected, value)
    if scheme.lower() == 'basic':
        try:
            decoded = base64.b64decode(value, validate=True).decode('ascii')
            username, separator, password = decoded.partition(':')
            return bool(separator) and username == 'snapgent' and token_matches(expected, password)
        except (ValueError, UnicodeError):
            return False
    return False
