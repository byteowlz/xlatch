import SwiftUI
import VisionKit
import AVFoundation

struct QRScanner: View {
    let onCode: (String) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var authorized = false
    @State private var error: String?
    var body: some View {
        NavigationStack {
            Group {
                if authorized && DataScannerViewController.isSupported && DataScannerViewController.isAvailable {
                    ScannerView(onCode: onCode, onError: { error = $0 }).ignoresSafeArea(edges: .bottom)
                } else if let error {
                    ContentUnavailableView("Camera unavailable", systemImage: "camera", description: Text(error))
                } else { ProgressView("Preparing camera…") }
            }.navigationTitle("Scan pairing code").navigationBarTitleDisplayMode(.inline)
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } } }
                .task {
                    authorized = await AVCaptureDevice.requestAccess(for: .video)
                    if !authorized { error = "Allow camera access in Settings, or use Paste pairing code." }
                    else if !DataScannerViewController.isSupported || !DataScannerViewController.isAvailable { error = "Use Paste pairing code on this device." }
                }
        }
    }
}

struct ScannerView: UIViewControllerRepresentable {
    let onCode: (String) -> Void
    let onError: (String) -> Void
    func makeCoordinator() -> Coordinator { Coordinator(onCode: onCode) }
    func makeUIViewController(context: Context) -> DataScannerViewController {
        let scanner = DataScannerViewController(recognizedDataTypes: [.barcode(symbologies: [.qr])], qualityLevel: .balanced, recognizesMultipleItems: false, isHighFrameRateTrackingEnabled: false, isPinchToZoomEnabled: true, isGuidanceEnabled: true, isHighlightingEnabled: true)
        scanner.delegate = context.coordinator
        do { try scanner.startScanning() } catch { DispatchQueue.main.async { onError(error.localizedDescription) } }
        return scanner
    }
    func updateUIViewController(_ controller: DataScannerViewController, context: Context) {}
    static func dismantleUIViewController(_ controller: DataScannerViewController, coordinator: Coordinator) { controller.stopScanning() }
    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        let onCode: (String) -> Void
        var finished = false
        init(onCode: @escaping (String) -> Void) { self.onCode = onCode }
        func dataScanner(_ scanner: DataScannerViewController, didAdd addedItems: [RecognizedItem], allItems: [RecognizedItem]) {
            guard !finished else { return }
            for case .barcode(let barcode) in addedItems {
                if let value = barcode.payloadStringValue { finished = true; scanner.stopScanning(); onCode(value); break }
            }
        }
    }
}
