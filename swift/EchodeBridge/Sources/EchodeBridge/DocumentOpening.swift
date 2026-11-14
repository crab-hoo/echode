import CEchodeFfi

private func receiveOpeningFailure(_ context: UnsafeMutableRawPointer?, _ message: UnsafePointer<CChar>?) {
    // Rust calls synchronously before returning null; the stack slot is still
    // live. Copy the borrowed diagnostic before the callback returns.
    context!.assumingMemoryBound(to: String?.self).pointee = message.map { String(cString: $0) }
}

func openDocument(path: String, creating: Bool) throws -> OpaquePointer {
    var message: String?
    let handle = withUnsafeMutablePointer(to: &message) { context in
        path.withCString { path in
            if creating {
                echode_document_create(path, context, receiveOpeningFailure)
            } else {
                echode_document_open(path, context, receiveOpeningFailure)
            }
        }
    }
    guard let handle else {
        throw BridgeError(message: message ?? "package acquisition failed without a diagnostic")
    }
    return handle
}
