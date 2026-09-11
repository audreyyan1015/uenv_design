"""Run the real Agent/MCP/RPC suite in an explicitly selected Python environment."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--python", required=True, help="Python in the isolated integration venv")
    parser.add_argument("--cargo", default=shutil.which("cargo"))
    args = parser.parse_args()
    if not args.cargo:
        parser.error("cargo was not found; supply --cargo")
    root = Path(__file__).resolve().parents[1]
    versions = json.loads(subprocess.check_output([args.python, "-c",
        "import importlib.metadata as m,json; print(json.dumps({n:m.version(n) for n in ['openhands-sdk','mcp']}))"], text=True))
    if versions != {"openhands-sdk": "1.15.0", "mcp": "1.26.0"}:
        raise SystemExit(f"Install scripts/requirements-integration.txt first; found {versions}")
    # Do not resolve a venv's interpreter symlink to the system Python.
    env = {**os.environ, "UENV_INTEGRATION_PYTHON": str(Path(args.python).absolute()),
           "OPENHANDS_SUPPRESS_BANNER": "1"}
    subprocess.run([args.cargo, "test", "-p", "uenv-reference-control", "--test", "rpc_transport"],
                   cwd=root, env=env, check=True)
    subprocess.run([args.cargo, "test", "-p", "uenv-reference-control", "--test", "agent_rpc",
                    "--", "--ignored", "--nocapture", "--test-threads=1"], cwd=root, env=env, check=True)
    print(json.dumps({"status": "passed", "dependencies": versions,
        "model": "scripted loopback HTTP", "agent": "real OpenHands Conversation",
        "mcp": "real authenticated streamable HTTP", "worker": "Rust AgentRuntime",
        "rpc": "separate Python process over inherited pipes"}))


if __name__ == "__main__":
    main()
