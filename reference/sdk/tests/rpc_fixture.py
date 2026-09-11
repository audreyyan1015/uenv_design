"""Minimal transport adversary; uses stdlib only, without importing the SDK."""
import importlib.util
from pathlib import Path
import sys
import time
import os

spec = importlib.util.spec_from_file_location("rpc", Path(__file__).parents[1]/"src/uenv/sdk/rpc.py")
rpc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rpc)
handlers = {}
peer = rpc.RpcPeer(sys.stdin.buffer, sys.stdout.buffer, handlers, start=False)
def nested(params):
    return peer.request("step", params)
def sleep(params):
    time.sleep(params["seconds"])
    return None
handlers.update({"echo": lambda value: value, "nested": nested, "sleep": sleep,
                 "crash": lambda _: os._exit(9)})
peer.start()
peer.wait()
