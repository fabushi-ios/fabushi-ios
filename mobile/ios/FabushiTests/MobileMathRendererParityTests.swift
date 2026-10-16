import XCTest
@testable import Fabushi

final class MobileMathRendererParityTests: XCTestCase {
    func testAssistantMathSplitRecognizesDesktopGrammarWithoutSingleDollarMath() {
        let source = #"Before \(x^2 + y^2\) middle $$\frac{1}{2}$$ after $not-math$"#
        let segments = splitMobileAssistantMath(source)
        XCTAssertEqual(
            segments,
            [
                .init(id: 0, kind: .text, text: "Before ", displayMode: false),
                .init(id: 1, kind: .math, text: "x^2 + y^2", displayMode: false),
                .init(id: 2, kind: .text, text: " middle ", displayMode: false),
                .init(id: 3, kind: .math, text: #"\frac{1}{2}"#, displayMode: true),
                .init(id: 4, kind: .text, text: " after $not-math$", displayMode: false),
            ]
        )
    }

    func testAssistantMathSplitRecognizesBracketDisplayMath() {
        XCTAssertEqual(
            splitMobileAssistantMath(#"A \[\sum_{i=1}^{n} i\] B"#),
            [
                .init(id: 0, kind: .text, text: "A ", displayMode: false),
                .init(id: 1, kind: .math, text: #"\sum_{i=1}^{n} i"#, displayMode: true),
                .init(id: 2, kind: .text, text: " B", displayMode: false),
            ]
        )
    }

    func testNativeMathParserAcceptsCommonFormulaAndFailsReadableOnInvalidInput() {
        XCTAssertNil(mobileMathExpressionError(#"\frac{-b \pm \sqrt{b^2-4ac}}{2a}"#))
        XCTAssertNotNil(mobileMathExpressionError(#"\frac{"#))
    }
}
