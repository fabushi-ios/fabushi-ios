import Foundation

enum IOSVNCClipboardPaste {
    static func buildTrustedNoVNCPasteScript(text: String) -> String {
        let encoded = (try? JSONEncoder().encode(text))
            .flatMap { String(data: $0, encoding: .utf8) } ?? "\"\""
        return """
        import("./app/ui.js")
          .then(function (m) {
            var rfb = m && m.default && m.default.rfb;
            var text = \(encoded);
            if (rfb && typeof rfb.clipboardPasteFrom === "function" && text) {
              rfb.clipboardPasteFrom(text);
              return true;
            }
            return false;
          })
          .catch(function () { return false; });
        """
    }

    static func resolveHostToBoxSync(text: String, didPaste: Bool) -> String? {
        didPaste && !text.isEmpty ? text : nil
    }
}
