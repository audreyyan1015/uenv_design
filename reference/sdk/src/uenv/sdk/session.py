"""Operations inside an already isolated Backend session.

This class does not create an OS sandbox. The platform launcher must start its
process in the selected Backend before passing it to EnvironmentContext.
"""
from copy import deepcopy
from pathlib import Path
import os
import signal
import subprocess
import threading


class Session:
    def __init__(self, identity, workspace):
        self.identity = deepcopy(identity)
        self.workspace = Path(workspace).resolve(strict=True)
        self._lock = threading.RLock()
        self._frozen = False
        self._processes = set()

    def _path(self, path):
        value = Path(path)
        target = (self.workspace / value).resolve()
        if not target.is_relative_to(self.workspace):
            raise ValueError('SESSION_PATH_OUTSIDE_WORKSPACE')
        return target

    def read_file(self, path):
        with self._lock:
            return self._path(path).read_bytes()

    def write_file(self, path, content):
        with self._lock:
            if self._frozen:
                raise RuntimeError('TOOLS_FROZEN')
            self._path(path).write_bytes(content)

    @staticmethod
    def _stop(process):
        if os.name == 'posix':
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        elif process.poll() is None:
            process.kill()
        process.wait()

    def run(self, argv, timeout_ms, *, cwd='.'):
        if not isinstance(argv, list) or not argv or not all(isinstance(v, str) for v in argv):
            raise TypeError('SESSION_COMMAND_REQUIRES_ARGV')
        if timeout_ms <= 0:
            raise TimeoutError('SESSION_COMMAND_TIMEOUT')
        with self._lock:
            if self._frozen:
                raise RuntimeError('TOOLS_FROZEN')
            process = subprocess.Popen(argv, cwd=self._path(cwd), stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                start_new_session=os.name == 'posix')
            self._processes.add(process)
        try:
            stdout, stderr = process.communicate(timeout=timeout_ms / 1000)
            return {'exit_code': process.returncode, 'stdout': stdout, 'stderr': stderr}
        finally:
            with self._lock:
                self._stop(process)
                self._processes.discard(process)

    def freeze_tools(self):
        with self._lock:
            self._frozen = True
            for process in tuple(self._processes):
                self._stop(process)
            self._processes.clear()


class RpcFileWriter:
    """Write-only artifact capability; no access to private scoring objects."""
    def __init__(self, peer):
        self.peer = peer

    def put_bytes(self, content, media_type):
        return self.peer.request('file.write_artifact',
            {'content': list(content), 'media_type': media_type})
