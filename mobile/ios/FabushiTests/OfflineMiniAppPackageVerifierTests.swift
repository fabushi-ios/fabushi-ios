import CryptoKit
import XCTest
@testable import Fabushi

final class OfflineMiniAppPackageVerifierTests: XCTestCase {
    private let html = Data("<!doctype html><html><body>offline</body></html>".utf8)
    private let script = Data("document.body.dataset.ready='1';".utf8)

    private func sha256(_ data: Data) -> String {
        SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }

    private func baseManifest(
        appId: String = "org.example.offline",
        publisherId: String = "org.example",
        packageClass: String = "catalog"
    ) -> [String: Any] {
        [
            "manifestVersion": 1,
            "packageClass": packageClass,
            "appId": appId,
            "publisherId": publisherId,
            "version": "1.0.0",
            "displayName": "Offline Example",
            "entrypoint": "index.html",
            "runtime": "web",
            "minHostApi": 1,
            "offlineCore": true,
            "requestedCapabilities": [],
            "networkPolicy": [
                "mode": "deny",
                "allowedOrigins": [],
            ],
            "files": [
                "index.html": [
                    "sha256": sha256(html),
                    "bytes": html.count,
                    "mime": "text/html",
                ],
                "assets/app.js": [
                    "sha256": sha256(script),
                    "bytes": script.count,
                    "mime": "text/javascript",
                ],
            ],
        ]
    }

    private func signedManifest(
        _ object: [String: Any],
        key: Curve25519.Signing.PrivateKey,
        keyId: String = "publisher-key-1"
    ) throws -> Data {
        var unsigned = object
        unsigned["signature"] = [
            "algorithm": "Ed25519",
            "keyId": keyId,
            "value": "",
        ]
        let unsignedData = try JSONSerialization.data(withJSONObject: unsigned, options: [.sortedKeys])
        let payload = try FabAppPackageVerifier.canonicalSigningPayloadForTests(manifestData: unsignedData)
        let signature = try key.signature(for: payload)
        let encoded = signature.base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
        unsigned["signature"] = [
            "algorithm": "Ed25519",
            "keyId": keyId,
            "value": encoded,
        ]
        return try JSONSerialization.data(withJSONObject: unsigned, options: [.sortedKeys])
    }

    private func trust(
        key: Curve25519.Signing.PrivateKey,
        appId: String = "org.example.offline",
        publisherId: String = "org.example",
        revoked: Bool = false
    ) -> FabAppCatalogTrust {
        FabAppCatalogTrust(
            appId: appId,
            publisherId: publisherId,
            keyId: "publisher-key-1",
            publicKey: key.publicKey.rawRepresentation,
            revoked: revoked
        )
    }

    func testValidCatalogPackageVerifiesExactPublisherSignatureAndAssets() throws {
        let key = Curve25519.Signing.PrivateKey()
        let manifest = try signedManifest(baseManifest(), key: key)

        let verified = try FabAppPackageVerifier.verifyCatalogPackage(
            manifestData: manifest,
            resources: [
                "index.html": html,
                "assets/app.js": script,
            ],
            trust: trust(key: key),
            hostApi: 1
        )

        XCTAssertEqual(verified.appId, "org.example.offline")
        XCTAssertEqual(verified.publisherId, "org.example")
        XCTAssertEqual(verified.entrypoint, "index.html")
        XCTAssertEqual(verified.packageClass, .catalog)
        XCTAssertTrue(verified.offlineCore)
        XCTAssertEqual(verified.files.count, 2)
    }

    func testNativeRuntimeCapabilityFailsClosedWithoutIOSNativeOwner() throws {
        let key = Curve25519.Signing.PrivateKey()
        var manifest = baseManifest()
        manifest["requestedCapabilities"] = ["native.runtime"]
        let signed = try signedManifest(manifest, key: key)

        XCTAssertThrowsError(
            try FabAppPackageVerifier.verifyCatalogPackage(
                manifestData: signed,
                resources: ["index.html": html, "assets/app.js": script],
                trust: trust(key: key),
                hostApi: 1
            )
        ) { error in
            XCTAssertEqual(error as? FabAppVerificationError, .invalidCapability)
        }
    }

    func testTamperedResourceFailsClosedAfterValidSignature() throws {
        let key = Curve25519.Signing.PrivateKey()
        let manifest = try signedManifest(baseManifest(), key: key)

        XCTAssertThrowsError(
            try FabAppPackageVerifier.verifyCatalogPackage(
                manifestData: manifest,
                resources: [
                    "index.html": Data("<h1>tampered</h1>".utf8),
                    "assets/app.js": script,
                ],
                trust: trust(key: key),
                hostApi: 1
            )
        ) { error in
            if case FabAppVerificationError.fileSizeMismatch("index.html") = error {
                return
            }
            if case FabAppVerificationError.fileDigestMismatch("index.html") = error {
                return
            }
            XCTFail("unexpected error: \(error)")
        }
    }

    func testPublisherBindingCannotBeReplacedBySelfSignedPackage() throws {
        let attacker = Curve25519.Signing.PrivateKey()
        let trusted = Curve25519.Signing.PrivateKey()
        let manifest = try signedManifest(baseManifest(), key: attacker)

        XCTAssertThrowsError(
            try FabAppPackageVerifier.verifyCatalogPackage(
                manifestData: manifest,
                resources: ["index.html": html, "assets/app.js": script],
                trust: trust(key: trusted),
                hostApi: 1
            )
        ) { error in
            XCTAssertEqual(error as? FabAppVerificationError, .invalidSignature)
        }
    }

    func testTrustIdentityAndRevocationFailBeforeActivation() throws {
        let key = Curve25519.Signing.PrivateKey()
        let manifest = try signedManifest(baseManifest(), key: key)

        XCTAssertThrowsError(
            try FabAppPackageVerifier.verifyCatalogPackage(
                manifestData: manifest,
                resources: ["index.html": html, "assets/app.js": script],
                trust: trust(key: key, appId: "org.example.other"),
                hostApi: 1
            )
        ) { error in
            XCTAssertEqual(error as? FabAppVerificationError, .trustMismatch)
        }

        XCTAssertThrowsError(
            try FabAppPackageVerifier.verifyCatalogPackage(
                manifestData: manifest,
                resources: ["index.html": html, "assets/app.js": script],
                trust: trust(key: key, revoked: true),
                hostApi: 1
            )
        ) { error in
            XCTAssertEqual(error as? FabAppVerificationError, .revokedTrust)
        }
    }

    func testTraversalUnexpectedAndCaseCollisionPathsFailClosed() throws {
        let key = Curve25519.Signing.PrivateKey()

        var traversal = baseManifest()
        traversal["entrypoint"] = "../index.html"
        traversal["files"] = [
            "../index.html": [
                "sha256": sha256(html),
                "bytes": html.count,
                "mime": "text/html",
            ],
        ]
        let traversalManifest = try signedManifest(traversal, key: key)
        XCTAssertThrowsError(
            try FabAppPackageVerifier.verifyCatalogPackage(
                manifestData: traversalManifest,
                resources: ["../index.html": html],
                trust: trust(key: key),
                hostApi: 1
            )
        )

        var collision = baseManifest()
        collision["files"] = [
            "index.html": [
                "sha256": sha256(html),
                "bytes": html.count,
                "mime": "text/html",
            ],
            "INDEX.HTML": [
                "sha256": sha256(html),
                "bytes": html.count,
                "mime": "text/html",
            ],
        ]
        let collisionManifest = try signedManifest(collision, key: key)
        XCTAssertThrowsError(
            try FabAppPackageVerifier.verifyCatalogPackage(
                manifestData: collisionManifest,
                resources: ["index.html": html, "INDEX.HTML": html],
                trust: trust(key: key),
                hostApi: 1
            )
        ) { error in
            guard case FabAppVerificationError.pathCollision = error else {
                return XCTFail("unexpected error: \(error)")
            }
        }

        let validManifest = try signedManifest(baseManifest(), key: key)
        XCTAssertThrowsError(
            try FabAppPackageVerifier.verifyCatalogPackage(
                manifestData: validManifest,
                resources: [
                    "index.html": html,
                    "assets/app.js": script,
                    "hidden.js": script,
                ],
                trust: trust(key: key),
                hostApi: 1
            )
        ) { error in
            XCTAssertEqual(error as? FabAppVerificationError, .unexpectedFile("hidden.js"))
        }
    }

    func testUnknownAndDuplicateSecurityFieldsAreRejected() throws {
        let key = Curve25519.Signing.PrivateKey()
        var unknown = baseManifest()
        unknown["nativeBridge"] = true
        let unknownManifest = try signedManifest(unknown, key: key)

        XCTAssertThrowsError(
            try FabAppPackageVerifier.verifyCatalogPackage(
                manifestData: unknownManifest,
                resources: ["index.html": html, "assets/app.js": script],
                trust: trust(key: key),
                hostApi: 1
            )
        ) { error in
            XCTAssertEqual(error as? FabAppVerificationError, .unknownField("nativeBridge"))
        }

        let duplicate = Data(#"{"manifestVersion":1,"manifestVersion":1}"#.utf8)
        XCTAssertThrowsError(
            try FabAppPackageVerifier.canonicalSigningPayloadForTests(manifestData: duplicate)
        ) { error in
            XCTAssertEqual(
                error as? FabAppVerificationError,
                .duplicateJSONKey("manifestVersion")
            )
        }
    }

    func testLocalGeneratedClassStaysOnDisjointUnimplementedTrustPath() throws {
        let key = Curve25519.Signing.PrivateKey()
        let manifest = try signedManifest(
            baseManifest(packageClass: "local-generated"),
            key: key
        )

        XCTAssertThrowsError(
            try FabAppPackageVerifier.verifyCatalogPackage(
                manifestData: manifest,
                resources: ["index.html": html, "assets/app.js": script],
                trust: trust(key: key),
                hostApi: 1
            )
        ) { error in
            XCTAssertEqual(
                error as? FabAppVerificationError,
                .unsupportedPackageClass
            )
        }
    }

    func testOfflineCoreRejectsNetworkAllowlistAndNewerHostAPI() throws {
        let key = Curve25519.Signing.PrivateKey()
        var networked = baseManifest()
        networked["networkPolicy"] = [
            "mode": "allowlist",
            "allowedOrigins": ["https://example.com"],
        ]
        let networkManifest = try signedManifest(networked, key: key)
        XCTAssertThrowsError(
            try FabAppPackageVerifier.verifyCatalogPackage(
                manifestData: networkManifest,
                resources: ["index.html": html, "assets/app.js": script],
                trust: trust(key: key),
                hostApi: 1
            )
        ) { error in
            XCTAssertEqual(error as? FabAppVerificationError, .invalidNetworkPolicy)
        }

        var future = baseManifest()
        future["minHostApi"] = 2
        let futureManifest = try signedManifest(future, key: key)
        XCTAssertThrowsError(
            try FabAppPackageVerifier.verifyCatalogPackage(
                manifestData: futureManifest,
                resources: ["index.html": html, "assets/app.js": script],
                trust: trust(key: key),
                hostApi: 1
            )
        ) { error in
            XCTAssertEqual(error as? FabAppVerificationError, .incompatibleHostAPI)
        }
    }
}
