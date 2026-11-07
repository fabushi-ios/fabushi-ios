// Grok-compatible import path only. The protocol implementation is owned by
// Electron main because Coordinator owns local-exec process supervision.
// Keep this file logic-free so it cannot become a second local-exec runtime.
export * from "../../electron-main/local-exec/local-exec-daemon-protocol.js";
