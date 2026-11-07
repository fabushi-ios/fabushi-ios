import { hostname } from "node:os";
import { join } from "node:path";

import type { JsonValue } from "@bufbuild/protobuf";

import { createDeadlinePolicy, createIdleWatchdogPolicy, createPollingPolicy, createRetryPolicy, realClock } from "../../internal/scheduling.js";
import { errorLogTag, errorMessage } from "../../shared/errors.js";
import { GATEWAY_AUTH_SCHEME } from "../../shared/gateway-wire.js";
import {
  DEFAULT_MAX_LOCAL_EXEC_FILE_BYTES, GATEWAY_LOCAL_EXEC_REQUESTS_PATH, GATEWAY_LOCAL_EXEC_RESPONSES_PATH,
  SAND_LOCAL_EXEC_CONTROL_POST_TIMEOUT_MS, SAND_LOCAL_EXEC_DATA_POST_TIMEOUT_MS, SAND_LOCAL_EXEC_HEARTBEAT_INTERVAL_MS,
  localExecFileTooLargeMessage, localExecUploadFrameTooLargeMessage, maxLocalExecUploadFrameBytes
} from "../../shared/local-exec-gateway.js";
import { describeLocalExec, type LocalExecMessage, type SandLocalToolRequest } from "../../shared/local-tool-permission-machinery.js";
import { containPath, resolveLocalExecRoot } from "./local-exec-machine.js";
import { ProductionNativeLocalExecFileAccess, type NativeLocalExecFileAccess } from "./local-exec-native-file-access.js";
import type { LocalExecConnection } from "./local-exec-daemon-protocol.js";

export class SandLocalExecStreamError extends Error {}
export const SSE_RECONNECT_MIN_MS = 1_000;
export const SSE_RECONNECT_MAX_MS = 10_000;
export const SSE_STALL_TIMEOUT_MS = 35_000;
export const STALE_CONNECTION_FAILURE_THRESHOLD = 3;
export const RESPONSE_POST_RETRIES = 3;
export const RESPONSE_POST_RETRY_DELAY_MS = 200;
export const DEFAULT_SAND_COMPUTER_ID = "this-computer";
export const LOCAL_EXEC_UPLOAD_CHUNK_BYTES = 256 * 1024;

export interface LocalExecDecodedMessage { id: number; readonly message?: { readonly case?: string; readonly value?: Record<string, unknown> }; }
export interface LocalExecExecutorOutput { readonly kind: "client" | "control"; readonly message: JsonValue; }
export interface LocalExecExecutor {
  decodeServerMessage(json: JsonValue): LocalExecDecodedMessage;
  execute(message: LocalExecDecodedMessage, signal: AbortSignal): AsyncIterable<LocalExecExecutorOutput>;
  cancel(execId: number): void;
  throwControl(error: string): JsonValue;
}

function normalizeDescribableMessage(
  decoded: LocalExecDecodedMessage
): { readonly message: LocalExecMessage } | undefined {
  const message = decoded.message;
  if (
    typeof message?.case !== "string" ||
    message.value == null ||
    typeof message.value !== "object" ||
    Array.isArray(message.value)
  ) return undefined;
  return { message: { case: message.case, value: message.value } };
}

function isLocalExecOverwriteAllowed(value: boolean | undefined): boolean {
  const overwrite = value !== false;
  if (overwrite === false) {
    return false;
  }
  return true;
}

export type LocalExecRequestFrame =
  | { readonly kind: "welcome"; readonly providerId: string }
  | { readonly kind: "retire-approval"; readonly approvalId: string }
  | { readonly kind: "exec"; readonly requestId: string; readonly approvalId?: string; readonly serverMessage: JsonValue }
  | { readonly kind: "upload"; readonly requestId: string; readonly approvalId?: string; readonly path: string; readonly bytesBase64: string; readonly sha256?: string; readonly expectedVersion?: string; readonly overwrite?: boolean }
  | { readonly kind: "upload-start"; readonly requestId: string; readonly approvalId?: string; readonly transferId: string; readonly path: string; readonly totalBytes: number; readonly sha256: string; readonly expectedVersion?: string; readonly overwrite?: boolean }
  | { readonly kind: "upload-chunk"; readonly requestId: string; readonly approvalId?: string; readonly transferId: string; readonly path: string; readonly offset: number; readonly bytesBase64: string }
  | { readonly kind: "upload-commit"; readonly requestId: string; readonly approvalId?: string; readonly transferId: string; readonly path: string }
  | { readonly kind: "download"; readonly requestId: string; readonly approvalId?: string; readonly path: string }
  | { readonly kind: "cancel"; readonly requestId: string };

export type LocalExecResponseFrame =
  | { readonly kind: "hello"; readonly localRoot: string; readonly terminalsFolder: string; readonly computerId: string; readonly label: string; readonly supervised?: boolean; readonly variant?: string }
  | { readonly kind: "ping"; readonly supervised?: boolean }
  | { readonly kind: "client" | "control"; readonly requestId: string; readonly message: JsonValue }
  | { readonly kind: "file"; readonly requestId: string; readonly bytesBase64?: string; readonly sha256?: string; readonly version?: string; readonly size?: number }
  | { readonly kind: "file-error"; readonly requestId: string; readonly error: string };

export interface SandLocalExecProviderOptions {
  readonly resolveConnection: () => Promise<LocalExecConnection>;
  readonly executor: LocalExecExecutor;
  readonly root?: string; readonly computerId?: string; readonly computerLabel?: string; readonly maxFileBytes?: number; readonly variant?: string;
  readonly fetch?: typeof fetch;
  readonly onConnectionStale?: () => Promise<void> | void;
  readonly isSupervised?: () => Promise<boolean | undefined> | boolean | undefined;
  readonly onApprovalRetired?: (approvalId: string) => Promise<void> | void;
  readonly isLocalUseBlocked?: (request: { readonly approvalId?: string; readonly describes?: SandLocalToolRequest; readonly terminalsFolder: string }) => Promise<string | undefined> | string | undefined;
  readonly onInflightChange?: (count: number) => void;
  readonly fileAccess?: NativeLocalExecFileAccess;
}

export function withLocalExecAuth(headers: Record<string, string>, connection: LocalExecConnection): Record<string, string> {
  const merged = { ...headers, ...connection.headers };
  if (connection.token != null && connection.token.length > 0) merged.authorization = `${GATEWAY_AUTH_SCHEME} ${connection.token}`;
  return merged;
}
export function isSameLocalExecConnection(a: LocalExecConnection, b: LocalExecConnection): boolean { return a.baseUrl === b.baseUrl && a.token === b.token && JSON.stringify(a.headers ?? {}) === JSON.stringify(b.headers ?? {}); }

export class SandLocalExecProvider {
  readonly root: string; readonly terminalsFolder: string; readonly computerId: string; readonly computerLabel: string; readonly maxFileBytes: number; readonly maxUploadFrameBytes: number;
  private readonly fetcher: typeof fetch; private closed = false; private abortController?: AbortController; private nextExecId = 1; private readonly inflight = new Map<string, { execId: number; controller: AbortController }>();
  private readonly uploads = new Map<string, { path: string; target: string; totalBytes: number; receivedBytes: number; expectedSha256: string; startingVersion?: string; overwrite: boolean }>();
  private readonly fileAccess: NativeLocalExecFileAccess;
  private readonly outbox: LocalExecResponseFrame[] = []; private flushing = false; private providerId: string | undefined;
  private readonly dataPostDeadline = createDeadlinePolicy(realClock, { name: "local-exec-data-post", timeoutMs: SAND_LOCAL_EXEC_DATA_POST_TIMEOUT_MS });
  private readonly controlPostDeadline = createDeadlinePolicy(realClock, { name: "local-exec-control-post", timeoutMs: SAND_LOCAL_EXEC_CONTROL_POST_TIMEOUT_MS });
  private readonly postRetry = createRetryPolicy(realClock, { name: "local-exec-response-post", maxAttempts: RESPONSE_POST_RETRIES, initialDelayMs: RESPONSE_POST_RETRY_DELAY_MS, maxDelayMs: RESPONSE_POST_RETRY_DELAY_MS });
  private readonly reconnectBackoff = createRetryPolicy(realClock, { name: "local-exec-sse-reconnect", maxAttempts: Number.MAX_SAFE_INTEGER, initialDelayMs: SSE_RECONNECT_MIN_MS, maxDelayMs: SSE_RECONNECT_MAX_MS });
  private readonly stallWatchdog = createIdleWatchdogPolicy(realClock, { name: "local-exec-sse-stall", idleMs: SSE_STALL_TIMEOUT_MS });
  private readonly heartbeat = createPollingPolicy(realClock, { name: "local-exec-heartbeat", intervalMs: SAND_LOCAL_EXEC_HEARTBEAT_INTERVAL_MS });

  constructor(private readonly options: SandLocalExecProviderOptions) {
    this.root = options.root ?? resolveLocalExecRoot(); this.terminalsFolder = join(this.root, "terminals");
    const host = hostname().trim(); this.computerId = options.computerId?.trim() || host || DEFAULT_SAND_COMPUTER_ID; this.computerLabel = options.computerLabel?.trim() || host || "this computer";
    this.maxFileBytes = options.maxFileBytes ?? DEFAULT_MAX_LOCAL_EXEC_FILE_BYTES; this.maxUploadFrameBytes = maxLocalExecUploadFrameBytes(this.maxFileBytes); this.fetcher = options.fetch ?? fetch;
    this.fileAccess = options.fileAccess ?? new ProductionNativeLocalExecFileAccess({ root: this.root });
  }
  start(): void { void this.runLoop(); }
  close(): void {
    this.closed = true;
    this.abortController?.abort();
    for (const entry of this.inflight.values()) entry.controller.abort();
    this.uploads.clear();
    this.fileAccess.close();
  }
  private enqueue(frame: LocalExecResponseFrame): void { this.outbox.push(frame); void this.flush(); }
  private async postBatch(frames: readonly LocalExecResponseFrame[], deadline = this.dataPostDeadline): Promise<boolean> {
    try { return await this.postRetry.runWithRetry(async () => { const connection = await this.options.resolveConnection(); const response = await deadline.run((signal) => this.fetcher(`${connection.baseUrl}${GATEWAY_LOCAL_EXEC_RESPONSES_PATH}`, { method: "POST", headers: withLocalExecAuth({ "content-type": "application/json" }, connection), body: JSON.stringify({ providerId: this.providerId, frames }), signal })); if (!response.ok) { const failure = new Error("response POST failed") as Error & { code?: string }; failure.code = `http_${response.status}`; throw failure; } return true; }); }
    catch (error) { console.error(`[local-exec-provider] dropping ${frames.length}-frame response batch after ${RESPONSE_POST_RETRIES} failed POSTs: ${errorLogTag(error)}`); return false; }
  }
  private async flush(): Promise<void> { if (this.flushing) return; this.flushing = true; try { while (this.outbox.length > 0 && !this.closed) { const frames = this.outbox.slice(); const posted = await this.postBatch(frames); this.outbox.splice(0, frames.length); if (!posted) break; } } finally { this.flushing = false; } }
  private async runLoop(): Promise<void> { let reconnectAttempt = 1; let failures = 0; while (!this.closed) { try { await this.streamRequests(() => { reconnectAttempt = 1; failures = 0; }); } catch { failures += 1; } if (this.closed) break; if (failures >= STALE_CONNECTION_FAILURE_THRESHOLD && this.options.onConnectionStale != null) { failures = 0; try { await this.options.onConnectionStale(); } catch (error) { console.error(`[local-exec-provider] stale-connection re-resolve failed: ${errorLogTag(error)}`); } } const delay = this.reconnectBackoff.schedule(reconnectAttempt); try { await delay.elapsed; } finally { delay.dispose(); } reconnectAttempt += 1; } }
  private async dialedConnectionState(dialed: LocalExecConnection): Promise<"same" | "moved" | "unreadable"> { try { return isSameLocalExecConnection(await this.options.resolveConnection(), dialed) ? "same" : "moved"; } catch { return "unreadable"; } }
  private async currentSupervised(): Promise<boolean | undefined> { return await this.options.isSupervised?.(); }
  async streamRequests(resetBackoff: () => void = () => {}): Promise<void> {
    const connection = await this.options.resolveConnection(); const controller = new AbortController(); this.abortController = controller;
    const response = await this.fetcher(`${connection.baseUrl}${GATEWAY_LOCAL_EXEC_REQUESTS_PATH}`, { headers: withLocalExecAuth({ accept: "text/event-stream" }, connection), signal: controller.signal });
    if (!response.ok || response.body == null) throw new SandLocalExecStreamError(`local-exec requests stream failed: ${response.status}`);
    resetBackoff(); void this.currentSupervised().then((supervised) => this.postBatch([{ kind: "hello", localRoot: this.root, terminalsFolder: this.terminalsFolder, computerId: this.computerId, label: this.computerLabel, ...(supervised === undefined ? {} : { supervised }), ...(this.options.variant === undefined ? {} : { variant: this.options.variant }) }], this.controlPostDeadline));
    const reader = response.body.getReader(); const decoder = new TextDecoder(); let buffer = ""; const stall = this.stallWatchdog.arm(() => controller.abort());
    const heartbeat = this.heartbeat.start(async () => { const supervised = await this.currentSupervised(); void this.postBatch([{ kind: "ping", ...(supervised === undefined ? {} : { supervised }) }], this.controlPostDeadline); if (await this.dialedConnectionState(connection) === "moved") controller.abort(); });
    try { let dropping = false; for (;;) { const { done, value } = await reader.read(); if (done) break; stall.kick(); buffer += decoder.decode(value, { stream: true }); if (dropping) { const boundary = buffer.indexOf("\n\n"); if (boundary < 0) { buffer = buffer.slice(-1); continue; } buffer = buffer.slice(boundary + 2); dropping = false; } let separator = buffer.indexOf("\n\n"); while (separator >= 0) { if (separator > this.maxUploadFrameBytes) this.rejectOversizedFrame(buffer); else this.dispatchEventBlock(buffer.slice(0, separator)); buffer = buffer.slice(separator + 2); separator = buffer.indexOf("\n\n"); } if (buffer.length > this.maxUploadFrameBytes) { this.rejectOversizedFrame(buffer); buffer = buffer.slice(-1); dropping = true; } } }
    finally { stall.dispose(); heartbeat.dispose(); this.providerId = undefined; }
  }
  private rejectOversizedFrame(prefix: string): void { const match = /"requestId"\s*:\s*"([^"\\]+)"/.exec(prefix.slice(0, 4096)); if (match?.[1] != null) this.enqueue({ kind: "file-error", requestId: match[1], error: localExecUploadFrameTooLargeMessage(this.maxFileBytes) }); }
  private dispatchEventBlock(block: string): void { const lines = block.split("\n").filter((line) => line.startsWith("data:")).map((line) => line.slice(5).trim()); if (lines.length === 0) return; try { void this.handleRequest(JSON.parse(lines.join("\n")) as LocalExecRequestFrame); } catch (error) { console.error(`[local-exec-provider] dropping unparseable request frame: ${errorLogTag(error)}`); } }
  async handleRequest(frame: LocalExecRequestFrame): Promise<void> {
    if (frame.kind === "retire-approval") { await this.options.onApprovalRetired?.(frame.approvalId); return; }
    if (frame.kind === "exec" || frame.kind === "upload" || frame.kind === "upload-start" || frame.kind === "upload-chunk" || frame.kind === "upload-commit" || frame.kind === "download") {
      const described = this.describeFrame(frame);
      const blocked = await this.options.isLocalUseBlocked?.({ ...(frame.approvalId === undefined ? {} : { approvalId: frame.approvalId }), ...(described === undefined ? {} : { describes: described }), terminalsFolder: this.terminalsFolder });
      if (blocked !== undefined) { this.refuse(frame.requestId, frame.kind, blocked); return; }
    }
    switch (frame.kind) {
      case "welcome": this.providerId = frame.providerId; return;
      case "exec": return this.handleExec(frame.requestId, frame.serverMessage);
      case "upload": return this.handleUpload(frame);
      case "upload-start": return this.handleUploadStart(frame);
      case "upload-chunk": return this.handleUploadChunk(frame);
      case "upload-commit": return this.handleUploadCommit(frame);
      case "download": return this.handleDownload(frame.requestId, frame.path);
      case "cancel": return this.handleCancel(frame.requestId);
    }
  }
  private describeFrame(frame: Extract<LocalExecRequestFrame, { kind: "exec" | "upload" | "upload-start" | "upload-chunk" | "upload-commit" | "download" }>): SandLocalToolRequest | undefined {
    if (frame.kind === "upload" || frame.kind === "upload-start" || frame.kind === "upload-chunk" || frame.kind === "upload-commit") return { action: "write-file", target: frame.path };
    if (frame.kind === "download") return { action: "read-file", target: frame.path, attachToResourcePath: frame.path };
    try { const decoded = this.options.executor.decodeServerMessage(frame.serverMessage); const normalized = normalizeDescribableMessage(decoded); return normalized === undefined ? undefined : describeLocalExec(normalized, this.terminalsFolder); } catch { return undefined; }
  }
  private refuse(requestId: string, kind: "exec" | "upload" | "upload-start" | "upload-chunk" | "upload-commit" | "download", reason: string): void { if (kind === "exec") this.enqueue({ kind: "control", requestId, message: this.options.executor.throwControl(reason) }); else this.enqueue({ kind: "file-error", requestId, error: reason }); }
  private async handleExec(requestId: string, json: JsonValue): Promise<void> { let message: LocalExecDecodedMessage; try { message = this.options.executor.decodeServerMessage(json); } catch (error) { this.enqueue({ kind: "control", requestId, message: this.options.executor.throwControl(errorMessage(error)) }); return; } const execId = this.nextExecId++; message.id = execId; const controller = new AbortController(); this.inflight.set(requestId, { execId, controller }); this.options.onInflightChange?.(this.inflight.size); try { for await (const output of this.options.executor.execute(message, controller.signal)) this.enqueue({ kind: output.kind, requestId, message: output.message }); } finally { this.inflight.delete(requestId); this.options.onInflightChange?.(this.inflight.size); } }
  private handleCancel(requestId: string): void { const entry = this.inflight.get(requestId); if (entry == null) return; this.options.executor.cancel(entry.execId); entry.controller.abort(); }
  private atomicHeadroomBytes(): number {
    return Math.min(this.maxFileBytes, 8 * 1024 * 1024);
  }

  private async handleUpload(frame: Extract<LocalExecRequestFrame, { kind: "upload" }>): Promise<void> {
    const { requestId, path, bytesBase64 } = frame;
    try {
      const approximate = Math.floor(bytesBase64.length * 3 / 4);
      if (approximate > this.maxFileBytes) { this.enqueue({ kind: "file-error", requestId, error: localExecFileTooLargeMessage(approximate, this.maxFileBytes) }); return; }
      const data = Buffer.from(bytesBase64, "base64");
      if (data.length > this.maxFileBytes) { this.enqueue({ kind: "file-error", requestId, error: localExecFileTooLargeMessage(data.length, this.maxFileBytes) }); return; }
      const target = await containPath({ root: this.root, path });
      const committed = await this.fileAccess.writeAtomic({
        target,
        bytesBase64,
        ...(frame.sha256 === undefined ? {} : { sha256: frame.sha256 }),
        ...(frame.expectedVersion === undefined ? {} : { expectedVersion: frame.expectedVersion }),
        overwrite: isLocalExecOverwriteAllowed(frame.overwrite),
        maxBytes: this.maxFileBytes,
        headroomBytes: this.atomicHeadroomBytes(),
      });
      this.enqueue({ kind: "file", requestId, sha256: committed.sha256, version: committed.version, size: committed.size });
    } catch (error) { this.enqueue({ kind: "file-error", requestId, error: errorMessage(error) }); }
  }

  private async cleanupUpload(transferId: string): Promise<void> {
    const upload = this.uploads.get(transferId);
    if (upload === undefined) return;
    this.uploads.delete(transferId);
    await this.fileAccess.abortUpload({ target: upload.target, transferId }).catch(() => {});
  }

  private async handleUploadStart(frame: Extract<LocalExecRequestFrame, { kind: "upload-start" }>): Promise<void> {
    const { requestId, transferId, path, totalBytes } = frame;
    try {
      if (!Number.isSafeInteger(totalBytes) || totalBytes < 0 || totalBytes > this.maxFileBytes) throw new Error(localExecFileTooLargeMessage(totalBytes, this.maxFileBytes));
      if (!/^[0-9a-fA-F]{64}$/.test(frame.sha256)) throw new Error("Upload SHA-256 is invalid");
      if (this.uploads.has(transferId)) throw new Error("Upload transferId is already active");
      const target = await containPath({ root: this.root, path });
      const started = await this.fileAccess.startUpload({
        target,
        transferId,
        totalBytes,
        ...(frame.expectedVersion === undefined ? {} : { expectedVersion: frame.expectedVersion }),
        overwrite: isLocalExecOverwriteAllowed(frame.overwrite),
        maxBytes: this.maxFileBytes,
        headroomBytes: this.atomicHeadroomBytes(),
      });
      this.uploads.set(transferId, {
        path,
        target,
        totalBytes,
        receivedBytes: 0,
        expectedSha256: frame.sha256.toLowerCase(),
        ...(started.startingVersion === undefined ? {} : { startingVersion: started.startingVersion }),
        overwrite: isLocalExecOverwriteAllowed(frame.overwrite),
      });
      this.enqueue({ kind: "file", requestId, size: 0 });
    } catch (error) {
      await this.cleanupUpload(transferId);
      this.enqueue({ kind: "file-error", requestId, error: errorMessage(error) });
    }
  }

  private async handleUploadChunk(frame: Extract<LocalExecRequestFrame, { kind: "upload-chunk" }>): Promise<void> {
    const { requestId, transferId, path, offset, bytesBase64 } = frame;
    try {
      const upload = this.uploads.get(transferId);
      if (upload === undefined || upload.path !== path) throw new Error("Upload transfer is unknown or bound to another path");
      if (!Number.isSafeInteger(offset) || offset !== upload.receivedBytes) throw new Error("Upload chunk offset is stale or out of order");
      const approximate = Math.floor(bytesBase64.length * 3 / 4);
      if (approximate > LOCAL_EXEC_UPLOAD_CHUNK_BYTES) throw new Error("Upload chunk exceeds the bounded chunk size");
      const data = Buffer.from(bytesBase64, "base64");
      if (data.length === 0 && upload.totalBytes !== 0) throw new Error("Upload chunk is empty");
      if (data.length > LOCAL_EXEC_UPLOAD_CHUNK_BYTES || upload.receivedBytes + data.length > upload.totalBytes) throw new Error("Upload chunk exceeds the declared transfer size");
      const revalidatedTarget = await containPath({ root: this.root, path });
      if (revalidatedTarget !== upload.target) throw new Error("File path changed during chunk transfer; refusing a possible symlink or mount race");
      const written = await this.fileAccess.writeChunk({ target: upload.target, transferId, offset, bytesBase64 });
      const expectedSize = upload.receivedBytes + data.length;
      if (written.size !== expectedSize) throw new Error("Native file helper returned an unexpected staging size");
      upload.receivedBytes = written.size;
      this.enqueue({ kind: "file", requestId, size: upload.receivedBytes });
    } catch (error) {
      await this.cleanupUpload(transferId);
      this.enqueue({ kind: "file-error", requestId, error: errorMessage(error) });
    }
  }

  private async handleUploadCommit(frame: Extract<LocalExecRequestFrame, { kind: "upload-commit" }>): Promise<void> {
    const { requestId, transferId, path } = frame;
    const upload = this.uploads.get(transferId);
    if (upload === undefined || upload.path !== path) { this.enqueue({ kind: "file-error", requestId, error: "Upload transfer is unknown or bound to another path" }); return; }
    this.uploads.delete(transferId);
    try {
      if (upload.receivedBytes !== upload.totalBytes) throw new Error("Upload commit refused because not all declared bytes were received");
      const target = await containPath({ root: this.root, path });
      if (target !== upload.target) throw new Error("File path changed during chunk transfer; refusing a possible symlink or mount race");
      const committed = await this.fileAccess.commitUpload({
        target: upload.target,
        transferId,
        totalBytes: upload.totalBytes,
        sha256: upload.expectedSha256,
        ...(upload.startingVersion === undefined ? {} : { startingVersion: upload.startingVersion }),
        overwrite: upload.overwrite,
        maxBytes: this.maxFileBytes,
      });
      this.enqueue({ kind: "file", requestId, sha256: committed.sha256, version: committed.version, size: committed.size });
    } catch (error) {
      await this.fileAccess.abortUpload({ target: upload.target, transferId }).catch(() => {});
      this.enqueue({ kind: "file-error", requestId, error: errorMessage(error) });
    }
  }

  private async handleDownload(requestId: string, path: string): Promise<void> {
    try {
      const target = await containPath({ root: this.root, path });
      const file = await this.fileAccess.read({ target, maxBytes: this.maxFileBytes });
      this.enqueue({ kind: "file", requestId, bytesBase64: file.bytesBase64, sha256: file.sha256, version: file.version, size: file.size });
    } catch (error) { this.enqueue({ kind: "file-error", requestId, error: errorMessage(error) }); }
  }
}
