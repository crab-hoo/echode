import EchodeBridge
import SwiftUI

struct WorkspaceHistoryView: View {
    let controller: ProjectController
    let close: () -> Void
    @State private var revisions: [SavedRevision] = []
    /// The document the listed revisions belong to.
    @State private var document: DocumentID?
    @State private var notice: String?
    @State private var loaded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text(text("Saved revisions")).font(.headline)
            if loaded && revisions.isEmpty {
                Text(text("No saved revision")).foregroundStyle(.secondary)
            } else {
                List(Array(revisions.enumerated()), id: \.element.id) { index, revision in
                    HStack {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(Self.stamp(revision.time))
                            Text(String(revision.id.prefix(12)))
                                .font(.caption.monospaced())
                                .foregroundStyle(.secondary)
                        }
                        Spacer()
                        if index == 0 {
                            Text(text("Current revision")).font(.caption).foregroundStyle(.secondary)
                        } else {
                            Button(text("Restore")) { Task { await restore(revision.id) } }
                                .disabled(controller.busy)
                        }
                    }
                }
                .frame(minHeight: 220)
            }
            if let notice { Text(text(notice)).font(.caption).foregroundStyle(.secondary) }
            HStack {
                Spacer()
                Button(text("Close"), action: close).keyboardShortcut(.cancelAction)
            }
        }
        .padding(20)
        .frame(width: 440)
        .task { await reload() }
    }

    private func reload() async {
        document = controller.documentID
        revisions = await controller.savedRevisions()
        loaded = true
    }

    private func restore(_ id: String) async {
        guard let document, let report = await controller.restoreRevision(id, documentID: document) else { return }
        revisions = report.revisions
        loaded = true
        notice = report.changed ? nil : "Already at this revision"
    }

    private static func stamp(_ seconds: Int64) -> String {
        Date(timeIntervalSince1970: TimeInterval(seconds)).formatted(date: .abbreviated, time: .standard)
    }
}
