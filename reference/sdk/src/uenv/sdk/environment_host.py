"""Environment and sandbox tools share one Backend-managed Python process."""
import asyncio
from copy import deepcopy
import time
import threading
from types import SimpleNamespace
from . import EnvironmentContext, task_from_wire, to_wire
from .rpc import RpcError
from .tools import ToolHost


class EnvironmentHost:
    def __init__(self, package, component, registry, session, artifacts, definitions=None):
        # Bootstrap has verified and installed the exact package before imports.
        self.package, self.component, self.registry = package, component, registry
        self.session, self.artifacts = session, artifacts
        self.definitions = definitions or {}
        self.environment = None
        self.context = None
        self.tools = ToolHost(registry)
        self.loop = asyncio.new_event_loop()
        self.loop_lock = threading.RLock()
        self._reset = False
        self._closed = False

    def bind(self, peer):
        return {'environment.prepare': self.prepare, 'environment.reset': self.reset,
                'environment.state_snapshot': self.state_snapshot,
                'tools.prepare': self.prepare_tools, 'tools.validate': self.validate_tool,
                'tools.execute': self.execute_tool, 'tools.freeze': self.freeze,
                'host.close': self.close}

    def prepare(self, params):
        if self.environment is not None or params['dataset_package'] != self.component:
            raise RpcError('ENVIRONMENT_PACKAGE_MISMATCH')
        if params['session'] != self.session.identity:
            raise RpcError('ENVIRONMENT_SESSION_MISMATCH')
        config = params['environment']
        expected = self.package.get('environment_config_model')
        expected = expected.__schema_id__ if expected else 'uenv://schemas/vnext/EmptyConfig'
        if config['schema_ref'] != expected:
            raise RpcError('ENVIRONMENT_CONFIG_SCHEMA_MISMATCH')
        self.registry.validate('TypedConfig', config)
        self.environment = self.package['environment'](config['data'])

    def _context(self, remaining, seed=None):
        seed = self.context.seed if seed is None and self.context else seed
        self.context = EnvironmentContext(self.registry, self.artifacts, lambda: False,
            time.monotonic() + remaining / 1000, seed, self.session)
        return self.context

    def reset(self, params):
        if self.environment is None or self._reset:
            raise RpcError('ENVIRONMENT_RESET_STATE_INVALID')
        self.registry.validate('TaskSpec', params['task'])
        task = task_from_wire(params['task'], self.package['input_model'])
        self._reset = True
        result = to_wire(self.environment.reset(task, self._context(params['remaining_timeout_ms'], params['seed'])))
        self.registry.validate('Observation', result)
        return result

    def prepare_tools(self, params):
        if self.environment is None or params['session'] != self.session.identity:
            raise RpcError('TOOL_SESSION_MISMATCH')
        if any(b['execution_scope'] != 'sandbox' or 'native_agent' in b for b in params['tools']):
            raise RpcError('TOOL_EXECUTION_SCOPE_MISMATCH')
        contexts = {b['name']: SimpleNamespace(config=deepcopy(b['config']['data']),
            environment=self.environment, session=self.session, freeze=self.session.freeze_tools)
            for b in params['tools']}
        return self.tools.prepare(params['tools'], self.definitions, contexts, {})

    def validate_tool(self, params):
        self.tools.validate_call(params['binding'], params['call'])

    def execute_tool(self, params):
        self.validate_tool(params)
        with self.loop_lock:
            return self.loop.run_until_complete(self.tools.execute(params['call']))

    def freeze(self, params):
        with self.loop_lock:
            self.loop.run_until_complete(self.tools.freeze())
            self.session.freeze_tools()

    def state_snapshot(self, params):
        if not self.tools.frozen:
            raise RpcError('TOOLS_NOT_FROZEN')
        state = self.environment.state_snapshot(self._context(params['remaining_timeout_ms']))
        result = to_wire(state)
        if result is not None:
            expected = self.package.get('state_model')
            if expected is None or not isinstance(state, expected):
                raise RpcError('ENVIRONMENT_STATE_SCHEMA_MISMATCH')
            self.registry.validate('TypedConfig', result)
        return result

    def close(self, params=None):
        if self._closed:
            return None
        try:
            self.loop.run_until_complete(self.tools.freeze())
        finally:
            try:
                if self.environment is not None:
                    self.environment.close(self._context(5000, seed=self.context.seed if self.context else 0))
            finally:
                self._closed = True
                self.loop.close()
