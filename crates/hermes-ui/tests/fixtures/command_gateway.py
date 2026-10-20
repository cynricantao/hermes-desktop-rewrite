"""Deterministic gateway simulation, NOT a Hermes backend or model.

Only loopback is bound. Implements enough JSON-RPC and discovery HTTP for the
real Rust controller/composer tests. It never reads Hermes files or runs tools.
"""
import base64
import hashlib
import json
import socketserver
import struct
import threading
from urllib.parse import parse_qs, urlsplit

LOCK = threading.RLock()
CLIENTS = []
CALLS = []
SESSIONS = 0
PENDING_FAILURES = {}
REJECT_NEXT_CREATE = False
PENDING_CREATE_FAILURE = None
CREATE_REJECTION = threading.Event()


def frame(conn, value):
    body = json.dumps(value).encode()
    header = bytes([0x81])
    if len(body) < 126:
        header += bytes([len(body)])
    elif len(body) < 65536:
        header += bytes([126]) + struct.pack('!H', len(body))
    else:
        header += bytes([127]) + struct.pack('!Q', len(body))
    with LOCK:
        conn.sendall(header + body)


def event(kind, sid, payload):
    with LOCK:
        for conn in CLIENTS[:]:
            try:
                frame(conn, {'jsonrpc': '2.0', 'method': 'event', 'params': {
                    'type': kind, 'session_id': sid, 'payload': payload}})
            except OSError:
                CLIENTS.remove(conn)


ASKS = 0
QUESTIONS = {
    'clarify': [
        {'qid': 'q0', 'question': 'Which scope?', 'choices': ['Small (Recommended)', 'Large'], 'multi_select': False},
        {'qid': 'q1', 'question': 'Which parts?', 'choices': ['A', 'B', 'C'], 'multi_select': True},
        {'qid': 'q2', 'question': 'Anything else?', 'choices': None, 'multi_select': False},
    ],
    'clarify-one': [{'qid': 'q0', 'question': 'Proceed?', 'choices': ['Yes', 'No'], 'multi_select': False}],
    'clarify-empty': [{'qid': '', 'question': ''}],
}


def ask(kind, sid):
    """Send one server->client request to every connected client; return its id."""
    global ASKS
    with LOCK:
        ASKS += 1
        rid = f'srq-fixture{ASKS}'
    if kind.startswith('clarify'):
        method, params = 'clarify', {'questions': QUESTIONS[kind]}
    elif kind == 'sudo':
        method, params = 'sudo', {'command': 'sudo pacman -S fixture'}
    else:
        method, params = 'secret', {'prompt': 'Fixture API key', 'env_var': 'FIXTURE_KEY'}
    with LOCK:
        for conn in CLIENTS[:]:
            frame(conn, {'jsonrpc': '2.0', 'id': rid, 'method': method, 'params': {'session_id': sid, **params}})
    return rid


def exact(stream, n):
    data = stream.read(n)
    if len(data) != n:
        raise EOFError()
    return data


class Handler(socketserver.StreamRequestHandler):
    def handle(self):
        global SESSIONS, REJECT_NEXT_CREATE, PENDING_CREATE_FAILURE
        request = self.rfile.readline().decode().split()
        if len(request) < 2:
            return
        headers = {}
        while (line := self.rfile.readline()) not in (b'\r\n', b''):
            key, value = line.decode().split(':', 1)
            headers[key.lower()] = value.strip()
        if headers.get('upgrade', '').lower() != 'websocket':
            url = urlsplit(request[1])
            if url.path == '/api/status':
                # Deliberately differs from the app's HERMES_HOME. Recovery must
                # use the attached backend's identity, not the launch environment.
                body = json.dumps({'hermes_home': '/fixture/backend'}).encode()
            elif url.path == '/api/test/state':
                with LOCK:
                    body = json.dumps(CALLS).encode()
            elif url.path == '/api/test/reject-create':
                with LOCK:
                    REJECT_NEXT_CREATE = True
                    CREATE_REJECTION.clear()
                body = b'{}'
            elif url.path == '/api/test/wait-create':
                body = json.dumps({'pending': CREATE_REJECTION.wait(8)}).encode()
            elif url.path == '/api/test/fail-create':
                with LOCK:
                    pending, PENDING_CREATE_FAILURE = PENDING_CREATE_FAILURE, None
                if pending:
                    conn, request_id = pending
                    frame(conn, {'jsonrpc': '2.0', 'id': request_id, 'error': {
                        'code': 5031, 'message': 'Simulated session creation failure'}})
                body = b'{}'
            elif url.path == '/api/test/finish':
                sid = parse_qs(url.query)['sid'][0]
                status = parse_qs(url.query).get('status', ['complete'])[0]
                event('message.complete', sid, {'text': 'Simulated reply complete.', 'status': status})
                body = b'{}'
            elif url.path == '/api/test/fail':
                sid = parse_qs(url.query)['sid'][0]
                with LOCK:
                    conn, request_id = PENDING_FAILURES.pop(sid)
                frame(conn, {'jsonrpc': '2.0', 'id': request_id, 'error': {'code': 4001, 'message': 'Simulated late submission failure'}})
                body = b'{}'
            elif url.path == '/api/test/ask':
                q = parse_qs(url.query)
                body = json.dumps({'id': ask(q['kind'][0], q['sid'][0])}).encode()
            elif url.path == '/api/test/cancel':
                q = parse_qs(url.query)
                event('request.cancel', q['sid'][0], {'id': q['id'][0], 'method': 'clarify', 'reason': 'interrupted'})
                body = b'{}'
            elif url.path == '/api/test/side':
                sid = parse_qs(url.query)['sid'][0]
                event('background.complete', sid, {'task_id': 'bg_fixture', 'text': 'Simulated background answer.'})
                body = b'{}'
            else:
                body = b'<script>window.__HERMES_SESSION_TOKEN__ = "fixture";</script>' if url.path == '/' else b'{}'
            self.wfile.write(b'HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: ' + str(len(body)).encode() + b'\r\n\r\n' + body)
            return
        digest = hashlib.sha1((headers['sec-websocket-key'] + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()
        self.wfile.write(b'HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ' + base64.b64encode(digest) + b'\r\n\r\n')
        self.wfile.flush()
        with LOCK:
            CLIENTS.append(self.request)
        try:
            while True:
                first, second = exact(self.rfile, 2)
                if first & 15 == 8:
                    break
                size = second & 127
                if size == 126:
                    size = struct.unpack('!H', exact(self.rfile, 2))[0]
                elif size == 127:
                    size = struct.unpack('!Q', exact(self.rfile, 8))[0]
                if size > 2_000_000:
                    raise ValueError('fixture frame too large')
                mask = exact(self.rfile, 4) if second & 128 else bytes(4)
                data = bytes(c ^ mask[i % 4] for i, c in enumerate(exact(self.rfile, size)))
                msg = json.loads(data)
                if 'method' not in msg:
                    # The client's answer to a server->client request.
                    with LOCK:
                        CALLS.append({'method': '<response>', 'params': msg})
                    continue
                method, params = msg['method'], msg.get('params', {})
                with LOCK:
                    CALLS.append({'method': method, 'params': params})
                result, error = {}, None
                if method == 'session.create':
                    with LOCK:
                        if REJECT_NEXT_CREATE:
                            REJECT_NEXT_CREATE = False
                            PENDING_CREATE_FAILURE = (self.request, msg['id'])
                            CREATE_REJECTION.set()
                            continue
                        SESSIONS += 1
                        sid = f's{SESSIONS}'
                    result = {'session_id': sid, 'stored_session_id': 'stored-' + sid}
                elif method == 'session.resume' and params['session_id'] == 'stored-replay':
                    result = {'session_id': 'resumed-replay', 'stored_session_id': 'stored-replay', 'messages': [{'role': 'user', 'text': 'replayed chat'}],
                              'open_requests': [{'id': 'srq-replayed', 'method': 'clarify', 'params': {
                                  'session_id': 'resumed-replay', 'answers': {'q0': 'Small'}, 'questions': QUESTIONS['clarify'][:2]}}]}
                elif method == 'session.resume':
                    result = {'session_id': 'resumed-' + params['session_id'], 'stored_session_id': params['session_id'], 'messages': [{'role': 'user', 'text': 'PRIVATE_SKILL_BODY', 'display_kind': 'hidden'}, {'role': 'user', 'text': '/fixture-skill'}]}
                elif method == 'command.resolve':
                    name = params['name']
                    result = {'canonical': {'q': 'queue', 'background': 'bg', 'tasks': 'agents', 'reset': 'new'}.get(name, name)}
                elif method == 'complete.slash':
                    text = params.get('text', '')
                    names = ['help', 'queue', 'steer', 'bg', 'fixture-skill']
                    result = {'replace_from': 1, 'items': [{'text': n, 'display': '/' + n, 'meta': 'Simulated completion'} for n in names if n.startswith(text[1:])]}
                elif method == 'commands.catalog':
                    result = {'commands': {'/danger-fails': {'desktop': None}}, 'skills': {'/fixture-skill': {}}, 'pairs': [['/bg', 'Background task'], ['/fixture-skill', 'Simulated skill']]}
                elif method == 'model.options':
                    result = {'model': 'fixture-model', 'provider': 'fixture', 'providers': []}
                elif method == 'config.get':
                    result = {'value': 'fixture'}
                elif method == 'session.steer':
                    result = {'status': {'reject-steer': 'rejected', 'accepted-steer': 'redirected', 'unknown-steer': 'unexpected'}.get(params['text'], 'queued')}
                elif method == 'clipboard.paste':
                    result = {'attached': True, 'path': '/fixture/image.png', 'name': 'fixture.png', 'count': 1}
                elif method == 'prompt.submit' and params.get('text') == 'fail-later':
                    with LOCK:
                        PENDING_FAILURES[params['session_id']] = (self.request, msg['id'])
                    continue
                elif method in ('prompt.background', 'prompt.btw'):
                    result = {'task_id': 'bg_fixture'}
                elif method == 'slash.exec':
                    command = '/' + params.get('command', '').lstrip('/')
                    if command.startswith('/fixture-skill'):
                        error = {'code': 4018, 'message': 'skill command: use command.dispatch for /fixture-skill'}
                    elif command.startswith('/quick'):
                        error = {'code': -32601, 'message': 'missing'}
                    elif command.startswith('/danger-fails'):
                        error = {'code': 5030, 'message': 'simulated failure after execution'}
                    else:
                        result = {'output': 'Simulated command output.'}
                elif method == 'command.dispatch':
                    if params['name'] == 'fixture-skill':
                        result = {'type': 'skill', 'message': 'PRIVATE_SKILL_BODY', 'display': '/fixture-skill'}
                    elif params['name'] == 'quick':
                        result = {'type': 'alias', 'target': 'queue add'}
                    elif params['name'] == 'quick-loop':
                        result = {'type': 'alias', 'target': 'quick-loop'}
                    else:
                        error = {'code': 4011, 'message': 'Unknown fixture command'}
                reply = {'jsonrpc': '2.0', 'id': msg['id']}
                reply['error' if error else 'result'] = error or result
                frame(self.request, reply)
        except (EOFError, OSError):
            pass
        finally:
            with LOCK:
                if self.request in CLIENTS:
                    CLIENTS.remove(self.request)


class Server(socketserver.ThreadingTCPServer):
    daemon_threads = True
    allow_reuse_address = True


with Server(('127.0.0.1', 0), Handler) as server:
    print(server.server_address[1], flush=True)
    server.serve_forever()
