"""Real role hosts for the cross-process pipeline test; only LLM is scripted.

This verifies contracts and process routing, not Linux sandbox isolation.
"""
import json
import os
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import threading
from uenv.sdk import SchemaRegistry, Session, tool
from uenv.sdk.component_host import ComponentHost
from uenv.sdk.environment_host import EnvironmentHost
from uenv.sdk.scorer_host import ScorerHost
from uenv.sdk.model_host import ModelHost
from package_loader import load_package


ROOT = Path(__file__).resolve().parents[3]


def package():
    return load_package(ROOT / 'reference/datasets/gsm8k')


def identity():
    # This fixture contains no task or private scoring data.
    return json.loads(os.environ['UENV_TEST_COMPONENT'])


@tool
def add(left: int, right: int, *, context) -> str:
    # Any accidental execution in Agent/Model Host fails: only the environment
    # role receives a Session and the actual dataset instance.
    assert context.session.identity['session_id'] == 'rpc-pipeline'
    assert context.environment is not None
    context.session.write_file('sum.txt', str(left + right).encode())
    return context.session.read_file('sum.txt').decode()


def create_environment():
    workspace = os.environ['UENV_TEST_WORKSPACE']
    session = Session({'session_id': 'rpc-pipeline', 'backend': json.loads(os.environ['UENV_TEST_BACKEND'])}, workspace)
    return EnvironmentHost(package(), identity(), SchemaRegistry.bundled(), session, None, {'tools/add': add})


def create_scorer():
    return ScorerHost(package(), identity(), SchemaRegistry.bundled())


def create_agent():
    return ComponentHost(SchemaRegistry.bundled(), {'tools/add': add})


def create_model():
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_POST(self):
            request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            assert request['model'] == 'example'
            assert 'private_data' not in json.dumps(request)
            message, reason = {'content': '5'}, 'stop'
            if request.get('tools') and not any(m['role'] == 'tool' for m in request['messages']):
                message = {'content': 'calculate', 'tool_calls': [{'id': 'call-1', 'type': 'function',
                    'function': {'name': 'add', 'arguments': '{"left":2,"right":3}'}}]}
                reason = 'tool_calls'
            data = json.dumps({'choices': [{'message': message, 'finish_reason': reason}],
                               'usage': {'completion_tokens': 1}}).encode()
            self.send_response(200)
            self.send_header('Content-Length', str(len(data)))
            self.end_headers()
            self.wfile.write(data)

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()

    class FixtureModel(ModelHost):
        def bind(self, peer):
            return {**super().bind(peer), 'fixture.endpoint': lambda _: f'http://127.0.0.1:{server.server_port}/v1'}

        def close(self, params=None):
            server.shutdown()
            server.server_close()
            thread.join(timeout=3)

    return FixtureModel(SchemaRegistry.bundled(), {'add': add})
