import EchodeAppCore
import Foundation

runArrangeProbe(reportPath: CommandLine.arguments.dropFirst().first ?? "target/spike-app.json")
