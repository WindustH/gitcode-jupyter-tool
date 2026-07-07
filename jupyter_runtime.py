"""Private runtime primitives used by jupyterd."""

from __future__ import annotations

import base64
import hashlib
import json
import os
import re
import secrets
import socket
import ssl
import struct
import sys
import time
import urllib.request
from dataclasses import dataclass
from typing import Any, Callable
from urllib.parse import urlparse


DEFAULT_CDP_LIST_URL = "http://127.0.0.1:9222/json"
ANSI_RE = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]")
TERMINAL_CLOSED_MARK = "__JUPYTER_TOOL_TERMINAL_CLOSED__"


class JupyterRuntimeError(RuntimeError):
    pass


def strip_terminal_noise(text: str) -> str:
    return ANSI_RE.sub("", text).replace("\r", "")


def eprint(message: str) -> None:
    print(message, file=sys.stderr)


class WebSocket:
    """Small RFC6455 client covering the text frames used by CDP/Jupyter."""

    def __init__(
        self,
        url: str,
        headers: dict[str, str] | None = None,
        timeout: float = 15.0,
    ) -> None:
        self.url = url
        self.parsed = urlparse(url)
        if self.parsed.scheme not in {"ws", "wss"}:
            raise JupyterRuntimeError(f"unsupported websocket scheme: {self.parsed.scheme}")

        host = self.parsed.hostname
        if not host:
            raise JupyterRuntimeError(f"websocket URL has no host: {url}")
        port = self.parsed.port or (443 if self.parsed.scheme == "wss" else 80)
        raw = socket.create_connection((host, port), timeout=timeout)
        if self.parsed.scheme == "wss":
            raw = ssl.create_default_context().wrap_socket(raw, server_hostname=host)
        raw.settimeout(timeout)
        self.sock = raw
        self._handshake(headers or {})

    def fileno(self) -> int:
        return self.sock.fileno()

    def close(self) -> None:
        try:
            self._send_frame(b"", opcode=0x8)
        except OSError:
            pass
        try:
            self.sock.close()
        except OSError:
            pass

    def send_text(self, text: str) -> None:
        self._send_frame(text.encode("utf-8"), opcode=0x1)

    def recv_text(self, timeout: float | None = None) -> str | None:
        previous_timeout = self.sock.gettimeout()
        if timeout is not None:
            self.sock.settimeout(timeout)
        try:
            while True:
                first = self._recv_exact(2)
                b0, b1 = first[0], first[1]
                opcode = b0 & 0x0F
                masked = bool(b1 & 0x80)
                length = b1 & 0x7F
                if length == 126:
                    length = struct.unpack("!H", self._recv_exact(2))[0]
                elif length == 127:
                    length = struct.unpack("!Q", self._recv_exact(8))[0]

                mask = self._recv_exact(4) if masked else b""
                payload = self._recv_exact(length) if length else b""
                if masked:
                    payload = bytes(byte ^ mask[index % 4] for index, byte in enumerate(payload))

                if opcode == 0x8:
                    return None
                if opcode == 0x9:
                    self._send_frame(payload, opcode=0xA)
                    continue
                if opcode == 0xA:
                    continue
                if opcode in {0x1, 0x0}:
                    return payload.decode("utf-8", "replace")
        finally:
            if timeout is not None:
                self.sock.settimeout(previous_timeout)

    def _handshake(self, extra_headers: dict[str, str]) -> None:
        host = self.parsed.netloc
        path = self.parsed.path or "/"
        if self.parsed.query:
            path += "?" + self.parsed.query
        key = base64.b64encode(os.urandom(16)).decode("ascii")
        headers = {
            "Host": host,
            "Upgrade": "websocket",
            "Connection": "Upgrade",
            "Sec-WebSocket-Key": key,
            "Sec-WebSocket-Version": "13",
            **extra_headers,
        }
        request = [f"GET {path} HTTP/1.1", *(f"{name}: {value}" for name, value in headers.items()), "", ""]
        self.sock.sendall("\r\n".join(request).encode("ascii"))

        response = b""
        while b"\r\n\r\n" not in response:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise JupyterRuntimeError("websocket handshake closed early")
            response += chunk
            if len(response) > 65536:
                raise JupyterRuntimeError("websocket handshake response is too large")

        head = response.split(b"\r\n\r\n", 1)[0].decode("iso-8859-1")
        lines = head.split("\r\n")
        if not lines or " 101 " not in lines[0]:
            raise JupyterRuntimeError(f"websocket handshake failed: {lines[0] if lines else head}")
        expected = base64.b64encode(
            hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode("ascii")).digest()
        ).decode("ascii")
        actual = ""
        for line in lines[1:]:
            if ":" not in line:
                continue
            name, value = line.split(":", 1)
            if name.lower() == "sec-websocket-accept":
                actual = value.strip()
                break
        if actual and actual != expected:
            raise JupyterRuntimeError("websocket accept key mismatch")

    def _send_frame(self, payload: bytes, opcode: int) -> None:
        header = bytearray([0x80 | opcode])
        length = len(payload)
        if length < 126:
            header.append(0x80 | length)
        elif length < 65536:
            header.append(0x80 | 126)
            header.extend(struct.pack("!H", length))
        else:
            header.append(0x80 | 127)
            header.extend(struct.pack("!Q", length))
        mask = os.urandom(4)
        header.extend(mask)
        masked = bytes(byte ^ mask[index % 4] for index, byte in enumerate(payload))
        self.sock.sendall(bytes(header) + masked)

    def _recv_exact(self, length: int) -> bytes:
        chunks = bytearray()
        while len(chunks) < length:
            chunk = self.sock.recv(length - len(chunks))
            if not chunk:
                raise JupyterRuntimeError("websocket closed")
            chunks.extend(chunk)
        return bytes(chunks)


class CdpClient:
    def __init__(self, websocket_url: str, debug: bool = False) -> None:
        self.ws = WebSocket(websocket_url)
        self.debug = debug
        self.next_id = 1
        self.contexts: dict[int, dict[str, Any]] = {}
        self.binding_handlers: dict[str, Callable[[str], None]] = {}

    def close(self) -> None:
        self.ws.close()

    def call(self, method: str, params: dict[str, Any] | None = None, timeout: float = 20.0) -> dict[str, Any]:
        message_id = self.next_id
        self.next_id += 1
        self.ws.send_text(json.dumps({"id": message_id, "method": method, "params": params or {}}))
        deadline = time.monotonic() + timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise JupyterRuntimeError(f"CDP call timed out: {method}")
            message = self._recv_message(timeout=remaining)
            if message.get("id") != message_id:
                self._handle_event(message)
                continue
            if "error" in message:
                raise JupyterRuntimeError(f"CDP {method} failed: {message['error']}")
            return message.get("result", {})

    def evaluate(
        self,
        context_id: int,
        expression: str,
        *,
        await_promise: bool = False,
        timeout_ms: int = 20000,
    ) -> Any:
        result = self.call(
            "Runtime.evaluate",
            {
                "contextId": context_id,
                "expression": expression,
                "awaitPromise": await_promise,
                "returnByValue": True,
                "timeout": timeout_ms,
            },
            timeout=(timeout_ms / 1000.0) + 5.0,
        )
        if "exceptionDetails" in result:
            text = result["exceptionDetails"].get("text") or result["exceptionDetails"]
            raise JupyterRuntimeError(f"remote JavaScript failed: {text}")
        remote = result.get("result", {})
        if "value" in remote:
            return remote["value"]
        if remote.get("type") == "undefined":
            return None
        return remote

    def add_binding(self, name: str, handler: Callable[[str], None]) -> None:
        self.binding_handlers[name] = handler
        self.call("Runtime.addBinding", {"name": name})

    def pump(self, timeout: float = 0.1) -> bool:
        try:
            message = self._recv_message(timeout=timeout)
        except socket.timeout:
            return False
        self._handle_event(message)
        return True

    def pump_for(self, seconds: float) -> None:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            self.pump(max(0.01, min(0.1, deadline - time.monotonic())))

    def _recv_message(self, timeout: float) -> dict[str, Any]:
        raw = self.ws.recv_text(timeout=timeout)
        if raw is None:
            raise JupyterRuntimeError("CDP websocket closed")
        if self.debug:
            eprint(f"[cdp] {raw[:500]}")
        return json.loads(raw)

    def _handle_event(self, message: dict[str, Any]) -> None:
        method = message.get("method")
        params = message.get("params", {})
        if method == "Runtime.executionContextCreated":
            context = params.get("context", {})
            if context.get("auxData", {}).get("type") == "default":
                self.contexts[int(context["id"])] = context
            return
        if method == "Runtime.bindingCalled":
            handler = self.binding_handlers.get(params.get("name", ""))
            if handler:
                handler(str(params.get("payload", "")))


@dataclass
class BrowserSession:
    cdp: CdpClient
    context_id: int
    href: str
    target_url: str = ""


class BrowserTerminal:
    def __init__(
        self,
        session: BrowserSession,
        on_output: Callable[[str], None],
        *,
        rows: int,
        cols: int,
    ) -> None:
        self.session = session
        self.on_output = on_output
        self.rows = rows
        self.cols = cols
        self.binding_name = f"jupyterShOutput_{secrets.token_hex(8)}"
        self.state_name = f"__jupyterShTerminal_{secrets.token_hex(8)}"

    def start(self) -> dict[str, Any]:
        self.session.cdp.add_binding(self.binding_name, self.on_output)
        expression = START_TERMINAL_JS % (
            json.dumps(self.binding_name),
            json.dumps(self.state_name),
            self.rows,
            self.cols,
        )
        return self.session.cdp.evaluate(
            self.session.context_id,
            expression,
            await_promise=True,
            timeout_ms=30000,
        )

    def send(self, text: str) -> None:
        expression = SEND_STDIN_JS % (json.dumps(self.state_name), json.dumps(text))
        result = self.session.cdp.evaluate(self.session.context_id, expression)
        if not result or not result.get("ok"):
            raise JupyterRuntimeError(f"failed to write remote terminal stdin: {result}")

    def set_size(self, rows: int, cols: int) -> None:
        expression = SET_SIZE_JS % (json.dumps(self.state_name), rows, cols)
        self.session.cdp.evaluate(self.session.context_id, expression)

    def close(self) -> None:
        expression = CLOSE_TERMINAL_JS % json.dumps(self.state_name)
        try:
            self.session.cdp.evaluate(
                self.session.context_id,
                expression,
                await_promise=True,
                timeout_ms=10000,
            )
        except JupyterRuntimeError:
            pass


START_TERMINAL_JS = r"""
(async function(binding, stateName, rows, cols) {
  const out = (text) => window[binding](String(text));
  const cookie = document.cookie || "";
  const xsrfPair = cookie.split("; ").find((item) => item.startsWith("_xsrf=")) || "";
  const xsrf = decodeURIComponent(xsrfPair.split("=").slice(1).join("="));
  const markers = ["/lab", "/tree", "/notebooks", "/terminals"];
  let base = location.pathname.replace(/\/$/, "");
  for (const marker of markers) {
    const index = base.indexOf(marker);
    if (index >= 0) {
      base = base.slice(0, index);
      break;
    }
  }
  const api = `${base}/api/terminals`;
  const create = await fetch(api, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      "X-XSRFToken": xsrf,
    },
    body: "{}",
  });
  if (!create.ok) {
    throw new Error(`terminal create failed: ${create.status} ${await create.text()}`);
  }
  const model = await create.json();
  const wsUrl = `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}${base}/terminals/websocket/${model.name}`;
  const termWs = new WebSocket(wsUrl);
  window[stateName] = { ws: termWs, api, name: model.name, xsrf };

  termWs.onmessage = (event) => {
    try {
      const msg = JSON.parse(event.data);
      if (Array.isArray(msg)) {
        if (typeof msg[1] === "string") out(msg[1]);
      } else {
        out(String(event.data));
      }
    } catch (err) {
      out(String(event.data));
    }
  };
  termWs.onclose = () => out("\n__JUPYTER_TOOL_TERMINAL_CLOSED__\n");
  termWs.onerror = () => out("\n[jupyterd: remote terminal websocket error]\n");

  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("terminal websocket open timeout")), 15000);
    termWs.onopen = () => {
      clearTimeout(timer);
      termWs.send(JSON.stringify(["set_size", rows, cols]));
      resolve();
    };
  });
  return { name: model.name, base, href: location.href };
})(%s, %s, %d, %d)
"""

SEND_STDIN_JS = r"""
(() => {
  const state = window[%s];
  if (!state) return { ok: false, reason: "missing terminal state" };
  if (state.ws.readyState !== WebSocket.OPEN) {
    return { ok: false, readyState: state.ws.readyState };
  }
  state.ws.send(JSON.stringify(["stdin", %s]));
  return { ok: true, bufferedAmount: state.ws.bufferedAmount };
})()
"""

SET_SIZE_JS = r"""
(() => {
  const state = window[%s];
  if (!state || state.ws.readyState !== WebSocket.OPEN) return { ok: false };
  state.ws.send(JSON.stringify(["set_size", %d, %d]));
  return { ok: true };
})()
"""

CLOSE_TERMINAL_JS = r"""
(async function(stateName) {
  const state = window[stateName];
  if (!state) return;
  try { state.ws.close(); } catch (err) {}
  try {
    await fetch(`${state.api}/${state.name}`, {
      method: "DELETE",
      headers: { "X-XSRFToken": decodeURIComponent(state.xsrf || "") },
    });
  } catch (err) {}
  delete window[stateName];
})(%s)
"""


def fetch_json(url: str, timeout: float = 10.0) -> Any:
    request = urllib.request.Request(url, headers={"Accept": "application/json"})
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(request, timeout=timeout) as response:
        return json.loads(response.read().decode("utf-8"))


def connect_browser(args: Any) -> BrowserSession:
    pages = fetch_json(args.cdp_list_url)
    if not isinstance(pages, list):
        raise JupyterRuntimeError(f"unexpected CDP target list: {pages!r}")
    candidates = [page for page in pages if page.get("type") == "page"]
    if args.target_url_contains:
        candidates = [page for page in candidates if args.target_url_contains in page.get("url", "")]
    if not candidates:
        available = "\n".join(f"- {page.get('url', '')}" for page in pages if page.get("type") == "page")
        raise JupyterRuntimeError(
            "no matching Chrome page found. Start Chrome with --remote-debugging-port=9222 "
            f"and open the notebook page.\nAvailable pages:\n{available}"
        )

    cdp = CdpClient(candidates[0]["webSocketDebuggerUrl"], debug=args.debug)
    cdp.call("Page.enable")
    cdp.call("Runtime.enable")
    cdp.pump_for(0.5)

    hrefs: dict[int, str] = {}
    deadline = time.monotonic() + args.context_wait
    while time.monotonic() < deadline:
        for context_id in list(cdp.contexts):
            if context_id in hrefs:
                continue
            try:
                href = cdp.evaluate(context_id, "location.href")
            except JupyterRuntimeError:
                continue
            hrefs[context_id] = str(href)
            if args.page_url_contains in hrefs[context_id]:
                return BrowserSession(
                    cdp=cdp,
                    context_id=context_id,
                    href=hrefs[context_id],
                    target_url=candidates[0].get("url", ""),
                )
        cdp.pump(0.2)

    cdp.close()
    seen = "\n".join(f"- context {cid}: {href}" for cid, href in hrefs.items()) or "(no default contexts)"
    raise JupyterRuntimeError(f"no Jupyter context matched {args.page_url_contains!r}.\nSeen contexts:\n{seen}")
