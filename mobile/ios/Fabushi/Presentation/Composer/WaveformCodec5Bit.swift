//
//  WaveformCodec5Bit.swift
//  Fabushi
//
//  Production 5-bit waveform bit-packing audio codec matching Telegram iOS specification.
//  Encodes 100 amplitude samples into exactly 63 packed bytes (500 bits + 4 padding bits).
//  Provides dynamic ceiling compression, lossless bitstream packing/unpacking, and peak resampling.
//

import Foundation
import CoreGraphics

public enum WaveformCodecError: Error, Equatable, Sendable {
    case corruptedWaveformBufferSize(expected: Int, actual: Int)
    case invalidSampleCount(expected: Int, actual: Int)
}

public struct WaveformCodec5Bit: Sendable {
    public static let sampleCount: Int = 100
    public static let packedByteCount: Int = 63 // ceil(100 * 5 / 8)
    public static let barWidth: CGFloat = 2.0
    public static let barSpacing: CGFloat = 1.0
    public static let maximumLevel: UInt8 = 31

    /// Encodes floating-point amplitude samples [0.0...1.0] into exactly 63 packed bytes.
    /// Applies dynamic ceiling compression so peak level occupies full 5-bit range (0..31).
    public static func encode(samples: [Double]) -> [UInt8] {
        let normalizedSamples = resample(samples: samples, targetCount: sampleCount)
        let peak = normalizedSamples.max() ?? 0.0
        let scale: Double = peak > 0.0001 ? (31.0 / peak) : 0.0

        let quantized: [UInt8] = normalizedSamples.map { sample in
            let scaled = sample * scale
            let rounded = Int(round(scaled))
            return UInt8(clamping: min(31, max(0, rounded)))
        }

        var packed = [UInt8](repeating: 0, count: packedByteCount)
        var bitOffset = 0

        for value in quantized {
            for bit in 0..<5 {
                let bitVal = (value >> bit) & 1
                let byteIdx = (bitOffset + bit) / 8
                let bitIdx = (bitOffset + bit) % 8
                packed[byteIdx] |= (bitVal << bitIdx)
            }
            bitOffset += 5
        }
        return packed
    }

    /// Decodes 63 packed bytes back into 100 5-bit integer amplitude levels (0..31).
    /// Throws `WaveformCodecError.corruptedWaveformBufferSize` if byte count != 63.
    public static func decode(packed: [UInt8]) throws -> [UInt8] {
        guard packed.count == packedByteCount else {
            throw WaveformCodecError.corruptedWaveformBufferSize(
                expected: packedByteCount,
                actual: packed.count
            )
        }

        var result = [UInt8](repeating: 0, count: sampleCount)
        var bitOffset = 0

        for i in 0..<sampleCount {
            var value: UInt8 = 0
            for bit in 0..<5 {
                let byteIdx = (bitOffset + bit) / 8
                let bitIdx = (bitOffset + bit) % 8
                let bitVal = (packed[byteIdx] >> bitIdx) & 1
                value |= (bitVal << bit)
            }
            result[i] = value
            bitOffset += 5
        }
        return result
    }

    /// Resamples arbitrary sample count to exactly 100 samples.
    /// Handles downsampling (peak hold binning) and upsampling (linear interpolation).
    public static func resample(samples: [Double], targetCount: Int = 100) -> [Double] {
        guard !samples.isEmpty else {
            return [Double](repeating: 0.0, count: targetCount)
        }
        if samples.count == targetCount {
            return samples.map { min(1.0, max(0.0, $0)) }
        }

        var result = [Double](repeating: 0.0, count: targetCount)

        if samples.count > targetCount {
            // Downsample via peak binning
            let step = Double(samples.count) / Double(targetCount)
            for i in 0..<targetCount {
                let startIdx = Int(Double(i) * step)
                let endIdx = min(samples.count, max(startIdx + 1, Int(Double(i + 1) * step)))
                var maxVal: Double = 0.0
                for idx in startIdx..<endIdx {
                    maxVal = max(maxVal, samples[idx])
                }
                result[i] = min(1.0, max(0.0, maxVal))
            }
        } else {
            // Upsample via linear interpolation
            let step = Double(samples.count - 1) / Double(targetCount - 1)
            for i in 0..<targetCount {
                let exactIndex = Double(i) * step
                let lower = Int(floor(exactIndex))
                let upper = min(samples.count - 1, lower + 1)
                let fraction = exactIndex - Double(lower)
                let val = samples[lower] * (1.0 - fraction) + samples[upper] * fraction
                result[i] = min(1.0, max(0.0, val))
            }
        }

        return result
    }

    /// Calculates which bar index (0..<sampleCount) represents current playback progress [0.0...1.0].
    public static func activeBarIndex(for progress: Double, totalBars: Int = 100) -> Int {
        let clampedProgress = min(1.0, max(0.0, progress))
        let index = Int(round(clampedProgress * Double(totalBars)))
        return min(totalBars, max(0, index))
    }
}
