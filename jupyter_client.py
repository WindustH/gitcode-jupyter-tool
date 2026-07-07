#!/usr/bin/env python3
"""HTTP client helpers for jupyterd."""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys
import time
import urllib.request
from typing import Any
from urllib.parse import urlparse


DEFAULT_API_URL = "http://127.0.0.1:18787"
DEFAULT_STREAM_URL = "tcp://127.0.0.1:18788"
DEFAULT_LOG = "/tmp/jupyterd.log"


class JupyterClientError(RuntimeError):
    pass


def local_urlopen(req: urllib.request.Request, timeout: float) -> Any:
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    return opener.open(req, timeout=timeout)


def request(api_url: str, path: str, payload: dict[str, Any] | None = None, timeout: float = 30.0) -> dict[str, Any]:
    url = api_url.rstrip("/") + path
    data = json.dumps(payload or {}).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=data,
        headers={"Content-Type": "application/json", "Accept": "application/json"},
        method="POST",
    )
    try:
        with local_urlopen(req, timeout=timeout) as response:
            return json.loads(response.read().decode("utf-8"))
    except Exception as exc:
        raise JupyterClientError(f"jupyterd request failed for {url}: {exc}") from exc


def parse_tcp_url(url: str) -> tuple[str, int]:
    parsed = urlparse(url)
    if parsed.scheme and parsed.scheme != "tcp":
        raise JupyterClientError(f"unsupported stream URL scheme: {url}")
    host = parsed.hostname or "127.0.0.1"
    port = parsed.port or 18788
    return host, port


def wait_job_result(api_url: str, job_id: str, *, timeout: float, poll_interval: float = 0.1) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        payload = request(api_url, "/v1/job", {"id": job_id}, timeout=5.0)
        if not payload.get("ok"):
            raise JupyterClientError(str(payload.get("error") or payload))
        job = payload.get("job") or {}
        status = job.get("status")
        if status == "succeeded":
            result = job.get("result")
            if isinstance(result, dict):
                return result
            return {"ok": True, "result": result}
        if status == "failed":
            return {"ok": False, "error": job.get("error", "jupyterd job failed")}
        time.sleep(poll_interval)
    return {"ok": False, "error": f"jupyterd job {job_id} timed out after {timeout:.1f}s", "exit_code": 124}


def health(api_url: str) -> bool:
    try:
        req = urllib.request.Request(api_url.rstrip("/") + "/v1/health")
        with local_urlopen(req, timeout=2.0) as response:
            payload = json.loads(response.read().decode("utf-8"))
        return bool(payload.get("ok"))
    except Exception:
        return False


def start_daemon(
    *,
    api_url: str,
    stream_url: str,
    headless: bool,
    log_path: str,
    timeout: float,
) -> None:
    if health(api_url):
        return

    parsed = urlparse(api_url)
    host = parsed.hostname or "127.0.0.1"
    port = str(parsed.port or 18787)
    stream_host, stream_port = parse_tcp_url(stream_url)
    daemon = pathlib.Path(__file__).resolve().with_name("jupyterd")
    log_file_path = pathlib.Path(log_path).expanduser()
    log_file_path.parent.mkdir(parents=True, exist_ok=True)
    command = [
        str(daemon),
        "--listen-host",
        host,
        "--listen-port",
        port,
        "--stream-host",
        stream_host,
        "--stream-port",
        str(stream_port),
    ]
    if headless:
        command.append("--headless")
    else:
        command.append("--visible")
    with log_file_path.open("ab") as log_file:
        subprocess.Popen(command, stdout=log_file, stderr=log_file, start_new_session=True)

    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if health(api_url):
            return
        time.sleep(0.25)
    raise JupyterClientError(f"jupyterd did not start within {timeout:.1f}s; log: {log_file_path}")


def ensure_daemon(
    api_url: str,
    *,
    auto_start: bool,
    stream_url: str = DEFAULT_STREAM_URL,
    headless: bool,
    log_path: str,
    timeout: float,
) -> None:
    if health(api_url):
        return
    if auto_start:
        start_daemon(api_url=api_url, stream_url=stream_url, headless=headless, log_path=log_path, timeout=timeout)
        return
    raise JupyterClientError(f"jupyterd is not running at {api_url}; start it with dev/jupyter-tool/jupyterd")


def print_error(message: str) -> None:
    print(message, file=sys.stderr)
