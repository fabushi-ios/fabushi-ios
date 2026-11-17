#!/usr/bin/env python3
"""Ephemeral loopback gpui_telegram_server builder and process lifecycle manager.

Supports both:
1. Upgraded in-tree `source/desktop_core` (once M1 merges `routes/mod.rs`), and
2. Automatic shallow checkout of canonical upstream `bhrum/gpui-telegram` pinned to
   `c40494379cd03f71a2c1f57bae5f85d677cb0e3e` inside `$RUNNER_TEMP` / `$TMPDIR`
   while `source/desktop_core` is still pre-M1 `v0.1.0`.

Stages compiled binaries outside repo nested `target/` directories so
`scripts/clean_build_garbage.py` never deletes the server executable during `xcodebuild`.
"""

from __future__ import annotations

import json
import os
import shutil
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any

UPSTREAM_REPO_URL = "https://github.com/bhrum/gpui-telegram.git"
UPSTREAM_COMMIT_SHA = "c40494379cd03f71a2c1f57bae5f85d677cb0e3e"


def resolve_temp_root() -> Path:
    """Resolve CI-safe temp directory without hardcoding /tmp."""
    raw = (
        os.environ.get("RUNNER_TEMP")
        or os.environ.get("TMPDIR")
        or tempfile.gettempdir()
    )
    path = Path(raw).resolve()
    path.mkdir(parents=True, exist_ok=True)
    return path


@dataclass(frozen=True)
class ServerWorkspaceInfo:
    manifest_path: Path
    workspace_dir: Path
    source_mode: str
    commit_sha: str


def is_local_desktop_core_upgraded(repo_root: Path) -> bool:
    """Detect whether in-tree source/desktop_core has the M1 v1.2.0/0.1.5 modular server."""
    routes_mod = (
        repo_root
        / "source"
        / "desktop_core"
        / "crates"
        / "gpui_telegram_server"
        / "src"
        / "routes"
        / "mod.rs"
    )
    cargo_toml = repo_root / "source" / "desktop_core" / "Cargo.toml"
    return routes_mod.is_file() and cargo_toml.is_file()


def resolve_server_workspace(repo_root: Path, temp_root: Path | None = None) -> ServerWorkspaceInfo:
    """Resolve either upgraded local source/desktop_core or pinned upstream checkout."""
    resolved_temp = temp_root or resolve_temp_root()
    if is_local_desktop_core_upgraded(repo_root):
        head = subprocess.check_output(
            ["git", "-C", str(repo_root), "rev-parse", "HEAD"],
            text=True,
        ).strip()
        workspace_dir = repo_root / "source" / "desktop_core"
        return ServerWorkspaceInfo(
            manifest_path=workspace_dir / "Cargo.toml",
            workspace_dir=workspace_dir,
            source_mode="local_desktop_core_upgraded",
            commit_sha=head,
        )

    cache_base = Path(
        os.environ.get("GPUI_UPSTREAM_CACHE_DIR")
        or str(resolved_temp / "gpui-upstream-cache")
    ).resolve()
    clone_dir = cache_base / f"gpui-telegram-{UPSTREAM_COMMIT_SHA[:8]}"
    manifest = clone_dir / "Cargo.toml"

    needs_fetch = True
    if manifest.is_file() and (clone_dir / ".git").exists():
        try:
            current_sha = subprocess.check_output(
                ["git", "-C", str(clone_dir), "rev-parse", "HEAD"],
                text=True,
                stderr=subprocess.DEVNULL,
            ).strip()
            if current_sha == UPSTREAM_COMMIT_SHA:
                needs_fetch = False
        except subprocess.CalledProcessError:
            needs_fetch = True

    if needs_fetch:
        if clone_dir.exists():
            shutil.rmtree(clone_dir, ignore_errors=True)
        clone_dir.mkdir(parents=True, exist_ok=True)
        subprocess.run(["git", "init", str(clone_dir)], check=True)
        subprocess.run(
            [
                "git",
                "-C",
                str(clone_dir),
                "fetch",
                "--depth",
                "1",
                UPSTREAM_REPO_URL,
                UPSTREAM_COMMIT_SHA,
            ],
            check=True,
        )
        subprocess.run(
            ["git", "-C", str(clone_dir), "checkout", "--detach", "FETCH_HEAD"],
            check=True,
        )
        verified_sha = subprocess.check_output(
            ["git", "-C", str(clone_dir), "rev-parse", "HEAD"],
            text=True,
        ).strip()
        if verified_sha != UPSTREAM_COMMIT_SHA:
            raise RuntimeError(
                f"Upstream pin mismatch: expected {UPSTREAM_COMMIT_SHA}, got {verified_sha}"
            )

    return ServerWorkspaceInfo(
        manifest_path=manifest,
        workspace_dir=clone_dir,
        source_mode="upstream_pinned_c4049437",
        commit_sha=UPSTREAM_COMMIT_SHA,
    )


def ensure_server_binary(
    repo_root: Path,
    temp_root: Path | None = None,
) -> tuple[Path, ServerWorkspaceInfo]:
    """Compile gpui_telegram_server and stage executable outside repo nested target/."""
    resolved_temp = temp_root or resolve_temp_root()
    ws_info = resolve_server_workspace(repo_root, resolved_temp)

    cargo_target_dir = Path(
        os.environ.get("GPUI_SERVER_CARGO_TARGET_DIR")
        or str(resolved_temp / "cargo-target-gpui-server")
    ).resolve()
    cargo_target_dir.mkdir(parents=True, exist_ok=True)

    manifest_text = ws_info.manifest_path.read_text(encoding="utf-8")
    use_ci_profile = "[profile.ci]" in manifest_text

    cmd = [
        "cargo",
        "build",
        "--manifest-path",
        str(ws_info.manifest_path),
        "-p",
        "gpui_telegram_server",
        "--bin",
        "gpui_telegram_server",
    ]
    if use_ci_profile:
        cmd.extend(["--profile", "ci"])

    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(cargo_target_dir)
    env["CARGO_INCREMENTAL"] = "0"

    subprocess.run(cmd, env=env, check=True)

    candidates = [
        cargo_target_dir / "ci" / "gpui_telegram_server",
        cargo_target_dir / "debug" / "gpui_telegram_server",
        cargo_target_dir / "release" / "gpui_telegram_server",
    ]
    built_bin = next((p for p in candidates if p.is_file()), None)
    if built_bin is None:
        raise FileNotFoundError(
            f"gpui_telegram_server binary not found under {cargo_target_dir}"
        )

    stage_dir = resolved_temp / "gpui-e2e-bin"
    stage_dir.mkdir(parents=True, exist_ok=True)
    staged_bin = stage_dir / "gpui_telegram_server"
    shutil.copy2(built_bin, staged_bin)
    staged_bin.chmod(0o755)
    return staged_bin, ws_info


def allocate_loopback_port() -> int:
    """Reserve an ephemeral IPv4 port on 127.0.0.1."""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


class LoopbackServerInstance:
    """Manages an isolated gpui_telegram_server daemon on 127.0.0.1:<port>."""

    def __init__(
        self,
        binary_path: Path,
        temp_root: Path | None = None,
        port: int | None = None,
        keep_artifacts_on_stop: bool = False,
    ) -> None:
        self.binary_path = binary_path
        self.temp_root = temp_root or resolve_temp_root()
        self.port = port or allocate_loopback_port()
        self.keep_artifacts_on_stop = keep_artifacts_on_stop
        self.run_dir = self.temp_root / f"gpui_e2e_run_{uuid.uuid4().hex[:12]}"
        self.db_dir = self.run_dir / "db"
        self.media_dir = self.run_dir / "media_cas"
        self.cwd_dir = self.run_dir / "cwd"
        self.log_path = self.run_dir / "server.log"
        self._proc: subprocess.Popen[bytes] | None = None
        self._log_handle: Any = None
        self.status_payload: dict[str, Any] = {}

    @property
    def base_url(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    @property
    def ws_url(self) -> str:
        return f"ws://127.0.0.1:{self.port}/api/v1/ws"

    def start(self, readiness_timeout_sec: float = 30.0) -> dict[str, Any]:
        for directory in (self.db_dir, self.media_dir, self.cwd_dir):
            directory.mkdir(parents=True, exist_ok=True)

        db_file = (self.db_dir / "gpui_telegram.db").resolve()
        env = os.environ.copy()
        env.update(
            {
                "HOST": "127.0.0.1",
                "PORT": str(self.port),
                "DATABASE_URL": f"sqlite://{db_file}",
                "MEDIA_STORAGE_PATH": str(self.media_dir.resolve()),
                "DEFAULT_AGENT_MODEL": "deepseek-flash",
                "GPUI_OFFLINE_TEST": "1",
                "RUST_LOG": env.get("RUST_LOG", "info"),
            }
        )
        # Ensure debug OTP codes remain enabled for deterministic phone/email E2E tests.
        env.pop("GPUI_HIDE_DEBUG_CODE", None)

        self._log_handle = open(self.log_path, "wb")
        self._proc = subprocess.Popen(
            [str(self.binary_path)],
            cwd=str(self.cwd_dir),
            env=env,
            stdout=self._log_handle,
            stderr=subprocess.STDOUT,
        )

        deadline = time.monotonic() + readiness_timeout_sec
        last_error = "not started"
        while time.monotonic() < deadline:
            if self._proc.poll() is not None:
                tail = self.read_log_tail()
                raise RuntimeError(
                    f"gpui_telegram_server exited prematurely with code {self._proc.returncode}:\n{tail}"
                )
            try:
                with urllib.request.urlopen(f"{self.base_url}/health", timeout=1.5) as resp:
                    health_body = resp.read().decode("utf-8", errors="replace").strip()
                    if resp.status != 200 or health_body != "OK":
                        last_error = f"/health status={resp.status} body={health_body!r}"
                        time.sleep(0.1)
                        continue
                with urllib.request.urlopen(
                    f"{self.base_url}/api/v1/status", timeout=1.5
                ) as status_resp:
                    status_json = json.loads(
                        status_resp.read().decode("utf-8", errors="replace")
                    )
                    if (
                        status_resp.status == 200
                        and status_json.get("status") == "healthy"
                    ):
                        self.status_payload = status_json
                        return status_json
                    last_error = f"/api/v1/status payload={status_json!r}"
            except (urllib.error.URLError, TimeoutError, ConnectionError, ValueError) as exc:
                last_error = str(exc)
            time.sleep(0.1)

        tail = self.read_log_tail()
        self.stop()
        raise TimeoutError(
            f"Timed out waiting for gpui_telegram_server on {self.base_url} ({last_error}):\n{tail}"
        )

    def read_log_tail(self, max_bytes: int = 8192) -> str:
        if self._log_handle is not None:
            try:
                self._log_handle.flush()
            except OSError:
                pass
        if not self.log_path.is_file():
            return "<no server log>"
        raw = self.log_path.read_bytes()
        return raw[-max_bytes:].decode("utf-8", errors="replace")

    def copy_log_to(self, destination: Path) -> None:
        if self._log_handle is not None:
            try:
                self._log_handle.flush()
            except OSError:
                pass
        if self.log_path.is_file():
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(self.log_path, destination)

    def stop(self) -> None:
        if self._proc is not None:
            if self._proc.poll() is None:
                self._proc.terminate()
                try:
                    self._proc.wait(timeout=5.0)
                except subprocess.TimeoutExpired:
                    self._proc.kill()
                    self._proc.wait(timeout=3.0)
            self._proc = None
        if self._log_handle is not None:
            try:
                self._log_handle.close()
            except OSError:
                pass
            self._log_handle = None
        if not self.keep_artifacts_on_stop and self.run_dir.exists():
            shutil.rmtree(self.run_dir, ignore_errors=True)

    def __enter__(self) -> LoopbackServerInstance:
        self.start()
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.stop()
