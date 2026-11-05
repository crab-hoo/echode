"""Selected checks require execution evidence or an explicit manual deferral."""

import argparse
from contextlib import redirect_stderr, redirect_stdout
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import select
import shlex
import signal
import subprocess
import sys
import tempfile
import time
import unittest
import unittest.mock
from unittest.mock import patch

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / filename)
    loaded = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loaded)
    return loaded


verify = module("verify", "verify.py")


def load_contracts():
    return module("check_product_contracts", "check_product_contracts.py")
hook = module("check_on_stop", "check_on_stop.py")
wait_for_verify = module("wait_for_verify", "wait_for_verify.py")


def arguments(**overrides):
    options = dict(base=None, all=False, full=False, only=[], run=[], defer=[], reason=None, record_only=False,
                   plan=False, force=False)
    options.update(overrides)
    return argparse.Namespace(**options)


class PolicyTests(unittest.TestCase):
    def setUp(self):
        self.policy = verify.load_policy(ROOT)

    def selected(self, *paths):
        # Which checks a path concerns. The task tier narrows this for full-tier checks.
        return set(verify.select_checks(self.policy, list(paths), full=True))

    def test_export_presets_contract_covers_validated_commands_and_document_boundary(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "export-presets-ui")
        self.assertEqual((contract["runner"], contract["suite"]), ("swift", "ExportPresetTests"))
        self.assertEqual(contract["test"], "contractPresetsUseValidatedFieldsAndRemainIndependentOfDocumentDefaults")
        for path in ["swift/EchodeApp/Sources/EchodeAppCore/ExportPresets.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/ExportPresetControls.swift",
                     "swift/EchodeApp/Tests/EchodeAppTests/ExportPresetTests.swift",
                     "swift/EchodeApp/Tests/EchodeAppTests/ExportPresetStorageTests.swift"]:
            self.assertTrue({"check-swift", "check-product-contracts", "check-real-hal"} <= self.selected(path))
        for fault in contract["negative_controls"]:
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)

    def test_export_defaults_contract_covers_precedence_and_validated_sheet_commands(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "export-defaults-ui")
        self.assertEqual((contract["runner"], contract["suite"]), ("swift", "ExportDefaultsTests"))
        self.assertEqual(contract["test"], "contractDefaultsUseValidatedInputWithoutChangingProjectHistory")
        for path in ["swift/EchodeApp/Sources/EchodeAppCore/ExportSettings.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/WorkspaceExportView.swift",
                     "swift/EchodeApp/Tests/EchodeAppTests/ExportDefaultsTests.swift"]:
            self.assertTrue({"check-swift", "check-product-contracts", "check-real-hal"} <= self.selected(path))
        for fault in contract["negative_controls"]:
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)

    def test_export_input_contract_covers_raw_text_and_the_sheet_gate(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "export-input-ui")
        self.assertEqual((contract["runner"], contract["suite"]), ("swift", "ExportInputTests"))
        self.assertEqual(contract["test"], "contractInvalidNumericTextCannotExportThePreviousValue")
        for path in ["swift/EchodeApp/Sources/EchodeAppCore/WorkspaceExportView.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/ExportSettingsDraft.swift",
                     "swift/EchodeApp/Tests/EchodeAppTests/ExportInputTests.swift",
                     "swift/EchodeApp/Tests/EchodeAppTests/ExportSettingsDraftTests.swift"]:
            self.assertTrue({"check-swift", "check-product-contracts", "check-real-hal"} <= self.selected(path))
        fault = contract["negative_control"]
        self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)

    def test_export_reveal_contract_covers_the_completed_job_and_action(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "export-reveal-ui")
        self.assertEqual((contract["runner"], contract["suite"]), ("swift", "ExportRevealTests"))
        self.assertEqual(contract["test"], "contractRevealUsesOnlyTheCompletedJobsPinnedDestination")
        for path in ["swift/EchodeApp/Sources/EchodeAppCore/ExportJob.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/WorkspaceExportView.swift",
                     "swift/EchodeApp/Tests/EchodeAppTests/ExportRevealTests.swift"]:
            self.assertTrue({"check-swift", "check-product-contracts", "check-real-hal"} <= self.selected(path))
        for fault in contract["negative_controls"]:
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)

    def test_gui_export_destination_contract_covers_capture_before_async_preparation(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "export-destination-ui")
        self.assertEqual((contract["runner"], contract["suite"]), ("swift", "ExportDestinationTests"))
        self.assertEqual(contract["test"], "contractExportPinsTheDestinationBeforePreparingItsSnapshot")
        for path in ["swift/EchodeApp/Sources/EchodeAppCore/ExportJob.swift",
                     "swift/EchodeApp/Tests/EchodeAppTests/ExportDestinationTests.swift"]:
            self.assertTrue({"check-swift", "check-product-contracts", "check-real-hal"} <= self.selected(path))
        fault = contract["negative_control"]
        self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)

    def test_export_destination_contract_covers_source_protection_and_fixture_paths(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "export-destination-cli")
        self.assertEqual((contract["runner"], contract["package"]), ("rust", "echode-cli"))
        self.assertEqual(contract["target"], ["--test", "export_destination"])
        for path in ["crates/echode-cli/src/destination.rs",
                     "crates/echode-cli/tests/export_destination.rs",
                     "crates/echode-cli/tests/support/output.rs"]:
            self.assertTrue({"check", "check-swift", "check-product-contracts"} <= self.selected(path))
        fault = contract["negative_control"]
        self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)

    def test_peak_normalization_contracts_cover_cli_output_and_gui_handoff(self):
        contracts = {c["id"]: c for c in self.policy["product_contracts"]}
        cli = contracts["export-normalization-cli"]
        ui = contracts["export-normalization-ui"]
        self.assertEqual((cli["runner"], cli["package"]), ("rust", "echode-cli"))
        self.assertEqual((ui["runner"], ui["suite"]), ("swift", "ExportNormalizationTests"))
        for path in ["crates/echode-cli/src/normalization.rs",
                     "crates/echode-cli/tests/export_normalization.rs",
                     "swift/EchodeApp/Sources/EchodeAppCore/ExportSettings.swift",
                     "swift/EchodeApp/Tests/EchodeAppTests/ExportNormalizationTests.swift"]:
            self.assertTrue({"check", "check-swift", "check-product-contracts"} <= self.selected(path))
        for contract in [cli, ui]:
            fault = contract["negative_control"]
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)

    def test_gui_export_contract_tracks_the_cli_and_its_snapshot_boundary(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "export-snapshot-ui")
        self.assertEqual((contract["runner"], contract["suite"]), ("swift", "ExportTests"))
        self.assertIn("echode-cli", self.policy["product_contract_inputs"]["swift"]["packages"])
        for path in ["swift/EchodeApp/Sources/EchodeAppCore/ExportJob.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/WorkspaceExportView.swift",
                     "crates/echode-history/src/export.rs"]:
            self.assertTrue({"check", "check-swift", "check-product-contracts"} <= self.selected(path))
        fault = contract["negative_control"]
        source = (ROOT / fault["file"]).read_text()
        self.assertEqual(source.count(fault["old"]), 1)
        self.assertIn("document.history(.list)", fault["new"])

    def test_portable_export_settings_contract_covers_the_single_writer_and_gui(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "export-settings-ui")
        self.assertEqual((contract["runner"], contract["suite"]), ("swift", "ExportSettingsTests"))
        for path in ["crates/echode-history/src/export_settings.rs", "crates/echode-document/src/worker.rs",
                     "swift/EchodeApp/Sources/EchodeAppCore/ExportSettings.swift"]:
            self.assertTrue({"check", "check-swift", "check-product-contracts"} <= self.selected(path))
        fault = contract["negative_control"]
        self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)

    def test_export_cancellation_contracts_cover_control_and_publication(self):
        contracts = {c["id"]: c for c in self.policy["product_contracts"]}
        ui = contracts["export-cancellation-ui"]
        publish = contracts["export-cancellation-publish"]
        self.assertEqual((ui["runner"], ui["suite"]), ("swift", "ExportCancellationTests"))
        self.assertEqual((publish["runner"], publish["package"]), ("rust", "echode-cli"))
        for path in ["crates/echode-cli/src/cancellation.rs",
                     "swift/EchodeApp/Tests/EchodeAppTests/ExportCancellationTests.swift"]:
            self.assertTrue({"check", "check-swift", "check-product-contracts"} <= self.selected(path))
        for contract in [ui, publish]:
            fault = contract["negative_control"]
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)

    def test_gui_export_progress_contract_covers_the_transport_and_cli(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "export-progress-ui")
        self.assertEqual((contract["runner"], contract["suite"]), ("swift", "ExportProgressTests"))
        for path in ["crates/echode-cli/src/progress.rs",
                     "swift/EchodeApp/Sources/EchodeAppCore/ExportProgress.swift",
                     "swift/EchodeApp/Tests/EchodeAppTests/ExportProgressTests.swift"]:
            self.assertTrue({"check", "check-swift", "check-product-contracts"} <= self.selected(path))
        fault = contract["negative_control"]
        self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)

    def test_export_diagnostics_contract_covers_the_product_log_reader(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "export-diagnostics-ui")
        self.assertEqual((contract["runner"], contract["suite"]), ("swift", "ExportDiagnosticsTests"))
        for path in ["swift/EchodeApp/Sources/EchodeAppCore/ExportJob.swift",
                     "swift/EchodeApp/Tests/EchodeAppTests/ExportDiagnosticsTests.swift"]:
            self.assertTrue({"check-swift", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"} <= self.selected(path))
        fault = contract["negative_control"]
        self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        self.assertIn("prefix(65_536)", fault["new"])

    def test_typed_edit_generation_and_product_boundary_remain_selected(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "typed-edit-bridge")
        self.assertEqual((contract["runner"], contract["suite"]), ("swift", "TypedEditTests"))
        fault = contract["negative_control"]
        self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for path in ("crates/echode-actions/examples/swift_actions.rs",
                     "crates/echode-actions/examples/swift_actions/tests.rs",
                     "swift/EchodeBridge/Sources/EchodeBridge/EditAction.swift",
                     "swift/EchodeBridge/Tests/EchodeBridgeTests/Fixtures/edit-actions.json",
                     "swift/EchodeApp/Tests/EchodeAppTests/TypedEditTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan",
                             "check-bridge-tsan", "check-bridge-miri"} <= self.selected(path))
        makefile = (ROOT / "Makefile").read_text()
        self.assertIn("\t$(MAKE) check-swift-actions", makefile)
        command = subprocess.run(["make", "-n", "check-swift-actions"], cwd=ROOT,
                                 check=True, capture_output=True, text=True).stdout
        self.assertIn("--example swift_actions -- --check", command)
        self.assertNotIn("--write", command)

    def test_typed_piano_controller_keeps_generated_actions_and_verification(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "typed-piano-controller")
        self.assertEqual((contract["runner"], contract["suite"]), ("swift", "TypedPianoEditTests"))
        fault = contract["negative_control"]
        controller = (ROOT / fault["file"]).read_text()
        self.assertEqual(controller.count(fault["old"]), 1)
        notes = controller[controller.index("public func insertNote("):controller.index("public func undo()")]
        self.assertEqual(notes.count("performActions("), 8)
        for untyped in ("performEdit(", "JSONSerialization", '"type":', "[String: Any]"):
            self.assertNotIn(untyped, notes)
        for filename in ("PianoMoveBatch.swift", "PianoResizeBatch.swift", "PianoStep.swift", "NoteVelocityBatch.swift"):
            path = "swift/EchodeApp/Sources/EchodeAppCore/" + filename
            source = (ROOT / path).read_text()
            self.assertIn("func actions(in snapshot: Snapshot?) -> [EditAction]?", source)
            self.assertNotIn("[String: Any]", source)
            self.assertNotIn('"type":', source)
            self.assertTrue({"check", "check-product-contracts"} <= self.selected(path))
        self.assertIn("check-product-contracts", self.selected("swift/EchodeApp/Tests/EchodeAppTests/TypedPianoEditTests.swift"))

    def test_typed_track_device_controller_keeps_generated_actions_and_verification(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "typed-track-device-controller")
        self.assertEqual((contract["runner"], contract["suite"]), ("swift", "TypedTrackDeviceEditTests"))
        fault = contract["negative_control"]
        controller = (ROOT / fault["file"]).read_text()
        self.assertEqual(controller.count(fault["old"]), 1)
        for identifier in ("fx-bypass-ui", "fx-offline-ui"):
            related = next(c for c in self.policy["product_contracts"] if c["id"] == identifier)["negative_control"]
            self.assertEqual(related["file"], fault["file"])
            self.assertEqual(controller.count(related["old"]), 1, identifier)
        tracks = controller[controller.index("func insertTrack("):controller.index("public func selectTrack(")]
        devices = controller[controller.index("public func insertPlugin("):controller.index("public func insertNote(")]
        source = (ROOT / "swift/EchodeApp/Sources/EchodeAppCore/DeviceParameters.swift").read_text()
        pins = source[source.index("public func setPinned("):source.index("private func send(")]
        for migrated in (tracks, devices, pins):
            self.assertIn("performActions(", migrated)
            for untyped in ("performEdit(", "await edit(", "JSONSerialization", '"type":', "[String: Any]"):
                self.assertNotIn(untyped, migrated)
        self.assertIn("selectingAddedTrack: selectingAddedTrack", controller)
        for path in ("swift/EchodeApp/Sources/EchodeAppCore/ProjectController.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/DeviceParameters.swift",
                     "swift/EchodeApp/Tests/EchodeAppTests/TypedTrackDeviceEditTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"} <= self.selected(path))

    def test_typed_mixer_controller_keeps_generated_actions_and_verification(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "typed-mixer-controller")
        self.assertEqual((contract["runner"], contract["suite"]), ("swift", "TypedMixerEditTests"))
        controller = (ROOT / "swift/EchodeApp/Sources/EchodeAppCore/ProjectController.swift").read_text()
        for fault in contract["negative_controls"]:
            self.assertEqual(controller.count(fault["old"]), 1)
        mixer = controller[controller.index("public func mixer("):controller.index("/// Runs document work")]
        self.assertEqual(mixer.count("performActions("), 4)
        for filename in ("MixerSwitchBatch.swift", "MixerValueBatch.swift", "PianoTracks.swift"):
            source = (ROOT / "swift/EchodeApp/Sources/EchodeAppCore" / filename).read_text()
            self.assertIn("performActions(", source)
            mixer += source
        for untyped in ("performEdit(", "await edit(", "editTransaction(", "JSONSerialization", '"type":', "[String: Any]"):
            self.assertNotIn(untyped, mixer)
        self.assertIn("finishingPreview: finishingPreview, selectingAddedTrack:", controller)
        for path in ("swift/EchodeApp/Sources/EchodeAppCore/ProjectController.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/MixerSwitchBatch.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/MixerValueBatch.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/PianoTracks.swift",
                     "swift/EchodeApp/Tests/EchodeAppTests/TypedMixerEditTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"} <= self.selected(path))

    def test_document_turns_stay_out_of_the_c_abi_crate(self):
        sources = ROOT / "crates/echode-ffi/src"
        tests = ("tests.rs", "snapshot_ownership.rs")
        for path in sorted(sources.glob("*.rs")):
            if path.name in tests or path.name.endswith("_tests.rs"):
                continue
            source = path.read_text()
            for name in ("catch_unwind", "DocumentHost", "mpsc::", "Session"):
                self.assertNotIn(name, source, path.name)
        ids = {c["id"]: c for c in self.policy["product_contracts"]}
        for identifier in ("document-worker-completion", "document-lane-completion"):
            contract = ids[identifier]
            self.assertEqual((contract["runner"], contract["package"]), ("rust", "echode-ffi"))
            self.assertTrue(contract["test"].startswith("completion_tests::contract_"))
            fault = contract["negative_control"]
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1, identifier)
        self.assertEqual(ids["queued-stop"]["package"], "echode-document")
        for path in ("crates/echode-document/src/worker.rs", "crates/echode-document/src/ticket.rs"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan",
                             "check-bridge-miri"} <= self.selected(path))

    def test_document_operations_take_a_required_id_and_share_one_check(self):
        sources = ROOT / "swift/EchodeApp/Sources/EchodeAppCore"
        controller = (sources / "ProjectController.swift").read_text()
        self.assertEqual(controller.count("session.id == id"), 1)
        # A header compares the document bound to it with a gesture's own payload.
        for path in sorted(sources.glob("*.swift")):
            source = path.read_text()
            if path.name not in ("ArrangeHeaderView.swift", "TrackFXNames.swift"):
                self.assertNotRegex(source, r"[dD]ocumentID == |== (self\.)?documentID\b", path.name)
            for declaration in re.findall(r"func [^\n]*documentID[^\n:]*: DocumentID\?[^\n]*", source):
                self.assertIn("setNavigation(", declaration, path.name)
        ids = {c["id"]: c for c in self.policy["product_contracts"]}
        for identifier in ("document-session-drag-ui", "document-session-audio-ui",
                           "document-session-restore-ui", "document-session-stale-ui"):
            contract = ids[identifier]
            self.assertEqual((contract["runner"], contract["suite"]), ("swift", "DocumentSessionTests"))
            faults = contract.get("negative_controls") or [contract["negative_control"]]
            for fault in faults:
                self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1, identifier)
        for path in ("swift/EchodeApp/Sources/EchodeAppCore/DocumentSession.swift",
                     "swift/EchodeApp/Tests/EchodeAppTests/DocumentSessionTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"} <= self.selected(path))

    def test_typed_clip_controller_keeps_generated_actions_and_verification(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "typed-clip-controller")
        self.assertEqual((contract["runner"], contract["suite"]), ("swift", "TypedClipEditTests"))
        for fault in contract["negative_controls"]:
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        related = next(c for c in self.policy["product_contracts"] if c["id"] == "sample-import-target")["negative_control"]
        self.assertEqual((ROOT / related["file"]).read_text().count(related["old"]), 1)
        for filename in ("ArrangeClipMove.swift", "AudioImportRequest.swift", "ArrangeAudioDrop.swift"):
            source = (ROOT / "swift/EchodeApp/Sources/EchodeAppCore" / filename).read_text()
            self.assertIn("performActions(", source)
        # No controller rebuilds an edit from an untyped dictionary; Document keeps only its raw JSON boundary.
        for source in (ROOT / "swift/EchodeApp/Sources/EchodeAppCore").glob("*.swift"):
            for untyped in ("editTransaction(", "performEdit(", "JSONSerialization", '"type":', "[String: Any]"):
                self.assertNotIn(untyped, source.read_text(), source.name)
        reference = (ROOT / "swift/EchodeBridge/Sources/EchodeBridge/EditText.swift").read_text()
        self.assertIn("case ref, field", reference)
        for path in ("swift/EchodeApp/Sources/EchodeAppCore/ArrangeClipMove.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/AudioImportRequest.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/ArrangeAudioDrop.swift",
                     "swift/EchodeApp/Tests/EchodeAppTests/TypedClipEditTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"} <= self.selected(path))

    def test_model_units_contracts_cover_the_abi_and_swift_consumers(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        native = contracts["model-constants-abi"]
        self.assertEqual((native["runner"], native["package"], native["target"]),
                         ("rust", "echode-ffi", ["--test", "model_constants"]))
        ui = contracts["model-units-ui"]
        self.assertEqual((ui["runner"], ui["suite"]), ("swift", "ModelUnitsTests"))
        for contract in (native, ui):
            for fault in contract.get("negative_controls", [contract.get("negative_control")]):
                self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for path in ("crates/echode-ffi/src/constants.rs", "crates/echode-model/src/device.rs",
                     "swift/EchodeBridge/Sources/EchodeBridge/ModelLimits.swift",
                     "swift/EchodeApp/Tests/EchodeAppTests/ModelUnitsTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan",
                             "check-bridge-tsan", "check-bridge-miri"} <= self.selected(path))

    def test_integer_loop_draft_contract_covers_transport_and_beat_text(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "integer-loop-draft-ui")
        self.assertEqual((contract["runner"], contract["suite"]), ("swift", "LoopDraftTests"))
        self.assertEqual(len(contract["negative_controls"]), 3)
        for fault in contract["negative_controls"]:
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for path in ("swift/EchodeApp/Sources/EchodeAppCore/RulerLoop.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/LoopEntry.swift",
                     "swift/EchodeApp/Tests/EchodeAppTests/LoopDraftTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan",
                             "check-bridge-tsan", "check-bridge-miri"} <= self.selected(path))

    def test_helper_timeout_contracts_and_realtime_selection(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for key, package, target in (
            ("plugin-main-call-ownership", "echode-plugin-host", ["--lib"]),
            ("plugin-main-timeout-state", "echode-plugin-host", ["--test", "audit"]),
            ("plugin-control-request-ids", "echode-sandbox", ["--lib"]),
            ("plugin-control-save-order", "echode-sandbox", ["--lib"]),
        ):
            contract = contracts[key]
            self.assertEqual((contract["runner"], contract["package"], contract["target"]),
                             ("rust", package, target))
            faults = contract.get("negative_controls", [contract.get("negative_control")])
            for fault in faults:
                self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for path in (
            "crates/echode-plugin-host/src/main_requests.rs",
            "crates/echode-plugin-host/src/main_requests_tests.rs",
            "crates/echode-plugin-host/src/threads.rs",
            "crates/echode-sandbox/src/control.rs",
            "crates/echode-sandbox/src/session_correlation_tests.rs",
        ):
            self.assertTrue({"check", "check-product-contracts", "rtsan"} <= self.selected(path))

    def test_mcp_local_arguments_contract_rejects_overrides_before_connecting(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "mcp-local-arguments")
        self.assertEqual((contract["runner"], contract["package"], contract["target"]),
                         ("rust", "echode-api", ["--test", "mcp_arguments"]))
        self.assertEqual(contract["test"], "contract_local_tool_arguments_preserve_backend_history_and_recovery")
        faults = contract["negative_controls"]
        self.assertEqual({fault["old"] for fault in faults}, {
            'if arguments.get("type").is_some() {',
            '.is_some_and(|object| object.is_empty())',
            '.is_some_and(|object| object.keys().any(|key| key != "actions"))',
        })
        for fault in faults:
            self.assertEqual(fault["file"], "crates/echode-api/src/mcp.rs")
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for path in ("crates/echode-api/tests/mcp_arguments.rs", "crates/echode-api/tests/support/mcp_client.rs"):
            self.assertTrue({"check", "check-product-contracts"} <= self.selected(path))
            self.assertFalse({"rtsan", "check-bridge-asan", "check-bridge-tsan"} & self.selected(path))

    def test_mcp_modern_metadata_contract_guards_edits_independently_of_legacy_state(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "mcp-modern-metadata")
        self.assertEqual((contract["runner"], contract["package"], contract["target"]),
                         ("rust", "echode-api", ["--test", "mcp_metadata"]))
        self.assertEqual(contract["test"], "contract_modern_metadata_guards_edits_without_inheriting_legacy_readiness")
        faults = contract["negative_controls"]
        self.assertEqual({fault["old"] for fault in faults}, {
            '.ok_or_else(|| ProtocolError::new(-32602, "protocol version must be a string"))?',
            'if version != MODERN {', 'if !meta[CAPABILITIES_KEY].is_object() {',
        })
        for fault in faults:
            self.assertEqual(fault["file"], "crates/echode-api/src/mcp.rs")
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for path in ("crates/echode-api/tests/mcp_metadata.rs", "crates/echode-api/tests/support/mcp_client.rs"):
            self.assertTrue({"check", "check-product-contracts"} <= self.selected(path))
            self.assertFalse({"rtsan", "check-bridge-asan", "check-bridge-tsan"} & self.selected(path))

    def test_mcp_legacy_lifecycle_contract_guards_readiness_and_id_replay(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "mcp-legacy-lifecycle")
        self.assertEqual((contract["runner"], contract["package"], contract["target"]),
                         ("rust", "echode-api", ["--test", "mcp_lifecycle"]))
        self.assertEqual(contract["test"], "contract_legacy_lifecycle_and_replay_guards_preserve_durable_history")
        faults = contract["negative_controls"]
        self.assertEqual({fault["old"] for fault in faults}, {
            'if method != "ping" && self.lifecycle != Lifecycle::Ready {',
            '&& value.get("params").is_none_or(Value::is_object)',
            'if self.legacy_ids.contains(&id.to_string()) {',
        })
        for fault in faults:
            self.assertEqual(fault["file"], "crates/echode-api/src/mcp.rs")
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for path in ("crates/echode-api/tests/mcp_lifecycle.rs", "crates/echode-api/tests/support/mcp_client.rs"):
            self.assertTrue({"check", "check-product-contracts"} <= self.selected(path))
            self.assertFalse({"rtsan", "check-bridge-asan", "check-bridge-tsan"} & self.selected(path))

    def test_mcp_request_identity_contract_blocks_edits_before_connecting(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "mcp-request-identity")
        self.assertEqual((contract["runner"], contract["package"], contract["target"]),
                         ("rust", "echode-api", ["--test", "mcp_requests"]))
        self.assertEqual(contract["test"], "contract_invalid_mcp_ids_and_notifications_preserve_edits_history_and_recovery")
        faults = contract["negative_controls"]
        self.assertEqual({fault["old"] for fault in faults}, {
            'if valid_id.is_none() {', '            return None;',
        })
        for fault in faults:
            self.assertEqual(fault["file"], "crates/echode-api/src/mcp.rs")
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for path in ("crates/echode-api/tests/mcp_requests.rs", "crates/echode-api/tests/support/mcp_session.rs"):
            self.assertTrue({"check", "check-product-contracts"} <= self.selected(path))
            self.assertFalse({"rtsan", "check-bridge-asan", "check-bridge-tsan"} & self.selected(path))

    def test_mcp_response_envelope_contracts_validate_each_boundary_independently(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for name, target, test in (
            ("mcp-version-envelope", "mcp_version", "contract_invalid_version_envelopes_do_not_authorize_edits"),
            ("mcp-backend-envelope", "mcp_responses", "contract_invalid_backend_envelopes_preserve_uncertain_edits_without_retrying"),
        ):
            contract = contracts[name]
            self.assertEqual((contract["runner"], contract["package"], contract["target"]),
                             ("rust", "echode-api", ["--test", target]))
            self.assertEqual(contract["test"], test)
            faults = contract["negative_controls"]
            self.assertEqual(len(faults), 3)
            self.assertEqual({fault["old"] for fault in faults}, {
                'if response["jsonrpc"] != "2.0"',
                '|| response["id"] != id',
                '(response.get("result").is_some() == response.get("error").is_some())',
            })
            for fault in faults:
                self.assertEqual(fault["file"], "crates/echode-api/src/mcp.rs")
                self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for path in ("crates/echode-api/src/mcp.rs", "crates/echode-api/tests/mcp_version.rs",
                     "crates/echode-api/tests/mcp_responses.rs", "crates/echode-api/tests/support/mcp_envelopes.rs"):
            self.assertTrue({"check", "check-product-contracts"} <= self.selected(path))
            self.assertFalse({"rtsan", "check-bridge-asan", "check-bridge-tsan"} & self.selected(path))

    def test_mcp_backend_version_contract_rejects_edits_before_dispatch(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "mcp-backend-version")
        self.assertEqual((contract["runner"], contract["package"], contract["target"]),
                         ("rust", "echode-api", ["--test", "mcp_version"]))
        self.assertEqual(contract["test"], "contract_backend_version_gate_preserves_edits_history_and_recovery")
        fault = contract["negative_control"]
        self.assertEqual(fault["file"], "crates/echode-api/src/mcp.rs")
        self.assertEqual(fault["old"], 'if version["result"]["version"] != 1 {')
        self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for path in (fault["file"], "crates/echode-api/tests/mcp_version.rs"):
            self.assertTrue({"check", "check-product-contracts"} <= self.selected(path))
            self.assertFalse({"rtsan", "check-bridge-asan", "check-bridge-tsan"} & self.selected(path))

    def test_mcp_backend_error_contract_preserves_uncertain_edit_outcomes(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "mcp-backend-errors")
        self.assertEqual((contract["runner"], contract["package"], contract["target"]),
                         ("rust", "echode-api", ["--test", "mcp_responses"]))
        self.assertEqual(contract["test"], "contract_malformed_backend_errors_preserve_uncertain_edits_without_retrying")
        fault = contract["negative_control"]
        self.assertEqual(fault["file"], "crates/echode-api/src/mcp.rs")
        self.assertIn('error["code"]', fault["old"])
        self.assertIn('error["message"]', fault["old"])
        self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for path in (fault["file"], "crates/echode-api/tests/mcp_responses.rs"):
            self.assertTrue({"check", "check-product-contracts"} <= self.selected(path))
            self.assertFalse({"rtsan", "check-bridge-asan", "check-bridge-tsan"} & self.selected(path))

    def test_mcp_framing_contracts_cover_stdio_and_backend_boundaries(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for name, test in (
            ("mcp-stdio-framing", "contract_unterminated_mcp_input_preserves_edits_history_and_recovery"),
            ("mcp-backend-framing", "contract_unterminated_backend_frames_do_not_authorize_or_acknowledge_edits"),
        ):
            contract = contracts[name]
            self.assertEqual((contract["runner"], contract["package"], contract["target"]),
                             ("rust", "echode-api", ["--test", "mcp_framing"]))
            self.assertEqual(contract["test"], test)
            fault = contract["negative_control"]
            self.assertEqual(fault["file"], "crates/echode-api/src/mcp.rs")
            self.assertIn("!line.ends_with", fault["old"])
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for path in ("crates/echode-api/src/mcp.rs", "crates/echode-api/tests/mcp_framing.rs"):
            self.assertTrue({"check", "check-product-contracts"} <= self.selected(path))
            self.assertFalse({"rtsan", "check-bridge-asan", "check-bridge-tsan"} & self.selected(path))

    def test_rpc_envelope_contracts_keep_public_entry_points_and_negative_controls(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for name in ("rpc-request-envelope", "rpc-notification-boundary"):
            contract = contracts[name]
            self.assertEqual((contract["runner"], contract["package"], contract["target"]),
                             ("rust", "echode-api", ["--test", "request_validation"]))
            self.assertEqual(len(contract["negative_controls"]), 2)
            for mutation in contract["negative_controls"]:
                self.assertEqual(mutation["file"], "crates/echode-api/src/rpc.rs")
        for path in ("crates/echode-api/src/rpc.rs", "crates/echode-api/tests/request_validation.rs"):
            self.assertTrue({"check", "check-product-contracts"} <= self.selected(path))
            self.assertFalse({"rtsan", "check-bridge-asan", "check-bridge-tsan"} & self.selected(path))

    def test_rpc_json_whitespace_contract_covers_both_public_entry_points(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "rpc-json-whitespace")
        self.assertEqual((contract["runner"], contract["package"], contract["target"]),
                         ("rust", "echode-api", ["--test", "request_whitespace"]))
        self.assertEqual(contract["test"], "contract_non_json_whitespace_preserves_edits_history_and_recovery")
        fault = contract["negative_control"]
        self.assertEqual(fault["file"], "crates/echode-api/src/rpc.rs")
        self.assertIn("trim_matches", fault["old"])
        self.assertIn("line.trim()", fault["new"])
        self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for path in (fault["file"], "crates/echode-api/tests/request_whitespace.rs"):
            self.assertTrue({"check", "check-product-contracts"} <= self.selected(path))
            self.assertFalse({"rtsan", "check-bridge-asan", "check-bridge-tsan"} & self.selected(path))

    def test_rpc_framing_contract_rejects_eof_before_dispatch(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "rpc-request-framing")
        self.assertEqual((contract["runner"], contract["package"], contract["target"]),
                         ("rust", "echode-api", ["--test", "request_framing"]))
        self.assertEqual(contract["test"], "contract_unterminated_socket_frames_preserve_edits_history_and_recovery")
        self.assertEqual(contract["negative_control"]["file"], "crates/echode-api/src/serve.rs")
        self.assertIn("!line.ends_with", contract["negative_control"]["old"])
        for path in ("crates/echode-api/src/serve.rs", "crates/echode-api/src/serve_tests.rs",
                     "crates/echode-api/tests/request_framing.rs"):
            self.assertTrue({"check", "check-product-contracts"} <= self.selected(path))
            self.assertFalse({"rtsan", "check-bridge-asan", "check-bridge-tsan"} & self.selected(path))

    def test_platform_configuration_selects_all_automatic_checks_and_product_workgroup_contracts(self):
        automatic = {check["id"] for check in self.policy["checks"] if check["mode"] == "automatic"}
        for path in (".cargo/config.toml", "scripts/build-platform.env"):
            self.assertTrue(automatic <= self.selected(path))
        self.assertIn("scripts/build-platform.env", self.policy["evidence_inputs"])
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        self.assertEqual(contracts["live-workgroup"]["package"], "echode-plugin-host")
        self.assertEqual(contracts["shared-placement-pcm"]["package"], "echode-audio-io")
        self.assertIn("control.workgroup", contracts["live-workgroup"]["negative_control"]["old"])

    def test_graph_preparation_contract_uses_the_product_helper_and_rejects_silence(self):
        contract = next(item for item in self.policy["product_contracts"] if item["id"] == "plugin-state-poll")
        self.assertEqual(contract["package"], "echode-plugin-host")
        self.assertEqual(contract["target"], ["--test", "hosted_playback"])
        self.assertEqual(contract["test"], "contract_preparing_a_graph_keeps_current_audio_running_under_rtsan")
        self.assertIn("return Ok(())", contract["negative_control"]["new"])

    def test_background_graph_uses_held_real_helpers_and_audio_checks(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "background-graph")
        self.assertEqual((contract["runner"], contract["package"], contract["target"]),
                         ("rust", "echode-plugin-host", ["--test", "hosted_playback"]))
        self.assertTrue(contract["test"].startswith("background_graph::contract_"))
        self.assertIn("self.background = false", contract["negative_control"]["new"])
        for path in ("crates/echode-engine/src/playback_preparation.rs",
                     "crates/echode-engine/src/playback_preparation_tests.rs",
                     "crates/echode-plugin-host/tests/hosted_playback/background_graph.rs"):
            self.assertTrue({"check", "check-product-contracts", "rtsan"} <= self.selected(path))

    def test_stop_contract_checks_an_oscillating_waveform_and_cancellation_at_both_gates(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        self.assertEqual(contracts["transport-declick"]["test"], "stream::tests::contract_stop_preserves_waveform_while_fading_under_rtsan")
        self.assertIn("stop_gain", contracts["transport-declick"]["negative_control"]["old"])
        self.assertEqual(contracts["transport-declick"]["negative_control"]["file"], "crates/echode-engine/src/playback.rs")
        self.assertEqual(contracts["transport-cancellation"]["negative_control"]["file"], "crates/echode-audio-io/src/transport.rs")

    def test_stopped_processing_and_device_interaction_use_product_boundaries(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for name in ("stopped-plugin-tails", "stopped-plugin-output"):
            self.assertEqual(contracts[name]["package"], "echode-plugin-host")
            self.assertEqual(contracts[name]["target"], ["--test", "hosted_playback"])
            self.assertTrue(contracts[name]["test"].startswith("stopped_processing::contract_"))
        self.assertEqual(contracts["stop-cleanup"]["test"], "contract_live_discontinuity_cleanup_never_waits_for_a_reset")
        ui = contracts["device-edit-enabled-ui"]
        self.assertEqual((ui["runner"], ui["suite"]), ("swift", "DeviceParameterTests"))
        self.assertIn("exclusive: Bool = true", ui["negative_control"]["new"])

    def test_onset_contract_checks_the_helper_and_sample_accurate_note_delivery(self):
        contract = next(item for item in self.policy["product_contracts"] if item["id"] == "plugin-onset")
        self.assertEqual(contract["package"], "echode-plugin-host")
        self.assertEqual(contract["test"], "contract_plugin_note_onsets_preserve_source_samples_under_rtsan")
        self.assertEqual(contract["negative_control"]["file"], "crates/echode-plugin-host/src/events.rs")
        self.assertIn("note.time", contract["negative_control"]["old"])
        self.assertIn("0,", contract["negative_control"]["new"])

    def test_mix_controls_select_product_ownership_and_live_audio_checks(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for contract_id, swift_test in (("track-mix-ui", "contractHeaderMixerGesturesPersistOnceAndUndo"),
                                       ("clip-mix-ui", "contractClipHandlesPersistGainAndFadesInDocumentTime")):
            self.assertIn(swift_test, contracts[contract_id]["test"])
        self.assertEqual(contracts["live-mix-meter"]["package"], "echode-engine")
        self.assertEqual(contracts["live-mix-meter"]["target"], ["--test", "block_size"])
        for path in ("swift/EchodeApp/Sources/EchodeAppCore/MixControls.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/MixInspectorView.swift",
                     "swift/EchodeBridge/Sources/EchodeBridge/Controls.swift",
                     "crates/echode-ffi/src/controls.rs"):
            with self.subTest(path=path):
                self.assertTrue({"check-product-contracts", "check-bridge-asan", "check-bridge-tsan",
                                 "check-bridge-miri"} <= self.selected(path))
        self.assertIn("rtsan", self.selected("crates/echode-engine/src/meter.rs"))

    def test_phase1_readiness_and_acceptance_use_product_paths(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        expected = {"loop-head-residency": "echode-audio-io",
                    "pre-fader-source": "echode-engine", "phase1-acceptance": "echode-cli"}
        for name, package in expected.items():
            with self.subTest(contract=name):
                self.assertEqual(contracts[name]["package"], package)
                self.assertEqual(contracts[name]["runner"], "rust")
        self.assertTrue(contracts["loop-head-residency"]["test"].endswith("_under_rtsan"))
        self.assertIn("*prepared = None;", contracts["loop-head-residency"]["negative_control"]["new"])
        self.assertTrue(contracts["pre-fader-source"]["test"].endswith("_under_rtsan"))
        check = next(c for c in self.policy["checks"] if c["id"] == "check-product-contracts")
        self.assertIn("echode-cli", check["packages"])
        for path in ("crates/echode-audio-io/src/clips.rs", "crates/echode-engine/src/playback.rs"):
            self.assertTrue({"rtsan", "check-product-contracts"} <= self.selected(path))
        self.assertTrue({"check-bridge-asan", "check-bridge-tsan"}
                        <= self.selected("crates/echode-audio-io/src/clips.rs"))

    def test_fx_chain_controls_keep_their_product_contract_and_bridge_checks(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        self.assertEqual(contracts["fx-chain-ui"]["suite"], "DeviceChainTests")
        self.assertIn("track != target", contracts["fx-chain-ui"]["negative_control"]["old"])
        self.assertEqual(contracts["fx-inspector-ui"]["suite"], "InspectorFXTests")
        self.assertEqual(contracts["fx-inspector-ui"]["negative_control"]["old"], "add(track.id)")
        self.assertEqual(contracts["fx-bypass-ui"]["suite"], "DeviceChainTests")
        self.assertEqual(contracts["fx-batch-ui"]["suite"], "PluginBatchTests")
        self.assertIn("selectedTracks", contracts["fx-batch-ui"]["negative_control"]["old"])
        self.assertEqual(contracts["plugin-bypass"]["package"], "echode-plugin-host")
        self.assertEqual(contracts["fx-offline-ui"]["suite"], "DeviceOfflineTests")
        self.assertEqual(contracts["plugin-offline"]["package"], "echode-plugin-host")
        self.assertEqual(contracts["offline-export"]["package"], "echode-cli")
        for path in ("swift/EchodeApp/Sources/EchodeAppCore/DeviceChain.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/TrackSelection.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/PluginInsertion.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/WorkspaceDeviceView.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/WorkspaceInspectorView.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/InspectorAddFX.swift"):
            self.assertTrue({"check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected(path))
        self.assertTrue({"check", "check-product-contracts", "rtsan", "spike-isolation"}
                        <= self.selected("crates/echode-engine/src/bypass.rs"))

    def test_parameter_list_retry_keeps_the_native_control_and_bridge_checks(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "device-list-retry-ui")
        self.assertEqual((contract["runner"], contract["suite"]), ("swift", "DeviceParameterRetryTests"))
        self.assertIn("onRetry", contract["negative_control"]["old"])
        for path in ("swift/EchodeApp/Sources/EchodeAppCore/DeviceParameters.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/WorkspaceDeviceView.swift",
                     "swift/EchodeApp/Tests/EchodeAppTests/DeviceParameterRetryTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected(path))

    def test_device_preparation_keeps_status_and_native_column_checks(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "device-preparation-ui")
        self.assertEqual((contract["runner"], contract["suite"]), ("swift", "DevicePreparationTests"))
        self.assertIn("outputOpening: outputOpening", contract["negative_control"]["old"])
        for path in ("swift/EchodeBridge/Sources/EchodeBridge/Audio.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/WorkspaceDeviceView.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/ProjectController.swift",
                     "swift/EchodeApp/Tests/EchodeAppTests/DevicePreparationTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected(path))

    def test_positioned_track_insertion_keeps_order_and_ui_contracts(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        self.assertEqual(contracts["track-insertion-ui"]["suite"], "TrackInsertionTests")
        self.assertIn("atEnd", contracts["track-insertion-ui"]["negative_control"]["old"])
        self.assertEqual(contracts["track-insertion-order"]["package"], "echode-history")
        self.assertIn("into_folder", contracts["track-insertion-order"]["negative_control"]["old"])
        for path in ("swift/EchodeApp/Sources/EchodeAppCore/TrackInsertion.swift",
                     "crates/echode-model/src/model.rs"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected(path))
        self.assertTrue({"check", "check-product-contracts", "spike-history"}
                        <= self.selected("crates/echode-actions/src/action.rs"))

    def test_track_duplication_keeps_ui_and_model_contracts(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        self.assertEqual(contracts["track-duplicate-ui"]["suite"], "TrackDuplicationTests")
        self.assertIn("selectedTracks", contracts["track-duplicate-ui"]["negative_control"]["old"])
        self.assertEqual(contracts["track-duplicate-model"]["target"], ["--test", "track_duplication"])
        self.assertIn("copied_contents", contracts["track-duplicate-model"]["negative_control"]["old"])
        for path in ("swift/EchodeApp/Sources/EchodeAppCore/TrackDuplication.swift",
                     "crates/echode-model/src/duplicate.rs"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected(path))

    def test_folder_projection_uses_native_input_and_owned_product_snapshots(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        self.assertEqual(contracts["folder-disclosure-ui"]["suite"], "FolderDisclosureTests")
        self.assertIn("onToggleFolder", contracts["folder-disclosure-ui"]["negative_control"]["old"])
        self.assertEqual(contracts["folder-projection-ownership"]["package"], "echode-ffi")
        self.assertIn("snapshot_ownership::", contracts["folder-projection-ownership"]["test"])
        for path in ("crates/echode-ffi/src/projection.rs", "swift/EchodeBridge/Sources/EchodeBridge/TrackHierarchy.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/TrackFolderLayout.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan", "check-bridge-miri"}
                            <= self.selected(path))

    def test_sample_browser_keeps_search_and_captured_target_contracts(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        self.assertEqual(contracts["sample-library-search"]["suite"], "SampleBrowserTests")
        self.assertEqual(contracts["sample-import-target"]["suite"], "SampleBrowserTests")
        self.assertIn("libraryFiles", contracts["sample-library-search"]["negative_control"]["old"])
        self.assertIn("selection", contracts["sample-import-target"]["negative_control"]["new"])
        for path in ("swift/EchodeApp/Sources/EchodeAppCore/SampleLibrary.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/AudioImportRequest.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected(path))

    def test_sample_file_drops_keep_native_url_and_atomic_import_contracts(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        self.assertEqual(contracts["sample-browser-content-ui"]["suite"], "SampleBrowserContentTests")
        self.assertEqual(contracts["sample-drop-ui"]["suite"], "SampleAudioDropTests")
        self.assertIn(".shift", contracts["sample-drop-ui"]["negative_control"]["old"])
        self.assertEqual(contracts["sample-drop-entry-ui"]["negative_control"]["file"],
                         "swift/EchodeApp/Sources/EchodeAppCore/ArrangeView.swift")
        self.assertEqual(contracts["audio-import-transaction"]["package"], "echode-history")
        for path in ("swift/EchodeApp/Sources/EchodeAppCore/ArrangeAudioDrop.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/FileBrowserState.swift",
                     "crates/echode-history/src/host.rs"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected(path))

    def test_sample_audition_keeps_the_raw_fade_and_selection_contracts(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        engine = contracts["sample-audition"]
        self.assertEqual((engine["runner"], engine["package"], engine["target"]), ("rust", "echode-audio-io", ["--lib"]))
        self.assertTrue(engine["test"].startswith("audition::tests::contract_") and engine["test"].endswith("_under_rtsan"))
        self.assertEqual(engine["negative_control"]["file"], "crates/echode-engine/src/audition.rs")
        ui = contracts["sample-audition-ui"]
        self.assertEqual((ui["runner"], ui["suite"]), ("swift", "SampleBrowserTests"))
        self.assertIn("audio", ui["negative_control"]["old"])
        click = contracts["sample-browser-click-ui"]
        self.assertEqual((click["runner"], click["suite"]), ("swift", "SampleBrowserTests"))
        self.assertEqual(click["negative_control"]["file"], "swift/EchodeApp/Sources/EchodeAppCore/WorkspaceFileList.swift")
        for path in ("crates/echode-engine/src/audition.rs", "crates/echode-audio-io/src/audition.rs"):
            self.assertTrue({"check", "check-product-contracts", "rtsan"} <= self.selected(path))
        for path in ("crates/echode-document/src/service.rs", "swift/EchodeApp/Sources/EchodeAppCore/SampleAudition.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected(path))

    def test_streaming_audition_selects_audio_and_bridge_checks(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        stream = contracts["sample-audition-stream"]
        self.assertEqual((stream["runner"], stream["package"]), ("rust", "echode-audio-io"))
        self.assertEqual(stream["negative_control"]["file"], "crates/echode-audio-io/src/audition_load.rs")
        self.assertEqual(contracts["sample-preview-ui"]["suite"], "SamplePreviewTests")
        self.assertEqual(contracts["sample-audition-loading"]["package"], "echode-audio-io")
        self.assertIn("spike-audition", self.selected("crates/echode-audio-io/src/audition_load.rs"))
        for path in ("crates/echode-audio-io/src/audition_load.rs", "crates/echode-engine/src/resample_stream.rs"):
            self.assertTrue({"check", "check-product-contracts", "rtsan", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected(path))
        self.assertTrue({"check", "check-product-contracts", "rtsan", "check-bridge-asan", "check-bridge-tsan"}
                        <= self.selected("crates/echode-history/src/media.rs"))
        self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                        <= self.selected("swift/EchodeApp/Sources/EchodeAppCore/WorkspaceSamplePreview.swift"))

    def test_audition_controls_keep_residency_position_and_gain_contracts(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        self.assertEqual(contracts["sample-audition-resident"]["package"], "echode-audio-io")
        self.assertTrue(contracts["sample-audition-position"]["test"].endswith("_under_rtsan"))
        self.assertEqual(contracts["sample-audition-gain"]["package"], "echode-engine")
        self.assertEqual(contracts["sample-audition-controls-ui"]["suite"], "SampleAuditionControlTests")
        for path in ("crates/echode-audio-io/src/audition_load.rs", "crates/echode-engine/src/audition.rs"):
            self.assertTrue({"check", "check-product-contracts", "rtsan"} <= self.selected(path))
        for path in ("swift/EchodeApp/Sources/EchodeAppCore/WorkspaceAuditionControls.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/WorkspaceSamplePreview.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected(path))

    def test_output_opening_keeps_the_session_and_worker_contracts(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        session = contracts["output-opening"]
        self.assertEqual((session["runner"], session["package"], session["target"]), ("rust", "echode-audio-io", ["--lib"]))
        self.assertTrue(session["test"].startswith("session::tests::contract_") and session["test"].endswith("_under_rtsan"))
        self.assertEqual(session["negative_control"]["file"], "crates/echode-audio-io/src/session.rs")
        worker = contracts["output-opening-worker"]
        self.assertEqual((worker["runner"], worker["package"], worker["target"]), ("rust", "echode-ffi", ["--lib"]))
        self.assertTrue(worker["test"].startswith("opening_tests::contract_"))
        self.assertIn("audio.opening()", worker["negative_control"]["old"])
        for path in ("crates/echode-audio-io/src/opening.rs", "crates/echode-audio-io/src/session.rs"):
            self.assertTrue({"check", "check-product-contracts", "rtsan"} <= self.selected(path))
        for path in ("crates/echode-document/src/worker.rs", "crates/echode-document/src/device.rs"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected(path))

    def test_output_prewarm_keeps_the_session_worker_and_controller_contracts(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for name in ("output-prewarm", "output-prewarm-play"):
            contract = contracts[name]
            self.assertEqual((contract["runner"], contract["package"], contract["target"]), ("rust", "echode-audio-io", ["--lib"]))
            self.assertTrue(contract["test"].startswith("session::tests::contract_") and contract["test"].endswith("_under_rtsan"))
            self.assertEqual(contract["negative_control"]["file"], "crates/echode-audio-io/src/session.rs")
        self.assertNotEqual(contracts["output-prewarm"]["negative_control"], contracts["output-prewarm-play"]["negative_control"])
        worker = contracts["output-prepare-worker"]
        self.assertEqual((worker["runner"], worker["package"]), ("rust", "echode-ffi"))
        self.assertTrue(worker["test"].startswith("opening_tests::contract_"))
        controller = contracts["output-prepare-on-open"]
        self.assertEqual((controller["runner"], controller["suite"]), ("swift", "TransportRequestTests"))
        self.assertTrue(controller["negative_control"]["file"].endswith("ProjectController.swift"))
        for path in ("crates/echode-engine/src/playback_media.rs", "crates/echode-engine/src/playback.rs",
                     "crates/echode-audio-io/src/session.rs", "crates/echode-audio-io/src/clips.rs"):
            self.assertTrue({"check", "check-product-contracts", "rtsan"} <= self.selected(path))
        self.assertTrue({"check", "check-product-contracts"}
                        <= self.selected("swift/EchodeApp/Sources/EchodeAppCore/ProjectController.swift"))

    def test_cli_dither_changes_select_the_export_measurement_without_realtime_checks(self):
        for path in ("crates/echode-cli/src/dither.rs", "crates/echode-cli/src/dither_tests.rs",
                     "crates/echode-cli/tests/dither.rs"):
            checks = self.selected(path)
            self.assertTrue({"check", "check-product-contracts", "spike-render"} <= checks)
            self.assertNotIn("rtsan", checks)
            self.assertNotIn("spike-audio", checks)

    def test_bundled_instrument_selects_audio_isolation_and_export_regressions(self):
        for path in ("crates/echode-sine-pad/src/lib.rs", "crates/echode-sine-pad/Cargo.toml"):
            self.assertTrue({"check", "check-product-contracts", "rtsan", "spike-isolation", "spike-render"}
                            <= self.selected(path))
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        self.assertEqual(contracts["explicit-sine-pad"]["target"], ["--test", "sine_pad"])
        self.assertEqual(contracts["no-implicit-instrument"]["package"], "echode-engine")

    def test_model_changes_include_transitive_bridge_audio_and_measurements(self):
        checks = self.selected("crates/echode-model/src/time.rs")
        self.assertTrue({"check", "rtsan", "check-bridge-features", "check-bridge-asan",
                         "check-bridge-tsan", "check-bridge-miri", "spike-engine-smoke",
                         "spike-history", "spike-draw", "spike-bridge", "spike-render"} <= checks)

    def test_startup_failure_uses_the_product_history_path_and_a_shutdown_negative_control(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "startup-recovery")
        self.assertEqual(contract["package"], "echode-history")
        self.assertEqual(contract["target"], ["--lib"])
        self.assertIn("Request::Shutdown", contract["negative_control"]["old"])
        self.assertTrue({"check", "check-product-contracts", "check-bridge-features",
                         "check-bridge-asan", "check-bridge-tsan"}
                        <= self.selected("crates/echode-history/src/autosave.rs"))

    def test_recovery_consent_keeps_the_host_ffi_and_ui_boundaries_selected(self):
        contracts = {c["id"]: c for c in self.policy["product_contracts"]}
        rust = contracts["recovery-consent"]
        self.assertEqual((rust["package"], rust["target"]), ("echode-ffi", ["--lib"]))
        self.assertEqual(len(rust["negative_controls"]), 2)
        self.assertEqual(contracts["recovery-consent-ui"]["suite"], "RecoveryConsentTests")
        for path in ("crates/echode-history/src/host.rs", "crates/echode-ffi/src/recovery_tests.rs",
                     "swift/EchodeBridge/Sources/EchodeBridge/History.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/RecoveryConsent.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan",
                             "check-bridge-tsan", "check-bridge-miri"} <= self.selected(path))

    def test_noop_edits_cover_the_session_host_and_real_mcp_entry(self):
        contracts = {c["id"]: c for c in self.policy["product_contracts"]}
        self.assertEqual((contracts["noop-edits"]["package"], contracts["noop-edits"]["target"]),
                         ("echode-history", ["--test", "noop_edits"]))
        self.assertEqual((contracts["mcp-noop-edits"]["package"], contracts["mcp-noop-edits"]["target"]),
                         ("echode-cli", ["--test", "mcp"]))
        self.assertEqual(len(contracts["noop-edits"]["negative_controls"]), 2)
        self.assertEqual((contracts["noop-edits-ui"]["runner"], contracts["noop-edits-ui"]["suite"]),
                         ("swift", "NoopEditTests"))
        for name in ("noop-edits", "mcp-noop-edits"):
            contract = contracts[name]
            faults = contract.get("negative_controls") or [contract["negative_control"]]
            self.assertTrue(all(f["file"] == "crates/echode-actions/src/session.rs" for f in faults))
        for path in ("crates/echode-actions/src/session.rs", "crates/echode-history/tests/noop_edits.rs",
                     "crates/echode-cli/tests/mcp.rs"):
            self.assertTrue({"check", "check-product-contracts"} <= self.selected(path))
        for path in ("crates/echode-document/src/worker.rs",
                     "swift/EchodeApp/Tests/EchodeAppTests/NoopEditTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan",
                             "check-bridge-tsan", "check-bridge-miri"} <= self.selected(path))

    def test_tsan_runtime_is_checked_with_its_negative_control_and_pinned_binaries(self):
        checks = {check["id"]: check for check in self.policy["checks"]}
        self.assertIn("check-tsan-runtime", self.selected("spikes/bridge/sanitizers/Errno.c"))
        self.assertIn("scripts/tsan_runtime.py", checks["check-bridge-tsan"]["inputs"])
        for name in ("check-tsan-runtime", "check-bridge-tsan"):
            inputs = checks[name]["tool_inputs"]
            self.assertIn("target/toolchains/tsan/manifest.json", inputs)
            for variant in ("baseline", "fixed"):
                self.assertTrue(any(f"/{variant}/libclang_rt.tsan_" in path for path in inputs))

    def test_bridge_changes_do_not_launch_unrelated_hardware(self):
        checks = self.selected("spikes/bridge/swift/CBridge.swift")
        self.assertTrue({"check-bridge-asan", "check-bridge-tsan", "check-bridge-miri", "spike-bridge"} <= checks)
        self.assertNotIn("spike-input", checks)
        self.assertNotIn("rtsan", checks)

    def test_product_bridge_and_app_select_ownership_checks(self):
        for path in ("crates/echode-document/src/service.rs", "crates/echode-ffi/src/document.rs",
                     "swift/EchodeBridge/Sources/EchodeBridge/Document.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/ProjectController.swift"):
            with self.subTest(path=path):
                self.assertTrue({"check", "check-bridge-asan", "check-bridge-tsan", "check-bridge-miri"} <= self.selected(path))
                self.assertTrue({"rtsan", "spike-engine-smoke", "spike-audio"} <= self.selected(path))

    def test_product_audio_selects_realtime_and_an_explicit_silent_hal_probe(self):
        for path in ("crates/echode-audio-io/src/stream.rs", "crates/echode-engine/src/playback.rs",
                     "swift/EchodeBridge/Sources/EchodeBridge/Audio.swift"):
            with self.subTest(path=path):
                self.assertTrue({"check", "rtsan", "spike-engine-smoke", "spike-audio"} <= self.selected(path))
                self.assertNotIn("spike-input", self.selected(path))
                if path.startswith("crates/echode-audio-io"):
                    self.assertIn("check-bridge-asan", self.selected(path))

    def test_arrange_pitch_contracts_and_headless_query_measurement_are_selected(self):
        for path in ("crates/echode-draw/src/query.rs", "crates/echode-model/src/model.rs",
                     "swift/EchodeApp/Sources/EchodeAppCore/ArrangeFrame.swift"):
            self.assertIn("spike-arrange-query", self.selected(path))
        self.assertNotIn("spike-arrange-query", self.selected("crates/echode-engine/src/mix.rs"))
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        self.assertEqual(contracts["arrange-note-pitch"]["package"], "echode-draw")
        self.assertEqual(contracts["arrange-note-pitch-ui"]["suite"], "ArrangePitchTests")
        self.assertEqual(len(contracts["arrange-note-pitch-ui"]["negative_controls"]), 2)
        for name in ("arrange-note-pitch", "arrange-note-pitch-ui"):
            contract = contracts[name]
            faults = contract.get("negative_controls") or [contract["negative_control"]]
            for fault in faults:
                self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)

    def test_product_drawing_has_a_separate_real_display_probe(self):
        self.assertIn("spike-app", self.selected("swift/EchodeApp/Sources/EchodeAppCore/ArrangeView.swift"))
        for file in ("PianoRollView.swift", "PianoGeometry.swift", "PianoFrame.swift", "PianoOffscreenProbe.swift", "ArrangeGeometry.swift",
                     "ArrangeHeaderView.swift", "ArrangePaneView.swift", "ArrangeFrame.swift",
                     "MixControls.swift", "MixInspectorView.swift",
                     "ArrangeLabels.swift", "ArrangePainter.swift", "ArrangeOffscreenProbe.swift", "WorkspaceView.swift",
                     "WorkspaceState.swift", "WorkspaceDivider.swift", "WorkspaceNoteEditorView.swift", "CanvasRenderer.swift", "CanvasSurface.swift", "CanvasBackground.swift", "FrameFeedbackGate.swift"):
            with self.subTest(file=file):
                checks = self.selected("swift/EchodeApp/Sources/EchodeAppCore/" + file)
                self.assertTrue({"spike-app", "spike-playhead", "spike-arrange-frame", "spike-piano-frame", "spike-draw", "check-bridge-asan", "check-bridge-tsan", "check-bridge-miri"} <= checks)
        self.assertNotIn("spike-app", self.selected("crates/echode-engine/src/voice.rs"))
        self.assertNotIn("spike-playhead", self.selected("crates/echode-engine/src/voice.rs"))

    def test_workspace_layout_keeps_command_and_native_stack_contracts(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for name, source in (("workspace-layout-ui", "WorkspaceCommand.swift"),
                             ("workspace-stack-ui", "WorkspaceCenterStack.swift")):
            contract = contracts[name]
            self.assertEqual((contract["runner"], contract["suite"]), ("swift", "WorkspaceLayoutTests"))
            self.assertTrue(contract["negative_control"]["file"].endswith(source))
        for name in ("WorkspaceLayout.swift", "WorkspaceCenterStack.swift", "WorkspaceState.swift", "WorkspaceCommand.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan",
                             "spike-app", "spike-playhead"} <= self.selected("swift/EchodeApp/Sources/EchodeAppCore/" + name))

    def test_canvas_resize_keeps_surface_and_header_regressions_in_the_product_gate(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for name, source in (("canvas-resize-ui", "CanvasSurface.swift"), ("header-width-ui", "ArrangePaneView.swift"),
                             ("arrange-camera-ui", "ArrangePaneView.swift"),
                             ("arrange-camera-input-ui", "ArrangePaneView.swift")):
            contract = contracts[name]
            self.assertEqual((contract["runner"], contract["suite"]), ("swift", "CanvasResizeTests"))
            faults = contract.get("negative_controls") or [contract["negative_control"]]
            self.assertTrue(all(fault["file"].endswith(source) for fault in faults))
        canvas_faults = contracts["canvas-resize-ui"]["negative_controls"]
        self.assertIn("scaleAxesIndependently", canvas_faults[0]["new"])
        self.assertIn("contentsGravity", canvas_faults[1]["new"])
        self.assertIn("camera.constrain", contracts["arrange-camera-ui"]["negative_control"]["new"])
        self.assertIn("camera.originTrack", contracts["arrange-camera-input-ui"]["negative_control"]["new"])
        mismatch = contracts["drawable-mismatch-ui"]
        self.assertEqual((mismatch["runner"], mismatch["suite"], mismatch["test"]),
                         ("swift", "CanvasSubmissionTests", "contractStaleDrawableSubmissionDrawsAndRetries"))
        self.assertTrue(mismatch["negative_controls"][0]["file"].endswith("CanvasRenderer.swift"))
        self.assertIn("if submission.mismatched", mismatch["negative_controls"][0]["new"])
        self.assertIn("presentBackground", mismatch["negative_controls"][0]["new"])
        self.assertIn("drawnRevision = revision", mismatch["negative_controls"][1]["new"])
        self.assertIn("staleDrawable: false", mismatch["negative_controls"][2]["new"])
        self.assertTrue(mismatch["negative_controls"][3]["file"].endswith("ArrangeOffscreenProbe.swift"))
        self.assertTrue(mismatch["negative_controls"][4]["file"].endswith("ArrangePainter.swift"))
        self.assertIn("intersection(bounds)", mismatch["negative_controls"][4]["old"])
        for name in ("CanvasSurface.swift", "CanvasBackground.swift", "CanvasRenderer.swift", "ArrangePaneView.swift", "ArrangeView.swift", "PianoRollView.swift"):
            checks = self.selected("swift/EchodeApp/Sources/EchodeAppCore/" + name)
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan",
                             "spike-arrange-frame", "spike-piano-frame", "spike-playhead"} <= checks)

    def test_shared_piano_viewport_contract_covers_switches_and_document_lifetime(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "piano-shared-viewport-ui")
        self.assertEqual((contract["runner"], contract["suite"], contract["test"]),
                         ("swift", "PianoViewportTests", "contractTrackSwitchesKeepTheSharedPianoViewport"))
        self.assertEqual(len(contract["negative_controls"]), 3)
        for fault in contract["negative_controls"]:
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in ("PianoRollView.swift", "WorkspaceState.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"} <=
                            self.selected("swift/EchodeApp/Sources/EchodeAppCore/" + source))
        self.assertIn("check-product-contracts", self.selected("swift/EchodeApp/Tests/EchodeAppTests/PianoViewportTests.swift"))

    def test_clip_duplicate_key_contracts_cover_native_input_window_and_local_ownership(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for name, suite in (("clip-duplicate-key-ui", "ClipDuplicationTests"),
                            ("clip-duplicate-key-guards-ui", "ClipDuplicationBoundaryTests")):
            contract = contracts[name]
            self.assertEqual((contract["runner"], contract["suite"]), ("swift", suite))
            self.assertEqual(len(contract["negative_controls"]), 2)
            for fault in contract["negative_controls"]:
                self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in ("Sources/EchodeAppCore/ArrangeView.swift", "Sources/EchodeAppCore/ArrangeClipMove.swift",
                       "Tests/EchodeAppTests/ClipDuplicationTests.swift", "Tests/EchodeAppTests/ClipDuplicationBoundaryTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected("swift/EchodeApp/" + source))

    def test_native_scan_deadline_contract_covers_fixture_sources_and_failure_controls(self):
        contract = next(item for item in self.policy["product_contracts"] if item["id"] == "scan-deadline")
        self.assertEqual((contract["runner"], contract["package"], contract["target"], contract["test"]),
                         ("rust", "echode-plugin-host", ["--test", "scan"],
                          "audit_rea08_descendant_pipe_does_not_extend_deadline"))
        self.assertEqual(len(contract["negative_controls"]), 2)
        for fault in contract["negative_controls"]:
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in ("Cargo.toml", "src/scan.rs", "tests/scan.rs", "tests/support/scan_helper.rs"):
            self.assertTrue({"check", "check-product-contracts", "spike-isolation"}
                            <= self.selected("crates/echode-plugin-host/" + source))

    def test_mixer_switch_contracts_cover_captured_groups_order_and_document_scope(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for name, suite in (("mixer-switch-batch-ui", "MixerSwitchBatchTests"),
                            ("mixer-switch-order-ui", "MixerSwitchBoundaryTests")):
            contract = contracts[name]
            self.assertEqual((contract["runner"], contract["suite"]), ("swift", suite))
            self.assertEqual(len(contract["negative_controls"]), 2)
            for fault in contract["negative_controls"]:
                self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in ("Sources/EchodeAppCore/MixerSwitchBatch.swift", "Sources/EchodeAppCore/MixControls.swift",
                       "Sources/EchodeAppCore/ArrangeHeaderView.swift", "Sources/EchodeAppCore/MixInspectorView.swift",
                       "Sources/EchodeAppCore/WorkspaceView.swift", "Sources/EchodeAppCore/ProjectController.swift",
                       "Tests/EchodeAppTests/MixerSwitchBatchTests.swift", "Tests/EchodeAppTests/MixerSwitchBoundaryTests.swift",
                       "Tests/EchodeAppTests/MixerSwitchValidationTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected("swift/EchodeApp/" + source))

    def test_mixer_drag_contracts_cover_groups_preview_audio_bounds_and_history(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for name in ("mixer-drag-batch-ui", "mixer-drag-bounds-ui", "mixer-group-audio"):
            contract = contracts[name]
            faults = contract.get("negative_controls", [contract.get("negative_control")])
            self.assertTrue(faults)
            for fault in faults:
                self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in ("Sources/EchodeAppCore/MixerFaderBatch.swift", "Tests/EchodeAppTests/MixerDragBatchTests.swift",
                       "Tests/EchodeAppTests/MixerDragBoundaryTests.swift", "Tests/EchodeAppTests/MixerDragAudioTests.swift"):
            self.assertTrue({"check-swift", "check-product-contracts", "check-real-hal"}
                            <= self.selected("swift/EchodeApp/" + source))
        self.assertTrue({"check", "rtsan", "check-product-contracts"}
                        <= self.selected("crates/echode-engine/src/playback.rs"))

    def test_mixer_value_contracts_cover_numeric_groups_common_bounds_and_document_scope(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for name, suite, faults in (("mixer-value-batch-ui", "MixerValueBatchTests", 3),
                                    ("mixer-value-bounds-ui", "MixerValueBoundaryTests", 3)):
            contract = contracts[name]
            self.assertEqual((contract["runner"], contract["suite"]), ("swift", suite))
            self.assertEqual(len(contract["negative_controls"]), faults)
            for fault in contract["negative_controls"]:
                self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in ("Sources/EchodeAppCore/MixerValueBatch.swift", "Sources/EchodeAppCore/MixControls.swift",
                       "Sources/EchodeAppCore/ValueControl.swift",
                       "Sources/EchodeAppCore/ArrangeHeaderView.swift", "Sources/EchodeAppCore/MixInspectorView.swift",
                       "Tests/EchodeAppTests/MixerValueBatchTests.swift", "Tests/EchodeAppTests/MixerValueBoundaryTests.swift",
                       "Tests/EchodeAppTests/MixerValueOrderTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected("swift/EchodeApp/" + source))

    def test_mixer_step_contracts_cover_native_capture_common_limits_and_old_controls(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for name, suite, faults in (("mixer-step-ui", "MixerStepTests", 2),
                                    ("mixer-step-bounds-ui", "MixerStepBoundaryTests", 3)):
            contract = contracts[name]
            self.assertEqual((contract["runner"], contract["suite"]), ("swift", suite))
            self.assertEqual(len(contract["negative_controls"]), faults)
            for fault in contract["negative_controls"]:
                self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in ("Sources/EchodeAppCore/MixerValueBatch.swift", "Sources/EchodeAppCore/MixControls.swift",
                       "Sources/EchodeAppCore/ArrangeHeaderView.swift", "Sources/EchodeAppCore/MixerSwitchBatch.swift",
                       "Tests/EchodeAppTests/MixerStepTests.swift", "Tests/EchodeAppTests/MixerStepBoundaryTests.swift",
                       "Tests/EchodeAppTests/MixerStepOrderTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected("swift/EchodeApp/" + source))

    def test_mixer_gesture_contracts_cover_document_replacement_and_callback_ownership(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for name, test, faults in (("mixer-gesture-document-ui", "contractOldHeaderPressCannotStartADragInTheReopenedDocument", 1),
                                   ("mixer-reset-document-ui", "contractOldResetAndEntryCannotFinishANewDocumentsPreview", 2)):
            contract = contracts[name]
            self.assertEqual((contract["runner"], contract["suite"], contract["test"]), ("swift", "MixerGestureDocumentTests", test))
            controls = contract.get("negative_controls", [contract.get("negative_control")])
            self.assertEqual(len(controls), faults)
            for fault in controls:
                self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in ("Sources/EchodeAppCore/ArrangeHeaderView.swift", "Tests/EchodeAppTests/MixerGestureDocumentTests.swift",
                       "Tests/EchodeAppTests/PluginBatchTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected("swift/EchodeApp/" + source))

    def test_mixer_drag_identity_contract_covers_quiet_values_history_and_reopen(self):
        contract = next(item for item in self.policy["product_contracts"] if item["id"] == "mixer-drag-identity-ui")
        self.assertEqual((contract["runner"], contract["suite"], contract["test"]),
                         ("swift", "MixerDragIdentityTests", "contractZeroHorizontalHeaderDragsPreserveQuietGainAndRedo"))
        fault = contract["negative_control"]
        self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in ("Sources/EchodeAppCore/MixControls.swift", "Tests/EchodeAppTests/MixerDragIdentityTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected("swift/EchodeApp/" + source))

    def test_waveform_boundary_silence_contract_covers_public_columns(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "waveform-boundary-silence")
        self.assertEqual((contract["runner"], contract["package"], contract["target"]),
                         ("rust", "echode-history", ["--test", "waveform_silence"]))
        self.assertEqual(contract["test"], "contract_waveform_columns_include_silence_across_source_and_placement_boundaries")
        fault = contract["negative_control"]
        self.assertEqual(fault["file"], "crates/echode-history/src/peaks.rs")
        self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in (fault["file"], "crates/echode-history/tests/waveform_silence.rs"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected(source))

    def test_stereo_waveform_contract_covers_both_averages_and_cache_version(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "stereo-waveform-average")
        self.assertEqual((contract["runner"], contract["package"], contract["target"]),
                         ("rust", "echode-history", ["--test", "stereo_waveform"]))
        self.assertEqual(contract["test"], "contract_stereo_waveforms_preserve_finite_averages_and_rebuild_legacy_caches")
        self.assertEqual(len(contract["negative_controls"]), 3)
        for fault in contract["negative_controls"]:
            self.assertEqual(fault["file"], "crates/echode-history/src/peaks.rs")
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in ("crates/echode-history/src/peaks.rs", "crates/echode-history/tests/stereo_waveform.rs"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected(source))

    def test_peak_cache_validation_contract_covers_public_waveform_loading(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "peak-cache-validation")
        self.assertEqual((contract["runner"], contract["package"], contract["target"]),
                         ("rust", "echode-history", ["--test", "peak_cache_validation"]))
        self.assertEqual(contract["test"], "contract_malformed_peak_caches_rebuild_from_unchanged_pcm")
        self.assertEqual(len(contract["negative_controls"]), 2)
        for fault in contract["negative_controls"]:
            self.assertEqual(fault["file"], "crates/echode-history/src/peaks.rs")
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in ("crates/echode-history/src/peaks.rs", "crates/echode-history/tests/peak_cache_validation.rs"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected(source))

    def test_canonical_pcm_validation_uses_public_reads_and_product_media_preparation(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "canonical-pcm-validation")
        self.assertEqual((contract["runner"], contract["package"], contract["target"]),
                         ("rust", "echode-audio-io", ["--test", "pcm_validation"]))
        self.assertEqual(contract["test"], "contract_stored_pcm_rejects_nonfinite_samples_before_clip_preparation")
        fault = contract["negative_control"]
        self.assertEqual(fault["file"], "crates/echode-history/src/media.rs")
        self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in (fault["file"], "crates/echode-audio-io/tests/pcm_validation.rs",
                       "crates/echode-audio-io/src/clips_tests.rs"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan", "rtsan"}
                            <= self.selected(source))

    def test_store_gitignore_contract_covers_import_and_autosave_entry_points(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "store-gitignore")
        self.assertEqual((contract["runner"], contract["package"], contract["target"]),
                         ("rust", "echode-history", ["--test", "store_gitignore"]))
        self.assertEqual(contract["test"], "contract_import_and_autosave_keep_exact_store_ignore_rules")
        fault = contract["negative_control"]
        self.assertEqual(fault["file"], "crates/echode-history/src/objects.rs")
        self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in (fault["file"], "crates/echode-history/tests/store_gitignore.rs",
                       "crates/echode-history/src/media.rs", "crates/echode-history/src/autosave.rs"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected(source))

    def test_audio_import_validation_rejects_external_samples_before_package_writes(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "audio-import-validation")
        self.assertEqual((contract["runner"], contract["package"], contract["target"]),
                         ("rust", "echode-history", ["--test", "audio_import_validation"]))
        self.assertEqual(contract["test"], "contract_audio_import_rejects_nonfinite_before_writing_and_preserves_history")
        fault = contract["negative_control"]
        self.assertEqual(fault["file"], "crates/echode-history/src/media.rs")
        self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in (fault["file"], "crates/echode-history/tests/audio_import_validation.rs"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected(source))
        self.assertIn("rtsan", self.selected(fault["file"]))

    def test_clip_identity_and_float_roundtrip_contracts_preserve_disk_and_input_boundaries(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        disk = contracts["package-float-roundtrip"]
        self.assertEqual((disk["runner"], disk["package"], disk["target"]),
                         ("rust", "echode-history", ["--test", "float_roundtrip"]))
        self.assertEqual(disk["test"], "contract_package_floats_preserve_quiet_gains_and_pan_bits")
        native = contracts["clip-drag-identity-ui"]
        self.assertEqual((native["runner"], native["suite"], native["test"]),
                         ("swift", "ClipDragIdentityTests", "contractOrthogonalClipDragsPreserveExactValuesAndRedo"))
        self.assertEqual(len(native["negative_controls"]), 3)
        for fault in [disk["negative_control"], *native["negative_controls"]]:
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in ("crates/echode-history/src/document.rs", "crates/echode-history/tests/float_roundtrip.rs",
                       "crates/echode-history/src/host.rs", "crates/echode-history/src/autosave.rs",
                       "swift/EchodeApp/Tests/EchodeAppTests/ClipDragIdentityTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected(source))

    def test_reset_target_contracts_cover_both_native_views_and_select_dynamic_checks(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for name, test, source in (
                ("mixer-reset-target-ui", "contractHeaderResetWaitDoesNotBorrowTheNextControlsPress", "ArrangeHeaderView.swift"),
                ("clip-reset-target-ui", "contractClipResetWaitKeepsOnlyTheMatchingHandlesPress", "ArrangeView.swift")):
            contract = contracts[name]
            self.assertEqual((contract["runner"], contract["suite"], contract["test"]), ("swift", "ValueControlTargetTests", test))
            faults = contract.get("negative_controls", [contract.get("negative_control")])
            fault = faults[0]
            if name == "mixer-reset-target-ui":
                self.assertEqual(len(faults), 2)
                self.assertTrue(faults[1]["file"].endswith("/MixerFaderBatch.swift"))
                self.assertEqual((ROOT / faults[1]["file"]).read_text().count(faults[1]["old"]), 1)
            self.assertTrue(fault["file"].endswith("/" + source))
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
            self.assertIn(".waiting(target)", fault["old"])
            self.assertIn(".waiting =", fault["new"])
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected("swift/EchodeApp/Sources/EchodeAppCore/" + source))
        self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                        <= self.selected("swift/EchodeApp/Tests/EchodeAppTests/ValueControlTargetTests.swift"))

    def test_clip_gestures_cover_document_replacement_and_callback_ownership(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for name, test in (("clip-gesture-document-ui", "contractOldClipControlsCannotEditTheReopenedDocument"),
                           ("clip-gesture-callback-ui", "contractClipGestureKeepsItsOriginalEditCallback")):
            contract = contracts[name]
            self.assertEqual((contract["runner"], contract["suite"], contract["test"]), ("swift", "ClipGestureDocumentTests", test))
            self.assertEqual(len(contract["negative_controls"]), 2)
            for fault in contract["negative_controls"]:
                self.assertTrue(fault["file"].endswith("/ArrangeView.swift"))
                self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in ("Sources/EchodeAppCore/ArrangeView.swift", "Tests/EchodeAppTests/ClipGestureDocumentTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected("swift/EchodeApp/" + source))

    def test_piano_time_guides_contract_covers_independent_lines_deduplication_and_priority(self):
        contract = next(item for item in self.policy["product_contracts"] if item["id"] == "piano-time-guides-ui")
        self.assertEqual((contract["runner"], contract["suite"], contract["test"]),
                         ("swift", "PianoTimeGuideTests", "contractBeatAndBarGuidesSurviveCoarseAndThinnedRhythmicGrids"))
        self.assertEqual(len(contract["negative_controls"]), 3)
        for source in ("Sources/EchodeAppCore/PianoFrame.swift", "Sources/EchodeAppCore/PianoOffscreenProbe.swift",
                       "Tests/EchodeAppTests/PianoTimeGuideTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan", "spike-piano-frame"} <=
                            self.selected("swift/EchodeApp/" + source))

    def test_rhythmic_piano_grid_contract_covers_native_choices_and_labels(self):
        contract = next(item for item in self.policy["product_contracts"] if item["id"] == "piano-rhythmic-grid-ui")
        self.assertEqual((contract["runner"], contract["suite"], contract["test"]),
                         ("swift", "PianoRhythmicGridTests", "contractTripletAndDottedControlsDriveInputLinesAndHistory"))
        self.assertEqual(len(contract["negative_controls"]), 3)
        for source in ("Sources/EchodeAppCore/PianoGrid.swift", "Sources/EchodeAppCore/PianoOffscreenProbe.swift",
                       "Tests/EchodeAppTests/PianoRhythmicGridTests.swift", "Tests/EchodeAppTests/PianoRhythmicGridBoundaryTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected("swift/EchodeApp/" + source))

    def test_piano_grid_contracts_cover_zoom_native_controls_and_viewport_ownership(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for name, suite, test in (
                ("piano-auto-grid-ui", "PianoGridTests", "contractAutomaticGridFollowsZoomAndDrivesNoteInput"),
                ("piano-fixed-grid-ui", "PianoGridControlTests", "contractFixedGridControlSharesStateCancelsPressesAndKeepsInputMemory")):
            contract = contracts[name]
            self.assertEqual((contract["runner"], contract["suite"], contract["test"]), ("swift", suite, test))
            self.assertEqual(len(contract["negative_controls"]), 3)
        for source in ("Sources/EchodeAppCore/PianoGrid.swift", "Sources/EchodeAppCore/PianoGeometry.swift",
                       "Sources/EchodeAppCore/PianoFrame.swift", "Tests/EchodeAppTests/PianoGridControlTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected("swift/EchodeApp/" + source))

    def test_piano_note_drag_contract_covers_modifiers_release_and_subgrid_lengths(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        contract = contracts["piano-note-drag-ui"]
        self.assertEqual((contract["runner"], contract["suite"], contract["test"]),
                         ("swift", "PianoDragTests", "contractShiftNoteDragsReleaseTheGridAndCommitOnce"))
        self.assertEqual(len(contract["negative_controls"]), 3)
        for fault in contract["negative_controls"]:
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in ("PianoRollView.swift", "PianoGeometry.swift"):
            checks = self.selected("swift/EchodeApp/Sources/EchodeAppCore/" + source)
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"} <= checks)
        self.assertIn("check-product-contracts", self.selected("swift/EchodeApp/Tests/EchodeAppTests/PianoDragTests.swift"))

    def test_piano_note_insertion_contract_covers_release_cancellation_and_model_history(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        insert, cancel = contracts["piano-note-insert-ui"], contracts["piano-note-insert-cancel-ui"]
        self.assertEqual((insert["runner"], insert["suite"], insert["test"]),
                         ("swift", "PianoInsertionTests", "contractBlankDragsInsertOnReleaseAndKeepUndoAndSave"))
        self.assertEqual((cancel["runner"], cancel["suite"], cancel["test"]),
                         ("swift", "PianoInsertionTests", "blankPressCancellationNeverInsertsOrLeavesAPreview"))
        self.assertEqual(len(insert["negative_controls"]), 3)
        for fault in insert["negative_controls"] + [cancel["negative_control"]]:
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in ("swift/EchodeApp/Sources/EchodeAppCore/PianoInsertion.swift",
                       "swift/EchodeApp/Tests/EchodeAppTests/PianoInsertionTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"} <= self.selected(source))

    def test_note_input_length_contracts_cover_clicks_and_shared_ephemeral_history(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        click, memory = contracts["piano-note-click-ui"], contracts["piano-note-input-length-ui"]
        self.assertEqual((click["runner"], click["suite"], click["test"]),
                         ("swift", "NoteInputLengthTests", "contractSingleClickInsertsOnceAndKeepsThePressedPosition"))
        self.assertEqual((memory["runner"], memory["suite"], memory["test"]),
                         ("swift", "NoteInputLengthTests", "contractInputLengthFollowsSelectionResizeAndHistoryAcrossTracks"))
        self.assertEqual(len(memory["negative_controls"]), 5)
        for fault in [click["negative_control"]] + memory["negative_controls"]:
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in ("swift/EchodeApp/Sources/EchodeAppCore/NoteInputLength.swift",
                       "swift/EchodeApp/Tests/EchodeAppTests/NoteInputLengthTests.swift",
                       "swift/EchodeApp/Tests/EchodeAppTests/NoteInputHistoryTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"} <= self.selected(source))

    def test_note_velocity_contracts_cover_release_memory_cancellation_and_native_model(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        ui = contracts["piano-note-velocity-ui"]
        cancel = contracts["piano-note-velocity-cancel-ui"]
        model = contracts["note-velocity-model"]
        self.assertEqual((ui["runner"], ui["suite"], ui["test"]),
                         ("swift", "NoteVelocityTests", "contractOptionDragChangesVelocityWithoutMovingTheNote"))
        self.assertEqual(cancel["test"], "cancelledVelocityDragsNeverCommit")
        self.assertEqual((model["runner"], model["package"], model["target"]),
                         ("rust", "echode-history", ["--test", "note_velocity"]))
        self.assertEqual(len(ui["negative_controls"]), 5)
        for fault in ui["negative_controls"] + [cancel["negative_control"], model["negative_control"]]:
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in ("swift/EchodeApp/Sources/EchodeAppCore/NoteVelocity.swift",
                       "swift/EchodeApp/Tests/EchodeAppTests/NoteVelocityTests.swift",
                       "crates/echode-model/src/note_velocity.rs"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"} <= self.selected(source))

    def test_velocity_lane_contracts_cover_input_frames_and_cancellation(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for name, test in (("piano-velocity-lane-ui", "contractLaneDragSharesVelocityHistoryAndPreservesGeometry"),
                           ("piano-velocity-lane-frame-ui", "contractLaneShowsAllPitchesAndSynchronizesPreviewAndAccessibility"),
                           ("piano-velocity-lane-cancel-ui", "laneGesturesCancelAndBlankClicksNeverInsert")):
            contract = contracts[name]
            self.assertEqual((contract["runner"], contract["suite"], contract["test"]), ("swift", "NoteVelocityLaneTests", test))
            for fault in contract.get("negative_controls", [contract.get("negative_control")]):
                self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for name in ("PianoVelocityLane.swift", "PianoVelocityAccessibility.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan",
                             "spike-piano-frame"} <= self.selected("swift/EchodeApp/Sources/EchodeAppCore/" + name))
        self.assertIn("check-product-contracts", self.selected("swift/EchodeApp/Tests/EchodeAppTests/NoteVelocityLaneTests.swift"))

    def test_piano_selection_contracts_cover_selection_cancellation_and_atomic_deletion(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for name, suite, test in (
                ("piano-note-selection-ui", "PianoSelectionTests", "contractRightDragSelectsIntersectingNotes"),
                ("piano-note-selection-cancel-ui", "PianoSelectionTests", "contractSelectionPressesCancelWithoutChangingSelection"),
                ("piano-note-selection-delete-ui", "PianoSelectionDeletionTests", "contractSelectedNotesDeleteAtomicallyAndRoundTripHistory"),
                ("piano-note-linked-delete-ui", "PianoSelectionDeletionTests", "linkedPlacementsDeleteEachContentNoteOnlyOnce")):
            contract = contracts[name]
            self.assertEqual((contract["runner"], contract["suite"], contract["test"]), ("swift", suite, test))
            for fault in contract.get("negative_controls", [contract.get("negative_control")]):
                self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in ("Sources/EchodeAppCore/PianoSelection.swift", "Tests/EchodeAppTests/PianoSelectionTests.swift",
                       "Tests/EchodeAppTests/PianoSelectionDeletionTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected("swift/EchodeApp/" + source))

    def test_batch_velocity_contracts_cover_group_limits_targets_memory_and_cancellation(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for name, suite, test in (
                ("piano-velocity-batch-ui", "NoteVelocityBatchTests", "contractSelectedVelocityDragsPreserveDifferencesAndHistory"),
                ("piano-velocity-batch-targets-ui", "NoteVelocityBatchBoundaryTests", "contractLinkedVelocityBatchesValidateAllTargetsAndAccessibility"),
                ("piano-velocity-batch-cancel-ui", "NoteVelocityBatchBoundaryTests", "contractBatchVelocityGesturesCancelOnSelectionChanges")):
            contract = contracts[name]
            self.assertEqual((contract["runner"], contract["suite"], contract["test"]), ("swift", suite, test))
            for fault in contract.get("negative_controls", [contract.get("negative_control")]):
                self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in ("Sources/EchodeAppCore/NoteVelocityBatch.swift", "Tests/EchodeAppTests/NoteVelocityBatchTests.swift",
                       "Tests/EchodeAppTests/NoteVelocityBatchBoundaryTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected("swift/EchodeApp/" + source))

    def test_piano_workflow_contracts_cover_targets_references_keys_and_document_lifetimes(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for name, suite, test in (
                ("piano-target-ui", "PianoWorkflowTests", "contractTrackSelectionKeepsThePianoTargetAndNoteSelection"),
                ("piano-arrange-open-ui", "PianoWorkflowTests", "contractArrangeDoubleClickOpensAnIndependentPianoLocation"),
                ("piano-reference-ui", "PianoReferenceTests", "contractReferenceNotesRenderBehindTheInputAndNeverOwnEdits"),
                ("piano-track-selector-ui", "PianoReferenceTests", "contractTrackSelectorSeparatesReferenceEditAndMixerState"),
                ("piano-key-step-ui", "PianoStepTests", "contractArrowRepeatsEditTheWholeSelectionOnTheirTurn"),
                ("piano-key-step-bounds-ui", "PianoStepTests", "contractKeyboardBoundsAndLinkedNotesStayAtomic"),
                ("piano-return-ui", "PianoWorkflowTests", "contractReturnLeavesPianoInsertionToExplicitActions"),
                ("browser-return-ui", "PianoWorkflowTests", "contractBrowserListsOwnReturnBeforeTheWorkspaceShortcut")):
            contract = contracts[name]
            self.assertEqual((contract["runner"], contract["suite"], contract["test"]), ("swift", suite, test))
            for fault in contract.get("negative_controls", [contract.get("negative_control")]):
                self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in ("Sources/EchodeAppCore/PianoTracks.swift", "Sources/EchodeAppCore/PianoTrackSelector.swift",
                       "Sources/EchodeAppCore/PianoStep.swift", "Tests/EchodeAppTests/PianoWorkflowTests.swift",
                       "Tests/EchodeAppTests/PianoReferenceTests.swift", "Tests/EchodeAppTests/PianoStepTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected("swift/EchodeApp/" + source))

    def test_note_copies_cover_product_history_targets_and_cancellation(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for name, suite, test in (
                ("piano-note-duplicate-ui", "PianoDuplicateTests", "contractCommandDragCopiesSelectionAndRoundTripsHistory"),
                ("piano-note-duplicate-targets-ui", "PianoDuplicateBoundaryTests", "contractCopiesShareBoundsDeduplicateLinksAndRejectStaleTargets"),
                ("piano-note-duplicate-cancel-ui", "PianoDuplicateCancellationTests", "contractCopyClicksAndCancelledDragsNeverWrite")):
            contract = contracts[name]
            self.assertEqual((contract["runner"], contract["suite"], contract["test"]), ("swift", suite, test))
            for fault in contract.get("negative_controls", [contract.get("negative_control")]):
                self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        model = contracts["note-duplicate-model"]
        self.assertEqual((model["runner"], model["package"], model["target"]),
                         ("rust", "echode-history", ["--test", "note_duplicate"]))
        self.assertEqual(model["test"], "contract_note_copies_preserve_sources_attributes_and_history")
        for source in ("Tests/EchodeAppTests/PianoDuplicateTests.swift", "Tests/EchodeAppTests/PianoDuplicateBoundaryTests.swift",
                       "Tests/EchodeAppTests/PianoDuplicateCancellationTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected("swift/EchodeApp/" + source))
        self.assertTrue({"check", "check-product-contracts", "rtsan", "check-bridge-asan", "check-bridge-tsan"}
                        <= self.selected("crates/echode-model/src/model.rs"))

    def test_batch_note_moves_cover_relative_geometry_targets_and_cancellation(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for name, suite, test in (
                ("piano-note-move-batch-ui", "PianoMoveBatchTests", "contractSelectedNotesMoveTogetherAndRoundTripHistory"),
                ("piano-note-move-batch-targets-ui", "PianoMoveBatchBoundaryTests", "contractLinkedMovesValidateEveryTargetAndShareBounds"),
                ("piano-note-move-batch-cancel-ui", "PianoMoveBatchCancellationTests", "contractGroupMovesCancelWithoutCommittingOrLosingSelection")):
            contract = contracts[name]
            self.assertEqual((contract["runner"], contract["suite"], contract["test"]), ("swift", suite, test))
            for fault in contract["negative_controls"]:
                self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in ("Sources/EchodeAppCore/PianoMoveBatch.swift", "Tests/EchodeAppTests/PianoMoveBatchTests.swift",
                       "Tests/EchodeAppTests/PianoMoveBatchBoundaryTests.swift", "Tests/EchodeAppTests/PianoMoveBatchCancellationTests.swift",
                       "Tests/EchodeAppTests/PianoMoveBatchHarness.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected("swift/EchodeApp/" + source))

    def test_batch_note_resizes_cover_both_edges_targets_and_cancellation(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for name, suite, test in (
                ("piano-note-resize-batch-ui", "PianoResizeBatchTests", "contractSelectedEdgesResizeTogetherAndRoundTripHistory"),
                ("piano-note-resize-batch-targets-ui", "PianoResizeBatchBoundaryTests", "contractLinkedResizesValidateTargetsAndShareEdgeBounds"),
                ("piano-note-resize-batch-cancel-ui", "PianoResizeBatchCancellationTests", "contractGroupResizesCancelWithoutCommittingOrLosingSelection")):
            contract = contracts[name]
            self.assertEqual((contract["runner"], contract["suite"], contract["test"]), ("swift", suite, test))
            for fault in contract["negative_controls"]:
                self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in ("Sources/EchodeAppCore/PianoResizeBatch.swift", "Tests/EchodeAppTests/PianoResizeBatchTests.swift",
                       "Tests/EchodeAppTests/PianoResizeBatchBoundaryTests.swift", "Tests/EchodeAppTests/PianoResizeBatchCancellationTests.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan", "check-bridge-tsan"}
                            <= self.selected("swift/EchodeApp/" + source))

    def test_piano_note_bounds_contract_covers_model_and_product_frame(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        ui, model = contracts["piano-note-clip-bounds-ui"], contracts["piano-note-clip-bounds"]
        self.assertEqual((ui["runner"], ui["suite"], ui["test"]),
                         ("swift", "PianoDragTests", "contractNoteDragsGrowClipWindowsAndKeepTheWholeNoteVisible"))
        self.assertEqual((model["runner"], model["package"], model["target"]),
                         ("rust", "echode-model", ["--test", "note_edit"]))
        self.assertEqual(model["test"], "time_edits_grow_only_the_target_fixed_window_and_keep_content_origin")
        # Each runner must rebuild the language it mutates, not link an unchanged ABI.
        self.assertTrue(ui["negative_control"]["file"].endswith(".swift"))
        self.assertEqual(model["negative_control"]["file"], "crates/echode-model/src/placement.rs")
        for contract in (ui, model):
            fault = contract["negative_control"]
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in (model["negative_control"]["file"], "crates/echode-model/tests/note_edit.rs",
                       "swift/EchodeApp/Tests/EchodeAppTests/PianoDragTests.swift"):
            self.assertIn("check-product-contracts", self.selected(source))

    def test_content_bar_moves_and_legacy_journals_have_native_contracts(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        for name, count in (("clip-content-bars", 3), ("journal-clip-bar-basis", 2)):
            contract = contracts[name]
            self.assertEqual((contract["runner"], contract["package"], contract["target"]),
                             ("rust", "echode-history", ["--test", "clip_bar_basis"]))
            self.assertEqual(len(contract["negative_controls"]), count)
            for fault in contract["negative_controls"]:
                self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        for source in ("crates/echode-model/src/clip_move.rs", "crates/echode-model/src/placement.rs",
                       "crates/echode-model/src/invariants.rs", "crates/echode-actions/src/action.rs",
                       "crates/echode-history/tests/clip_bar_basis.rs"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-asan",
                             "check-bridge-tsan", "check-bridge-miri"} <= self.selected(source))

    def test_piano_accessibility_builds_only_on_query(self):
        contract = next(item for item in self.policy["product_contracts"] if item["id"] == "piano-accessibility-ui")
        self.assertEqual((contract["runner"], contract["suite"]), ("swift", "PianoTests"))
        self.assertEqual(contract["test"], "contractPianoAccessibilityBuildsOnlyOnQueryAndReusesElements")
        self.assertTrue(contract["negative_control"]["file"].endswith("PianoRollView.swift"))
        self.assertIn("buildAccessibilityChildren", contract["negative_control"]["new"])

    def test_ruler_loop_editing_keeps_native_input_and_rt_block_comparisons(self):
        contracts = {item["id"]: item for item in self.policy["product_contracts"]}
        ui = contracts["piano-ruler-loop-ui"]
        self.assertEqual((ui["runner"], ui["suite"]), ("swift", "RulerLoopTests"))
        self.assertIn("onSetLoop", ui["negative_control"]["old"])
        arrange = contracts["arrange-ruler-ui"]
        self.assertEqual((arrange["runner"], arrange["suite"]), ("swift", "ArrangeRulerTests"))
        self.assertEqual(len(arrange["negative_controls"]), 2)
        self.assertIn("arrange-ruler-cancel-ui", contracts)
        handles = contracts["loop-handles-ui"]
        self.assertEqual((handles["runner"], handles["suite"]), ("swift", "RulerHandleTests"))
        self.assertEqual(len(handles["negative_controls"]), 3)
        self.assertEqual(len(contracts["loop-handles-cancel-ui"]["negative_controls"]), 2)
        follow = contracts["playhead-follow-ui"]
        self.assertEqual((follow["runner"], follow["suite"]), ("swift", "PlayheadFollowTests"))
        self.assertEqual(len(follow["negative_controls"]), 3)
        for fault in follow["negative_controls"]:
            self.assertEqual((ROOT / fault["file"]).read_text().count(fault["old"]), 1)
        self.assertTrue(contracts["playhead-follow-state-ui"]["negative_control"]["file"].endswith("WorkspaceState.swift"))
        engine = contracts["loop-entry"]
        self.assertEqual((engine["package"], engine["target"]), ("echode-engine", ["--test", "block_size"]))
        self.assertTrue(engine["test"].endswith("_under_rtsan"))
        for path in ("swift/EchodeApp/Sources/EchodeAppCore/RulerLoop.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/RulerHandles.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/PlayheadFollow.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/PianoRollView.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/ArrangeRuler.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/ArrangeFrame.swift"):
            self.assertTrue({"check", "check-product-contracts", "check-bridge-tsan"} <= self.selected(path))
        self.assertTrue({"rtsan", "check-product-contracts"} <= self.selected("crates/echode-engine/src/playback.rs"))

    def test_chrome_layout_keeps_status_row_and_play_button_sizes(self):
        contract = next(item for item in self.policy["product_contracts"] if item["id"] == "transport-layout-ui")
        self.assertEqual((contract["runner"], contract["suite"]), ("swift", "WorkspaceChromeTests"))
        self.assertEqual(contract["test"], "contractStatusRowAndPlayButtonKeepTheirSizeWhilePreparing")
        self.assertTrue(contract["negative_control"]["file"].endswith("WorkspaceView.swift"))
        self.assertIn("opacity(preparing ? 1 : 0)", contract["negative_control"]["old"])
        self.assertIn("if preparing", contract["negative_control"]["new"])
        for path in ("swift/EchodeApp/Sources/EchodeAppCore/WorkspaceView.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/WorkspaceTransportView.swift"):
            self.assertIn("check-product-contracts", self.selected(path))

    def test_playhead_window_regression_is_an_explicit_spike(self):
        self.assertIn("spike-playhead", self.selected("swift/EchodeApp/Tests/EchodeAppTests/PlayheadTests.swift"))
        self.assertIn("spike-playhead", self.selected("swift/EchodeApp/Sources/EchodeAppCore/CanvasRenderer.swift"))
        self.assertNotIn("spike-playhead", self.selected("crates/echode-engine/src/voice.rs"))
        recipe = (ROOT / "Makefile").read_text().split("spike-playhead:\n", 1)[1].split("\n\n", 1)[0]
        self.assertIn("EchodePlayheadProbe", recipe)
        self.assertIn("DYLD_FRAMEWORK_PATH", recipe)
        self.assertNotIn("swift test", recipe)
        probe = (ROOT / "swift/EchodeApp/Sources/EchodePlayheadProbe/Main.swift").read_text()
        self.assertIn("app.run()", probe)
        self.assertIn("applicationDidFinishLaunching", probe)
        normal = (ROOT / "swift/EchodeApp/Tests/EchodeAppTests/PlayheadTests.swift").read_text()
        self.assertNotIn("ShownProject", normal)
        self.assertNotIn("beginMeasurement", recipe)

    def test_hosted_playback_keeps_realtime_deadlines_in_the_manual_probe(self):
        self.assertIn("spike-isolation", self.selected("crates/echode-plugin-host/tests/hosted_playback.rs"))
        recipe = (ROOT / "Makefile").read_text().split("spike-isolation:\n", 1)[1].split("\n\n", 1)[0]
        self.assertIn("--test hosted_playback -- --include-ignored --test-threads=1", recipe)
        self.assertIn("--test isolation -- --include-ignored --test-threads=1", recipe)

    def test_input_has_both_headless_tests_and_an_explicit_hardware_check(self):
        self.assertEqual(self.selected("spikes/input/Sources/EchodeInputCore/Gesture.swift"),
                         {"check-format", "check-verification", "check", "check-swift", "check-product-contracts", "check-real-hal", "check-input", "spike-input"})

    def test_isolation_and_dependency_changes_select_their_extra_checks(self):
        checks = self.selected("crates/echode-sandbox/src/lib.rs")
        self.assertTrue({"rtsan", "spike-engine-smoke", "spike-isolation"} <= checks)
        self.assertIn("advisories", self.selected("crates/echode-api/Cargo.toml"))

    def test_callback_work_per_track_is_measured_without_a_device(self):
        for path in ("crates/echode-engine/src/playback.rs", "crates/echode-engine/src/plugins.rs",
                     "crates/echode-plugin-host/tests/track_scale.rs"):
            with self.subTest(path=path):
                self.assertIn("spike-plugin-tracks", self.selected(path))
        self.assertNotIn("spike-plugin-tracks", self.selected("crates/echode-draw/src/query.rs"))
        check = {item["id"]: item for item in self.policy["checks"]}["spike-plugin-tracks"]
        self.assertEqual(check["mode"], "manual")
        self.assertNotIn("device_context", check)
        recipe = (ROOT / "Makefile").read_text().split("spike-plugin-tracks:\n", 1)[1].split("\n\n", 1)[0]
        self.assertIn("--test track_scale -- --ignored --exact measure_callback_cost_as_tracks_grow", recipe)

    def test_score_scale_is_an_explicit_measurement(self):
        # Run before and after an FCA-01 to FCA-05 fix. Regressions are pinned by
        # deterministic tests, so no change selects this release measurement.
        for path in ("crates/echode-realtime/src/lib.rs", "crates/echode-engine/src/playback.rs",
                     "crates/echode-model/src/model.rs", "crates/echode-engine/tests/scale.rs"):
            with self.subTest(path=path):
                self.assertNotIn("spike-scale", self.selected(path))
        check = {item["id"]: item for item in self.policy["checks"]}["spike-scale"]
        self.assertEqual(check["mode"], "manual")
        self.assertEqual(check["areas"], [])
        self.assertIn("crates/echode-engine/tests/scale.rs", check["inputs"])
        self.assertNotIn("device_context", check)
        recipe = (ROOT / "Makefile").read_text().split("spike-scale:\n", 1)[1].split("\n\n", 1)[0]
        self.assertIn("--test scale -- --ignored --exact measure_playback_scale", recipe)

    def test_common_settings_do_not_require_unrelated_measurements(self):
        automatic = {check["id"] for check in self.policy["checks"] if check["mode"] == "automatic"}
        for path in self.policy["global_inputs"]:
            with self.subTest(path=path):
                expected = automatic | ({"advisories"} if path in ("Cargo.toml", "Cargo.lock", "deny.toml") else set())
                self.assertEqual(self.selected(path), expected)
        self.assertEqual(set(verify.select_checks(self.policy, [], all_checks=True)),
                         {check["id"] for check in self.policy["checks"]})

    def test_infrastructure_runs_all_portable_checks_without_opening_windows(self):
        expected = {check["id"] for check in self.policy["checks"] if check["mode"] == "automatic"}
        for path in ("scripts/verify.py", "scripts/build-platform.env", ".cargo/config.toml"):
            self.assertEqual(self.selected(path), expected)
        # Without the diff base the policy and Makefile cannot be compared, so every automatic check runs.
        for path in ("Makefile", "scripts/verification.json"):
            self.assertEqual(self.selected(path), expected)
        self.assertEqual(self.selected("docs/PLAN.md"), {"check-format", "check-verification", "check", "check-swift", "check-product-contracts", "check-real-hal"})

    def test_a_helper_script_selects_only_the_checks_that_run_it(self):
        always = {"check-format", "check-verification", "check", "check-swift", "check-product-contracts", "check-real-hal"}
        self.assertEqual(self.selected("scripts/check-bridge-address.sh"),
                         always | {"check-bridge-asan", "check-bridge-tsan"})
        self.assertEqual(self.selected("scripts/check-bridge-miri.sh"), always | {"check-bridge-miri"})
        self.assertEqual(self.selected("scripts/diagnose.py"), always)
        self.assertIn("rtsan", self.selected("Packaging/EchodeSinePad-Info.plist"))
        self.assertNotIn("check-bridge-miri", self.selected("scripts/check-bridge-address.sh"))

    def test_policy_and_makefile_edits_select_only_the_redefined_checks(self):
        makefile = (ROOT / "Makefile").read_text()
        before = verify.definition_identities(self.policy, makefile)
        def redefined(policy=None, text=None):
            after = verify.definition_identities(policy or self.policy, makefile if text is None else text)
            return {name for name in after if after[name] != before.get(name)}
        self.assertEqual(redefined(text="# Only a comment.\n" + makefile.replace("\n\n", "\n\n# note\n\n")), set())
        rtsan = makefile.replace("RTSAN_ENABLE=1 cargo test -q -p echode-engine", "RTSAN_ENABLE=1 cargo test -p echode-engine")
        self.assertEqual(redefined(text=rtsan), {"rtsan"})
        # A shared sub-make target belongs to every check that calls it.
        plugins = makefile.replace("bundled-plugins:\n\tbash scripts/build-bundled-plugins.sh",
                                   "bundled-plugins:\n\tbash scripts/build-bundled-plugins.sh debug")
        self.assertEqual(redefined(text=plugins), {"check", "check-swift", "check-product-contracts", "check-contract-controls", "rtsan"})
        policy = json.loads(json.dumps(self.policy))
        policy["product_contracts"].append(dict(policy["product_contracts"][0], id="another-contract"))
        self.assertEqual(redefined(policy), {"check-product-contracts", "check-contract-controls"})
        policy = json.loads(json.dumps(self.policy))
        next(check for check in policy["checks"] if check["id"] == "rtsan")["timeout"] += 1
        self.assertEqual(redefined(policy), {"rtsan"})
        policy = json.loads(json.dumps(self.policy))
        policy["areas"]["audio"].append("docs/**")
        self.assertEqual(redefined(policy), set())
        policy["global_inputs"].append("extra.toml")
        self.assertEqual(redefined(policy), {check["id"] for check in self.policy["checks"]})
        always = {"check-format", "check-verification", "check", "check-swift", "check-product-contracts", "check-real-hal"}
        self.assertEqual(set(verify.select_checks(self.policy, ["Makefile"], redefined=set())), always)
        self.assertEqual(set(verify.select_checks(self.policy, ["Makefile"], redefined={"rtsan", "spike-audio"})),
                         always | {"rtsan"})

    def test_every_contract_depends_on_a_subset_of_the_gate_inputs(self):
        contracts = load_contracts()
        check = next(c for c in self.policy["checks"] if c["id"] == "check-product-contracts")
        folders = verify.package_directories(ROOT, check["packages"])
        patterns = self.policy["global_inputs"] + self.policy["evidence_inputs"] + check["inputs"]
        self.assertIn("product_contract_inputs", check["policy_inputs"])
        for contract in self.policy["product_contracts"]:
            with self.subTest(contract=contract["id"]):
                own_patterns, own_folders = contracts.contract_inputs(ROOT, self.policy, contract)
                self.assertTrue(set(own_patterns) <= set(patterns))
                self.assertTrue(own_folders <= folders)

    def test_long_engine_measurement_is_explicit_and_the_short_probe_is_separate(self):
        checks = self.selected("crates/echode-engine/src/playback.rs")
        self.assertIn("spike-engine-smoke", checks)
        self.assertNotIn("spike-engine", checks)
        long = next(c for c in self.policy["checks"] if c["id"] == "spike-engine")
        self.assertTrue(long["extended"])
        text = (ROOT / "Makefile").read_text()
        short = text.split("spike-engine-smoke:\n", 1)[1].split("\n\n", 1)[0]
        self.assertIn("--seconds 10 ", short)
        self.assertIn("--native-rate", short)
        self.assertIn("--report target/spike-s2-smoke.txt", short)
        self.assertNotIn("--seconds 600", short)

    def test_required_mutations_belong_to_the_contract_inputs(self):
        check = next(c for c in self.policy["checks"] if c["id"] == "check-product-contracts")
        folders = verify.package_directories(ROOT, check["packages"])
        for contract in self.policy["product_contracts"]:
            faults = contract.get("negative_controls") or [contract["negative_control"]]
            for fault in faults:
                path = fault["file"]
                with self.subTest(contract=contract["id"]):
                    self.assertTrue(verify.matches([path], check["inputs"])
                                    or any(path.startswith(folder + "/") for folder in folders))

    def test_shell_entry_points_and_their_helpers_are_declared_inputs(self):
        parsed = verify.makefile_rules((ROOT / "Makefile").read_text())
        for check in self.policy["checks"]:
            identity = verify.makefile_identity(parsed, check.get("make_target", check["id"]))
            self.assertEqual(identity["missing"], [])
            # Every sub-make target the check calls counts as part of its recipe.
            pending = ["\n".join(line for block in identity["rules"].values() for line in block)]
            visited = set()
            patterns = self.policy["global_inputs"] + self.policy["evidence_inputs"] + check.get("inputs", [])
            while pending:
                text = "\n".join(line for line in pending.pop().splitlines() if not line.lstrip().startswith("#"))
                for helper in re.findall(r"scripts/[A-Za-z0-9_./-]+\.(?:py|sh|env|txt)\b", text):
                    if helper in visited or not (ROOT / helper).is_file():
                        continue
                    visited.add(helper)
                    with self.subTest(check=check["id"], helper=helper):
                        self.assertTrue(verify.matches([helper], patterns), "undeclared command dependency")
                    pending.append((ROOT / helper).read_text())

    def test_required_behavioral_contracts_are_automatic_for_every_change(self):
        check = next(c for c in self.policy["checks"] if c["id"] == "check-product-contracts")
        self.assertEqual(check["mode"], "automatic")
        self.assertTrue(check["always"])
        for path in ("crates/echode-engine/src/plugins.rs", "docs/PLAN.md",
                     "swift/EchodeApp/Sources/EchodeAppCore/ArrangeHeaderView.swift"):
            self.assertIn("check-product-contracts", self.selected(path))
        contracts = self.policy["product_contracts"]
        self.assertEqual({c["id"] for c in contracts},
                         {"export-presets-ui", "export-defaults-ui", "export-input-ui", "export-reveal-ui", "export-destination-ui", "export-destination-cli", "export-normalization-cli", "export-normalization-ui", "export-settings-ui", "export-snapshot-ui", "export-progress-ui", "export-diagnostics-ui", "export-cancellation-ui", "export-cancellation-publish", "explicit-sine-pad", "no-implicit-instrument", "startup-position", "macos-plugin-bundle", "header-key-routing",
                          "document-session-drag-ui", "document-session-audio-ui", "document-session-restore-ui", "document-session-stale-ui",
                          "document-worker-completion", "document-lane-completion",
                          "typed-clip-controller", "typed-mixer-controller", "typed-track-device-controller", "typed-piano-controller", "typed-edit-bridge", "model-constants-abi", "model-units-ui", "integer-loop-draft-ui",
                          "plugin-main-call-ownership", "plugin-main-timeout-state", "plugin-control-request-ids", "plugin-control-save-order",
                          "mcp-local-arguments", "mcp-modern-metadata", "mcp-legacy-lifecycle", "mcp-request-identity", "mcp-version-envelope", "mcp-backend-envelope", "mcp-backend-version", "mcp-backend-errors", "mcp-stdio-framing", "mcp-backend-framing", "rpc-json-whitespace", "rpc-request-framing", "rpc-request-envelope", "rpc-notification-boundary",
                          "store-gitignore", "audio-import-validation", "canonical-pcm-validation", "peak-cache-validation", "stereo-waveform-average", "waveform-boundary-silence", "stereo-media", "stereo-plugin-chain", "audio-loop-seam", "audio-fader-ramp",
                          "engine-measurement-completion", "pane-boundary", "canvas-resize-ui", "drawable-mismatch-ui", "header-width-ui", "transport-layout-ui", "arrange-camera-ui", "arrange-camera-input-ui", "piano-accessibility-ui", "workspace-layout-ui", "workspace-stack-ui", "pending-transport",
                          "transport-cancellation", "queued-stop", "plugin-transport-reset", "plugin-midi-panic",
                          "plugin-state-poll", "transport-declick", "stop-cleanup", "plugin-onset", "plugin-deadline-fade", "track-mix-ui", "clip-mix-ui", "live-mix-meter",
                          "loop-head-residency", "pre-fader-source", "phase1-acceptance",
                          "live-workgroup", "shared-placement-pcm", "cached-geometry", "mcp-edit", "fx-chain-ui",
                          "fx-bypass-ui", "fx-batch-ui", "fx-offline-ui", "plugin-offline", "offline-export",
                          "plugin-bypass", "startup-recovery", "sample-library-search", "sample-import-target",
                          "sample-audition", "sample-audition-ui", "sample-audition-resident", "sample-audition-position", "sample-audition-gain", "sample-audition-controls-ui", "sample-audition-stream", "sample-audition-loading", "sample-preview-ui", "sample-browser-click-ui", "sample-browser-content-ui", "sample-drop-ui", "sample-drop-entry-ui", "audio-import-transaction",
                          "track-insertion-ui", "track-insertion-order", "folder-disclosure-ui", "folder-projection-ownership",
                          "track-duplicate-ui", "track-duplicate-model", "clip-move-ui", "clip-move-model", "clip-duplicate-key-ui", "clip-duplicate-key-guards-ui", "mixer-switch-batch-ui", "mixer-switch-order-ui", "mixer-value-batch-ui", "mixer-value-bounds-ui", "mixer-step-ui", "mixer-step-bounds-ui", "mixer-drag-identity-ui", "mixer-drag-batch-ui", "mixer-drag-bounds-ui", "mixer-group-audio", "mixer-gesture-document-ui", "mixer-reset-document-ui", "scan-deadline", "clip-content-bars", "journal-clip-bar-basis", "plugin-params", "fx-params-ui",
                          "plugin-text-parse", "fx-params-clicks-ui", "mix-clicks-ui", "clip-clicks-ui", "mixer-reset-target-ui", "clip-reset-target-ui", "clip-drag-identity-ui", "package-float-roundtrip", "clip-gesture-document-ui", "clip-gesture-callback-ui",
                          "device-lane-held", "device-view-slow-ui", "device-view-long-ui", "device-disclosure-ui",
                          "plugin-window-helper", "plugin-window-relay", "plugin-window-undo-ui",
                          "fx-header-ui", "fx-inspector-ui", "plugin-window-open-ui", "plugin-window-visibility", "plugin-window-loop", "plugin-window-status-ui", "plugin-window-state",
                          "plugin-key-events", "plugin-key-socket", "plugin-key-ui", "fx-add-trailing-ui", "device-edit-enabled-ui", "stopped-plugin-tails", "stopped-plugin-output",
                          "stop-return-tails", "playback-start", "playback-start-ui", "piano-ruler-start-ui", "piano-ruler-loop-ui", "arrange-ruler-ui", "arrange-ruler-cancel-ui", "loop-handles-ui", "loop-handles-cancel-ui", "loop-entry",
                          "piano-target-ui", "piano-arrange-open-ui", "piano-reference-ui", "piano-track-selector-ui",
                          "piano-key-step-ui", "piano-key-step-bounds-ui", "piano-return-ui", "browser-return-ui",
                          "piano-velocity-batch-ui", "piano-velocity-batch-targets-ui", "piano-velocity-batch-cancel-ui",
                          "note-duplicate-model", "piano-note-duplicate-ui", "piano-note-duplicate-targets-ui", "piano-note-duplicate-cancel-ui",
                          "piano-auto-grid-ui", "piano-fixed-grid-ui", "piano-rhythmic-grid-ui", "piano-time-guides-ui",
                          "piano-note-move-batch-ui", "piano-note-move-batch-targets-ui", "piano-note-move-batch-cancel-ui",
                          "piano-note-resize-batch-ui", "piano-note-resize-batch-targets-ui", "piano-note-resize-batch-cancel-ui",
                          "piano-note-selection-ui", "piano-note-selection-cancel-ui", "piano-note-selection-delete-ui", "piano-note-linked-delete-ui", "piano-velocity-lane-ui", "piano-velocity-lane-frame-ui", "piano-velocity-lane-cancel-ui", "piano-shared-viewport-ui", "piano-note-velocity-ui", "piano-note-velocity-cancel-ui", "note-velocity-model", "piano-note-click-ui", "piano-note-input-length-ui", "piano-note-drag-ui", "piano-note-insert-ui", "piano-note-insert-cancel-ui", "piano-note-clip-bounds-ui", "piano-note-clip-bounds", "arrange-note-pitch", "arrange-note-pitch-ui", "playhead-follow-ui", "playhead-follow-state-ui",
                          "note-end-start", "track-event-routing", "helper-descriptor-limit", "transport-order-ui", "sample-list-space-ui", "plugin-list-ui",
                          "sample-overview", "sample-overview-ui", "journal-place-fades",
                          "recovery-consent", "recovery-consent-ui", "journal-invariants", "journal-confirmed-edits", "noop-edits", "mcp-noop-edits", "noop-edits-ui",
                          "device-list-retry-ui", "output-opening", "output-opening-worker", "background-graph", "device-preparation-ui",
                          "output-prewarm", "output-prewarm-play", "output-prepare-worker", "output-prepare-on-open"})
        for contract in contracts:
            faults = contract.get("negative_controls") or [contract["negative_control"]]
            self.assertTrue(all((ROOT / fault["file"]).is_file() for fault in faults))
        makefile = (ROOT / "Makefile").read_text()
        recipe = makefile.split("\ncheck-product-contracts:\n", 1)[1].split("\n\n", 1)[0]
        self.assertIn("check_product_contracts.py", recipe)
        self.assertIn("contract-prerequisites", recipe)
        prerequisites = makefile.split("\ncontract-prerequisites:\n", 1)[1].split("\n\n", 1)[0]
        self.assertIn("-p echode-ffi -p echode-plugin-host -p echode-test-plugin", prerequisites)

    def test_journal_invariants_use_the_product_recovery_boundary(self):
        contracts = {c["id"]: c for c in self.policy["product_contracts"]}
        for name in ("journal-invariants", "journal-confirmed-edits"):
            with self.subTest(contract=name):
                contract = contracts[name]
                self.assertEqual((contract["runner"], contract["package"], contract["target"]),
                                 ("rust", "echode-history", ["--test", "audit_recovery"]))
                self.assertTrue(contract["test"].startswith("audit_adv01_"))
                fault = contract["negative_control"]
                self.assertEqual(fault["file"], "crates/echode-history/src/document.rs")
                self.assertIn("project.check_invariants()", fault["old"])
        for path in ("crates/echode-history/src/document.rs",
                     "crates/echode-history/tests/audit_recovery.rs"):
            self.assertIn("check-product-contracts", self.selected(path))

    def test_parameter_contracts_cover_the_real_helper_and_the_panel(self):
        contracts = {c["id"]: c for c in self.policy["product_contracts"]}
        helper = contracts["plugin-params"]
        self.assertEqual((helper["runner"], helper["package"], helper["target"]),
                         ("rust", "echode-plugin-host", ["--test", "hosted_playback"]))
        self.assertTrue(helper["test"].startswith("device_params::contract_"))
        self.assertEqual(helper["negative_control"]["file"], "crates/echode-plugin-host/src/threads.rs")
        self.assertIn("params.flush", helper["negative_control"]["old"])
        panel = contracts["fx-params-ui"]
        self.assertEqual((panel["runner"], panel["suite"]), ("swift", "DeviceParameterTests"))
        self.assertEqual(panel["negative_control"]["file"], "swift/EchodeApp/Sources/EchodeAppCore/DeviceParameters.swift")
        # A held helper reply must not hold the control side or the screen.
        lane = contracts["device-lane-held"]
        self.assertEqual((lane["runner"], lane["package"], lane["target"]),
                         ("rust", "echode-plugin-host", ["--test", "hosted_playback"]))
        self.assertEqual(lane["negative_control"]["file"], "crates/echode-sandbox/src/session.rs")
        self.assertIn("try_lock", lane["negative_control"]["old"])
        for name in ("device-view-slow-ui", "device-view-long-ui"):
            with self.subTest(contract=name):
                self.assertEqual((contracts[name]["runner"], contracts[name]["suite"]), ("swift", "DeviceParameterTests"))
        for path in ("crates/echode-document/src/lane.rs", "crates/echode-sandbox/src/session.rs",
                     "swift/EchodeApp/Sources/EchodeAppCore/WorkspaceParameterView.swift"):
            with self.subTest(path=path):
                self.assertIn("check-product-contracts", self.selected(path))
        disclosure = contracts["device-disclosure-ui"]
        self.assertEqual((disclosure["runner"], disclosure["suite"]), ("swift", "DeviceDisclosureTests"))
        self.assertIn("column.preferredWidth", disclosure["negative_control"]["old"])
        self.assertIn("check-product-contracts", self.selected("swift/EchodeApp/Sources/EchodeAppCore/DeviceDisclosure.swift"))
        # These files open the output engine for device views.
        for path in ("swift/EchodeBridge/Sources/EchodeBridge/Device.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/DeviceParameters.swift"):
            with self.subTest(path=path):
                self.assertIn("rtsan", self.selected(path))
                self.assertIn("check-product-contracts", self.selected(path))

    def test_plugin_window_contracts_cover_the_real_helper_and_the_screen(self):
        contracts = {c["id"]: c for c in self.policy["product_contracts"]}
        for name, source in (("plugin-window-helper", "crates/echode-plugin-host/src/gui.rs"),
                             ("plugin-window-relay", "crates/echode-plugin-host/src/relay.rs")):
            with self.subTest(contract=name):
                helper = contracts[name]
                self.assertEqual((helper["runner"], helper["package"], helper["target"]),
                                 ("rust", "echode-plugin-host", ["--test", "hosted_playback"]))
                self.assertTrue(helper["test"].startswith("plugin_gui::contract_"))
                self.assertEqual(helper["negative_control"]["file"], source)
        for name in ("plugin-window-undo-ui", "plugin-window-open-ui", "plugin-key-ui", "fx-add-trailing-ui"):
            with self.subTest(contract=name):
                self.assertEqual((contracts[name]["runner"], contracts[name]["suite"]), ("swift", "PluginWindowTests"))
        header = contracts["fx-header-ui"]
        self.assertEqual((header["runner"], header["suite"]), ("swift", "TrackFXHeaderTests"))
        self.assertIn("onOpenPlugin", header["negative_control"]["old"])
        status = contracts["plugin-window-status-ui"]
        self.assertEqual((status["runner"], status["suite"]), ("swift", "PluginWindowStatusTests"))
        self.assertIn("entry.windowState", status["negative_control"]["old"])
        lifetime = contracts["plugin-window-state"]
        self.assertEqual((lifetime["runner"], lifetime["package"], lifetime["target"]),
                         ("rust", "echode-sandbox", ["--lib"]))
        self.assertIn("set_window_state(true)", lifetime["negative_control"]["old"])
        # A key window would take Space and shortcuts from Echode.
        self.assertIn("key: false", contracts["plugin-window-helper"]["negative_control"]["old"])
        for path in ("crates/echode-plugin-host/src/gui.rs", "crates/echode-plugin-host/src/relay.rs",
                     "crates/echode-document/src/activity.rs", "crates/echode-sandbox/src/gui.rs",
                     "crates/echode-plugin-host/src/gui_appkit.rs", "crates/echode-sandbox/src/shortcuts.rs",
                     "swift/EchodeApp/Sources/EchodeAppCore/PluginWindows.swift"):
            with self.subTest(path=path):
                self.assertIn("check-product-contracts", self.selected(path))
        # The processing thread writes the plugin's output events.
        self.assertIn("rtsan", self.selected("crates/echode-plugin-host/src/relay.rs"))

    def test_mcp_edits_use_the_real_cli_and_the_existing_contract_runner(self):
        contract = next(c for c in self.policy["product_contracts"] if c["id"] == "mcp-edit")
        self.assertEqual(contract["runner"], "rust")
        self.assertEqual(contract["package"], "echode-cli")
        self.assertEqual(contract["target"], ["--test", "mcp"])
        self.assertEqual(contract["negative_control"]["file"], "crates/echode-api/src/mcp.rs")
        self.assertIn("actions.transaction", contract["negative_control"]["old"])
        self.assertIn("project.summary", contract["negative_control"]["new"])
        self.assertIn("check-product-contracts", self.selected("crates/echode-api/src/mcp.rs"))
        self.assertNotIn("rtsan", self.selected("crates/echode-api/src/mcp.rs"))

    def test_policy_references_real_targets_and_never_rewrites_sound_references(self):
        targets = set(re.findall(r"^([a-z][a-z0-9-]*):", (ROOT / "Makefile").read_text(), re.MULTILINE))
        ids = {check["id"] for check in self.policy["checks"]}
        self.assertTrue(ids <= targets)
        self.assertTrue({check.get("make_target", check["id"]) for check in self.policy["checks"]} <= targets)
        self.assertNotIn("bless", ids)
        self.assertNotIn("known-bugs", ids)

    def test_public_ci_runs_only_pinned_static_checks(self):
        # ADR 0269: hosted runners are M1 / M2, and fork pull requests must not get a writable token.
        workflow = (ROOT / ".github/workflows/check.yml").read_text()
        self.assertRegex(workflow, r"(?m)^permissions:\n  contents: read$")
        self.assertNotIn("pull_request_target", workflow)
        uses = re.findall(r"uses: (\S+)", workflow)
        self.assertTrue(uses)
        for action in uses:
            self.assertRegex(action, r"^[\w.-]+/[\w.-]+@[0-9a-f]{40}$")
        self.assertIn("run: make check-portable", workflow)
        self.assertNotIn("verify.py", workflow)
        recipe = subprocess.run(["make", "-n", "check-portable"], cwd=ROOT, capture_output=True, text=True,
                                check=True).stdout
        for native in ("cargo test", "cargo build", "check_build_host", "verify.py"):
            self.assertNotIn(native, recipe)
        clippy = [line for line in recipe.splitlines() if line.startswith("cargo clippy")]
        self.assertEqual(len(clippy), 3)
        for line in clippy:
            self.assertIn("target-cpu=apple-m1", line)
            self.assertIn("--target-dir target/portable", line)

    def test_public_ci_installs_the_pinned_toolchain_before_cargo(self):
        # ADR 0269: rustup does not install the rust-toolchain.toml pin on demand, so on a fresh runner
        # every cargo call (cargo-deny's included) fails until the job installs it.
        workflow = (ROOT / ".github/workflows/check.yml").read_text()
        jobs = re.findall(r"(?m)^  ([\w-]+):\n((?:(?:    .*)?\n)*)", workflow.split("\njobs:\n", 1)[1])
        self.assertTrue(jobs)
        for name, body in jobs:
            with self.subTest(job=name):
                runs = re.findall(r"run: (.+)", body)
                cargo = [index for index, run in enumerate(runs) if re.match(r"(cargo|make) ", run)]
                self.assertTrue(cargo, "a job without cargo needs no Rust toolchain")
                self.assertIn("rustup toolchain install", runs[:cargo[0]])

    def test_published_files_name_no_home_directory(self):
        # ADR 0269: the public repository is anonymous; a home path names the machine's user.
        result = subprocess.run(["git", "grep", "--untracked", "-I", "-n", "-o", "-E", r"/Users/[A-Za-z0-9._-]+"],
                                cwd=ROOT, capture_output=True, text=True)
        self.assertIn(result.returncode, (0, 1), result.stderr)
        leaks = [line for line in result.stdout.splitlines() if not line.endswith(("/Users/someone", "/Users/Shared"))]
        self.assertEqual(leaks, [], "write <repo> or a relative path instead of a home directory")

    def test_aggregate_check_contains_the_contract_gate_once_and_base_does_not(self):
        for target, count in (("check", 1), ("check-base", 0)):
            with self.subTest(target=target):
                result = subprocess.run(["make", "-n", target], cwd=ROOT, capture_output=True, text=True, check=True)
                self.assertEqual(result.stdout.count("python3 scripts/check_product_contracts.py"), count)
                self.assertEqual(result.stdout.count("python3 -m unittest discover"), count)
        check = next(check for check in self.policy["checks"] if check["id"] == "check")
        self.assertEqual(check["make_target"], "check-base")

    def test_a_jobserver_handle_does_not_change_the_toolchain_identity(self):
        self.assertEqual(verify.make_flags_identity("-j4 --jobserver-auth=fifo:/tmp/GMfifo1 -- VERIFY_ARGS=--plan"),
                         ["-j4"])
        self.assertEqual(verify.make_flags_identity("-j4 --jobserver-fds=3,4"), ["-j4"])

    def test_selection_arguments_do_not_invalidate_evidence_but_build_flags_do(self):
        result = subprocess.CompletedProcess([], 0, stdout=b"fixture-v1", stderr=b"")
        with patch.object(verify.subprocess, "run", return_value=result) as query:
            with patch.dict(os.environ, {"MAKEFLAGS": ""}):
                plain = verify.tool_identity(ROOT)
            with patch.dict(os.environ, {"MAKEFLAGS": '-- VERIFY_ARGS=--defer\\ measure\\ --reason\\ "A\'s\\ display"'}):
                selected = verify.tool_identity(ROOT)
            with patch.dict(os.environ, {"MAKEFLAGS": "-j4 -- VERIFY_ARGS=--run\\ measure"}):
                parallel = verify.tool_identity(ROOT)
        self.assertEqual(plain, selected)
        self.assertNotEqual(plain, parallel)
        self.assertEqual(parallel["MAKEFLAGS"], ["-j4"])
        self.assertTrue(all(call.kwargs["env"]["RUSTUP_AUTO_INSTALL"] == "0" for call in query.call_args_list))

    def test_an_unrelated_path_prefix_does_not_change_the_toolchain_identity(self):
        result = subprocess.CompletedProcess([], 0, stdout=b"fixture-v1", stderr=b"")
        extra = "/Applications/Cursor.app/Contents/Resources/app/node_modules/@vscode/ripgrep/bin"
        with patch.object(verify.subprocess, "run", return_value=result):
            with patch.dict(os.environ, {"PATH": os.environ["PATH"]}):
                plain = verify.tool_identity(ROOT)
            with patch.dict(os.environ, {"PATH": extra + os.pathsep + os.environ["PATH"]}):
                hooked = verify.tool_identity(ROOT)
        self.assertEqual(plain, hooked)
        self.assertTrue(plain["toolchain"]["rustc"])

    def test_an_earlier_compiler_changes_the_toolchain_identity(self):
        result = subprocess.CompletedProcess([], 0, stdout=b"fixture-v1", stderr=b"")
        with tempfile.TemporaryDirectory() as temporary:
            binary = Path(temporary) / "rustc"
            binary.write_text("#!/bin/sh\n")
            binary.chmod(0o755)
            with patch.object(verify.subprocess, "run", return_value=result):
                with patch.dict(os.environ, {"PATH": os.environ["PATH"]}):
                    plain = verify.tool_identity(ROOT)
                with patch.dict(os.environ, {"PATH": temporary + os.pathsep + os.environ["PATH"]}):
                    changed = verify.tool_identity(ROOT)
        self.assertNotEqual(plain["toolchain"]["rustc"], changed["toolchain"]["rustc"])
        self.assertNotEqual(plain, changed)

    def test_audio_identity_tracks_the_default_output_without_unrelated_devices(self):
        output = {"_name": "output", "coreaudio_device_srate": 96_000,
                  "coreaudio_default_audio_output_device": "spaudio_yes"}
        other = {"_name": "input", "coreaudio_device_srate": 48_000}
        data = {"SPAudioDataType": [{"_items": [output, other]}]}
        def profile(*args, **kwargs):
            return subprocess.CompletedProcess(args, 0, stdout=json.dumps(data), stderr="")
        with patch.object(verify.subprocess, "run", side_effect=profile):
            initial = verify.audio_output_identity()
            self.assertIsNotNone(initial)
            other["coreaudio_device_srate"] = 44_100
            self.assertEqual(verify.audio_output_identity(), initial)
            output["coreaudio_device_srate"] = 48_000
            self.assertNotEqual(verify.audio_output_identity(), initial)
            data["SPAudioDataType"] = []
            self.assertIsNone(verify.audio_output_identity())


class EvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="echode-verification-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "scripts").mkdir()
        (self.root / "src").mkdir()
        (self.root / ".gitignore").write_text("/target\n")
        (self.root / "src/example.rs").write_text("original\n")
        (self.root / "Makefile").write_text("check:\n\t@echo check\n\nmeasure:\n\t@echo measure\n")
        policy = {"version": 1, "global_inputs": [], "infrastructure": [],
                  "areas": {"source": ["src/**"]}, "checks": [
                      {"id": "check", "mode": "automatic", "always": True, "areas": [], "inputs": ["**"], "timeout": 10, "purpose": "fixture"},
                      {"id": "measure", "mode": "manual", "areas": ["source"], "inputs": ["src/**"], "timeout": 10, "purpose": "fixture"}]}
        (self.root / verify.POLICY).write_text(json.dumps(policy))
        self.git("init", "--quiet")
        self.git("add", ".")
        # Commits belong only to this disposable fixture repository.
        self.git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "--quiet", "-m", "fixture")
        (self.root / "src/example.rs").write_text("first edit\n")

    def git(self, *args):
        return verify.git(self.root, *args)

    def report(self):
        return json.loads((self.root / "target/verification/report.json").read_text())

    def run_verify(self, options, runner=None, tools=None):
        runner = runner or (lambda *args: {"status": "passed"})
        with patch.object(verify, "tool_identity", return_value=tools or {"rust": "fixture-v1"}), \
             patch.object(verify, "run_check", side_effect=runner) as invocation, \
             redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
            code = verify.verify(self.root, options)
        return code, invocation.call_count

    def test_ignored_runtime_bytes_expire_only_the_dependent_check(self):
        policy = verify.load_policy(self.root)
        policy["checks"][1]["tool_inputs"] = ["target/runtime.dylib"]
        (self.root / verify.POLICY).write_text(json.dumps(policy))
        self.assertEqual(self.run_verify(arguments(run=["measure"])), (0, 2))
        runtime = self.root / "target/runtime.dylib"
        for data in (b"runtime-v1", b"runtime-v2"):
            runtime.write_bytes(data)
            self.assertEqual(self.run_verify(arguments(run=["measure"])), (0, 1))
            self.assertEqual(self.run_verify(arguments(run=["measure"])), (0, 0))
        runtime.unlink()
        self.assertEqual(self.run_verify(arguments(run=["measure"])), (0, 1))

    def test_tool_inputs_cannot_escape_the_repository_or_use_patterns(self):
        policy = verify.load_policy(self.root)
        for name in ("/tmp/outside", "../outside", "target/*"):
            policy["checks"][0]["tool_inputs"] = [name]
            (self.root / verify.POLICY).write_text(json.dumps(policy))
            with self.assertRaisesRegex(ValueError, "exact repository-relative"):
                verify.load_policy(self.root)

    def test_changed_paths_include_staged_untracked_deleted_and_renamed_files_with_spaces(self):
        self.git("mv", "src/example.rs", "src/renamed file.rs")
        (self.root / "src/new file.rs").write_text("untracked")
        (self.root / "Makefile").unlink()
        self.assertEqual(verify.changed_paths(self.root),
                         ["Makefile", "src/example.rs", "src/new file.rs", "src/renamed file.rs"])

    def test_document_edits_reuse_the_measurement_without_forging_a_new_run(self):
        self.assertEqual(self.run_verify(arguments(run=["measure"])), (0, 2))
        original = self.report()["results"]["measure"]
        (self.root / "README.md").write_text("updated instructions\n")
        self.assertEqual(self.run_verify(arguments()), (0, 1))
        self.assertEqual(self.report()["results"]["measure"], original)

    def test_committing_the_same_contents_does_not_expire_evidence(self):
        self.assertEqual(self.run_verify(arguments(run=["measure"])), (0, 2))
        base = self.git("rev-parse", "HEAD").strip().decode()
        self.git("add", ".")
        self.git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "--quiet", "-m", "edit")
        self.assertEqual(self.run_verify(arguments(base=base)), (0, 0))

    def test_extended_measurements_are_explicit_and_not_normal_requirements(self):
        path = self.root / verify.POLICY
        policy = json.loads(path.read_text())
        policy["checks"][1]["extended"] = True
        path.write_text(json.dumps(policy))
        self.assertEqual(self.run_verify(arguments()), (0, 1))
        self.assertNotIn("measure", self.report()["selected"])
        self.assertEqual(self.run_verify(arguments(run=["measure"])), (0, 1))
        self.assertEqual(self.report()["results"]["measure"]["status"], "passed")
        self.assertEqual(self.run_verify(arguments()), (0, 0))
        self.assertIn("通常の完了判定外", (self.root / "target/verification/report.md").read_text())

    def test_legacy_whole_tree_results_cannot_be_relabelled_as_scoped_passes(self):
        directory = self.root / "target/verification"
        directory.mkdir(parents=True)
        stamp = verify.fingerprint(self.root, verify.changed_paths(self.root), None, {"rust": "fixture-v1"})
        (directory / "report.json").write_text(json.dumps({"version": 1, "fingerprint": stamp,
            "results": {"check": {"status": "passed"}, "measure": {"status": "passed"}}}))
        self.assertEqual(self.run_verify(arguments(run=["measure"])), (0, 2))
        self.assertEqual(self.report()["version"], 2)

    def test_a_makefile_comment_keeps_evidence_and_a_recipe_edit_expires_only_its_check(self):
        tools = {"rust": "fixture-v1"}
        policy = verify.load_policy(self.root)
        policy["checks"][1]["mode"] = "automatic"
        (self.root / verify.POLICY).write_text(json.dumps(policy))
        self.git("add", ".")
        self.git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "--quiet", "-m", "base")
        initial = verify.check_fingerprints(self.root, policy, tools)
        makefile = self.root / "Makefile"
        makefile.write_text("# Explain the checks.\n" + makefile.read_text())
        # "check" hashes every file (inputs: **); "measure" depends only on its recipe.
        self.assertEqual(verify.check_fingerprints(self.root, policy, tools)["measure"], initial["measure"])
        self.assertEqual(verify.redefined_checks(self.root, policy), set())
        self.assertEqual(set(verify.plan_checks(self.root, policy, ["Makefile"])), {"check"})
        makefile.write_text(makefile.read_text().replace("@echo measure", "@echo measured"))
        self.assertNotEqual(verify.check_fingerprints(self.root, policy, tools)["measure"], initial["measure"])
        self.assertEqual(verify.redefined_checks(self.root, policy), {"measure"})
        self.assertEqual(set(verify.plan_checks(self.root, policy, ["Makefile"])), {"check", "measure"})
        # A base that cannot be read selects every automatic check.
        self.assertIsNone(verify.redefined_checks(self.root, policy, "no-such-ref"))
        self.assertEqual(set(verify.plan_checks(self.root, policy, ["Makefile"], "no-such-ref")), {"check", "measure"})

    def test_content_digests_are_reused_only_while_the_file_is_unchanged(self):
        path = self.root / "src/example.rs"
        first = verify.content_digest(path)
        before = len(verify.CONTENT_DIGESTS)
        self.assertEqual(verify.content_digest(path), first)
        self.assertEqual(len(verify.CONTENT_DIGESTS), before)
        status = path.stat()
        # Same size and restored mtime: ctime still changes, so the edit is seen.
        path.write_text("first EDIT\n")
        os.utime(path, ns=(status.st_atime_ns, status.st_mtime_ns))
        self.assertNotEqual(verify.content_digest(path), first)

    def test_force_also_reaches_evidence_kept_inside_a_check(self):
        process = unittest.mock.Mock(pid=1)
        process.wait.return_value = 0
        check = {"id": "check", "timeout": 10}
        # A value inherited from a forced outer run must not force this run's contracts.
        plain = {key: value for key, value in os.environ.items() if key != "ECHODE_VERIFY_FORCE"}
        for inherited in ({}, {"ECHODE_VERIFY_FORCE": "1"}):
            for force, expected in ((False, None), (True, "1")):
                with self.subTest(inherited=inherited, force=force), \
                     patch.dict(os.environ, {**plain, **inherited}, clear=True), \
                     patch.object(verify.subprocess, "Popen", return_value=process) as popen:
                    verify.run_check(self.root, check, self.root / "check.log", force)
                    self.assertEqual(popen.call_args.kwargs["env"].get("ECHODE_VERIFY_FORCE"), expected)

    def test_a_changed_check_definition_does_not_expire_another_checks_evidence(self):
        path = self.root / verify.POLICY
        policy = json.loads(path.read_text())
        policy["checks"][0]["inputs"] = ["src/**"]
        path.write_text(json.dumps(policy))
        self.assertEqual(self.run_verify(arguments(run=["measure"])), (0, 2))
        policy["checks"][1]["timeout"] += 1
        path.write_text(json.dumps(policy))
        self.assertEqual(self.run_verify(arguments(run=["measure"])), (0, 1))

    def test_local_dependency_changes_expire_only_the_checks_that_use_them(self):
        (self.root / "Cargo.toml").write_text(
            '[workspace]\nmembers = ["app", "dependency", "unrelated"]\n'
            '[workspace.dependencies]\nrenamed = { package="dependency", path="dependency" }\n')
        for name in ("app", "dependency", "unrelated"):
            folder = self.root / name
            (folder / "src").mkdir(parents=True)
            manifest = f'[package]\nname="{name}"\nversion="0.0.0"\n'
            if name == "app":
                manifest += '[target.\'cfg(unix)\'.build-dependencies]\nrenamed.workspace = true\n'
            (folder / "Cargo.toml").write_text(manifest)
            (folder / "src/lib.rs").write_text("original\n")
        path = self.root / verify.POLICY
        policy = json.loads(path.read_text())
        policy["checks"][1].update({"inputs": [], "packages": ["app"]})
        path.write_text(json.dumps(policy))
        options = arguments(only=["measure"], run=["measure"])
        self.assertEqual(self.run_verify(options), (0, 1))
        (self.root / "unrelated/src/lib.rs").write_text("unrelated edit\n")
        self.assertEqual(self.run_verify(options), (0, 0))
        source = self.root / "dependency/src/lib.rs"
        source.write_text("dependency edit\n")
        self.assertEqual(self.run_verify(options), (0, 1))
        source.chmod(0o755)
        self.assertEqual(self.run_verify(options), (0, 1))
        added = self.root / "dependency/src/new.rs"
        added.write_text("new input\n")
        self.assertEqual(self.run_verify(options), (0, 1))
        added.rename(added.with_name("renamed.rs"))
        self.assertEqual(self.run_verify(options), (0, 1))
        source.unlink()
        self.assertEqual(self.run_verify(options), (0, 1))

    def test_a_symlink_target_outside_the_package_is_still_an_input(self):
        resource = self.root / "resource.txt"
        resource.write_text("original resource")
        (self.root / "src/resource.rs").symlink_to("../resource.txt")
        options = arguments(only=["measure"], run=["measure"])
        self.assertEqual(self.run_verify(options), (0, 1))
        resource.write_text("changed resource")
        self.assertEqual(self.run_verify(options), (0, 1))
        with tempfile.TemporaryDirectory() as outside:
            target = Path(outside) / "resource.txt"
            target.write_text("outside resource")
            (self.root / "src/outside.rs").symlink_to(target)
            with self.assertRaisesRegex(ValueError, "symlinks must resolve"):
                self.run_verify(options)

    def test_a_linked_input_change_during_the_check_cannot_be_recorded_as_a_pass(self):
        target = self.root / "target/resource.txt"
        target.parent.mkdir()
        target.write_text("original resource")
        (self.root / "src/resource.rs").symlink_to("../target/resource.txt")
        def edit_while_running(*args):
            target.write_text("changed resource")
            return {"status": "passed"}
        self.assertEqual(self.run_verify(arguments(only=["measure"], run=["measure"]), edit_while_running), (1, 1))
        self.assertEqual(self.report()["results"]["measure"]["status"], "stale")

    def test_a_failed_measurement_is_not_hidden_by_an_unrelated_edit(self):
        def fail_measurement(root, check, log, force=False, base=None):
            return {"status": "failed" if check["id"] == "measure" else "passed"}
        self.assertEqual(self.run_verify(arguments(run=["measure"]), fail_measurement), (1, 2))
        (self.root / "README.md").write_text("unrelated edit")
        self.assertEqual(self.run_verify(arguments()), (1, 1))
        self.assertEqual(self.report()["results"]["measure"]["status"], "failed")

    def test_device_changes_expire_measurements_without_expiring_source_checks(self):
        path = self.root / verify.POLICY
        policy = json.loads(path.read_text())
        policy["checks"][1]["device_context"] = True
        path.write_text(json.dumps(policy))
        with patch.object(verify, "audio_output_identity", return_value="device-A"):
            self.assertEqual(self.run_verify(arguments(run=["measure"])), (0, 2))
            self.assertEqual(self.run_verify(arguments()), (0, 0))
        with patch.object(verify, "audio_output_identity", return_value="device-B"):
            self.assertEqual(self.run_verify(arguments()), (1, 0))
            self.assertEqual(self.report()["results"]["check"]["status"], "passed")
            self.assertEqual(self.run_verify(arguments(run=["measure"])), (0, 1))

    def test_unidentified_devices_cannot_produce_cached_passes_but_allow_deferral(self):
        path = self.root / verify.POLICY
        policy = json.loads(path.read_text())
        policy["checks"][1]["device_context"] = True
        path.write_text(json.dumps(policy))
        with patch.object(verify, "audio_output_identity", return_value=None):
            self.assertEqual(self.run_verify(arguments(run=["measure"])), (1, 2))
            self.assertEqual(self.report()["results"]["measure"]["status"], "failed")
            self.assertEqual(self.run_verify(arguments(defer=["measure"], reason="device metadata unavailable")), (0, 0))
            self.assertEqual(self.run_verify(arguments()), (0, 0))

    def test_committed_changes_require_an_explicit_base(self):
        base = self.git("rev-parse", "HEAD").strip().decode()
        self.git("add", ".")
        self.git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "--quiet", "-m", "edit")
        self.assertEqual(verify.changed_paths(self.root), [])
        self.assertEqual(verify.changed_paths(self.root, base), ["src/example.rs"])
        with self.assertRaises(subprocess.CalledProcessError):
            verify.changed_paths(self.root, "missing-ref")

    def test_deferral_is_not_a_pass_and_expires_after_another_edit(self):
        code, count = self.run_verify(arguments(defer=["measure"], reason="device unavailable"))
        self.assertEqual((code, count), (0, 1))
        self.assertEqual(self.report()["results"]["measure"]["status"], "deferred")
        code, count = self.run_verify(arguments())
        self.assertEqual((code, count), (0, 0))
        (self.root / "src/example.rs").write_text("second edit\n")
        code, count = self.run_verify(arguments())
        self.assertEqual((code, count), (1, 1))
        self.assertEqual(self.report()["results"]["measure"]["status"], "not_run")

    def test_selected_measurements_require_a_reason_without_opening_hardware(self):
        def invoke():
            code, count = self.run_verify(arguments())
            self.assertEqual(count, 1)
            return code
        self.assertIn("followup_message", hook.respond({"status": "completed"}, invoke))
        self.assertEqual(self.report()["results"]["check"]["status"], "passed")
        self.assertEqual(self.report()["results"]["measure"]["status"], "not_run")
        self.assertEqual(self.run_verify(arguments()), (1, 0))
        self.assertEqual(self.run_verify(arguments(all=True)), (1, 0))
        self.assertEqual(self.run_verify(arguments(defer=["measure"], reason="device unavailable")), (0, 0))

    def test_explicit_measurements_must_pass_and_failures_are_not_hidden_by_the_hook(self):
        self.assertEqual(self.run_verify(arguments(only=["measure"])), (1, 0))
        self.assertEqual(self.run_verify(arguments(run=["measure"])), (0, 2))
        self.assertEqual(self.report()["results"]["measure"]["status"], "passed")
        def fail_measurement(root, check, log, force=False, base=None):
            return {"status": "failed" if check["id"] == "measure" else "passed"}
        self.assertEqual(self.run_verify(arguments(run=["measure"], force=True), fail_measurement), (1, 2))
        response = hook.respond({"status": "completed"}, lambda: self.run_verify(arguments())[0])
        self.assertIn("followup_message", response)
        self.assertEqual(self.run_verify(arguments(run=["measure"])), (0, 1))

    def test_required_failures_still_trigger_a_stop_followup(self):
        response = hook.respond({"status": "completed"}, lambda: self.run_verify(
            arguments(), lambda *args: {"status": "failed", "exit_code": 7})[0])
        self.assertIn("followup_message", response)
        self.assertEqual(self.report()["results"]["check"]["status"], "failed")

    def test_automatic_checks_cannot_be_deferred_or_forged_by_an_empty_reason(self):
        with self.assertRaises(ValueError):
            self.run_verify(arguments(defer=["check"], reason="skip"))
        with self.assertRaises(ValueError):
            self.run_verify(arguments(defer=["measure"], reason=" "))
        with self.assertRaises(ValueError):
            self.run_verify(arguments(only=["unknown"]))

    def test_cached_success_expires_when_an_unchanged_dependency_or_tool_changes(self):
        options = arguments(only=["check"])
        self.assertEqual(self.run_verify(options), (0, 1))
        self.assertEqual(self.run_verify(options), (0, 0))
        (self.root / "Makefile").write_text("check:\n\t@echo changed dependency\n")
        self.assertEqual(self.run_verify(options), (0, 1))
        self.assertEqual(self.run_verify(options, tools={"rust": "fixture-v2"}), (0, 1))

    def test_concurrent_source_edits_cannot_be_recorded_as_success(self):
        def edit_while_running(*args):
            (self.root / "src/example.rs").write_text("edited while testing\n")
            return {"status": "passed"}
        self.assertEqual(self.run_verify(arguments(only=["check"]), edit_while_running), (1, 1))
        self.assertEqual(self.report()["results"]["check"]["status"], "stale")

    def test_failed_checks_are_recorded_and_run_again(self):
        failure = lambda *args: {"status": "failed", "exit_code": 7}
        options = arguments(only=["check"])
        self.assertEqual(self.run_verify(options, failure), (1, 1))
        self.assertEqual(self.run_verify(options), (0, 1))

    def test_actual_command_failure_keeps_its_diagnostic(self):
        (self.root / "Makefile").write_text("check:\n\t@echo deliberate failure\n\t@exit 7\n")
        log = self.root / "failure.log"
        result = verify.run_check(self.root, {"id": "check", "timeout": 10}, log)
        self.assertEqual(result["status"], "failed")
        self.assertIn("deliberate failure", log.read_text())

    def test_policy_can_split_an_aggregate_without_running_product_contracts_twice(self):
        (self.root / "Makefile").write_text("check:\n\t@exit 7\n\ncheck-base:\n\t@echo base only\n")
        log = self.root / "base.log"
        result = verify.run_check(self.root, {"id": "check", "make_target": "check-base", "timeout": 10}, log)
        self.assertEqual(result["status"], "passed")
        self.assertIn("Command: make check-base", log.read_text())
        self.assertIn("base only", log.read_text())

    def test_timeout_stops_the_child_process_group(self):
        # This checks watchdog behavior, not a product performance threshold.
        pipe = self.root / "child.fifo"
        os.mkfifo(pipe)
        reader = os.open(pipe, os.O_RDONLY | os.O_NONBLOCK)
        self.addCleanup(os.close, reader)
        (self.root / "child.py").write_text(
            "import os, time\n"
            "from pathlib import Path\n"
            "Path('child.pid').write_text(str(os.getpid()))\n"
            "writer = os.open('child.fifo', os.O_WRONLY)\n"
            "os.write(writer, b'ready')\n"
            "time.sleep(60)\n")
        def cleanup_child():
            pid_file = self.root / "child.pid"
            if pid_file.exists():
                try:
                    os.kill(int(pid_file.read_text()), signal.SIGKILL)
                except ProcessLookupError:
                    pass
        self.addCleanup(cleanup_child)
        (self.root / "Makefile").write_text(f"check:\n\t@{shlex.quote(sys.executable)} parent.py\n")
        log = self.root / "timeout.log"
        for outer_timeout, inner_timeout in ((3, 60), (10, 1)):
            with self.subTest(outer=outer_timeout, inner=inner_timeout):
                (self.root / "parent.py").write_text(
                    "import subprocess, sys\n"
                    f"subprocess.run([sys.executable, {str(ROOT / 'scripts/run-bridge-validation.py')!r}, "
                    f"'--timeout', '{inner_timeout}', '--log', 'child.log', sys.executable, 'child.py'])\n")
                result = verify.run_check(self.root, {"id": "check", "timeout": outer_timeout}, log)
                self.assertEqual(result["status"], "failed")
                if outer_timeout < inner_timeout:
                    self.assertEqual(result["error"], "timeout")
                    self.assertIn("VERIFICATION TIMEOUT", log.read_text())
                else:
                    self.assertIn("VALIDATION TIMEOUT", (self.root / "child.log").read_text())
                self.assertEqual(os.read(reader, 10), b"ready")
                readable, _, _ = select.select([reader], [], [], 3)
                self.assertTrue(readable, "the child still holds its writer open")
                self.assertEqual(os.read(reader, 10), b"")

    def test_compact_plan_and_failure_excerpt(self):
        reasons = [f"crates/echode-model/src/file_{idx}.rs" for idx in range(6)]
        summary = verify.summarize_reasons(reasons, limit=3)
        self.assertIn("ほか 3 件", summary)
        self.assertNotIn("file_5.rs", summary)
        log = self.root / "failed.log"
        log.write_text(
            "running 10 tests\n"
            + "".join(f"test ok_{idx} ... ok\n" for idx in range(9))
            + "test broken_case ... FAILED\n"
            + "failures:\n"
            + "---- broken_case stdout ----\n"
            + "thread 'broken_case' panicked at crates/echode-model/src/lib.rs:42:5:\n"
            + "assertion `left == right` failed\n"
        )
        excerpt = verify.failure_excerpt(log)
        self.assertIn("test broken_case ... FAILED", excerpt)
        self.assertIn("panicked at crates/echode-model/src/lib.rs:42:5", excerpt)
        self.assertNotIn("test ok_0 ... ok", excerpt)


class HookTests(unittest.TestCase):
    def test_only_the_completed_status_starts_verification(self):
        self.assertEqual(hook.respond({"status": "cancelled", "text": '"status":"completed"'},
                                      lambda: self.fail("should not run")), {})
        self.assertEqual(hook.respond({"status": "completed"}, lambda: 0), {})
        self.assertIn("followup_message", hook.respond({"status": "completed"}, lambda: 1))

    def test_real_hook_protocol_is_json_and_invalid_input_does_not_silently_pass(self):
        for event, expected in ((json.dumps({"status": "cancelled"}), {}), ("{invalid", None)):
            result = subprocess.run([str(ROOT / ".cursor/hooks/check-on-stop.sh")], cwd=ROOT,
                                    input=event, capture_output=True, text=True, check=True)
            response = json.loads(result.stdout)
            if expected is None:
                self.assertIn("followup_message", response)
            else:
                self.assertEqual(response, expected)


class WaitForVerifyTests(unittest.TestCase):
    def test_reads_pass_and_a_missing_lock_allows_an_edit(self):
        self.assertEqual(wait_for_verify.respond({"tool_name": "Read"}), {"permission": "allow"})
        root = Path(tempfile.mkdtemp())
        self.assertEqual(wait_for_verify.respond({"tool_name": "Shell"}, root, timeout=1), {"permission": "allow"})

    def test_an_edit_waits_until_the_verification_lock_is_released(self):
        root = Path(tempfile.mkdtemp())
        lock_path = root / "target/verification/.lock"
        lock_path.parent.mkdir(parents=True)
        lock_path.touch()
        ready = root / "ready"
        holder = subprocess.Popen([sys.executable, "-c",
            "import fcntl, time, sys\n"
            "from pathlib import Path\n"
            "handle = open(sys.argv[1], 'a')\n"
            "fcntl.flock(handle, fcntl.LOCK_EX)\n"
            "Path(sys.argv[2]).write_text('ready')\n"
            "time.sleep(0.35)\n",
            str(lock_path), str(ready)])
        self.addCleanup(holder.kill)
        deadline = time.monotonic() + 2
        while not ready.exists():
            if time.monotonic() > deadline:
                self.fail("lock holder did not start")
            time.sleep(0.01)
        started = time.monotonic()
        response = wait_for_verify.respond({"tool_name": "Write"}, root, timeout=2)
        self.assertEqual(response, {"permission": "allow"})
        self.assertGreaterEqual(time.monotonic() - started, 0.2)
        self.assertEqual(holder.wait(timeout=2), 0)

    def test_a_lock_held_past_the_timeout_denies_the_edit(self):
        root = Path(tempfile.mkdtemp())
        lock_path = root / "target/verification/.lock"
        lock_path.parent.mkdir(parents=True)
        lock_path.touch()
        ready = root / "ready"
        holder = subprocess.Popen([sys.executable, "-c",
            "import fcntl, time, sys\n"
            "from pathlib import Path\n"
            "handle = open(sys.argv[1], 'a')\n"
            "fcntl.flock(handle, fcntl.LOCK_EX)\n"
            "Path(sys.argv[2]).write_text('ready')\n"
            "time.sleep(30)\n",
            str(lock_path), str(ready)])
        self.addCleanup(holder.kill)
        deadline = time.monotonic() + 2
        while not ready.exists():
            if time.monotonic() > deadline:
                self.fail("lock holder did not start")
            time.sleep(0.01)
        response = wait_for_verify.respond({"tool_name": "Shell"}, root, timeout=0.15)
        holder.kill()
        holder.wait(timeout=2)
        self.assertEqual(response["permission"], "deny")
        self.assertIn("agent_message", response)

    def test_real_hook_allows_a_read_and_rejects_invalid_input(self):
        script = ROOT / ".cursor/hooks/wait-for-verify.sh"
        self.assertTrue(os.access(script, os.X_OK))
        allowed = subprocess.run([str(script)], cwd=ROOT, input=json.dumps({"tool_name": "Read"}),
                                 capture_output=True, text=True, check=True)
        self.assertEqual(json.loads(allowed.stdout), {"permission": "allow"})
        denied = subprocess.run([str(script)], cwd=ROOT, input="{invalid",
                                capture_output=True, text=True, check=True)
        self.assertEqual(json.loads(denied.stdout)["permission"], "deny")


if __name__ == "__main__":
    unittest.main()
