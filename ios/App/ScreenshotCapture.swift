import Foundation
import ImageIO
import UniformTypeIdentifiers
import Vision

struct ScreenshotCapture: Sendable {
    let jpeg: Data
    let ocr: String?
    static func prepare(_ data: Data, extractText: Bool) async throws -> ScreenshotCapture {
        try await Task.detached(priority: .userInitiated) {
            guard let source = CGImageSourceCreateWithData(data as CFData, nil),
                  let image = CGImageSourceCreateThumbnailAtIndex(source, 0, [
                    kCGImageSourceCreateThumbnailFromImageAlways: true,
                    kCGImageSourceThumbnailMaxPixelSize: 2048,
                    kCGImageSourceCreateThumbnailWithTransform: true
                  ] as CFDictionary) else { throw ClientError.message("The screenshot is not a supported image.") }
            let output = NSMutableData()
            guard let encoder = CGImageDestinationCreateWithData(output, UTType.jpeg.identifier as CFString, 1, nil) else { throw ClientError.message("Could not prepare screenshot.") }
            CGImageDestinationAddImage(encoder, image, [kCGImageDestinationLossyCompressionQuality: 0.85] as CFDictionary)
            guard CGImageDestinationFinalize(encoder) else { throw ClientError.message("Could not encode screenshot.") }
            var ocr: String?
            if extractText {
                let request = VNRecognizeTextRequest(); request.recognitionLevel = .accurate
                // OCR is enrichment: its failure must not discard the screenshot.
                if (try? VNImageRequestHandler(cgImage: image).perform([request])) != nil {
                    ocr = request.results?.compactMap { $0.topCandidates(1).first?.string }.joined(separator: "\n")
                }
            }
            return ScreenshotCapture(jpeg: output as Data, ocr: ocr)
        }.value
    }
}
