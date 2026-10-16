#!/usr/bin/env node
import { mkdir, rename, writeFile } from "node:fs/promises";
import { dirname, resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";

const MAX_SESSION_BYTES = 64 * 1024;
const MAX_LIFETIME_SECONDS = 5 * 60 * 60;
export const REQUEST_TIMEOUT_MS = 15_000;
export const MAX_LOGIN_ATTEMPTS = 3;
const RETRY_BASE_DELAY_MS = 750;

export class TransientLoginError extends Error {
  constructor(message) {
    super(message);
    this.name = "TransientLoginError";
  }
}

function required(name) {
  const value = String(process.env[name] || "").trim();
  if (!value) throw new Error(`${name} is required.`);
  return value;
}

function privateRunnerPath(value, runnerTemp, name) {
  const path = resolve(value);
  const root = resolve(runnerTemp);
  if (!path.startsWith(`${root}${sep}`)) {
    throw new Error(`${name} must live under RUNNER_TEMP.`);
  }
  return path;
}

function normalizeBaseURL(value) {
  const parsed = new URL(String(value || "https://api.ombhrum.com"));
  const local = ["127.0.0.1", "localhost", "::1"].includes(parsed.hostname);
  if ((!local && parsed.protocol !== "https:") || (local && !["http:", "https:"].includes(parsed.protocol))) {
    throw new Error("FABUSHI_API_BASE_URL must use HTTPS outside loopback.");
  }
  if (parsed.username || parsed.password || parsed.search || parsed.hash) {
    throw new Error("FABUSHI_API_BASE_URL must not contain credentials, query, or fragment data.");
  }
  return parsed.toString().replace(/\/$/u, "");
}

function validCredential(value) {
  return value.length >= 24 && value.length <= 16 * 1024 && !/\s/u.test(value);
}

function validDeviceId(value) {
  return /^gha-[0-9]+-[0-9]+-ios-app$/u.test(value);
}

function delay(ms) {
  if (ms <= 0) return Promise.resolve();
  return new Promise((resolveDelay) => setTimeout(resolveDelay, ms));
}

async function requestLoginAttempt(baseURL, username, password, deviceId, {
  fetchImpl,
  timeoutMs,
}) {
  const controller = new AbortController();
  let timeoutHandle;
  const timeout = new Promise((_, reject) => {
    timeoutHandle = setTimeout(() => {
      controller.abort();
      reject(new TransientLoginError("Fabushi CI login request timed out."));
    }, timeoutMs);
  });

  let response;
  try {
    response = await Promise.race([
      fetchImpl(`${baseURL}/api/auth/login`, {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({ username, password, deviceId }),
        signal: controller.signal,
      }),
      timeout,
    ]);
  } catch (error) {
    if (error instanceof TransientLoginError) throw error;
    if (error?.name === "AbortError") {
      throw new TransientLoginError("Fabushi CI login request timed out.");
    }
    throw new TransientLoginError("Fabushi CI login request failed.");
  } finally {
    clearTimeout(timeoutHandle);
  }

  let text;
  try {
    text = await response.text();
  } catch {
    throw new TransientLoginError("Fabushi CI login response could not be read.");
  }

  let payload = {};
  try {
    payload = text ? JSON.parse(text) : {};
  } catch {
    if (response.status === 429 || response.status >= 500) {
      throw new TransientLoginError("Fabushi CI login service returned invalid JSON while temporarily unavailable.");
    }
    throw new Error("Fabushi account service returned invalid JSON.");
  }

  if (!response.ok) {
    const code = String(payload?.error?.code || payload?.code || `http_${response.status}`);
    if (response.status === 429 || response.status >= 500) {
      throw new TransientLoginError(`Fabushi CI login temporarily unavailable: ${code}`);
    }
    throw new Error(`Fabushi CI login failed: ${code}`);
  }
  return payload;
}

export async function requestLogin(baseURL, username, password, deviceId, {
  fetchImpl = globalThis.fetch,
  timeoutMs = REQUEST_TIMEOUT_MS,
  maxAttempts = MAX_LOGIN_ATTEMPTS,
  retryBaseDelayMs = RETRY_BASE_DELAY_MS,
  onRetry = ({ nextAttempt, maxAttempts: attempts }) => {
    process.stderr.write(`Fabushi CI login transient failure; retrying attempt ${nextAttempt}/${attempts}.\n`);
  },
} = {}) {
  if (typeof fetchImpl !== "function") {
    throw new Error("Fetch implementation is unavailable.");
  }
  if (!Number.isInteger(maxAttempts) || maxAttempts < 1 || maxAttempts > 5) {
    throw new Error("maxAttempts must be an integer from 1 through 5.");
  }
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) {
    throw new Error("timeoutMs must be positive.");
  }

  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    try {
      return await requestLoginAttempt(baseURL, username, password, deviceId, {
        fetchImpl,
        timeoutMs,
      });
    } catch (error) {
      const retryable = error instanceof TransientLoginError;
      if (!retryable || attempt >= maxAttempts) throw error;
      onRetry({ attempt, nextAttempt: attempt + 1, maxAttempts });
      await delay(retryBaseDelayMs * attempt);
    }
  }
  throw new Error("Unreachable login retry state.");
}

export async function main() {
  if (process.env.GITHUB_ACTIONS !== "true") {
    throw new Error("Bounded CI sessions can be prepared only inside GitHub Actions.");
  }

  const username = required("FABUSHI_CI_TEST_USERNAME");
  const password = required("FABUSHI_CI_TEST_PASSWORD");
  const deviceId = required("DEVICE_ID");
  const runId = required("GITHUB_RUN_ID");
  const runAttempt = required("GITHUB_RUN_ATTEMPT");
  const runnerTemp = required("RUNNER_TEMP");
  const outputPath = privateRunnerPath(
    required("FABUSHI_CI_ACCOUNT_SESSION_FILE"),
    runnerTemp,
    "FABUSHI_CI_ACCOUNT_SESSION_FILE",
  );
  if (!validDeviceId(deviceId)) throw new Error("DEVICE_ID must be a protected iOS GitHub Actions device id.");
  if (!/^[0-9]+$/u.test(runId) || !/^[0-9]+$/u.test(runAttempt)) {
    throw new Error("GitHub run identity is invalid.");
  }

  const payload = await requestLogin(
    normalizeBaseURL(process.env.FABUSHI_API_BASE_URL),
    username,
    password,
    deviceId,
  );

  const accessToken = String(payload?.accessToken || "").trim();
  const refreshToken = String(payload?.refreshToken || "").trim();
  const tokenType = String(payload?.tokenType || "Bearer");
  const sourceDeviceId = String(payload?.deviceId || "").trim();
  const sourceUsername = String(payload?.username || payload?.user?.username || "").trim();
  const userId = String(payload?.userId || payload?.user?.id || "").trim();
  const nestedUserId = String(payload?.user?.id || "").trim();
  const expiresAt = Number(payload?.accessTokenExpiresAt || 0);
  const now = Math.floor(Date.now() / 1000);

  if (!validCredential(accessToken) || !validCredential(refreshToken)) {
    throw new Error("Fabushi login did not return a complete refreshable session.");
  }
  if (tokenType !== "Bearer" || sourceDeviceId !== deviceId || !sourceUsername || !userId || nestedUserId !== userId) {
    throw new Error("Fabushi login returned an inconsistent account identity.");
  }
  if (!Number.isSafeInteger(expiresAt) || expiresAt <= now + 30 || expiresAt > now + MAX_LIFETIME_SECONDS) {
    throw new Error("Fabushi access token is outside the bounded CI lifetime.");
  }

  const bounded = {
    accessToken,
    tokenType: "Bearer",
    accessTokenExpiresAt: expiresAt,
    sessionId: `ci-runner:${runId}:${runAttempt}`,
    deviceId,
    username: sourceUsername,
    userId,
    user: payload.user,
    provider: "github-actions",
    ciRunner: true,
  };
  const serialized = `${JSON.stringify(bounded, null, 2)}\n`;
  if (Buffer.byteLength(serialized) > MAX_SESSION_BYTES) {
    throw new Error("Bounded Fabushi CI session is unexpectedly large.");
  }

  await mkdir(dirname(outputPath), { recursive: true, mode: 0o700 });
  const temporary = `${outputPath}.${process.pid}.tmp`;
  await writeFile(temporary, serialized, { encoding: "utf8", mode: 0o600 });
  await rename(temporary, outputPath);
  process.stdout.write("Prepared bounded refresh-token-free Fabushi iOS application session.\n");
}

const isMain = process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href;
if (isMain) {
  await main();
}
