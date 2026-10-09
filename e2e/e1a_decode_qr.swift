// Decode a captured Fork window using the macOS Vision framework.
import Foundation
import Vision

guard CommandLine.arguments.count == 3 else {
    fatalError("usage: swift e1a_decode_qr.swift screenshot.png private-output.json")
}
let request = VNDetectBarcodesRequest()
request.symbologies = [.qr]
let handler = VNImageRequestHandler(url: URL(fileURLWithPath: CommandLine.arguments[1]))
try handler.perform([request])
let payloads = (request.results ?? []).compactMap { $0.payloadStringValue }
guard payloads.count == 1 else {
    fatalError("expected one QR; observed \(payloads.count)")
}
let output = URL(fileURLWithPath: CommandLine.arguments[2])
try payloads[0].write(to: output, atomically: true, encoding: .utf8)
try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: output.path)
print("Decoded one QR to private output")
