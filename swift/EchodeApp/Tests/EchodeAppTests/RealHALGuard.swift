import Foundation
import Testing
@testable import EchodeAppCore

// UI tests must never reach a real output device, so this bundle has no HAL:
// these definitions take the place of CoreAudio's when the test bundle links
// the Rust library. Only this bundle links them; the app, the probes and
// EchodeBridgeTests keep the real functions (ADR 0209).
//
// Each call fails as an unsupported operation, so the session stops at its
// first read, before it starts helpers or opens anything. That read comes
// first on every path and with any devices attached, so the refusal does not
// depend on timing. The arguments the SDK passes are not read here.

/// `kAudioHardwareIllegalOperationError` ('nope').
private let refused: Int32 = 0x6E6F_7065

/// `scripts/check_real_hal.py` finds the refusals by this text.
private let marker = "Real HAL reached:"

private func refuse(_ function: StaticString) -> Int32 {
    // A worker thread cannot tell which test it serves, so this issue has none.
    let thread = Thread.current.name.flatMap { $0.isEmpty ? nil : $0 } ?? "an unnamed thread"
    let report = "\(marker) \(function) on \(thread). Inject sendDevice, sendAudio or sendAudition; `make check-real-hal` names the test."
    FileHandle.standardError.write(Data((report + "\n").utf8))
    Issue.record(Comment(rawValue: report))
    return refused
}

@_cdecl("AudioObjectGetPropertyData")
func guardedGetPropertyData() -> Int32 { refuse("AudioObjectGetPropertyData") }

@_cdecl("AudioObjectGetPropertyDataSize")
func guardedGetPropertyDataSize() -> Int32 { refuse("AudioObjectGetPropertyDataSize") }

@_cdecl("AudioObjectSetPropertyData")
func guardedSetPropertyData() -> Int32 { refuse("AudioObjectSetPropertyData") }

@_cdecl("AudioObjectAddPropertyListenerBlock")
func guardedAddPropertyListenerBlock() -> Int32 { refuse("AudioObjectAddPropertyListenerBlock") }

@_cdecl("AudioObjectRemovePropertyListenerBlock")
func guardedRemovePropertyListenerBlock() -> Int32 { refuse("AudioObjectRemovePropertyListenerBlock") }

@_cdecl("AudioDeviceCreateIOProcID")
func guardedCreateIOProcID() -> Int32 { refuse("AudioDeviceCreateIOProcID") }

@_cdecl("AudioDeviceDestroyIOProcID")
func guardedDestroyIOProcID() -> Int32 { refuse("AudioDeviceDestroyIOProcID") }

@_cdecl("AudioDeviceStart")
func guardedStart() -> Int32 { refuse("AudioDeviceStart") }

@_cdecl("AudioDeviceStop")
func guardedStop() -> Int32 { refuse("AudioDeviceStop") }

@MainActor
struct RealHALGuardTests {
    /// `scripts/check_real_hal.py` runs this alone and requires the guard to
    /// stop it by name: a default controller's command still gets here. It runs
    /// only after the script has seen that this bundle binds no real HAL.
    @Test(.enabled(if: ProcessInfo.processInfo.environment["ECHODE_REAL_HAL_CONTROL"] != nil))
    func aDefaultControllerStopsAtTheGuard() async {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-real-hal-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.togglePlayback()
        await controller.close()
    }
}
