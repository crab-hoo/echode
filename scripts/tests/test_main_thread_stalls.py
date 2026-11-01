"""The stall report attributes main-thread silence to the test that held it."""

import importlib.util
import json
from pathlib import Path
import sys
import unittest

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))


def module():
    spec = importlib.util.spec_from_file_location("main_thread_stalls", ROOT / "scripts/main_thread_stalls.py")
    loaded = importlib.util.module_from_spec(spec)
    # dataclass resolves its module through sys.modules.
    sys.modules[spec.name] = loaded
    spec.loader.exec_module(loaded)
    return loaded


WATCH = """\
    0.000 START 1000.000
    0.400 IMAGE main AppKit ?
    0.400 BURST main
    0.400 frame dyld dlopen
    6.000 STALL 4.700 main_user=1.100 main_system=0.200 process=8.500 runnable=37.000 read_mb=1.5 pageins=2 running=40 waiting=25 uninterruptible=0 stopped=0 watch_gap=0.080 sample_block=0.010
    6.000 SAMPLE
    6.000 frame libclang_rt.tsan_osx_dynamic.dylib __tsan_memset
    6.000 frame EchodeAppTests $sPixel
    6.000 frame EchodeAppTests $sOwner
    6.000 frame libswift_Concurrency.dylib -[Some spaced symbol]
    6.100 SAMPLE
    6.100 frame dyld dyld-internal
    7.000 STALL 0.600
    7.000 SAMPLE
    7.000 frame libremovefile.dylib removefile
    7.000 frame EchodeAppTests $sThunk
    7.000 frame EchodeAppTests $sOther
    7.000 frame EchodeAppTests $sMacro
"""
NAMES = {"$sPixel": "CanvasReadback.pixel(x:y:)",
         "$sOwner": "closure #1 in CanvasSubmissionTests.contractStale()",
         "$sThunk": "defer #1 in MixerTests.closes()",
         "$sOther": "MixerTests.closes()",
         "$sMacro": "$s14EchodeAppTests05MixerC04TestfMp_"}


class ParseTests(unittest.TestCase):
    def setUp(self):
        self.tool = module()
        self.start, self.stalls = self.tool.parse_watch(WATCH)

    def test_stalls_keep_their_samples_and_ignore_burst_frames(self):
        self.assertEqual(self.start, 1000.0)
        self.assertEqual([stall.seconds for stall in self.stalls], [4.7, 0.6])
        self.assertAlmostEqual(self.stalls[0].start, 1.3)
        self.assertEqual([len(sample) for sample in self.stalls[0].samples], [4, 1])
        self.assertEqual(self.stalls[0].samples[0][3], ("libswift_Concurrency.dylib", "-[Some spaced symbol]"))

    def test_samples_name_the_outermost_test_even_past_an_unnamed_thunk(self):
        self.assertEqual(self.tool.label(self.stalls[0].samples[0], NAMES), "CanvasSubmissionTests.contractStale")
        self.assertEqual(self.tool.label(self.stalls[0].samples[1], NAMES), "(no test frame) [framework load]")
        self.assertEqual(self.tool.label(self.stalls[1].samples[0], NAMES), "MixerTests.closes [file removal]")

    def test_silence_is_counted_inside_the_deadline_window_only(self):
        # 1.3..6.0 and 6.4..7.0 from 1.3: the 5 s window ends at 6.3, before the second stall.
        self.assertAlmostEqual(self.tool.most_silence(self.stalls), 4.7)
        self.assertAlmostEqual(self.tool.most_silence(self.stalls, window=6.0), 5.3)
        self.assertEqual(self.tool.most_silence([]), 0.0)

    def test_an_issue_is_joined_with_the_stalls_before_it(self):
        events = "\n".join([
            "not json",
            json.dumps({"kind": "test", "payload": {"id": "ignored"}}),
            json.dumps({"kind": "event", "payload": {"kind": "testStarted", "instant": {"since1970": 1001.0}}}),
            json.dumps({"kind": "event", "payload": {"kind": "issueRecorded", "instant": {"since1970": 1006.5},
                                                     "messages": [{"text": "Status did not arrive"}]}}),
        ])
        issues = self.tool.issues_from_events(events, self.start)
        self.assertEqual(issues, [(6.5, "Status did not arrive")])
        lines = self.tool.summarize_run(self.stalls, issues, NAMES)
        self.assertIn("longest stall 4.70 s", lines[0])
        self.assertIn("issue at 6.500 s: Status did not arrive", lines[2])
        self.assertIn("CanvasSubmissionTests.contractStale", lines[3])
        self.assertIn("MixerTests.closes [file removal]", lines[4])

    def test_a_stall_keeps_what_the_thread_and_the_process_did(self):
        self.assertEqual(self.stalls[0].load["runnable"], 37.0)
        self.assertEqual(self.stalls[1].load, {})
        self.assertEqual(self.tool.describe_load(self.stalls[0]),
                         "main CPU 1.30 s, process 8.5 CPU-s, runnable 37.0 s, watcher gap 0.08 s")
        self.assertEqual(self.tool.describe_load(self.stalls[1]), "")
        lines = self.tool.summarize_run(self.stalls, [], NAMES)
        self.assertIn("longest stall load: main CPU 1.30 s", lines[1])

    def test_a_profile_is_split_by_activity_and_keeps_the_blocked_share(self):
        samples = self.tool.parse_profile("""\
    0.100 PROFILE 0.050 1
    0.100 frame dyld dlopen_internal
    0.100 frame EchodeAppTests $sOwner
    0.200 PROFILE 0.100 4
    0.200 frame libremovefile.dylib removefile
    0.200 frame EchodeAppTests $sThunk
    0.300 PROFILE 9.000 3
    0.300 frame AppKit -[NSButtonCell _doUserDisclosureExpandOrCollapseInRect:]
    0.300 frame EchodeAppTests $sOwner
    0.400 STALL 1.000
    0.400 frame EchodeAppTests $sIgnored
    0.500 PROFILE 9.100 1
    0.500 frame libsystem_kernel.dylib mach_msg2_trap
""")
        self.assertEqual([sample[:2] for sample in samples], [(0.05, 1), (0.1, 4), (9.0, 3), (9.1, 1)])
        self.assertEqual(len(samples[2][2]), 2)
        self.assertEqual([self.tool.profile_category(sample[2]) for sample in samples],
                         ["framework load", "file removal", "disclosure click (nested run loop)", "no test frame"])
        whole = self.tool.profile_table([samples, []])
        self.assertEqual(len(whole), 4)
        self.assertIn("0.03 s/run  blocked 100%  file removal", " ".join(whole))
        early = self.tool.profile_table([samples], end=8.0)
        self.assertEqual(len(early), 2)

    def test_attribution_is_averaged_over_runs(self):
        lines = self.tool.attribution([(self.stalls, NAMES), ([], {})], top=1)
        self.assertEqual(len(lines), 1)
        self.assertIn("0.12 s/run", lines[0])

    def test_only_test_binary_symbols_are_demangled(self):
        asked = []

        def demangle(names):
            asked.extend(names)
            return [NAMES.get(name, name) for name in names]

        names = self.tool.demangled(self.stalls, demangle)
        self.assertEqual(sorted(asked), sorted(NAMES))
        self.assertEqual(names["$sOwner"], NAMES["$sOwner"])
        self.assertEqual(self.tool.demangled([], demangle), {})


if __name__ == "__main__":
    unittest.main()
