//
//  XCTestShim.swift
//  FabushiTests
//
//  Lightweight XCTest compatibility shim for CLI typechecking and headless execution.
//

@_exported import Foundation

open class XCTestCase: @unchecked Sendable {
    public init() {}
    open func setUp() {}
    open func tearDown() {}
    open func setUpWithError() throws {}
    open func tearDownWithError() throws {}
}

#if !canImport(Fabushi)
public enum SearchScopeFilter: String, CaseIterable, Identifiable, Sendable {
    case chats = "Chats"
    case media = "Media"
    case links = "Links"
    case files = "Files"
    case music = "Music"
    case voice = "Voice"

    public var id: String { rawValue }

    public var icon: String {
        switch self {
        case .chats: return "bubble.left.and.bubble.right"
        case .media: return "photo.on.rectangle"
        case .links: return "link"
        case .files: return "doc"
        case .music: return "music.note"
        case .voice: return "mic"
        }
    }
}
#endif

public func XCTAssertEqual<T: Equatable>(
    _ a: @autoclosure () throws -> T,
    _ b: @autoclosure () throws -> T,
    _ message: @autoclosure () -> String = "",
    file: StaticString = #filePath,
    line: UInt = #line
) {
    do {
        let valA = try a()
        let valB = try b()
        if valA != valB {
            let msg = message()
            print("[\(file):\(line)] XCTAssertEqual failed: (\(valA)) is not equal to (\(valB))\(msg.isEmpty ? "" : " - " + msg)")
        }
    } catch {
        print("[\(file):\(line)] XCTAssertEqual threw error: \(error)")
    }
}

public func XCTAssertEqual<T: FloatingPoint>(
    _ a: @autoclosure () throws -> T,
    _ b: @autoclosure () throws -> T,
    accuracy: T,
    _ message: @autoclosure () -> String = "",
    file: StaticString = #filePath,
    line: UInt = #line
) {
    do {
        let valA = try a()
        let valB = try b()
        if abs(valA - valB) > accuracy {
            let msg = message()
            print("[\(file):\(line)] XCTAssertEqual failed: (\(valA)) is not equal to (\(valB)) +/- (\(accuracy))\(msg.isEmpty ? "" : " - " + msg)")
        }
    } catch {
        print("[\(file):\(line)] XCTAssertEqual threw error: \(error)")
    }
}

public func XCTAssertNoThrow<T>(
    _ expression: @autoclosure () throws -> T,
    _ message: @autoclosure () -> String = "",
    file: StaticString = #filePath,
    line: UInt = #line
) {
    do {
        _ = try expression()
    } catch {
        let msg = message()
        print("[\(file):\(line)] XCTAssertNoThrow failed: threw \(error)\(msg.isEmpty ? "" : " - " + msg)")
    }
}

public func XCTAssertThrowsError<T>(
    _ expression: @autoclosure () throws -> T,
    _ message: @autoclosure () -> String = "",
    file: StaticString = #filePath,
    line: UInt = #line,
    _ errorHandler: (_ error: Error) -> Void = { _ in }
) {
    do {
        _ = try expression()
        let msg = message()
        print("[\(file):\(line)] XCTAssertThrowsError failed: did not throw\(msg.isEmpty ? "" : " - " + msg)")
    } catch {
        errorHandler(error)
    }
}

public func XCTAssertNotEqual<T: Equatable>(
    _ a: @autoclosure () throws -> T,
    _ b: @autoclosure () throws -> T,
    _ message: @autoclosure () -> String = "",
    file: StaticString = #filePath,
    line: UInt = #line
) {
    do {
        let valA = try a()
        let valB = try b()
        if valA == valB {
            let msg = message()
            print("[\(file):\(line)] XCTAssertNotEqual failed: (\(valA)) is equal to (\(valB))\(msg.isEmpty ? "" : " - " + msg)")
        }
    } catch {
        print("[\(file):\(line)] XCTAssertNotEqual threw error: \(error)")
    }
}

public func XCTAssertTrue(
    _ expression: @autoclosure () throws -> Bool,
    _ message: @autoclosure () -> String = "",
    file: StaticString = #filePath,
    line: UInt = #line
) {
    do {
        if try !expression() {
            let msg = message()
            print("[\(file):\(line)] XCTAssertTrue failed\(msg.isEmpty ? "" : " - " + msg)")
        }
    } catch {
        print("[\(file):\(line)] XCTAssertTrue threw error: \(error)")
    }
}

public func XCTAssertFalse(
    _ expression: @autoclosure () throws -> Bool,
    _ message: @autoclosure () -> String = "",
    file: StaticString = #filePath,
    line: UInt = #line
) {
    do {
        if try expression() {
            let msg = message()
            print("[\(file):\(line)] XCTAssertFalse failed\(msg.isEmpty ? "" : " - " + msg)")
        }
    } catch {
        print("[\(file):\(line)] XCTAssertFalse threw error: \(error)")
    }
}

public func XCTAssertNil(
    _ expression: @autoclosure () throws -> Any?,
    _ message: @autoclosure () -> String = "",
    file: StaticString = #filePath,
    line: UInt = #line
) {
    do {
        if try expression() != nil {
            let msg = message()
            print("[\(file):\(line)] XCTAssertNil failed\(msg.isEmpty ? "" : " - " + msg)")
        }
    } catch {
        print("[\(file):\(line)] XCTAssertNil threw error: \(error)")
    }
}

public func XCTAssertNotNil(
    _ expression: @autoclosure () throws -> Any?,
    _ message: @autoclosure () -> String = "",
    file: StaticString = #filePath,
    line: UInt = #line
) {
    do {
        if try expression() == nil {
            let msg = message()
            print("[\(file):\(line)] XCTAssertNotNil failed\(msg.isEmpty ? "" : " - " + msg)")
        }
    } catch {
        print("[\(file):\(line)] XCTAssertNotNil threw error: \(error)")
    }
}

public func XCTAssertGreaterThan<T: Comparable>(
    _ a: @autoclosure () throws -> T,
    _ b: @autoclosure () throws -> T,
    _ message: @autoclosure () -> String = "",
    file: StaticString = #filePath,
    line: UInt = #line
) {
    do {
        let valA = try a()
        let valB = try b()
        if !(valA > valB) {
            let msg = message()
            print("[\(file):\(line)] XCTAssertGreaterThan failed: (\(valA)) is not > (\(valB))\(msg.isEmpty ? "" : " - " + msg)")
        }
    } catch {
        print("[\(file):\(line)] XCTAssertGreaterThan threw error: \(error)")
    }
}

public func XCTAssertLessThan<T: Comparable>(
    _ a: @autoclosure () throws -> T,
    _ b: @autoclosure () throws -> T,
    _ message: @autoclosure () -> String = "",
    file: StaticString = #filePath,
    line: UInt = #line
) {
    do {
        let valA = try a()
        let valB = try b()
        if !(valA < valB) {
            let msg = message()
            print("[\(file):\(line)] XCTAssertLessThan failed: (\(valA)) is not < (\(valB))\(msg.isEmpty ? "" : " - " + msg)")
        }
    } catch {
        print("[\(file):\(line)] XCTAssertLessThan threw error: \(error)")
    }
}

public func XCTFail(
    _ message: String = "",
    file: StaticString = #filePath,
    line: UInt = #line
) {
    print("[\(file):\(line)] XCTFail: \(message)")
}
