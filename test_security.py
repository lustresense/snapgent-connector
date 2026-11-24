import asyncio
import json
import unittest
import os
import tempfile
from pathlib import Path
import vscode_mcp
from unittest.mock import patch
import bridge
from bridge_security import fresh_nonce, authentication_proof, authorized_http

TOKEN = 'ab' * 32
ORIGIN = 'chrome-extension://' + 'a' * 32

class SecurityTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.manager = bridge.Bridge(auth_token=TOKEN, trusted_origins=(ORIGIN,))
        self.server = await bridge.websockets.serve(self.manager.handle_client, '127.0.0.1', 0)
        self.url = 'ws://127.0.0.1:' + str(self.server.sockets[0].getsockname()[1])

    async def asyncTearDown(self):
        self.server.close()
        await self.server.wait_closed()

    async def challenge(self, client):
        nonce = fresh_nonce()
        await client.send(json.dumps({'type': 'auth_hello', 'nonce': nonce}))
        challenge = json.loads(await client.recv())
        self.assertEqual(challenge['proof'], authentication_proof(TOKEN, 'server', nonce, challenge['nonce']))
        return authentication_proof(TOKEN, 'client', nonce, challenge['nonce'])

    async def assert_closed(self, client):
        with self.assertRaises(bridge.websockets.exceptions.ConnectionClosed) as caught:
            await asyncio.wait_for(client.recv(), 6)
        self.assertEqual(caught.exception.rcvd.code, 1008)

    async def test_unauthenticated_rpc_denied(self):
        async with bridge.websockets.connect(self.url) as client:
            await client.send(json.dumps({'type': 'list_tools', 'id': 1}))
            await self.assert_closed(client)

    async def test_wrong_proof_denied(self):
        async with bridge.websockets.connect(self.url, origin=ORIGIN) as client:
            await self.challenge(client)
            await client.send(json.dumps({'type': 'authenticate', 'proof': '00' * 32}))
            await self.assert_closed(client)

    async def test_unknown_extension_denied(self):
        async with bridge.websockets.connect(self.url, origin='chrome-extension://' + 'b' * 32) as client:
            await self.assert_closed(client)

    async def test_replay_denied(self):
        async with bridge.websockets.connect(self.url, origin=ORIGIN) as client:
            proof = await self.challenge(client)
            await client.send(json.dumps({'type': 'authenticate', 'proof': proof}))
            self.assertEqual(json.loads(await client.recv())['type'], 'authenticated')
            self.assertEqual(json.loads(await client.recv())['type'], 'connected')
        async with bridge.websockets.connect(self.url, origin=ORIGIN) as client:
            await self.challenge(client)
            await client.send(json.dumps({'type': 'authenticate', 'proof': proof}))
            await self.assert_closed(client)

    async def test_http_requires_authentication(self):
        from aiohttp import ClientSession, web
        runner = web.AppRunner(bridge._build_stream_app(bridge.EventHub(), auth_token=TOKEN), access_log=None)
        await runner.setup()
        site = web.TCPSite(runner, '127.0.0.1', 0)
        await site.start()
        url = 'http://127.0.0.1:' + str(site._server.sockets[0].getsockname()[1])
        try:
            async with ClientSession() as session:
                for path in ('/', '/status', '/events'):
                    async with session.get(url + path) as response:
                        self.assertEqual(response.status, 401)
                        self.assertNotIn('Access-Control-Allow-Origin', response.headers)
                async with session.get(url + '/status', headers={'Authorization': 'Bearer ' + TOKEN}) as response:
                    self.assertEqual(response.status, 200)
        finally:
            await runner.cleanup()

    def test_workspace_lock_survives_process_state_reset(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            first, second = root / 'first', root / 'second'
            first.mkdir()
            second.mkdir()
            with patch.dict(os.environ, {'ZS_WORKSPACE_LOCK_FILE': str(root / 'workspace.lock')}), patch.object(vscode_mcp, '_locked_workspace', None):
                self.assertEqual(vscode_mcp.lock_workspace(first), first.resolve())
                vscode_mcp._locked_workspace = None
                with self.assertRaises(RuntimeError):
                    vscode_mcp.lock_workspace(second)
                self.assertEqual(vscode_mcp.lock_workspace(first), first.resolve())

    @unittest.skipUnless(os.name == 'nt', 'Windows path restrictions')
    def test_windows_device_and_stream_paths_denied(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            for name in ('file.txt:secret', 'NUL', 'CON.txt', 'folder/COM1'):
                with self.subTest(name=name), self.assertRaises(ValueError):
                    vscode_mcp.normalize_target(name, root)

    def test_malformed_credentials(self):
        for header in (None, '', 'Basic !!!', 'Bearer wrong', 'Bearer ' + 'Ã©' * 64):
            self.assertFalse(authorized_http(header, TOKEN))
        self.assertTrue(authorized_http('Bearer ' + TOKEN, TOKEN))

if __name__ == '__main__':
    unittest.main()
