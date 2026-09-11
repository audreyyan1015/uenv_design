"""Worker-invoked OpenAI-compatible model transport.

All execution configuration is read from the Worker command. Framework LLM
objects never choose an endpoint, sampling parameters, credentials or retries.
"""
import json
import time
from copy import deepcopy
from urllib.request import Request, urlopen
from .rpc import RpcError


def text_content(parts):
    if any(part["kind"] != "text" for part in parts):
        raise RpcError("MODEL_CONTENT_KIND_NOT_SUPPORTED")
    return "".join(part["text"] for part in parts)


class OpenAIModelProvider:
    def __init__(self, definitions, resolve_credential=lambda ref: ""):
        self.definitions = definitions
        self.resolve_credential = resolve_credential

    def generate(self, command):
        model = command["model"]
        messages = []
        for message in command["messages"]:
            value = {"role": message["role"], "content": text_content(message["content"])}
            if "tool_call_id" in message:
                value["tool_call_id"] = message["tool_call_id"]
            if message.get("tool_calls"):
                value["tool_calls"] = [{"id": call["tool_call_id"], "type": "function",
                    "function": {"name": call["name"], "arguments": json.dumps(call["arguments"])}}
                    for call in message["tool_calls"]]
            messages.append(value)
        generation = model["generation"]
        body = {"model": model["model_id"], "messages": messages,
                "temperature": generation["temperature"], "top_p": generation["top_p"],
                "max_tokens": generation["max_output_tokens"], "seed": command["seed"]}
        if generation["stop"]:
            body["stop"] = generation["stop"]
        if 'training' in command:
            # Trainer-owned endpoint uses this same request to enforce its
            # actual weight version and return token/version facts.
            body['training'] = deepcopy(command['training'])
        bindings = {item["name"]: item for item in command["tools"]}
        if bindings:
            body["tools"] = [{"type": "function", "function": {"name": name,
                "description": self.definitions[name].description,
                "parameters": self.definitions[name].input_schema}}
                for name in bindings]
        headers = {"Content-Type": "application/json"}
        credential = self.resolve_credential(model["credential_ref"])
        if model["credential_ref"] and not credential:
            raise RpcError("MODEL_CREDENTIAL_NOT_FOUND")
        if credential:
            headers["Authorization"] = "Bearer " + credential
        request = Request(model["endpoint"].rstrip("/") + "/chat/completions",
                          data=json.dumps(body, allow_nan=False).encode(), headers=headers)
        started = time.monotonic()
        # POST is not retried after an uncertain failure: it could generate twice.
        if model["max_transport_retries"] != 0:
            raise RpcError("MODEL_RETRIES_NOT_SUPPORTED")
        with urlopen(request, timeout=command["remaining_timeout_ms"] / 1000) as response:
            raw = response.read(8 * 1024 * 1024 + 1)
        if len(raw) > 8 * 1024 * 1024:
            raise RpcError("MODEL_RESPONSE_LIMIT")
        data = json.loads(raw)
        choice = data["choices"][0]
        message = choice["message"]
        if message.get("reasoning_content") or message.get("refusal"):
            raise RpcError("MODEL_RESPONSE_KIND_NOT_SUPPORTED")
        content = message.get("content") or ""
        if not isinstance(content, str):
            raise RpcError("MODEL_CONTENT_KIND_NOT_SUPPORTED")
        result = {"generation_id": command["generation_id"], "model_id": model["model_id"],
            "source": model["source"], "messages": deepcopy(command["messages"]),
            "response": [{"kind": "text", "text": content}] if content else [],
            "output_token_count": data["usage"]["completion_tokens"],
            "finish_reason": choice["finish_reason"], "duration_ms": int((time.monotonic()-started)*1000)}
        if message.get("tool_calls"):
            calls = []
            for item in message["tool_calls"]:
                function = item["function"]
                binding = bindings.get(function["name"])
                if binding is None:
                    raise RpcError("TOOL_NOT_SELECTED")
                calls.append({"tool_call_id": item["id"], "generation_id": command["generation_id"],
                    "name": binding["name"], "implementation": binding["implementation"],
                    "arguments": json.loads(function["arguments"]),
                    "timeout_ms": command["remaining_timeout_ms"]})
            result["tool_calls"] = calls
        # Trainers may return the existing UEnv token fields; never fabricate them.
        for field in ("policy_version", "parameter_version", "tokenizer", "input_token_ids",
                      "output_token_ids", "output_logprobs", "loss_mask"):
            if field in data:
                result[field] = data[field]
        return result
