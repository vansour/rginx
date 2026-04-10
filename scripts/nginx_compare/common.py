from __future__ import annotations

import dataclasses
import pathlib
import socket
import subprocess


@dataclasses.dataclass(frozen=True)
class BenchmarkResult:
    server: str
    scenario: str
    tool: str
    complete_requests: int
    failed_requests: int
    requests_per_sec: float
    time_per_request_ms: float
    transfer_rate_kb_sec: float | None


@dataclasses.dataclass(frozen=True)
class UnsupportedScenario:
    server: str
    scenario: str
    reason: str


@dataclasses.dataclass(frozen=True)
class ReloadResult:
    server: str
    scenario: str
    reload_apply_ms: float


@dataclasses.dataclass
class ReservedPort:
    port: int
    _socket: socket.socket | None

    def release(self) -> None:
        if self._socket is None:
            return
        self._socket.close()
        self._socket = None

    def __int__(self) -> int:
        return self.port

    def __str__(self) -> str:
        return str(self.port)


def run(
    command: list[str],
    *,
    cwd: pathlib.Path | None = None,
    env: dict[str, str] | None = None,
    capture_output: bool = False,
) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(
        command,
        cwd=str(cwd) if cwd is not None else None,
        env=env,
        check=False,
        text=True,
        capture_output=capture_output,
    )
    if completed.returncode != 0:
        stdout = completed.stdout or ""
        stderr = completed.stderr or ""
        raise RuntimeError(
            f"command failed ({completed.returncode}): {' '.join(command)}\nstdout:\n{stdout}\nstderr:\n{stderr}"
        )
    return completed


def reserve_port() -> ReservedPort:
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.bind(("127.0.0.1", 0))
    return ReservedPort(port=sock.getsockname()[1], _socket=sock)


def port_number(port: int | ReservedPort, *, release: bool = False) -> int:
    if isinstance(port, ReservedPort):
        if release:
            port.release()
        return port.port
    return port


def write_text(path: pathlib.Path, contents: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(contents, encoding="utf-8")
