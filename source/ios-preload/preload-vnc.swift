import Foundation

enum IOSVNCRFBState: String, Equatable, Sendable {
    case connected
    case reconnecting
    case connecting
    case disconnecting
    case disconnected
}

struct IOSVNCSessionSignal: Equatable, Sendable {
    enum Phase: String, Equatable, Sendable {
        case connect = "rfb_connect"
        case disconnect = "rfb_disconnect"
        case reconnect
    }

    let phase: Phase
    let clean: Bool
}

struct IOSVNCCursorTelemetry: Equatable, Sendable {
    enum Kind: String, Equatable, Sendable {
        case click
        case drag
        case move
        case scroll
    }

    let x: Double
    let y: Double
    let kind: Kind
}

struct IOSVNCRFBSessionTracker: Equatable, Sendable {
    private var connectCount = 0
    private var reportedConnected = false
    private var lastState: IOSVNCRFBState?

    mutating func reset() {
        connectCount = 0
        reportedConnected = false
        lastState = nil
    }

    mutating func evaluate(_ state: IOSVNCRFBState) -> IOSVNCSessionSignal? {
        guard state != lastState else { return nil }
        lastState = state

        if state == .connected {
            connectCount += 1
            let phase: IOSVNCSessionSignal.Phase =
                connectCount > 1 ? .reconnect : .connect
            reportedConnected = true
            return .init(phase: phase, clean: true)
        }

        if reportedConnected && state != .connecting {
            reportedConnected = false
            return .init(
                phase: .disconnect,
                clean: state == .disconnecting
            )
        }

        return nil
    }
}

@MainActor
final class IOSVNCPreloadRuntime {
    static let messageHandlerName = "fabushiVNC"
    static let livenessSampleIntervalMilliseconds: Int64 = 1_000

    private var visibilityGate = IOSVNCViewerVisibilityGate()
    private var livenessDetector = IOSVNCLivenessDetector()
    private var sessionTracker = IOSVNCRFBSessionTracker()

    func updateViewerVisibility(_ visible: Bool) -> Bool {
        visibilityGate.update(visible)
    }

    var viewerIsVisible: Bool {
        visibilityGate.visible
    }

    func ingestRFBState(_ state: IOSVNCRFBState) -> IOSVNCSessionSignal? {
        if state != .connected {
            livenessDetector.reset()
        }
        return sessionTracker.evaluate(state)
    }

    func sampleLiveness(
        nowMilliseconds: Int64,
        counters: IOSVNCLivenessCounters
    ) -> IOSVNCLivenessReport? {
        livenessDetector.sample(
            nowMilliseconds: nowMilliseconds,
            counters: counters
        )
    }

    func resetSession() {
        sessionTracker.reset()
        livenessDetector.reset()
    }

    func resetLiveness() {
        livenessDetector.reset()
    }

    func trustedClipboardPasteScript(text: String) -> String {
        IOSVNCClipboardPaste.buildTrustedNoVNCPasteScript(text: text)
    }

    func resolveClipboardSync(text: String, didPaste: Bool) -> String? {
        IOSVNCClipboardPaste.resolveHostToBoxSync(text: text, didPaste: didPaste)
    }

    static func rfbState(from body: Any) -> IOSVNCRFBState? {
        guard let object = body as? [String: Any],
              object["kind"] as? String == "rfb_state",
              let raw = object["state"] as? String
        else { return nil }
        return IOSVNCRFBState(rawValue: raw)
    }

    static func livenessCounters(from body: Any) -> IOSVNCLivenessCounters? {
        guard let object = body as? [String: Any],
              object["kind"] as? String == "liveness",
              let counters = object["counters"] as? [String: Any],
              let keys = integer(counters["keys"]),
              let clicks = integer(counters["clicks"]),
              let moves = integer(counters["moves"]),
              let drawOps = integer(counters["drawOps"]),
              let inBytes = integer(counters["inBytes"])
        else { return nil }

        let value = IOSVNCLivenessCounters(
            keys: keys,
            clicks: clicks,
            moves: moves,
            drawOps: drawOps,
            inBytes: inBytes
        )
        return value.isValid ? value : nil
    }

    static func cursorTelemetry(from body: Any) -> IOSVNCCursorTelemetry? {
        guard let object = body as? [String: Any],
              object["kind"] as? String == "cursor",
              let x = number(object["x"]),
              let y = number(object["y"]),
              x >= 0,
              y >= 0,
              let rawKind = object["type"] as? String,
              let kind = IOSVNCCursorTelemetry.Kind(rawValue: rawKind)
        else { return nil }

        return .init(x: x, y: y, kind: kind)
    }

    private static func integer(_ value: Any?) -> Int64? {
        if let value = value as? Int64 { return value }
        if let value = value as? Int { return Int64(value) }
        if let value = value as? NSNumber {
            let double = value.doubleValue
            guard double.isFinite,
                  double.rounded(.towardZero) == double,
                  double >= Double(Int64.min),
                  double <= Double(Int64.max)
            else { return nil }
            return value.int64Value
        }
        return nil
    }

    private static func number(_ value: Any?) -> Double? {
        if let value = value as? Double {
            return value.isFinite ? value : nil
        }
        if let value = value as? Int {
            return Double(value)
        }
        if let value = value as? Int64 {
            return Double(value)
        }
        if let value = value as? NSNumber {
            let double = value.doubleValue
            return double.isFinite ? double : nil
        }
        return nil
    }

    static let bootstrapScript = #"""
    (function () {
      if (window.__fabushiIOSVNCNativeBridgeInstalled) return;
      window.__fabushiIOSVNCNativeBridgeInstalled = true;

      var handler = window.webkit &&
        window.webkit.messageHandlers &&
        window.webkit.messageHandlers.fabushiVNC;
      if (!handler || typeof handler.postMessage !== "function") return;

      function post(payload) {
        try { handler.postMessage(payload); } catch (_error) {}
      }

      function currentState() {
        var root = document.documentElement;
        if (!root) return "disconnected";
        if (root.classList.contains("noVNC_connected")) return "connected";
        if (root.classList.contains("noVNC_reconnecting")) return "reconnecting";
        if (root.classList.contains("noVNC_connecting")) return "connecting";
        if (root.classList.contains("noVNC_disconnecting")) return "disconnecting";
        return "disconnected";
      }

      var lastState = null;
      var observer = null;
      function evaluateState() {
        var next = currentState();
        if (next === lastState) return;
        lastState = next;
        post({ kind: "rfb_state", state: next });
      }

      function installStateObserver() {
        var root = document.documentElement;
        if (!root || observer) return;
        observer = new MutationObserver(evaluateState);
        observer.observe(root, { attributes: true, attributeFilter: ["class"] });
        evaluateState();
      }

      if (document.documentElement) installStateObserver();
      else document.addEventListener("DOMContentLoaded", installStateObserver, { once: true });

      var counters = { keys: 0, clicks: 0, moves: 0, drawOps: 0, inBytes: 0 };
      window.__fabushiVncLivenessBeacon = counters;
      var PRIMARY_BUTTON_MASK_BITS = 0x07;
      var lastButtonMask = 0;
      var lastMovePostAt = 0;

      function postCursor(x, y, type) {
        if (!Number.isFinite(x) || !Number.isFinite(y) || x < 0 || y < 0) return;
        var now = Date.now();
        if (type === "move" && now - lastMovePostAt < 50) return;
        if (type === "move") lastMovePostAt = now;
        post({ kind: "cursor", x: x, y: y, type: type });
      }

      import("./core/rfb.js").then(function (m) {
        var messages = m.default && m.default.messages;
        if (!messages) return;

        if (typeof messages.keyEvent === "function") {
          var keyEvent = messages.keyEvent;
          messages.keyEvent = function (sock, keysym, down) {
            if (down) counters.keys += 1;
            return keyEvent.apply(this, arguments);
          };
        }

        if (typeof messages.QEMUExtendedKeyEvent === "function") {
          var qemuKeyEvent = messages.QEMUExtendedKeyEvent;
          messages.QEMUExtendedKeyEvent = function (sock, keysym, down) {
            if (down) counters.keys += 1;
            return qemuKeyEvent.apply(this, arguments);
          };
        }

        if (typeof messages.pointerEvent === "function") {
          var pointerEvent = messages.pointerEvent;
          messages.pointerEvent = function (sock, x, y, mask) {
            var pressed = mask & ~lastButtonMask & PRIMARY_BUTTON_MASK_BITS;
            var wheel = mask & ~PRIMARY_BUTTON_MASK_BITS;
            var type = wheel
              ? "scroll"
              : pressed
                ? "click"
                : (mask & PRIMARY_BUTTON_MASK_BITS)
                  ? "drag"
                  : "move";
            if (pressed) counters.clicks += 1;
            else counters.moves += 1;
            postCursor(x, y, type);
            lastButtonMask = mask;
            return pointerEvent.apply(this, arguments);
          };
        }
      }).catch(function () {});

      import("./core/display.js").then(function (m) {
        var Display = m.default;
        if (!Display || !Display.prototype || typeof Display.prototype._damage !== "function") return;
        var damage = Display.prototype._damage;
        Display.prototype._damage = function () {
          counters.drawOps += 1;
          return damage.apply(this, arguments);
        };
      }).catch(function () {});

      import("./core/websock.js").then(function (m) {
        var Websock = m.default;
        if (!Websock || !Websock.prototype || typeof Websock.prototype._recvMessage !== "function") return;
        var recvMessage = Websock.prototype._recvMessage;
        Websock.prototype._recvMessage = function (event) {
          counters.inBytes += (event && event.data && event.data.byteLength) || 0;
          return recvMessage.apply(this, arguments);
        };
      }).catch(function () {});

      var sampler = window.setInterval(function () {
        if (currentState() !== "connected") return;
        post({
          kind: "liveness",
          counters: {
            keys: counters.keys,
            clicks: counters.clicks,
            moves: counters.moves,
            drawOps: counters.drawOps,
            inBytes: counters.inBytes
          }
        });
      }, 1000);

      window.addEventListener("pagehide", function () {
        if (observer) {
          try { observer.disconnect(); } catch (_error) {}
          observer = null;
        }
        window.clearInterval(sampler);
        post({ kind: "rfb_state", state: "disconnected" });
      }, { once: true });
    })();
    """#
}
