import assert from "node:assert/strict";
import test from "node:test";

import {
  TransientLoginError,
  requestLogin,
  validDeviceId,
} from "./prepare-ios-ci-session.mjs";

const base = "https://api.example.test";

test("only exact protected iOS CI device IDs are accepted", () => {
  assert.equal(validDeviceId("gha-1-1-ios-app"), true);
  assert.equal(validDeviceId("gha-38990602969-1-ios-offline"), true);
  for (const id of [
    "gha-1-1-ios-other",
    "gha-1-1-ios-app-extra",
    "gha-1-1-ios-offline-extra",
    "gha-1-1-android-app",
    "ios-app",
    "gha-1-ios-offline",
    "gha-abc-1-ios-app",
    "gha-1-attempt-ios-app",
    "gha-1-1-ios-app/../x",
    "",
    " ",
    "\tgha-1-1-ios-app",
    "gha-1-1-ios-app\n",
    "gha-1-1-ios-app\r",
    "gha-1- 1-ios-app",
  ]) {
    assert.equal(validDeviceId(id), false, JSON.stringify(id));
  }
  for (const value of [null, undefined, 1, {}, ["gha-1-1-ios-app"]]) {
    assert.equal(validDeviceId(value), false, String(value));
  }
});

test("stalled login settles through the referenced timeout instead of unresolved top-level await", async () => {
  let calls = 0;
  const fetchImpl = (_url, { signal }) => {
    calls += 1;
    return new Promise((_resolve, reject) => {
      signal.addEventListener("abort", () => {
        const error = new Error("aborted");
        error.name = "AbortError";
        reject(error);
      }, { once: true });
    });
  };

  await assert.rejects(
    requestLogin(base, "u", "p", "gha-1-1-ios-app", {
      fetchImpl,
      timeoutMs: 10,
      maxAttempts: 1,
      retryBaseDelayMs: 0,
      onRetry: () => {},
    }),
    (error) => error instanceof TransientLoginError && /timed out/u.test(error.message),
  );
  assert.equal(calls, 1);
});

test("429 and 5xx are retried with a strict attempt bound", async () => {
  let calls = 0;
  const fetchImpl = async () => {
    calls += 1;
    if (calls === 1) {
      return new Response(JSON.stringify({ code: "busy" }), {
        status: 503,
        headers: { "content-type": "application/json" },
      });
    }
    return new Response(JSON.stringify({ accessToken: "ok" }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  };

  const payload = await requestLogin(base, "u", "p", "gha-1-1-ios-app", {
    fetchImpl,
    timeoutMs: 100,
    maxAttempts: 3,
    retryBaseDelayMs: 0,
    onRetry: () => {},
  });
  assert.equal(payload.accessToken, "ok");
  assert.equal(calls, 2);
});

test("permanent authentication failures remain fail-closed and are not retried", async () => {
  let calls = 0;
  const fetchImpl = async () => {
    calls += 1;
    return new Response(JSON.stringify({ code: "invalid_credentials" }), {
      status: 401,
      headers: { "content-type": "application/json" },
    });
  };

  await assert.rejects(
    requestLogin(base, "u", "p", "gha-1-1-ios-app", {
      fetchImpl,
      timeoutMs: 100,
      maxAttempts: 3,
      retryBaseDelayMs: 0,
      onRetry: () => {},
    }),
    /Fabushi CI login failed: invalid_credentials/u,
  );
  assert.equal(calls, 1);
});
