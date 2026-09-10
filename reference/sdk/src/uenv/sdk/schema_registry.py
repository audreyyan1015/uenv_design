"""Offline package-extensible validation; no implicit network schema fetch."""
from copy import deepcopy
import hashlib
import json
from pathlib import Path
from jsonschema import Draft202012Validator, ValidationError
from referencing import Registry, Resource


def canonical_bytes(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':'), allow_nan=False).encode()


class SchemaRegistry:
    def __init__(self, contract_path=None):
        path = contract_path or Path(__file__).resolve().parents[5]/'contracts/uenv.schema.json'
        self.core = json.loads(Path(path).read_text(encoding='utf-8'))
        self.extensions = {}

    @classmethod
    def bundled(cls):
        registry = cls()
        design_root = Path(__file__).resolve().parents[5]
        for path in sorted((design_root/'contracts/extensions').glob('*.schema.json')):
            registry.register(json.loads(path.read_text(encoding='utf-8')))
        for path in sorted((design_root/'reference/generated/packages').glob('*/schemas/*.schema.json')):
            registry.register(json.loads(path.read_text(encoding='utf-8')))
        return registry

    def register(self, schema):
        schema = deepcopy(schema)
        Draft202012Validator.check_schema(schema)
        identifier = schema.get('$id')
        if not isinstance(identifier, str) or not identifier or identifier == self.core['$id']:
            raise ValueError('Extension requires a unique explicit $id')
        if identifier in self.extensions and canonical_bytes(self.extensions[identifier]) != canonical_bytes(schema):
            raise ValueError('Schema ID already registered with different content')
        self.extensions[identifier] = schema

    def register_artifact(self, reference, content):
        if len(content) != reference['size_bytes'] or 'sha256:'+hashlib.sha256(content).hexdigest() != reference['digest']:
            raise ValueError('Schema artifact integrity mismatch')
        self.register(json.loads(content))

    def bound_schema(self):
        core = deepcopy(self.core)
        core['$defs']['TypedConfig'] = {'oneOf': [
            {'type':'object', 'properties':{'schema_ref':{'const':identifier}, 'data':{'$ref':identifier}},
             'required':['schema_ref','data'], 'additionalProperties':False}
            for identifier in sorted(self.extensions)]} if self.extensions else False
        return core

    def validate(self, name, value):
        canonical_bytes(value)
        core = self.bound_schema()
        registry = Registry().with_resource(core['$id'], Resource.from_contents(core))
        for identifier, schema in self.extensions.items():
            registry = registry.with_resource(identifier, Resource.from_contents(schema))
        if name in core['$defs']:
            target = {'$ref':core['$id']+'#/$defs/'+name}
        elif name in self.extensions:
            target = {'$ref':name}
        else:
            raise ValidationError('Unknown schema: '+name)
        Draft202012Validator(target, registry=registry).validate(value)
        if name == 'ScoreResult':
            names = [metric['name'] for metric in value['metrics']]
            if len(names) != len(set(names)):
                raise ValidationError('Duplicate metric names')
