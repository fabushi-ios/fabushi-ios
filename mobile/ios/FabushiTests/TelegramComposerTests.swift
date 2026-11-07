//
//  TelegramComposerTests.swift
//  FabushiTests
//
//  Comprehensive native Swift unit test suite covering Features 20, 21, and 22:
//  Auto-Expanding Rich Input Bar (F20), Gesture-Driven Voice Recording (F21),
//  and 5-Bit Waveform Audio Bit-Packing Codec (F22).
//

#if canImport(XCTest)
import XCTest
#if canImport(Fabushi)
@testable import Fabushi
#endif
import SwiftUI

@MainActor
public final class TelegramComposerTests: XCTestCase, @unchecked Sendable {

    // =========================================================================
    // MARK: - FEATURE 20: Auto-Expanding Rich Input Bar
    // =========================================================================

    public func testTC_T1_F20_01_AutoExpandingTextHeightClamping() {
        // Assert initial clamped height is 36pt
        let initialHeight = TelegramComposerLayoutCalculator.clampedHeight(for: 1)
        XCTAssertEqual(initialHeight, 36.0, "Initial single-line height must be 36pt")

        // Assert 3 lines expand within bounds
        let threeLines = TelegramComposerLayoutCalculator.clampedHeight(for: 3)
        XCTAssertGreaterThan(threeLines, 36.0)
        XCTAssertLessThanOrEqual(threeLines, 120.0)

        // Assert 10 lines clamps strictly to 120pt max
        let tenLines = TelegramComposerLayoutCalculator.clampedHeight(for: 10)
        XCTAssertEqual(tenLines, 120.0, "Multi-line text must clamp to 120pt maximum")
    }

    public func testTC_T1_F20_03_SendVsMicButtonMorph() {
        XCTAssertFalse(TelegramComposerBar.shouldShowSendButton(text: ""))
        XCTAssertFalse(TelegramComposerBar.shouldShowSendButton(text: "   \n  "))
        XCTAssertTrue(TelegramComposerBar.shouldShowSendButton(text: "a"))
        XCTAssertTrue(TelegramComposerBar.shouldShowSendButton(text: "Hello Telegram"))
    }

    public func testTC_T1_F20_04_ReplyBannerHeightAndDismiss() {
        let bannerHeight = TelegramReplyBannerView.defaultHeight
        XCTAssertEqual(bannerHeight, 44.0, "Reply banner height must be exactly 44pt")

        let sampleMsg = TelegramStageMessage(
            id: 100, chatId: 1, senderId: 2, senderName: "Alice", text: "Original message", timestamp: 1000
        )
        var didDismiss = false
        var didTap = false

        let banner = TelegramReplyBannerView(
            replyMessage: sampleMsg,
            onTap: { didTap = true },
            onDismiss: { didDismiss = true }
        )
        XCTAssertNotNil(banner)
        banner.onTap()
        XCTAssertTrue(didTap)
        banner.onDismiss()
        XCTAssertTrue(didDismiss)
    }

    public func testTC_T2_F20_01_MaximumTextLengthClamp4096() {
        let oversized = String(repeating: "A", count: 10_000)
        let clamped = TelegramComposerTextLimiter.sanitize(oversized, limit: 4096)
        XCTAssertEqual(clamped.count, 4096, "Text must clamp to exactly 4,096 characters")
        XCTAssertTrue(TelegramComposerTextLimiter.isExceedingLimit(oversized, limit: 4096))
        XCTAssertFalse(TelegramComposerTextLimiter.isExceedingLimit(clamped, limit: 4096))
    }

    public func testTC_T2_F20_02_MultiLineHeightSaturation120pt() {
        let lines20Height = TelegramComposerLayoutCalculator.clampedHeight(for: 20)
        XCTAssertEqual(lines20Height, 120.0, "20 lines must saturate at 120pt")
        let content500pt = TelegramComposerLayoutCalculator.clampedHeight(contentHeight: 500.0)
        XCTAssertEqual(content500pt, 120.0, "500pt content height must clamp to 120pt")
    }

    public func testTC_T2_F20_03_HardwareKeyboardConnected() {
        let isHardware = TelegramComposerBar.isHardwareKeyboardConnected()
        #if canImport(UIKit)
        XCTAssertFalse(isHardware)
        #else
        XCTAssertTrue(isHardware)
        #endif
    }

    public func testTC_T2_F20_04_DraftPersistenceAcrossChatNav() {
        let chatId: Int64 = 8888
        let key = "telegram_draft_\(chatId)"
        UserDefaults.standard.set("Unsent draft message for test", forKey: key)

        let vm = ConversationStageViewModel(chatId: chatId)
        XCTAssertEqual(vm.draftText, "Unsent draft message for test")

        vm.draftText = ""
        vm.saveDraft()
        XCTAssertNil(UserDefaults.standard.string(forKey: key))
    }

    // =========================================================================
    // MARK: - FEATURE 21: Gesture-Driven Voice Recording
    // =========================================================================

    public func testTC_T1_F21_01_HoldToRecordTrigger() {
        let handler = VoiceRecordGestureHandler()
        handler.onTouchBegan()
        handler.onLongPressTriggered()
        XCTAssertTrue(handler.isHolding)
        XCTAssertFalse(handler.isLocked)
    }

    public func testTC_T1_F21_02_SlideLeftToCancel100ptThreshold() {
        let handler = VoiceRecordGestureHandler()
        handler.onLongPressTriggered()

        // Drag -50pt (not yet cancelled)
        handler.onDragChanged(translation: CGSize(width: -50, height: 0))
        XCTAssertTrue(handler.isHolding)

        // Drag -100pt (cancel triggered)
        handler.onDragChanged(translation: CGSize(width: -100, height: 0))
        XCTAssertEqual(handler.state, .cancelled)
    }

    public func testTC_T1_F21_03_SlideUpToLock80ptThreshold() {
        let handler = VoiceRecordGestureHandler()
        handler.onLongPressTriggered()

        // Drag up -50pt (not yet locked)
        handler.onDragChanged(translation: CGSize(width: 0, height: -50))
        XCTAssertTrue(handler.isHolding)

        // Drag up -80pt (lock triggered)
        handler.onDragChanged(translation: CGSize(width: 0, height: -80))
        XCTAssertTrue(handler.isLocked)
    }

    public func testTC_T1_F21_04_LiveWaveformBufferAndRollingWindow() {
        let handler = VoiceRecordGestureHandler()
        handler.onLongPressTriggered()

        for i in 0..<150 {
            handler.appendLiveSample(Double(i) / 150.0)
        }
        XCTAssertEqual(handler.liveSamples.count, 100, "Live samples buffer must cap at 100 samples")
    }

    public func testTC_T2_F21_01_MicroTapUnder150msDiscard() {
        let handler = VoiceRecordGestureHandler()
        var didSend = false
        handler.onTouchEnded(duration: 0.10) { shouldSend in
            didSend = shouldSend
        }
        XCTAssertFalse(didSend, "Taps under 500ms must not trigger voice message send")
        XCTAssertTrue(handler.showTapHint, "Micro tap should show tooltip hint")
    }

    public func testTC_T2_F21_03_ZeroMicrophonePermissionAlert() {
        let handler = VoiceRecordGestureHandler()
        handler.hasMicrophonePermission = false
        handler.onLongPressTriggered()
        XCTAssertFalse(handler.isHolding)
        XCTAssertTrue(handler.showPermissionAlert)
    }

    public func testTC_T2_F21_05_DragOutOfScreenBoundsClamps() {
        let handler = VoiceRecordGestureHandler()
        handler.onLongPressTriggered()

        // Drag far beyond left edge (-1000pt)
        handler.onDragChanged(translation: CGSize(width: -1000, height: -1000))
        XCTAssertEqual(handler.state, .cancelled)
    }

    // =========================================================================
    // MARK: - FEATURE 22: 5-Bit Waveform Codec
    // =========================================================================

    public func testTC_T1_F22_01_Encode100SamplesTo63Bytes() {
        let samples = [Double](repeating: 0.5, count: 100)
        let encoded = WaveformCodec5Bit.encode(samples: samples)
        XCTAssertEqual(encoded.count, 63, "Encoded waveform must be exactly 63 bytes")
    }

    public func testTC_T1_F22_02_DecodedSampleFidelityWithinQuantization() throws {
        var samples = [Double]()
        for i in 0..<100 {
            samples.append(Double(i) / 99.0)
        }
        let encoded = WaveformCodec5Bit.encode(samples: samples)
        let decoded = try WaveformCodec5Bit.decode(packed: encoded)

        XCTAssertEqual(decoded.count, 100)
        XCTAssertEqual(decoded.first, 0)
        XCTAssertEqual(decoded.last, 31)
    }

    public func testTC_T1_F22_03_ZeroAmplitudeEncoding() throws {
        let silence = [Double](repeating: 0.0, count: 100)
        let encoded = WaveformCodec5Bit.encode(samples: silence)
        let decoded = try WaveformCodec5Bit.decode(packed: encoded)

        XCTAssertTrue(decoded.allSatisfy { $0 == 0 }, "Silent audio must decode to level 0")
    }

    public func testTC_T1_F22_04_PeakAmplitudeNormalization() throws {
        // Low peak 0.2: dynamic ceiling must scale peak to level 31
        var lowPeak = [Double](repeating: 0.05, count: 100)
        lowPeak[50] = 0.2
        let encoded = WaveformCodec5Bit.encode(samples: lowPeak)
        let decoded = try WaveformCodec5Bit.decode(packed: encoded)

        XCTAssertEqual(decoded[50], 31, "Peak of 0.2 must scale to maximum 5-bit level 31")
    }

    public func testTC_T1_F22_05_PlaybackScrubSynchronization() {
        XCTAssertEqual(WaveformCodec5Bit.activeBarIndex(for: 0.0), 0)
        XCTAssertEqual(WaveformCodec5Bit.activeBarIndex(for: 0.5), 50)
        XCTAssertEqual(WaveformCodec5Bit.activeBarIndex(for: 1.0), 100)
    }

    public func testTC_T2_F22_01_CorruptedWaveformBufferSizeThrows() {
        let corrupted = [UInt8](repeating: 0, count: 60) // Not 63 bytes
        XCTAssertThrowsError(try WaveformCodec5Bit.decode(packed: corrupted)) { error in
            guard let codecError = error as? WaveformCodecError,
                  case .corruptedWaveformBufferSize(let exp, let act) = codecError else {
                XCTFail("Must throw corruptedWaveformBufferSize")
                return
            }
            XCTAssertEqual(exp, 63)
            XCTAssertEqual(act, 60)
        }
    }

    public func testTC_T2_F22_02_AllMaximumAmplitudeLevel31() throws {
        let maxed = [Double](repeating: 1.0, count: 100)
        let encoded = WaveformCodec5Bit.encode(samples: maxed)
        let decoded = try WaveformCodec5Bit.decode(packed: encoded)

        XCTAssertTrue(decoded.allSatisfy { $0 == 31 }, "All maximum samples must decode to 31")
    }

    public func testTC_T2_F22_03_AlternatingSquareWaveBitPacking() throws {
        var squareWave = [Double]()
        for i in 0..<100 {
            squareWave.append(i % 2 == 0 ? 0.0 : 1.0)
        }
        let encoded = WaveformCodec5Bit.encode(samples: squareWave)
        let decoded = try WaveformCodec5Bit.decode(packed: encoded)

        for i in 0..<100 {
            let expected: UInt8 = (i % 2 == 0) ? 0 : 31
            XCTAssertEqual(decoded[i], expected, "Sample \(i) mismatch in alternating square wave")
        }
    }

    public func testTC_T2_F22_04_PartialSamplesResampling() throws {
        let partial = [Double](repeating: 0.8, count: 35)
        let encoded = WaveformCodec5Bit.encode(samples: partial)
        XCTAssertEqual(encoded.count, 63)
        let decoded = try WaveformCodec5Bit.decode(packed: encoded)
        XCTAssertEqual(decoded.count, 100)
    }

    public func testTC_T2_F22_05_ExtremeAudioDurationResampling() throws {
        let extreme = [Double](repeating: 0.4, count: 10_000)
        let encoded = WaveformCodec5Bit.encode(samples: extreme)
        XCTAssertEqual(encoded.count, 63)
        let decoded = try WaveformCodec5Bit.decode(packed: encoded)
        XCTAssertEqual(decoded.count, 100)
    }
}
#endif
