import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { isAbsolute, relative, sep } from "node:path";

export const LOCAL_EXEC_NATIVE_FILE_ARG = "--local-exec-file-sandbox";
export const LOCAL_EXEC_NATIVE_FILE_ROOT_ENV = "FABUSHI_LOCAL_EXEC_FILE_ROOT";
const MAX_HELPER_STDERR_BYTES = 64 * 1024;

export interface NativeFileResult {
  readonly sha256?: string;
  readonly version?: string;
  readonly size?: number;
  readonly bytesBase64?: string;
  readonly startingVersion?: string;
}

export interface NativeLocalExecFileAccess {
  version(args: { readonly target: string; readonly maxBytes: number }): Promise<string | undefined>;
  read(args: { readonly target: string; readonly maxBytes: number }): Promise<Required<Pick<NativeFileResult, "bytesBase64" | "sha256" | "version" | "size">>>;
  writeAtomic(args: {
    readonly target: string;
    readonly bytesBase64: string;
    readonly sha256?: string;
    readonly expectedVersion?: string;
    readonly overwrite?: boolean;
    readonly maxBytes: number;
    readonly headroomBytes: number;
  }): Promise<Required<Pick<NativeFileResult, "sha256" | "version" | "size">>>;
  startUpload(args: {
    readonly target: string;
    readonly transferId: string;
    readonly totalBytes: number;
    readonly expectedVersion?: string;
    readonly overwrite?: boolean;
    readonly maxBytes: number;
    readonly headroomBytes: number;
  }): Promise<{ readonly startingVersion?: string }>;
  writeChunk(args: {
    readonly target: string;
    readonly transferId: string;
    readonly offset: number;
    readonly bytesBase64: string;
  }): Promise<{ readonly size: number }>;
  commitUpload(args: {
    readonly target: string;
    readonly transferId: string;
    readonly totalBytes: number;
    readonly sha256: string;
    readonly startingVersion?: string;
    readonly overwrite?: boolean;
    readonly maxBytes: number;
  }): Promise<Required<Pick<NativeFileResult, "sha256" | "version" | "size">>>;
  abortUpload(args: { readonly target: string; readonly transferId: string }): Promise<void>;
  close(): void;
}

interface NativeHelperEnvelope {
  readonly ok: boolean;
  readonly value?: Record<string, unknown>;
  readonly error?: string;
}

export interface NativeLocalExecFileAccessOptions {
  readonly root: string;
  readonly hostBinary?: string;
  readonly spawnImpl?: typeof spawn;
}

export class NativeLocalExecFileAccessError extends Error {}

function numberField(value: Record<string, unknown>, key: string): number {
  const field = value[key];
  if (typeof field !== "number" || !Number.isSafeInteger(field) || field < 0) {
    throw new NativeLocalExecFileAccessError("native local-exec file helper returned invalid " + key);
  }
  return field;
}

function stringField(value: Record<string, unknown>, key: string): string {
  const field = value[key];
  if (typeof field !== "string") {
    throw new NativeLocalExecFileAccessError("native local-exec file helper returned invalid " + key);
  }
  return field;
}

export class ProductionNativeLocalExecFileAccess implements NativeLocalExecFileAccess {
  private readonly spawnImpl: typeof spawn;
  private readonly children = new Set<ChildProcessWithoutNullStreams>();
  private closed = false;

  constructor(private readonly options: NativeLocalExecFileAccessOptions) {
    this.spawnImpl = options.spawnImpl ?? spawn;
  }

  private relativePath(target: string): string {
    const value = relative(this.options.root, target);
    if (
      value.length === 0 ||
      value === ".." ||
      value.startsWith(".." + sep) ||
      isAbsolute(value)
    ) {
      throw new NativeLocalExecFileAccessError(
        "native local-exec file target escaped the authorized root: " + target,
      );
    }
    return value;
  }

  private hostBinary(): string {
    const value = this.options.hostBinary ?? process.env.MAHAYANA_APP_HOST_BIN;
    if (value == null || value.trim().length === 0) {
      throw new NativeLocalExecFileAccessError(
        "MAHAYANA_APP_HOST_BIN is required for native local-exec file access",
      );
    }
    return value;
  }

  private async call(
    payload: Record<string, unknown>,
    outputLimitBytes: number,
  ): Promise<Record<string, unknown>> {
    if (this.closed) throw new NativeLocalExecFileAccessError("native local-exec file access is closed");
    const child = this.spawnImpl(this.hostBinary(), [LOCAL_EXEC_NATIVE_FILE_ARG], {
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true,
      env: {
        ...process.env,
        [LOCAL_EXEC_NATIVE_FILE_ROOT_ENV]: this.options.root,
      },
    });
    this.children.add(child);
    let stdout = Buffer.alloc(0);
    let stderr = Buffer.alloc(0);
    let outputExceeded = false;
    child.stdout.on("data", (chunk: Buffer) => {
      if (outputExceeded) return;
      stdout = Buffer.concat([stdout, chunk]);
      if (stdout.length > outputLimitBytes) {
        outputExceeded = true;
        child.kill();
      }
    });
    child.stderr.on("data", (chunk: Buffer) => {
      if (stderr.length >= MAX_HELPER_STDERR_BYTES) return;
      stderr = Buffer.concat([stderr, chunk]).subarray(0, MAX_HELPER_STDERR_BYTES);
    });
    const settled = new Promise<{ readonly code: number | null; readonly error?: Error }>((resolve) => {
      child.once("error", (error) => resolve({ code: null, error }));
      child.once("exit", (code) => resolve({ code }));
    });
    child.stdin.end(JSON.stringify(payload));
    const outcome = await settled;
    this.children.delete(child);
    if (outputExceeded) {
      throw new NativeLocalExecFileAccessError("native local-exec file helper exceeded its bounded output");
    }
    if (outcome.error != null) {
      throw new NativeLocalExecFileAccessError(
        "native local-exec file helper failed to spawn: " + outcome.error.message,
      );
    }
    if (outcome.code !== 0) {
      throw new NativeLocalExecFileAccessError(
        "native local-exec file helper exited " + outcome.code + ": " + stderr.toString("utf8").trim(),
      );
    }
    let envelope: NativeHelperEnvelope;
    try {
      envelope = JSON.parse(stdout.toString("utf8")) as NativeHelperEnvelope;
    } catch {
      throw new NativeLocalExecFileAccessError("native local-exec file helper returned invalid JSON");
    }
    if (envelope.ok !== true || envelope.value == null) {
      throw new NativeLocalExecFileAccessError(
        typeof envelope.error === "string" && envelope.error.length > 0
          ? envelope.error
          : "native local-exec file helper refused the request",
      );
    }
    return envelope.value;
  }

  async version(args: { readonly target: string; readonly maxBytes: number }): Promise<string | undefined> {
    const value = await this.call({
      op: "version",
      path: this.relativePath(args.target),
      maxBytes: args.maxBytes,
    }, 64 * 1024);
    const version = value.version;
    if (version === null || version === undefined) return undefined;
    if (typeof version !== "string") throw new NativeLocalExecFileAccessError("native local-exec file helper returned invalid version");
    return version;
  }

  async read(args: { readonly target: string; readonly maxBytes: number }): Promise<Required<Pick<NativeFileResult, "bytesBase64" | "sha256" | "version" | "size">>> {
    const outputLimit = Math.max(64 * 1024, Math.ceil(args.maxBytes * 4 / 3) + 128 * 1024);
    const value = await this.call({
      op: "read",
      path: this.relativePath(args.target),
      maxBytes: args.maxBytes,
    }, outputLimit);
    return {
      bytesBase64: stringField(value, "bytesBase64"),
      sha256: stringField(value, "sha256"),
      version: stringField(value, "version"),
      size: numberField(value, "size"),
    };
  }

  async writeAtomic(args: {
    readonly target: string;
    readonly bytesBase64: string;
    readonly sha256?: string;
    readonly expectedVersion?: string;
    readonly overwrite?: boolean;
    readonly maxBytes: number;
    readonly headroomBytes: number;
  }): Promise<Required<Pick<NativeFileResult, "sha256" | "version" | "size">>> {
    const value = await this.call({
      op: "write-atomic",
      path: this.relativePath(args.target),
      bytesBase64: args.bytesBase64,
      ...(args.sha256 === undefined ? {} : { sha256: args.sha256 }),
      ...(args.expectedVersion === undefined ? {} : { expectedVersion: args.expectedVersion }),
      overwrite: args.overwrite !== false,
      maxBytes: args.maxBytes,
      headroomBytes: args.headroomBytes,
    }, 64 * 1024);
    return {
      sha256: stringField(value, "sha256"),
      version: stringField(value, "version"),
      size: numberField(value, "size"),
    };
  }

  async startUpload(args: {
    readonly target: string;
    readonly transferId: string;
    readonly totalBytes: number;
    readonly expectedVersion?: string;
    readonly overwrite?: boolean;
    readonly maxBytes: number;
    readonly headroomBytes: number;
  }): Promise<{ readonly startingVersion?: string }> {
    const value = await this.call({
      op: "upload-start",
      path: this.relativePath(args.target),
      transferId: args.transferId,
      totalBytes: args.totalBytes,
      ...(args.expectedVersion === undefined ? {} : { expectedVersion: args.expectedVersion }),
      overwrite: args.overwrite !== false,
      maxBytes: args.maxBytes,
      headroomBytes: args.headroomBytes,
    }, 64 * 1024);
    const startingVersion = value.startingVersion;
    if (startingVersion === null || startingVersion === undefined) return {};
    if (typeof startingVersion !== "string") throw new NativeLocalExecFileAccessError("native local-exec file helper returned invalid startingVersion");
    return { startingVersion };
  }

  async writeChunk(args: {
    readonly target: string;
    readonly transferId: string;
    readonly offset: number;
    readonly bytesBase64: string;
  }): Promise<{ readonly size: number }> {
    const value = await this.call({
      op: "upload-chunk",
      path: this.relativePath(args.target),
      transferId: args.transferId,
      offset: args.offset,
      bytesBase64: args.bytesBase64,
    }, 64 * 1024);
    return { size: numberField(value, "size") };
  }

  async commitUpload(args: {
    readonly target: string;
    readonly transferId: string;
    readonly totalBytes: number;
    readonly sha256: string;
    readonly startingVersion?: string;
    readonly overwrite?: boolean;
    readonly maxBytes: number;
  }): Promise<Required<Pick<NativeFileResult, "sha256" | "version" | "size">>> {
    const value = await this.call({
      op: "upload-commit",
      path: this.relativePath(args.target),
      transferId: args.transferId,
      totalBytes: args.totalBytes,
      sha256: args.sha256,
      ...(args.startingVersion === undefined ? {} : { startingVersion: args.startingVersion }),
      overwrite: args.overwrite !== false,
      maxBytes: args.maxBytes,
    }, 64 * 1024);
    return {
      sha256: stringField(value, "sha256"),
      version: stringField(value, "version"),
      size: numberField(value, "size"),
    };
  }

  async abortUpload(args: { readonly target: string; readonly transferId: string }): Promise<void> {
    await this.call({
      op: "upload-abort",
      path: this.relativePath(args.target),
      transferId: args.transferId,
    }, 64 * 1024);
  }

  close(): void {
    this.closed = true;
    for (const child of this.children) child.kill();
    this.children.clear();
  }
}
