import AppKit
import ScreenCaptureKit

/// Hands out live frames of virtual displays to any number of layers.
/// Each display gets at most one SCStream, running at the highest frame rate
/// any attached layer asked for, and only while something is attached.
@MainActor
final class StreamHub {
    private var streams: [CGDirectDisplayID: DisplayStream] = [:]

    func attach(_ layer: CALayer, to displayID: CGDirectDisplayID, fps: Int) {
        let stream = streams[displayID] ?? DisplayStream(displayID: displayID)
        streams[displayID] = stream
        stream.attach(layer, fps: fps)
    }

    func detach(_ layer: CALayer, from displayID: CGDirectDisplayID) {
        streams[displayID]?.detach(layer)
    }

    /// Capture at 1 fps even with nothing attached, so `stillFrame` always
    /// has a recent frame for the open animation.
    func keepWarm(_ displayID: CGDirectDisplayID) {
        let stream = streams[displayID] ?? DisplayStream(displayID: displayID)
        streams[displayID] = stream
        stream.idleFPS = 1
    }

    func drop(_ displayID: CGDirectDisplayID) {
        streams.removeValue(forKey: displayID)?.shutdown()
    }

    /// The latest frame as a regular bitmap. IOSurface-backed layer contents
    /// ignore scale transforms on ancestors, so anything that animates a
    /// transform has to show a copy instead of the live surface.
    func stillFrame(of displayID: CGDirectDisplayID) -> CGImage? {
        streams[displayID]?.stillFrame()
    }
}

@MainActor
final class DisplayStream {
    let displayID: CGDirectDisplayID

    private struct Subscriber {
        weak var layer: CALayer?
        let fps: Int
    }

    private var subscribers: [ObjectIdentifier: Subscriber] = [:]
    /// Floor for the capture rate while the stream is alive.
    var idleFPS = 0 { didSet { reconcile() } }
    private var stream: SCStream?
    private var runningFPS = 0
    private var busy = false
    private var lastSurface: IOSurface?
    private let queue = DispatchQueue(label: "dev.sidescreen.capture", qos: .userInteractive)
    private lazy var output = FrameOutput { [weak self] surface in self?.present(surface) }

    init(displayID: CGDirectDisplayID) {
        self.displayID = displayID
    }

    func attach(_ layer: CALayer, fps: Int) {
        subscribers[ObjectIdentifier(layer)] = Subscriber(layer: layer, fps: fps)
        if let lastSurface { set(lastSurface, on: layer) }
        reconcile()
    }

    func detach(_ layer: CALayer) {
        subscribers.removeValue(forKey: ObjectIdentifier(layer))
        reconcile()
    }

    func stillFrame() -> CGImage? {
        guard let lastSurface else { return nil }
        let image = CIImage(ioSurface: lastSurface)
        return Self.imageContext.createCGImage(image, from: image.extent)
    }

    private static let imageContext = CIContext()

    func shutdown() {
        subscribers.removeAll()
        idleFPS = 0
        reconcile()
    }

    private var wantedFPS: Int {
        subscribers = subscribers.filter { $0.value.layer != nil }
        return max(subscribers.values.map(\.fps).max() ?? 0, idleFPS)
    }

    /// Converge the SCStream toward `wantedFPS`. Start, stop and reconfigure
    /// are async, so one runs at a time and we re-check when it finishes.
    private func reconcile() {
        guard !busy else { return }
        let wanted = wantedFPS
        if wanted == runningFPS { return }
        busy = true
        Task {
            if wanted == 0 {
                try? await stream?.stopCapture()
                stream = nil
                runningFPS = 0
            } else if let stream {
                try? await stream.updateConfiguration(configuration(fps: wanted))
                runningFPS = wanted
            } else {
                await start(fps: wanted)
            }
            busy = false
            reconcile()
        }
    }

    private func start(fps: Int) async {
        do {
            let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
            guard let display = content.displays.first(where: { $0.displayID == displayID }) else {
                // A just-created display can take a moment to show up.
                try await Task.sleep(for: .milliseconds(300))
                return
            }
            let filter = SCContentFilter(display: display, excludingWindows: [])
            let stream = SCStream(filter: filter, configuration: configuration(fps: fps), delegate: nil)
            try stream.addStreamOutput(output, type: .screen, sampleHandlerQueue: queue)
            try await stream.startCapture()
            self.stream = stream
            runningFPS = fps
        } catch {
            NSLog("Sidescreen: capture for display \(displayID) failed: \(error.localizedDescription)")
            try? await Task.sleep(for: .seconds(2))
        }
    }

    private func configuration(fps: Int) -> SCStreamConfiguration {
        let config = SCStreamConfiguration()
        if let mode = CGDisplayCopyDisplayMode(displayID) {
            config.width = mode.pixelWidth
            config.height = mode.pixelHeight
        }
        config.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(fps))
        config.pixelFormat = kCVPixelFormatType_32BGRA
        config.queueDepth = 4
        // The user's cursor stays on the host screen; agents draw their own.
        config.showsCursor = false
        return config
    }

    private func present(_ surface: IOSurface) {
        lastSurface = surface
        for subscriber in subscribers.values {
            if let layer = subscriber.layer { set(surface, on: layer) }
        }
    }

    private func set(_ surface: IOSurface, on layer: CALayer) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        layer.contents = surface
        CATransaction.commit()
    }
}

private final class FrameOutput: NSObject, SCStreamOutput, @unchecked Sendable {
    private let sink: @MainActor @Sendable (IOSurface) -> Void

    init(sink: @escaping @MainActor @Sendable (IOSurface) -> Void) {
        self.sink = sink
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen,
              sampleBuffer.isValid,
              let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false)
                as? [[SCStreamFrameInfo: Any]],
              let rawStatus = attachments.first?[.status] as? Int,
              SCFrameStatus(rawValue: rawStatus) == .complete,
              let pixelBuffer = sampleBuffer.imageBuffer,
              let surface = CVPixelBufferGetIOSurface(pixelBuffer)?.takeUnretainedValue()
        else { return }

        nonisolated(unsafe) let frame = surface
        let sink = self.sink
        DispatchQueue.main.async {
            MainActor.assumeIsolated { sink(frame) }
        }
    }
}
