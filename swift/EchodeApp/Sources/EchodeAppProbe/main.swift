import EchodeAppCore
import Foundation

if CommandLine.arguments.contains("--main-thread") {
    do { try await runMainThreadProbe(reportPath: CommandLine.arguments.last ?? "target/spike-main-thread.json") }
    catch { print(error); exit(1) }
} else if CommandLine.arguments.contains("--offscreen-piano") {
    do { try await runOffscreenPianoProbe(reportPath: CommandLine.arguments.last ?? "target/spike-piano-frame.json") }
    catch { print(error); exit(1) }
} else if CommandLine.arguments.contains("--offscreen") {
    do { try await runOffscreenArrangeProbe(reportPath: CommandLine.arguments.last ?? "target/spike-arrange-frame.json") }
    catch { print(error); exit(1) }
} else {
    runArrangeProbe(reportPath: CommandLine.arguments.dropFirst().first ?? "target/spike-app.json")
}
