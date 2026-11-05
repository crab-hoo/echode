import Observation
import SwiftUI

@MainActor @Observable
final class ExportPresetState {
    var expanded = false
    var name = ""
    var selectedName = ""
    var message: String?
    private(set) var entries: [String: ExportSettings] = [:]
    private(set) var readable = true

    init(defaults: UserDefaults) { refresh(defaults: defaults) }

    func refresh(defaults: UserDefaults) {
        do {
            entries = try ExportPresets.load(defaults: defaults)
            readable = true
            if entries[selectedName] == nil { selectedName = "" }
        } catch {
            entries = [:]; selectedName = ""; readable = false
            report(error)
        }
    }

    func report(_ error: Error) {
        message = (error as? ExportPresets.Failure ?? .unreadable).message
    }
}

@MainActor
struct ExportPresetControls: View {
    let state: ExportPresetState
    let draft: ExportSettingsDraft
    let defaults: UserDefaults
    let canEdit: () -> Bool
    @Environment(\.locale) private var locale

    var body: some View {
        @Bindable var state = state
        DisclosureGroup(isExpanded: $state.expanded) {
            if state.expanded {
                VStack(alignment: .leading, spacing: 8) {
                    HStack {
                        Picker(text("Preset"), selection: $state.selectedName) {
                            Text(text("Choose a preset")).tag("")
                            ForEach(state.entries.keys.sorted(), id: \.self) { Text($0).tag($0) }
                        }.accessibilityIdentifier("export.presets.selection")
                        Button(text("Apply Preset"), action: apply).disabled(state.selectedName.isEmpty)
                            .accessibilityIdentifier("export.presets.apply")
                        Button(text("Delete Preset"), action: delete).disabled(state.selectedName.isEmpty)
                            .accessibilityIdentifier("export.presets.delete")
                    }
                    HStack {
                        TextField(text("Preset name"), text: $state.name).accessibilityIdentifier("export.presets.name")
                        Button(text("Save Preset"), action: save)
                            .disabled(!state.readable || ExportPresets.normalizedName(state.name) == nil || validatedSettings == nil)
                            .accessibilityIdentifier("export.presets.save")
                    }
                    Text(text(state.message ?? "Presets stay on this Mac. Applying changes only the current fields."))
                        .font(.caption).foregroundStyle(.secondary).accessibilityIdentifier("export.presets.status")
                }.controlSize(.small).padding(.top, 6)
            }
        } label: {
            Text(text("Export Presets")).accessibilityIdentifier("export.presets")
        }.disabled(!canEdit())
            .onAppear { state.refresh(defaults: defaults) }
    }

    private var validatedSettings: ExportSettings? { try? draft.resolve(locale: locale).get() }

    func save() {
        guard canEdit(), let settings = validatedSettings else { return }
        do {
            let name = try ExportPresets.save(name: state.name, settings: settings, defaults: defaults)
            state.refresh(defaults: defaults)
            state.name = name; state.selectedName = name
            state.message = "Export preset saved on this Mac."
        } catch { state.report(error) }
    }

    func apply() {
        guard canEdit() else { return }
        let name = state.selectedName
        state.refresh(defaults: defaults)
        guard state.readable else { return }
        guard let settings = state.entries[name] else { state.report(ExportPresets.Failure.missing); return }
        draft.reset(settings: settings, locale: locale)
        state.message = "Export preset applied to the current fields."
    }

    func delete() {
        guard canEdit() else { return }
        do {
            try ExportPresets.delete(name: state.selectedName, defaults: defaults)
            state.refresh(defaults: defaults)
            state.message = "Export preset deleted from this Mac."
        } catch { state.report(error) }
    }
}
