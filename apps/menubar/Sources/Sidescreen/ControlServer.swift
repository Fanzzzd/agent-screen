import Foundation

/// Local control channel for the MCP server: newline-delimited JSON over a
/// Unix socket that only the current user can open (mode 0600).
///
///     → {"id": 1, "method": "createDisplay", "params": {...}}
///     ← {"id": 1, "result": {...}}
///     ← {"id": 1, "error": {"code": "not_found", "message": "..."}}
///
/// Requests on one connection may complete out of order; match them by id.
@MainActor
final class ControlServer {
    typealias Handler = @MainActor (_ connection: Int, _ method: String, _ params: [String: Any]) async throws
        -> [String: Any]

    static var socketPath: String {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Sidescreen", isDirectory: true)
        try? FileManager.default.createDirectory(at: base, withIntermediateDirectories: true,
                                                 attributes: [.posixPermissions: 0o700])
        return base.appendingPathComponent("control.sock").path
    }

    private let handle: Handler
    private let onDisconnect: @MainActor (Int) -> Void
    private var listener: Int32 = -1
    private var acceptSource: DispatchSourceRead?
    private var connections: [Int: Connection] = [:]
    private var nextConnectionID = 1

    init(handle: @escaping Handler, onDisconnect: @escaping @MainActor (Int) -> Void) {
        self.handle = handle
        self.onDisconnect = onDisconnect
    }

    func start() throws {
        let path = Self.socketPath
        unlink(path)
        listener = socket(AF_UNIX, SOCK_STREAM, 0)
        guard listener >= 0 else { throw ControlError.failed("socket() failed: \(errno)") }

        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let capacity = MemoryLayout.size(ofValue: address.sun_path)
        guard path.utf8.count < capacity else { throw ControlError.failed("socket path too long") }
        withUnsafeMutableBytes(of: &address.sun_path) { buffer in
            buffer.copyBytes(from: path.utf8)
            buffer[path.utf8.count] = 0
        }
        let bound = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                bind(listener, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard bound == 0 else { throw ControlError.failed("bind() failed: \(errno)") }
        chmod(path, 0o600)
        guard listen(listener, 16) == 0 else { throw ControlError.failed("listen() failed: \(errno)") }

        let source = DispatchSource.makeReadSource(fileDescriptor: listener, queue: .main)
        source.setEventHandler { [weak self] in
            MainActor.assumeIsolated { self?.acceptClient() }
        }
        source.resume()
        acceptSource = source
    }

    private func acceptClient() {
        let fd = accept(listener, nil, nil)
        guard fd >= 0 else { return }
        let id = nextConnectionID
        nextConnectionID += 1
        let connection = Connection(fd: fd)
        connections[id] = connection
        connection.onLine = { [weak self] line in self?.received(line, on: id) }
        connection.onClose = { [weak self] in
            guard let self else { return }
            self.connections.removeValue(forKey: id)
            self.onDisconnect(id)
        }
        connection.start()
    }

    private func received(_ line: Data, on connectionID: Int) {
        guard let object = try? JSONSerialization.jsonObject(with: line) as? [String: Any],
              let method = object["method"] as? String
        else { return }
        let id = object["id"]
        let params = object["params"] as? [String: Any] ?? [:]
        Task {
            var reply: [String: Any] = ["id": id ?? NSNull()]
            do {
                reply["result"] = try await handle(connectionID, method, params)
            } catch let error as ControlError {
                reply["error"] = ["code": error.code, "message": error.message]
            } catch {
                reply["error"] = ["code": "failed", "message": "\(error)"]
            }
            connections[connectionID]?.send(reply)
        }
    }
}

enum ControlError: Error {
    case invalid(String)
    case notFound(String)
    case ended(String)
    case takeover(String)
    case failed(String)

    var code: String {
        switch self {
        case .invalid: "invalid"
        case .notFound: "not_found"
        case .ended: "ended"
        case .takeover: "takeover"
        case .failed: "failed"
        }
    }

    var message: String {
        switch self {
        case .invalid(let m), .notFound(let m), .ended(let m), .takeover(let m), .failed(let m): m
        }
    }
}

/// One client socket. Reads on the main queue, writes on a private queue so
/// a large screenshot never blocks the UI.
@MainActor
private final class Connection {
    let fd: Int32
    var onLine: ((Data) -> Void)?
    var onClose: (() -> Void)?

    private var buffer = Data()
    private var source: DispatchSourceRead?
    private let writer = DispatchQueue(label: "dev.sidescreen.control.write")

    init(fd: Int32) {
        self.fd = fd
        var on: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))
    }

    func start() {
        let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: .main)
        source.setEventHandler { [weak self] in
            MainActor.assumeIsolated { self?.readAvailable() }
        }
        source.resume()
        self.source = source
    }

    private func readAvailable() {
        var chunk = [UInt8](repeating: 0, count: 64 * 1024)
        let count = read(fd, &chunk, chunk.count)
        guard count > 0 else { return close() }
        buffer.append(contentsOf: chunk[0..<count])
        while let newline = buffer.firstIndex(of: 0x0A) {
            let line = buffer[buffer.startIndex..<newline]
            buffer.removeSubrange(buffer.startIndex...newline)
            if !line.isEmpty { onLine?(Data(line)) }
        }
    }

    func send(_ object: [String: Any]) {
        guard var line = try? JSONSerialization.data(withJSONObject: object) else { return }
        line.append(0x0A)
        let data = line
        let fd = self.fd
        writer.async {
            data.withUnsafeBytes { raw in
                var offset = 0
                while offset < raw.count {
                    let written = write(fd, raw.baseAddress! + offset, raw.count - offset)
                    if written <= 0 { return }
                    offset += written
                }
            }
        }
    }

    private func close() {
        source?.cancel()
        source = nil
        let fd = self.fd
        writer.async { Darwin.close(fd) }
        onClose?()
    }
}
