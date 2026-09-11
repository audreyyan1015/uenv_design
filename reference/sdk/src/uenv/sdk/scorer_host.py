"""Only this role receives private scoring materials and scoring callbacks."""
import time
from . import ScoreInput, ScoringContext, task_from_wire, model_from_envelope, to_wire
from .rpc import RpcError


class ScorerHost:
    def __init__(self, package, component, registry):
        self.package, self.component, self.registry = package, component, registry
        self.called = False
        self.closed = False

    def bind(self, peer):
        self.peer = peer
        return {'scorer.score': self.score, 'host.close': self.close}

    def score(self, params):
        if self.called or self.closed or params['dataset_package'] != self.component:
            raise RpcError('SCORER_CALL_STATE_INVALID')
        config, request = params['config'], params['request']
        model = self.package.get('scorer_config_model')
        if config['schema_ref'] != (model.__schema_id__ if model else 'uenv://schemas/vnext/EmptyConfig'):
            raise RpcError('SCORER_CONFIG_SCHEMA_MISMATCH')
        self.registry.validate('TypedConfig', config)
        self.registry.validate('ScoreInput', request)
        values = {'task': task_from_wire(request['task'], self.package['input_model']),
                  'final_answer': request['final_answer'], 'trajectory_ref': request['trajectory_ref']}
        for field in ('private_data', 'state'):
            if field in request:
                model = self.package.get(field + '_model')
                if model is None:
                    raise RpcError('SCORER_INPUT_SCHEMA_MISMATCH')
                values[field] = model_from_envelope(model, request[field])
        self.called = True
        context = ScoringContext(time.monotonic() + params['remaining_timeout_ms']/1000,
            self.registry, lambda: False,
            run_harness=lambda value: self.peer.request('scoring.run_harness', value),
            read_artifact=lambda ref: bytes(self.peer.request('scoring.read_artifact', ref)))
        return to_wire(self.package['scorer'](config['data']).score(ScoreInput(**values), context))

    def close(self, params=None):
        self.closed = True
