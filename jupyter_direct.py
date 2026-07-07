"""Direct GitCode/Jupyter API backend for jupyterd.

This module intentionally keeps credential values out of logs. Cookies are
loaded from a local 0600 cache or supplied by jupyterd after CDP extraction.
"""

from __future__ import annotations

import base64
import email.utils
import http.cookies
import json
import os
import pathlib
import re
import secrets
import socket
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from typing import Any, Callable

import jupyter_runtime


DEFAULT_AUTH_CACHE = "~/.config/jupyter-tool/auth.json"
GITCODE_HOST_SUFFIX = "gitcode.com"
REQUIRED_COOKIE_NAMES = {"GITCODE_ACCESS_TOKEN", "GITCODE_REFRESH_TOKEN"}


class DirectError(RuntimeError):
    pass


class AuthError(DirectError):
    pass


class ApiError(DirectError):
    def __init__(self, message: str, *, status: int | None = None, body: str = "") -> None:
        super().__init__(message)
        self.status = status
        self.body = body


@dataclass
class NotebookInfo:
    notebook_id: str
    learning_id: str
    notebook_name: str
    provisioning_status: str
    base_url: str
    lab_url: str
    target_url: str
    raw: dict[str, Any]


def now() -> float:
    return time.time()


def is_gitcode_domain(domain: str) -> bool:
    clean = domain.lstrip(".").lower()
    return clean == GITCODE_HOST_SUFFIX or clean.endswith("." + GITCODE_HOST_SUFFIX)


def domain_matches(cookie_domain: str, host: str) -> bool:
    domain = cookie_domain.lower()
    host = host.lower()
    if domain.startswith("."):
        clean = domain[1:]
        return host == clean or host.endswith("." + clean)
    return host == domain


def cookie_expired(cookie: dict[str, Any], *, margin: float = 0.0) -> bool:
    expires = cookie.get("expires")
    try:
        expires_float = float(expires)
    except (TypeError, ValueError):
        return False
    if expires_float < 0:
        return False
    return expires_float <= now() + margin


def normalize_cookie(cookie: dict[str, Any]) -> dict[str, Any]:
    return {
        "name": str(cookie.get("name", "")),
        "value": str(cookie.get("value", "")),
        "domain": str(cookie.get("domain", "")),
        "path": str(cookie.get("path") or "/"),
        "expires": cookie.get("expires", -1),
        "secure": bool(cookie.get("secure", True)),
        "httpOnly": bool(cookie.get("httpOnly", False)),
        "sameSite": cookie.get("sameSite"),
    }


def parse_set_cookie_headers(url: str, headers: Any) -> list[dict[str, Any]]:
    host = urllib.parse.urlparse(url).hostname or ""
    values: list[str] = []
    if hasattr(headers, "get_all"):
        values = list(headers.get_all("Set-Cookie") or [])
    else:
        value = headers.get("Set-Cookie") if headers else None
        if value:
            values = [value]

    parsed: list[dict[str, Any]] = []
    for value in values:
        jar = http.cookies.SimpleCookie()
        try:
            jar.load(value)
        except http.cookies.CookieError:
            continue
        for morsel in jar.values():
            domain = morsel["domain"] or host
            path = morsel["path"] or "/"
            expires = -1.0
            if morsel["max-age"]:
                try:
                    expires = now() + int(morsel["max-age"])
                except ValueError:
                    expires = -1.0
            elif morsel["expires"]:
                try:
                    expires = email.utils.parsedate_to_datetime(morsel["expires"]).timestamp()
                except Exception:
                    expires = -1.0
            parsed.append(
                {
                    "name": morsel.key,
                    "value": morsel.value,
                    "domain": domain,
                    "path": path,
                    "expires": expires,
                    "secure": bool(morsel["secure"]),
                    "httpOnly": bool(morsel["httponly"]),
                    "sameSite": morsel["samesite"] or None,
                }
            )
    return parsed


class CookieAuth:
    def __init__(self, cookies: list[dict[str, Any]], *, source: str = "") -> None:
        self.cookies = [normalize_cookie(cookie) for cookie in cookies if cookie.get("name") and cookie.get("value")]
        self.source = source

    @classmethod
    def from_cache(cls, path: str | pathlib.Path) -> "CookieAuth":
        cache = pathlib.Path(path).expanduser()
        data = json.loads(cache.read_text(encoding="utf-8"))
        cookies = data.get("cookies")
        if not isinstance(cookies, list):
            raise AuthError(f"auth cache has no cookies: {cache}")
        return cls(cookies, source=str(cache))

    def save(self, path: str | pathlib.Path) -> None:
        cache = pathlib.Path(path).expanduser()
        cache.parent.mkdir(parents=True, exist_ok=True)
        data = {
            "version": 1,
            "created": now(),
            "source": self.source,
            "cookies": self.cookies,
        }
        tmp = cache.with_name(cache.name + f".tmp-{secrets.token_hex(4)}")
        tmp.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        os.chmod(tmp, 0o600)
        tmp.replace(cache)
        try:
            os.chmod(cache, 0o600)
        except OSError:
            pass

    def valid(self, *, margin: float = 60.0) -> bool:
        available = {
            cookie["name"]
            for cookie in self.cookies
            if is_gitcode_domain(str(cookie.get("domain", ""))) and not cookie_expired(cookie, margin=margin)
        }
        return REQUIRED_COOKIE_NAMES.issubset(available)

    def update(self, cookies: list[dict[str, Any]]) -> None:
        by_key: dict[tuple[str, str, str], dict[str, Any]] = {}
        for cookie in self.cookies:
            key = (cookie["domain"], cookie["path"], cookie["name"])
            by_key[key] = cookie
        for cookie in cookies:
            normalized = normalize_cookie(cookie)
            if not is_gitcode_domain(normalized["domain"]):
                continue
            key = (normalized["domain"], normalized["path"], normalized["name"])
            if cookie_expired(normalized):
                by_key.pop(key, None)
            else:
                by_key[key] = normalized
        self.cookies = list(by_key.values())

    def cookie_header(self, url: str) -> str:
        parsed = urllib.parse.urlparse(url)
        host = parsed.hostname or ""
        path = parsed.path or "/"
        selected = []
        for cookie in self.cookies:
            if cookie_expired(cookie):
                continue
            if not domain_matches(str(cookie.get("domain", "")), host):
                continue
            cookie_path = str(cookie.get("path") or "/")
            if not path.startswith(cookie_path.rstrip("/") or "/"):
                continue
            selected.append(cookie)
        selected.sort(key=lambda item: len(str(item.get("path") or "/")), reverse=True)
        return "; ".join(f"{cookie['name']}={cookie['value']}" for cookie in selected)

    def xsrf_token(self, url: str) -> str:
        parsed = urllib.parse.urlparse(url)
        host = parsed.hostname or ""
        for cookie in self.cookies:
            if cookie.get("name") == "_xsrf" and domain_matches(str(cookie.get("domain", "")), host):
                return str(cookie.get("value", ""))
        return ""

    def redacted_summary(self) -> dict[str, Any]:
        domains: dict[str, int] = {}
        names: list[str] = []
        for cookie in self.cookies:
            domain = str(cookie.get("domain", ""))
            domains[domain] = domains.get(domain, 0) + 1
            if is_gitcode_domain(domain):
                names.append(str(cookie.get("name", "")))
        return {"domains": domains, "names": sorted(set(names))}


class HttpClient:
    def __init__(self, auth: CookieAuth, *, cache_path: str | pathlib.Path | None = None) -> None:
        self.auth = auth
        self.cache_path = pathlib.Path(cache_path).expanduser() if cache_path else None
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def request(
        self,
        method: str,
        url: str,
        *,
        data: bytes | None = None,
        json_data: Any | None = None,
        headers: dict[str, str] | None = None,
        timeout: float = 30.0,
        expect_json: bool = True,
    ) -> Any:
        request_headers = {
            "User-Agent": "Mozilla/5.0 jupyter-tool",
            "Accept": "application/json, text/plain, */*",
        }
        if headers:
            request_headers.update(headers)
        cookie_header = self.auth.cookie_header(url)
        if cookie_header:
            request_headers["Cookie"] = cookie_header
        if json_data is not None:
            data = json.dumps(json_data).encode("utf-8")
            request_headers.setdefault("Content-Type", "application/json")
        xsrf = self.auth.xsrf_token(url)
        if xsrf and method.upper() not in {"GET", "HEAD", "OPTIONS"}:
            request_headers.setdefault("X-XSRFToken", xsrf)

        req = urllib.request.Request(url, data=data, headers=request_headers, method=method.upper())
        try:
            with self.opener.open(req, timeout=timeout) as response:
                body = response.read()
                self._update_cookies(url, response.headers)
        except urllib.error.HTTPError as exc:
            body_text = exc.read().decode("utf-8", "replace")
            self._update_cookies(url, exc.headers)
            if exc.code in {401, 403}:
                raise AuthError(f"authenticated request rejected with HTTP {exc.code}: {url}") from exc
            raise ApiError(f"HTTP {exc.code}: {url}", status=exc.code, body=body_text) from exc

        if not expect_json:
            return body
        text = body.decode("utf-8", "replace")
        try:
            return json.loads(text) if text else None
        except json.JSONDecodeError as exc:
            raise ApiError(f"expected JSON from {url}", body=text[:1000]) from exc

    def _update_cookies(self, url: str, headers: Any) -> None:
        cookies = parse_set_cookie_headers(url, headers)
        if not cookies:
            return
        self.auth.update(cookies)
        if self.cache_path:
            self.auth.save(self.cache_path)


def extract_cookies_from_cdp(cdp_list_url: str, *, debug: bool = False) -> CookieAuth:
    targets = jupyter_runtime.fetch_json(cdp_list_url, timeout=5.0)
    pages = [target for target in targets if target.get("type") == "page"]
    if not pages:
        raise AuthError("no Chrome page is available for cookie extraction")
    cdp = jupyter_runtime.CdpClient(pages[0]["webSocketDebuggerUrl"], debug=debug)
    try:
        cdp.call("Page.enable")
        cdp.call("Network.enable")
        cdp.call("Runtime.enable")
        cdp.pump_for(1.0)
        cookies = cdp.call("Network.getAllCookies").get("cookies", [])
        gitcode_cookies = [cookie for cookie in cookies if is_gitcode_domain(str(cookie.get("domain", "")))]
        auth = CookieAuth(gitcode_cookies, source="chrome-cdp")
        if not auth.valid(margin=0):
            raise AuthError("Chrome profile does not contain valid GitCode login cookies")
        return auth
    finally:
        cdp.close()


def notebook_from_insert(info: dict[str, Any], *, notebook_path: str, user_name: str = "username") -> NotebookInfo:
    notebook_id = str(info.get("notebook_id") or "")
    if not notebook_id:
        raise ApiError(f"GitCode insert response has no notebook_id: {info!r}")
    match = re.match(r"(?P<learning>.+)-(?P<name>notebook\d+)-(?P<suffix>[^-]+)$", notebook_id)
    if not match:
        raise ApiError(f"unexpected GitCode notebook_id format: {notebook_id}")
    learning_id = match.group("learning")
    notebook_name = match.group("name")
    base_url = f"https://aihub-run.gitcode.com/learning/{learning_id}/{notebook_name}"
    lab_path = notebook_path.strip("/")
    lab_url = f"{base_url}/lab"
    if lab_path:
        lab_url += f"/tree/{lab_path}"
    target_url = (
        f"https://ai.gitcode.com/user/{urllib.parse.quote(user_name)}/notebookcann/lab"
        f"?cannNotebookId={urllib.parse.quote(notebook_id)}"
    )
    return NotebookInfo(
        notebook_id=notebook_id,
        learning_id=learning_id,
        notebook_name=notebook_name,
        provisioning_status=str(info.get("provisioning_status") or ""),
        base_url=base_url,
        lab_url=lab_url,
        target_url=target_url,
        raw=info,
    )


def notebook_from_lab_url(href: str, *, target_url: str = "", raw: dict[str, Any] | None = None) -> NotebookInfo:
    parsed = urllib.parse.urlparse(href)
    match = re.match(r"/learning/([^/]+)/(notebook\d+)(?:/|$)", parsed.path)
    if not match:
        raise ApiError(f"state href is not a GitCode notebook URL: {href}")
    learning_id, notebook_name = match.group(1), match.group(2)
    base_url = f"{parsed.scheme or 'https'}://{parsed.netloc}/learning/{learning_id}/{notebook_name}"
    notebook_id = ""
    if target_url:
        query = urllib.parse.parse_qs(urllib.parse.urlparse(target_url).query)
        notebook_id = (query.get("cannNotebookId") or [""])[0]
    if not notebook_id:
        notebook_id = f"{learning_id}-{notebook_name}"
    return NotebookInfo(
        notebook_id=notebook_id,
        learning_id=learning_id,
        notebook_name=notebook_name,
        provisioning_status="READY",
        base_url=base_url,
        lab_url=href,
        target_url=target_url,
        raw=raw or {},
    )


def insert_notebook(client: HttpClient, args: Any) -> NotebookInfo:
    params = {
        "repoUrl": args.repo_url,
        "ttl": str(args.ttl),
        "diskSize": args.disk_size,
        "path": args.notebook_path,
        "scanFilePath": args.scan_file_path,
        "__s": "aihub",
    }
    url = "https://web-api.gitcode.com/aihub/api/v1/notebookcann/insert?" + urllib.parse.urlencode(params)
    info = client.request(
        "GET",
        url,
        headers={"Origin": "https://ai.gitcode.com", "Referer": "https://ai.gitcode.com/"},
        timeout=args.insert_timeout,
    )
    return notebook_from_insert(info, notebook_path=args.notebook_path, user_name=args.gitcode_user)


def probe_notebook(client: HttpClient, notebook: NotebookInfo, *, timeout: float = 15.0) -> dict[str, Any]:
    return client.request(
        "GET",
        notebook.base_url + "/api/status",
        headers={"Referer": notebook.lab_url, "Origin": "https://aihub-run.gitcode.com"},
        timeout=timeout,
    )


class ApiTerminal:
    def __init__(
        self,
        client: HttpClient,
        notebook: NotebookInfo,
        on_output: Callable[[str], None],
        *,
        rows: int,
        cols: int,
        timeout: float = 30.0,
    ) -> None:
        self.client = client
        self.notebook = notebook
        self.on_output = on_output
        self.rows = rows
        self.cols = cols
        self.timeout = timeout
        self.name = ""
        self.ws: jupyter_runtime.WebSocket | None = None
        self.closed = False

    @property
    def href(self) -> str:
        return self.notebook.lab_url

    def start(self) -> dict[str, Any]:
        model = self.client.request(
            "POST",
            self.notebook.base_url + "/api/terminals",
            json_data={},
            headers={"Origin": "https://aihub-run.gitcode.com", "Referer": self.notebook.lab_url},
            timeout=self.timeout,
        )
        self.name = str(model["name"])
        websocket_url = (
            f"wss://aihub-run.gitcode.com/learning/{self.notebook.learning_id}/"
            f"{self.notebook.notebook_name}/terminals/websocket/{urllib.parse.quote(self.name)}"
        )
        cookie_header = self.client.auth.cookie_header(self.notebook.base_url + "/terminals/websocket/" + self.name)
        self.ws = jupyter_runtime.WebSocket(
            websocket_url,
            headers={"Cookie": cookie_header, "Origin": "https://aihub-run.gitcode.com"},
            timeout=self.timeout,
        )
        self.send_json(["set_size", self.rows, self.cols])
        return {"name": self.name, "base": self.notebook.base_url, "href": self.notebook.lab_url}

    def send_json(self, payload: Any) -> None:
        if self.ws is None:
            raise DirectError("terminal websocket is not open")
        self.ws.send_text(json.dumps(payload, ensure_ascii=False))

    def send(self, text: str) -> None:
        self.send_json(["stdin", text])

    def set_size(self, rows: int, cols: int) -> None:
        self.rows = rows
        self.cols = cols
        self.send_json(["set_size", rows, cols])

    def read_once(self, timeout: float = 0.1) -> str:
        if self.ws is None or self.closed:
            return ""
        try:
            message = self.ws.recv_text(timeout=timeout)
        except socket.timeout:
            return ""
        if message is None:
            self.closed = True
            return "\n__JUPYTER_TOOL_TERMINAL_CLOSED__\n"
        try:
            payload = json.loads(message)
        except json.JSONDecodeError:
            return message
        if isinstance(payload, list) and len(payload) > 1 and isinstance(payload[1], str):
            return payload[1]
        return ""

    def close(self) -> None:
        self.closed = True
        if self.ws is not None:
            try:
                self.ws.close()
            except OSError:
                pass
            self.ws = None
        if self.name:
            try:
                self.client.request(
                    "DELETE",
                    self.notebook.base_url + "/api/terminals/" + urllib.parse.quote(self.name),
                    headers={"Origin": "https://aihub-run.gitcode.com", "Referer": self.notebook.lab_url},
                    timeout=10.0,
                    expect_json=False,
                )
            except Exception:
                pass
            self.name = ""


def encode_file_payload(payload: bytes) -> str:
    return base64.encodebytes(payload).decode("ascii")
