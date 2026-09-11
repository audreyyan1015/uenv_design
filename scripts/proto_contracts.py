"""Build generated transports and JSON validators from proto field annotations."""
import importlib
import json
from pathlib import Path
import subprocess
import shutil
import os
import sys
from google.protobuf import descriptor_pb2
import grpc_tools


def load_contracts(root):
    source = root / 'contracts/proto'
    generated = root / 'contracts/generated/python'
    generated.mkdir(parents=True, exist_ok=True)
    descriptor = root / 'contracts/generated/uenv.pb'
    sources = sorted(source.glob('uenv/v1/*.proto'))
    env = dict(os.environ)
    env['PYTHONPATH'] = os.pathsep.join([str(root / '.dependencies'), env.get('PYTHONPATH', '')])
    subprocess.run([sys.executable, '-m', 'grpc_tools.protoc',
        '-I' + str(source), '-I' + str(Path(grpc_tools.__file__).parent / '_proto'),
        '--python_out=' + str(generated), '--grpc_python_out=' + str(generated),
        '--descriptor_set_out=' + str(descriptor), '--include_imports',
        *[p.relative_to(source).as_posix() for p in sources]], cwd=source, env=env, check=True)
    sdk = root / 'reference/sdk/src/uenv/v1'
    sdk.mkdir(parents=True, exist_ok=True)
    (sdk / '__init__.py').write_text('"""Generated from contracts/proto; do not edit."""\n', encoding='utf-8')
    for path in (generated / 'uenv/v1').glob('*_pb2.py'):
        shutil.copyfile(path, sdk / path.name)
    sys.path.insert(0, str(generated))
    if 'uenv' in sys.modules:
        sys.modules['uenv'].__path__.append(str(generated / 'uenv'))
    annotations = importlib.import_module('uenv.v1.annotations_pb2')
    files = descriptor_pb2.FileDescriptorSet.FromString(descriptor.read_bytes())
    primitive = {1: 'number', 2: 'number', 3: 'integer', 4: 'integer', 5: 'integer',
                 8: 'boolean', 9: 'string', 13: 'integer', 18: 'integer'}
    definitions, configs = {}, []
    for file in files.file:
        if file.package != 'uenv.v1':
            continue
        for message in file.message_type:
            rules = json.loads(message.options.Extensions[annotations.message_rule] or '{}')
            if not message.field:
                # EmptyConfig is an empty object; TypedConfig's abstract
                # envelope constraints are supplied by the registered schemas.
                if 'oneOf' in rules:
                    definitions[message.name] = rules
                    continue
            properties, required = {}, []
            for field in message.field:
                if field.type in primitive:
                    item = {'type': primitive[field.type]}
                elif field.type_name == '.google.protobuf.Struct':
                    item = {'type': 'object'}
                elif field.type_name == '.google.protobuf.Value':
                    item = {}
                else:
                    item = {'$ref': '#/$defs/' + field.type_name.rsplit('.', 1)[-1]}
                schema = {'type': 'array', 'items': item} if field.label == field.LABEL_REPEATED else item
                constraints = json.loads(field.options.Extensions[annotations.field_rule] or '{}')
                if field.label == field.LABEL_REPEATED and 'items' in constraints:
                    schema['items'].update(constraints.pop('items'))
                schema.update(constraints)
                properties[field.name] = schema
                if field.options.Extensions[annotations.required]:
                    required.append(field.name)
            definitions[message.name] = {'type': 'object', **rules,
                                         'properties': properties, 'required': required}
            if message.options.Extensions[annotations.extension_schema]:
                configs.append(message.name)
    return definitions, configs
