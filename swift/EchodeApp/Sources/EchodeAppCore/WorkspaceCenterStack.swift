import SwiftUI

/// The product's center column, shared by normal and editor-only layouts.
/// Explicit heights keep opening devices from splitting the piano roll in half.
struct WorkspaceCenterStack<Arrange: View, Editor: View, Devices: View>: View {
    let layout: WorkspaceLayout
    let sizes: WorkspaceDimensions
    let resize: (Double) -> Void
    let finish: () -> Void
    @ViewBuilder var arrange: () -> Arrange
    @ViewBuilder var editor: () -> Editor
    @ViewBuilder var devices: () -> Devices

    var body: some View {
        VStack(spacing: 0) {
            if !layout.editorsMaximized {
                arrange().frame(height: sizes.arrange).clipped()
            }
            if sizes.spacer > 0 {
                Color.clear.frame(height: sizes.spacer).overlay {
                    if !layout.editor && !layout.devices {
                        Text(text("Open Note Editor or Devices")).foregroundStyle(.secondary)
                    }
                }
            }
            if layout.editor {
                if !layout.editorsMaximized {
                    WorkspaceDivider(pane: .editor, value: sizes.editor, resize: resize, finish: finish)
                        .frame(height: sizes.separator)
                }
                editor().frame(height: sizes.editor).clipped()
            }
            if layout.devices {
                if layout.editor || !layout.editorsMaximized {
                    Color(nsColor: .separatorColor).frame(height: sizes.separator).accessibilityHidden(true)
                }
                devices().frame(height: sizes.devices).clipped()
            }
        }.frame(maxWidth: .infinity).clipped()
    }
}
