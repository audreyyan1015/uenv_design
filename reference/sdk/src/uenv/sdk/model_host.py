"""Trusted model transport process; never imports or runs an Agent."""
from .model_provider import OpenAIModelProvider


class ModelHost:
    def __init__(self, registry, definitions, credential_resolver=lambda ref: ''):
        self.registry = registry
        self.provider = OpenAIModelProvider(definitions, credential_resolver)

    def bind(self, peer):
        return {'model.generate': self.generate, 'host.close': self.close}

    def generate(self, command):
        self.registry.validate('ModelSpec', command['model'])
        for message in command['messages']:
            self.registry.validate('Message', message)
        for binding in command['tools']:
            self.registry.validate('ResolvedToolBinding', binding)
        return self.provider.generate(command)

    def close(self, params=None):
        return None
