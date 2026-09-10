"""Small typed-model layer used by dataset authors in the executable design.

The editable source is the Python class. JSON Schema and the internal
``schema_ref/data`` envelope are generated at publication and transport
boundaries, respectively.
"""
from __future__ import annotations

from copy import deepcopy
from dataclasses import dataclass
from types import UnionType
from typing import Annotated, Any, ClassVar, Literal, Union, get_args, get_origin, get_type_hints


@dataclass(frozen=True)
class Field:
    description: str
    minimum: int | float | None = None
    min_length: int | None = None


class _ExternalSchema:
    __schema_ref__: ClassVar[str]


class ArtifactRef(_ExternalSchema):
    __schema_ref__ = "urn:uenv:vnext:contracts#/$defs/ArtifactRef"


class ContentPart(_ExternalSchema):
    __schema_ref__ = "urn:uenv:vnext:contracts#/$defs/ContentPart"


class EvaluationPlan(_ExternalSchema):
    __schema_ref__ = "uenv://schemas/vnext/EvaluationPlan"


class UEnvModel:
    """Base for package-owned data with strict fields and generated schema."""

    __schema_id__: ClassVar[str | None] = None

    def __init__(self, **values: Any):
        hints = _model_hints(type(self))
        unknown = set(values) - set(hints)
        if unknown:
            raise TypeError(f"Unknown fields for {type(self).__name__}: {sorted(unknown)}")
        for name, annotation in hints.items():
            if name in values:
                value = values[name]
            elif hasattr(type(self), name):
                value = deepcopy(getattr(type(self), name))
            else:
                raise TypeError(f"Missing field for {type(self).__name__}: {name}")
            object.__setattr__(self, name, _restore(annotation, value))

    def __getitem__(self, name: str) -> Any:
        return getattr(self, name)

    def model_dump(self) -> dict[str, Any]:
        return {name: _dump(getattr(self, name)) for name in _model_hints(type(self))}


def _model_hints(model: type[UEnvModel]) -> dict[str, Any]:
    return {
        name: annotation
        for name, annotation in get_type_hints(model, include_extras=True).items()
        if get_origin(annotation) is not ClassVar and not name.startswith("__")
    }


def _dump(value: Any) -> Any:
    if isinstance(value, UEnvModel):
        return value.model_dump()
    if isinstance(value, list):
        return [_dump(item) for item in value]
    if isinstance(value, tuple):
        return [_dump(item) for item in value]
    if isinstance(value, dict):
        return {key: _dump(item) for key, item in value.items()}
    return deepcopy(value)


def _restore(annotation: Any, value: Any) -> Any:
    """Restore nested package models after the transport schema was validated."""
    if get_origin(annotation) is Annotated:
        annotation = get_args(annotation)[0]
    origin, args = get_origin(annotation), get_args(annotation)
    if origin in (Union, UnionType):
        candidates = [item for item in args if item is not type(None)]
        if value is None:
            return None
        if len(candidates) == 1:
            return _restore(candidates[0], value)
    if isinstance(annotation, type) and issubclass(annotation, UEnvModel):
        return annotation(**value) if isinstance(value, dict) else deepcopy(value)
    if origin is list and isinstance(value, list):
        return [_restore(args[0], item) for item in value]
    if origin is dict and isinstance(value, dict):
        return {key: _restore(args[1], item) for key, item in value.items()}
    return deepcopy(value)


def bind_schema(model: type[UEnvModel], schema_id: str) -> None:
    if not isinstance(model, type) or not issubclass(model, UEnvModel):
        raise TypeError("Package model must inherit UEnvModel")
    existing = model.__dict__.get("__schema_id__")
    if existing not in (None, schema_id):
        raise ValueError(f"Model already bound to another schema: {model.__name__}")
    model.__schema_id__ = schema_id


def model_to_envelope(value: UEnvModel) -> dict[str, Any]:
    schema_id = type(value).__schema_id__
    if not schema_id:
        raise ValueError(f"Model is not bound to a published schema: {type(value).__name__}")
    return {"schema_ref": schema_id, "data": value.model_dump()}


def model_from_envelope(model: type[UEnvModel], value: dict[str, Any]) -> UEnvModel:
    if value.get("schema_ref") != model.__schema_id__:
        raise ValueError(f"Schema does not match {model.__name__}")
    return model(**deepcopy(value["data"]))


def model_json_schema(model: type[UEnvModel], schema_id: str) -> dict[str, Any]:
    bind_schema(model, schema_id)
    return {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": schema_id,
        **_model_schema(model, ()),
    }


def _model_schema(model: type[UEnvModel], ancestors: tuple[type, ...]) -> dict[str, Any]:
    if model in ancestors:
        raise TypeError("Recursive package models are not supported by the reference generator")
    properties: dict[str, Any] = {}
    required: list[str] = []
    for name, annotation in _model_hints(model).items():
        properties[name] = _annotation_schema(annotation, (*ancestors, model))
        if not hasattr(model, name):
            required.append(name)
        else:
            properties[name]["default"] = _dump(getattr(model, name))
    return {
        "title": model.__name__,
        "description": (model.__doc__ or "").strip(),
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": False,
    }


def _annotation_schema(annotation: Any, ancestors: tuple[type, ...] = ()) -> dict[str, Any]:
    metadata: list[Any] = []
    if get_origin(annotation) is Annotated:
        annotation, *metadata = get_args(annotation)
    origin = get_origin(annotation)
    args = get_args(annotation)

    if annotation is str:
        schema: dict[str, Any] = {"type": "string"}
    elif annotation is int:
        schema = {"type": "integer"}
    elif annotation is float:
        schema = {"type": "number"}
    elif annotation is bool:
        schema = {"type": "boolean"}
    elif origin is Literal:
        values = list(args)
        scalar = type(values[0]) if values else str
        schema = {"type": {str: "string", int: "integer", bool: "boolean"}.get(scalar, "string"), "enum": values}
    elif origin is list:
        schema = {"type": "array", "items": _annotation_schema(args[0], ancestors)}
    elif origin is dict:
        if args[0] is not str:
            raise TypeError("JSON object keys must be strings")
        schema = {"type": "object", "additionalProperties": _annotation_schema(args[1], ancestors)}
    elif origin in (Union, UnionType):
        non_null = [item for item in args if item is not type(None)]
        if len(non_null) == 1 and len(non_null) != len(args):
            schema = {"anyOf": [_annotation_schema(non_null[0], ancestors), {"type": "null"}]}
        else:
            raise TypeError("Reference models support optional types, not ambiguous unions")
    elif isinstance(annotation, type) and issubclass(annotation, _ExternalSchema):
        schema = {"$ref": annotation.__schema_ref__}
    elif isinstance(annotation, type) and issubclass(annotation, UEnvModel):
        schema = _model_schema(annotation, ancestors)
    else:
        raise TypeError(f"Unsupported model annotation: {annotation!r}")

    for item in metadata:
        field = item if isinstance(item, Field) else Field(str(item))
        if field.description:
            schema["description"] = field.description
        if field.minimum is not None:
            schema["minimum"] = field.minimum
        if field.min_length is not None:
            schema["minLength"] = field.min_length
    return schema
