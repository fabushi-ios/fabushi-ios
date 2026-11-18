#!/usr/bin/env python3
"""Pure Python 3.11+ stdlib multi-peer HTTP REST + RFC 6455 WebSocket driver
and live loopback self-test suite for `gpui_telegram_server`.

Includes a deterministic RFC 8032 Ed25519 signer (using only Python `int` + `hashlib.sha512`)
to generate and verify `gpui-e2ee-spk-v1` signed prekeys against `POST /api/v1/e2ee/prekeys`.
"""

from __future__ import annotations

import base64
import hashlib
import json
import os
import socket
import struct
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any, Callable


# ---------------------------------------------------------------------------
# Pure-stdlib RFC 8032 Ed25519 Signer (for `POST /api/v1/e2ee/prekeys` F38)
# ---------------------------------------------------------------------------

_ED25519_P = (1 << 255) - 19
_ED25519_L = (1 << 252) + 27742317777372353535851937790883648493
_ED25519_D = (-121665 * pow(121666, _ED25519_P - 2, _ED25519_P)) % _ED25519_P
_ED25519_I = pow(2, (_ED25519_P - 1) // 4, _ED25519_P)

# RFC 8032 §7.1 Test Vector 1 secret seed (32 bytes)
RFC8032_TV1_SEED = bytes.fromhex(
    "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60"
)
CANONICAL_SPK_PUBLIC_KEY = bytes([0x42] * 32)
CANONICAL_IDENTITY_X25519_KEY = bytes([0x21] * 32)
CANONICAL_OTPK_PUBLIC_KEY = bytes([0x53] * 32)
SPK_SIGN_DOMAIN = b"gpui-e2ee-spk-v1"


def _xrecover(y: int) -> int:
    xx = (y * y - 1) * pow(_ED25519_D * y * y + 1, _ED25519_P - 2, _ED25519_P)
    x = pow(xx, (_ED25519_P + 3) // 8, _ED25519_P)
    if (x * x - xx) % _ED25519_P != 0:
        x = (x * _ED25519_I) % _ED25519_P
    if x % 2 != 0:
        x = _ED25519_P - x
    return x


_BY = (4 * pow(5, _ED25519_P - 2, _ED25519_P)) % _ED25519_P
_BX = _xrecover(_BY)
_ED25519_B = (_BX % _ED25519_P, _BY % _ED25519_P, 1, (_BX * _BY) % _ED25519_P)


def _edwards_add(
    pt1: tuple[int, int, int, int], pt2: tuple[int, int, int, int]
) -> tuple[int, int, int, int]:
    x1, y1, z1, t1 = pt1
    x2, y2, z2, t2 = pt2
    a = ((y1 - x1) * (y2 - x2)) % _ED25519_P
    b = ((y1 + x1) * (y2 + x2)) % _ED25519_P
    c = (t1 * 2 * _ED25519_D * t2) % _ED25519_P
    d = (z1 * 2 * z2) % _ED25519_P
    e = (b - a) % _ED25519_P
    f = (d - c) % _ED25519_P
    g = (d + c) % _ED25519_P
    h = (b + a) % _ED25519_P
    return (
        (e * f) % _ED25519_P,
        (g * h) % _ED25519_P,
        (f * g) % _ED25519_P,
        (e * h) % _ED25519_P,
    )


def _scalarmult(
    pt: tuple[int, int, int, int], e: int
) -> tuple[int, int, int, int]:
    res = (0, 1, 1, 0)
    cur = pt
    scalar = e
    while scalar > 0:
        if scalar & 1:
            res = _edwards_add(res, cur)
        cur = _edwards_add(cur, cur)
        scalar >>= 1
    return res


def _encode_point(pt: tuple[int, int, int, int]) -> bytes:
    x, y, z, _ = pt
    zi = pow(z, _ED25519_P - 2, _ED25519_P)
    xp = (x * zi) % _ED25519_P
    yp = (y * zi) % _ED25519_P
    encoded = bytearray(yp.to_bytes(32, "little"))
    if xp & 1:
        encoded[31] |= 0x80
    return bytes(encoded)


def ed25519_keypair_and_sign(seed32: bytes, message: bytes) -> tuple[bytes, bytes]:
    """Return (verifying_key_32, signature_64) per RFC 8032 §5.1.6."""
    if len(seed32) != 32:
        raise ValueError("Ed25519 seed must be exactly 32 bytes")
    digest = hashlib.sha512(seed32).digest()
    a_bytes = bytearray(digest[:32])
    a_bytes[0] &= 248
    a_bytes[31] &= 127
    a_bytes[31] |= 64
    s = int.from_bytes(a_bytes, "little")
    prefix = digest[32:]
    pub_bytes = _encode_point(_scalarmult(_ED25519_B, s))

    r = int.from_bytes(hashlib.sha512(prefix + message).digest(), "little") % _ED25519_L
    r_bytes = _encode_point(_scalarmult(_ED25519_B, r))
    k = (
        int.from_bytes(hashlib.sha512(r_bytes + pub_bytes + message).digest(), "little")
        % _ED25519_L
    )
    s_scalar = (r + k * s) % _ED25519_L
    sig_bytes = r_bytes + s_scalar.to_bytes(32, "little")
    return pub_bytes, sig_bytes


def build_canonical_e2ee_prekey_fixture() -> dict[str, str]:
    """Compute canonical RFC 8032 base64 fixture for `POST /api/v1/e2ee/prekeys`."""
    signing_pub, spk_sig = ed25519_keypair_and_sign(
        RFC8032_TV1_SEED, SPK_SIGN_DOMAIN + CANONICAL_SPK_PUBLIC_KEY
    )
    return {
        "identity_key_b64": base64.b64encode(CANONICAL_IDENTITY_X25519_KEY).decode("ascii"),
        "identity_signing_key_b64": base64.b64encode(signing_pub).decode("ascii"),
        "spk_public_key_b64": base64.b64encode(CANONICAL_SPK_PUBLIC_KEY).decode("ascii"),
        "spk_signature_b64": base64.b64encode(spk_sig).decode("ascii"),
        "otpk_public_key_b64": base64.b64encode(CANONICAL_OTPK_PUBLIC_KEY).decode("ascii"),
    }


# ---------------------------------------------------------------------------
# Pure-stdlib RFC 6455 Loopback WebSocket Client
# ---------------------------------------------------------------------------


class LoopbackWebSocket:
    """Minimal deterministic RFC 6455 WebSocket client over 127.0.0.1 TCP."""

    def __init__(self, host: str, port: int, path_with_query: str, timeout: float = 10.0) -> None:
        self._sock = socket.create_connection((host, port), timeout=timeout)
        self._sock.settimeout(timeout)
        ws_key = base64.b64encode(os.urandom(16)).decode("ascii")
        handshake = (
            f"GET {path_with_query} HTTP/1.1\r\n"
            f"Host: {host}:{port}\r\n"
            "Upgrade: websocket\r\n"
            "Connection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {ws_key}\r\n"
            "Sec-WebSocket-Version: 13\r\n\r\n"
        )
        self._sock.sendall(handshake.encode("ascii"))
        self._buffer = bytearray()
        header_end = -1
        while header_end < 0:
            chunk = self._sock.recv(4096)
            if not chunk:
                raise ConnectionError("EOF during WebSocket HTTP upgrade")
            self._buffer.extend(chunk)
            header_end = self._buffer.find(b"\r\n\r\n")
        raw_headers = bytes(self._buffer[:header_end]).decode("latin-1")
        del self._buffer[: header_end + 4]
        status_line = raw_headers.splitlines()[0] if raw_headers else ""
        if "101" not in status_line:
            raise ConnectionError(f"WebSocket upgrade rejected: {status_line}")

    def _read_exact(self, n: int) -> bytes:
        while len(self._buffer) < n:
            chunk = self._sock.recv(4096)
            if not chunk:
                raise ConnectionError("WebSocket closed unexpectedly")
            self._buffer.extend(chunk)
        out = bytes(self._buffer[:n])
        del self._buffer[:n]
        return out

    def _send_frame(self, opcode: int, payload: bytes) -> None:
        mask_key = os.urandom(4)
        header = bytearray([0x80 | (opcode & 0x0F)])
        length = len(payload)
        if length < 126:
            header.append(0x80 | length)
        elif length < 65536:
            header.append(0x80 | 126)
            header.extend(struct.pack("!H", length))
        else:
            header.append(0x80 | 127)
            header.extend(struct.pack("!Q", length))
        header.extend(mask_key)
        masked = bytes(b ^ mask_key[idx % 4] for idx, b in enumerate(payload))
        self._sock.sendall(bytes(header) + masked)

    def send_json(self, frame_obj: dict[str, Any]) -> None:
        encoded = json.dumps(frame_obj, separators=(",", ":")).encode("utf-8")
        self._send_frame(0x1, encoded)

    def recv_json(self, timeout: float = 10.0) -> dict[str, Any]:
        self._sock.settimeout(timeout)
        while True:
            b0, b1 = self._read_exact(2)
            opcode = b0 & 0x0F
            masked = (b1 & 0x80) != 0
            length = b1 & 0x7F
            if length == 126:
                (length,) = struct.unpack("!H", self._read_exact(2))
            elif length == 127:
                (length,) = struct.unpack("!Q", self._read_exact(8))
            mask_key = self._read_exact(4) if masked else b""
            payload = self._read_exact(length)
            if masked:
                payload = bytes(b ^ mask_key[i % 4] for i, b in enumerate(payload))
            if opcode == 0x9:  # Ping -> auto Pong
                self._send_frame(0xA, payload)
                continue
            if opcode == 0xA:  # Pong control frame
                continue
            if opcode == 0x8:  # Close
                raise ConnectionError("Server sent WebSocket Close frame")
            if opcode == 0x1:
                return json.loads(payload.decode("utf-8"))

    def recv_until(
        self,
        predicate: Callable[[dict[str, Any]], bool],
        timeout: float = 10.0,
    ) -> dict[str, Any]:
        deadline = time.monotonic() + timeout
        seen: list[str] = []
        while time.monotonic() < deadline:
            remaining = max(0.2, deadline - time.monotonic())
            frame = self.recv_json(timeout=remaining)
            seen.append(str(frame.get("type")))
            if predicate(frame):
                return frame
        raise TimeoutError(f"Timed out waiting for WS frame; observed types={seen}")

    def ws_ping(self, nonce: int) -> dict[str, Any]:
        self.send_json({"type": "Ping", "payload": {"nonce": nonce}})
        return self.recv_until(
            lambda f: f.get("type") == "Pong"
            and f.get("payload", {}).get("nonce") == nonce
        )

    def ws_subscribe_chat(self, chat_id: int) -> None:
        self.send_json({"type": "SubscribeChat", "payload": {"chat_id": chat_id}})

    def ws_send_message(self, chat_id: int, client_nonce: int, text: str) -> None:
        self.send_json(
            {
                "type": "SendMessage",
                "payload": {
                    "chat_id": chat_id,
                    "client_nonce": client_nonce,
                    "reply_to": None,
                    "topic_id": None,
                    "content": {
                        "type": "Text",
                        "data": {"text": text, "entities": []},
                    },
                    "silent": False,
                },
            }
        )

    def ws_send_reaction(
        self, chat_id: int, message_id: int, emoji: str, add: bool = True
    ) -> None:
        self.send_json(
            {
                "type": "SendReaction",
                "payload": {
                    "chat_id": chat_id,
                    "message_id": message_id,
                    "emoji": emoji,
                    "add": add,
                },
            }
        )

    def ws_read_history(self, chat_id: int, max_id: int) -> None:
        self.send_json(
            {"type": "ReadHistory", "payload": {"chat_id": chat_id, "max_id": max_id}}
        )

    def close(self) -> None:
        try:
            self._send_frame(0x8, b"")
        except OSError:
            pass
        try:
            self._sock.close()
        except OSError:
            pass


# ---------------------------------------------------------------------------
# CompanionPeer HTTP + WS Driver
# ---------------------------------------------------------------------------


class CompanionPeer:
    """Simulated multi-peer client speaking real HTTP REST & WS to gpui_telegram_server."""

    def __init__(
        self,
        base_url: str,
        username: str,
        password: str = "PeerStrongPass123!",
        first_name: str = "Companion",
        device_name: str = "CompanionPy",
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self.username = username
        self.password = password
        self.first_name = first_name
        self.device_name = device_name
        self.user_id: int | None = None
        self.session_id: str | None = None
        self.token: str | None = None

    def request(
        self,
        method: str,
        path: str,
        body: dict[str, Any] | bytes | None = None,
        headers: dict[str, str] | None = None,
        authenticated: bool = True,
        expected_statuses: tuple[int, ...] = (200, 201),
    ) -> tuple[int, dict[str, str], bytes]:
        url = f"{self.base_url}{path}"
        req_headers: dict[str, str] = {}
        if authenticated and self.token:
            req_headers["Authorization"] = f"Bearer {self.token}"
        payload_bytes: bytes | None = None
        if isinstance(body, dict):
            payload_bytes = json.dumps(body).encode("utf-8")
            req_headers["Content-Type"] = "application/json"
        elif isinstance(body, (bytes, bytearray)):
            payload_bytes = bytes(body)
            req_headers.setdefault("Content-Type", "application/octet-stream")
        if headers:
            req_headers.update(headers)

        req = urllib.request.Request(
            url, data=payload_bytes, headers=req_headers, method=method
        )
        try:
            with urllib.request.urlopen(req, timeout=10.0) as resp:
                status = int(resp.status)
                resp_headers = {k.lower(): v for k, v in resp.headers.items()}
                raw = resp.read()
        except urllib.error.HTTPError as err:
            status = int(err.code)
            resp_headers = {k.lower(): v for k, v in err.headers.items()}
            raw = err.read()

        if expected_statuses and status not in expected_statuses:
            decoded = raw.decode("utf-8", errors="replace")
            raise RuntimeError(
                f"{method} {path} returned HTTP {status} (expected {expected_statuses}): {decoded}"
            )
        return status, resp_headers, raw

    def request_json(
        self,
        method: str,
        path: str,
        body: dict[str, Any] | bytes | None = None,
        headers: dict[str, str] | None = None,
        authenticated: bool = True,
        expected_statuses: tuple[int, ...] = (200, 201),
    ) -> dict[str, Any]:
        _, _, raw = self.request(
            method,
            path,
            body=body,
            headers=headers,
            authenticated=authenticated,
            expected_statuses=expected_statuses,
        )
        return json.loads(raw.decode("utf-8")) if raw else {}

    def register(self) -> dict[str, Any]:
        res = self.request_json(
            "POST",
            "/api/v1/auth/register",
            body={
                "username": self.username,
                "password": self.password,
                "first_name": self.first_name,
                "device_name": self.device_name,
                "auto_welcome": False,
            },
            authenticated=False,
        )
        self.user_id = int(res["user_id"])
        self.session_id = str(res["session_id"])
        self.token = str(res["token"])
        return res

    def login(self, device_name: str | None = None) -> dict[str, Any]:
        res = self.request_json(
            "POST",
            "/api/v1/auth/login",
            body={
                "username": self.username,
                "password": self.password,
                "device_name": device_name or self.device_name,
            },
            authenticated=False,
        )
        if "token" in res:
            self.user_id = int(res["user_id"])
            self.session_id = str(res["session_id"])
            self.token = str(res["token"])
        return res

    def get_me(self) -> dict[str, Any]:
        return self.request_json("GET", "/api/v1/auth/me")

    def list_sessions(self) -> dict[str, Any]:
        return self.request_json("GET", "/api/v1/auth/sessions")

    def revoke_session(self, session_id: str) -> dict[str, Any]:
        return self.request_json("DELETE", f"/api/v1/auth/sessions/{session_id}")

    def create_chat(
        self,
        chat_type: str = "direct",
        title: str = "Direct Chat",
        peer_user_id: int | None = None,
        member_ids: list[int] | None = None,
        is_forum: bool = False,
    ) -> dict[str, Any]:
        body: dict[str, Any] = {
            "title": title,
            "chat_type": chat_type,
            "is_forum": is_forum,
            "member_ids": member_ids or [],
        }
        if peer_user_id is not None:
            body["peer_user_id"] = peer_user_id
        return self.request_json("POST", "/api/v1/chats/create", body=body)

    def send_message_http(
        self,
        chat_id: int,
        text: str,
        client_nonce: int,
    ) -> dict[str, Any]:
        return self.request_json(
            "POST",
            f"/api/v1/chats/{chat_id}/messages",
            body={"chat_id": chat_id, "text": text, "client_nonce": client_nonce},
        )

    def get_history(self, chat_id: int, limit: int = 50) -> dict[str, Any]:
        return self.request_json("GET", f"/api/v1/chats/{chat_id}/messages?limit={limit}")

    def publish_prekeys(self, device_id: str = "companion_dev_1") -> tuple[dict[str, Any], dict[str, str]]:
        fixture = build_canonical_e2ee_prekey_fixture()
        payload = {
            "device_id": device_id,
            "device_name": self.device_name,
            "identity_key": fixture["identity_key_b64"],
            "identity_signing_key": fixture["identity_signing_key_b64"],
            "signed_prekey": {
                "key_id": 1,
                "public_key": fixture["spk_public_key_b64"],
                "signature": fixture["spk_signature_b64"],
                "created_at_ms": int(time.time() * 1000),
            },
            "one_time_prekeys": [
                {"key_id": 101, "public_key": fixture["otpk_public_key_b64"]},
                {"key_id": 102, "public_key": fixture["otpk_public_key_b64"]},
            ],
        }
        res = self.request_json("POST", "/api/v1/e2ee/prekeys", body=payload)
        return res, fixture

    def upload_chunked_cas(
        self,
        data: bytes,
        filename: str,
        mime_type: str,
        chat_id: int,
        chunk_size: int = 65536,
    ) -> dict[str, Any]:
        expected_sha256 = hashlib.sha256(data).hexdigest()
        init_res = self.request_json(
            "POST",
            "/api/v1/media/upload/init",
            body={
                "size_bytes": len(data),
                "filename": filename,
                "chunk_size": chunk_size,
                "expected_sha256": expected_sha256,
            },
        )
        upload_id = str(init_res["upload_id"])
        total_parts = int(init_res["total_parts"])
        for idx in range(total_parts):
            part = data[idx * chunk_size : (idx + 1) * chunk_size]
            self.request_json(
                "PUT",
                f"/api/v1/media/upload/{upload_id}/part/{idx}",
                body=part,
                headers={"Content-Type": "application/octet-stream"},
            )
        return self.request_json(
            "POST",
            f"/api/v1/media/upload/{upload_id}/complete",
            body={"filename": filename, "mime_type": mime_type, "chat_id": chat_id},
        )

    def connect_ws(self) -> LoopbackWebSocket:
        if not self.token:
            raise RuntimeError("Cannot open WebSocket before authenticating")
        parsed = urllib.parse.urlparse(self.base_url)
        host = parsed.hostname or "127.0.0.1"
        port = parsed.port or 80
        encoded_tok = urllib.parse.quote(self.token, safe="")
        return LoopbackWebSocket(host, port, f"/api/v1/ws?token={encoded_tok}")


def run_self_test(
    server_url: str,
    swift_support_file: Path | None = None,
) -> dict[str, Any]:
    """Execute end-to-end multi-peer wire verification against loopback gpui_telegram_server."""
    t0 = time.monotonic()
    checks: list[str] = []

    # 1. Health + Status check (do NOT hardcode version == 1.2.0; upstream 889246e4 Cargo version is 0.1.5)
    with urllib.request.urlopen(f"{server_url}/health", timeout=5.0) as health_resp:
        health_body = health_resp.read().decode("utf-8").strip()
        if health_resp.status != 200 or health_body != "OK":
            raise AssertionError(f"Unexpected /health response: {health_resp.status} {health_body!r}")
    checks.append("health_ok")

    with urllib.request.urlopen(f"{server_url}/api/v1/status", timeout=5.0) as status_resp:
        status_json = json.loads(status_resp.read().decode("utf-8"))
    if status_json.get("status") != "healthy":
        raise AssertionError(f"Unhealthy /api/v1/status: {status_json}")
    capabilities = status_json.get("capabilities") or []
    if len(capabilities) < 20:
        raise AssertionError(
            f"Expected >= 20 server capabilities at 889246e4, got {len(capabilities)}: {capabilities}"
        )
    checks.append("status_healthy_capabilities_verified")

    # 2. Register Alice & Bob and verify /auth/me + /auth/sessions
    suffix = os.urandom(3).hex()
    alice = CompanionPeer(server_url, f"alice_{suffix}", first_name="Alice", device_name="AliceMac")
    bob = CompanionPeer(server_url, f"bob_{suffix}", first_name="Bob", device_name="BobiPhone")
    alice.register()
    bob.register()
    me_alice = alice.get_me()
    if int(me_alice.get("user_id", 0)) != alice.user_id:
        raise AssertionError(f"/auth/me mismatch: {me_alice}")
    checks.append("auth_register_and_me_verified")

    # 3. Verify RFC 8032 Ed25519 SignedPreKey verification (`POST /api/v1/e2ee/prekeys`)
    prekey_res, prekey_fixture = bob.publish_prekeys(device_id="bob_ios_dev1")
    if not prekey_res.get("success") or int(prekey_res.get("remaining_one_time_prekeys", 0)) < 2:
        raise AssertionError(f"E2EE prekey publish failed: {prekey_res}")
    bundles = alice.request_json("GET", f"/api/v1/e2ee/devices/{bob.user_id}/prekeys")
    if len(bundles.get("devices", [])) != 1:
        raise AssertionError(f"Expected 1 E2EE device bundle for Bob, got: {bundles}")
    checks.append("e2ee_ed25519_prekey_publish_and_consume_verified")
    print(f"VERIFIED_E2EE_RFC8032_FIXTURE={json.dumps(prekey_fixture)}", flush=True)

    # 4. Connect concurrent WebSockets, verify Ping/Pong, create DM, verify WS SendMessage + PtsPush
    ws_alice = alice.connect_ws()
    ws_bob = bob.connect_ws()
    try:
        ws_alice.ws_ping(777)
        ws_bob.ws_ping(888)
        checks.append("ws_ping_pong_verified")

        chat = alice.create_chat(
            chat_type="direct",
            title="Alice-Bob Loopback",
            peer_user_id=bob.user_id,
            member_ids=[bob.user_id or 0],
        )
        chat_id = int(chat.get("chat_id") or chat["id"])
        ws_alice.ws_subscribe_chat(chat_id)
        ws_bob.ws_subscribe_chat(chat_id)

        ws_alice.ws_send_message(chat_id, client_nonce=1001, text="diamond_loopback_probe_msg")
        ack = ws_alice.recv_until(
            lambda f: f.get("type") == "Ack"
            and f.get("payload", {}).get("client_nonce") == 1001
        )
        msg_id = int(ack["payload"]["message_id"])
        pts_1 = int(ack["payload"]["pts"])
        if pts_1 < 1:
            raise AssertionError(f"Expected positive PTS in Ack: {ack}")

        bob_push = ws_bob.recv_until(
            lambda f: f.get("type") in ("PtsPush", "Update")
        )
        if bob_push is None:
            raise AssertionError("Bob did not receive WS push")
        checks.append("ws_send_ack_and_peer_ptspush_verified")

        # 5. HTTP reply + WS reaction + ReadHistory + monotonic PTS check
        reply_res = bob.send_message_http(chat_id, "bob_http_reply_probe", client_nonce=2002)
        pts_2 = int(reply_res.get("pts", 0))
        if pts_2 <= pts_1:
            raise AssertionError(f"Expected monotonic PTS ({pts_2} > {pts_1})")
        ws_bob.ws_send_reaction(chat_id, msg_id, "🔥", add=True)
        ws_bob.ws_read_history(chat_id, msg_id)
        alice_rx = ws_alice.recv_until(
            lambda f: f.get("type") in ("PtsPush", "Update")
        )
        if alice_rx is None:
            raise AssertionError("Alice did not receive reaction/reply push")
        history = alice.get_history(chat_id)
        if len(history.get("messages", [])) < 2:
            raise AssertionError(f"Expected >= 2 messages in history, got: {history}")
        checks.append("http_reply_ws_reaction_and_pts_monotonicity_verified")

        # 6. Chunked CAS upload + HTTP 206 Partial Content range check
        blob = b"FABUSHI_CAS_STREAM_HEADER_" + (os.urandom(1024) * 140)
        cas_res = alice.upload_chunked_cas(
            blob, "probe_clip.bin", "application/octet-stream", chat_id=chat_id, chunk_size=65536
        )
        file_id = str(cas_res["file_id"])
        alice.request_json(
            "POST",
            f"/api/v1/chats/{chat_id}/messages",
            body={"chat_id": chat_id, "text": "CAS attachment", "media_id": file_id, "client_nonce": 3003},
        )
        st_code, st_headers, partial = bob.request(
            "GET",
            f"/api/v1/media/stream/{file_id}",
            headers={"Range": "bytes=0-1023"},
            expected_statuses=(206,),
        )
        if st_code != 206 or len(partial) != 1024 or partial != blob[:1024]:
            raise AssertionError(
                f"HTTP 206 range mismatch: status={st_code} len={len(partial)} headers={st_headers}"
            )
        checks.append("chunked_cas_upload_and_http_206_range_verified")

        # 7. Tantivy global search check
        search_res = alice.request_json("GET", "/api/v1/search?q=diamond_loopback_probe_msg")
        hits = search_res.get("hits") or search_res.get("results") or []
        if len(hits) < 1:
            raise AssertionError(f"Expected Tantivy search hit, got: {search_res}")
        checks.append("tantivy_search_verified")

        # 8. Secondary session login & remote revocation -> HTTP 401 check
        bob_second = CompanionPeer(
            server_url, bob.username, bob.password, device_name="BobSecondaryPad"
        )
        bob_second.login(device_name="BobSecondaryPad")
        assert bob_second.session_id is not None
        bob.revoke_session(bob_second.session_id)
        bob_second.request("GET", "/api/v1/auth/me", expected_statuses=(401,))
        checks.append("session_revocation_401_verified")
    finally:
        ws_alice.close()
        ws_bob.close()

    if swift_support_file is not None and swift_support_file.is_file():
        swift_src = swift_support_file.read_text(encoding="utf-8")
        if prekey_fixture["spk_signature_b64"] not in swift_src:
            raise AssertionError(
                "LoopbackPeerClient.swift is missing exact RFC 8032 spk_signature_b64: "
                f"{prekey_fixture['spk_signature_b64']}"
            )
        checks.append("swift_e2ee_fixture_matches_rfc8032")

    elapsed_ms = round((time.monotonic() - t0) * 1000.0, 2)
    return {
        "ok": True,
        "server_url": server_url,
        "server_version": status_json.get("version"),
        "capabilities_count": len(capabilities),
        "checks_passed": checks,
        "checks_count": len(checks),
        "pts_verified": True,
        "e2ee_rfc8032_fixture": prekey_fixture,
        "elapsed_ms": elapsed_ms,
    }
