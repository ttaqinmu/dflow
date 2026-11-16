"""Checks for macOS sample tree conversion and SVG output."""

import tempfile
from pathlib import Path
import unittest
import xml.etree.ElementTree as ET

import cpu_flamegraph


SAMPLE = """Analysis of sampling profile_workload (pid 12) every 2 milliseconds
Call graph:
    5 Thread_1
      5 start
        5 profile_workload::main::habc
          3 dflow_core::analysis::extract_snapshot::h1234567890abcdef
            2 parse  (in profile_workload) + 10
          2 dflow_core::git::capture_comparison::h1234567890abcdef

Total number in stack (recursive counted multiple, when >=5):
"""


class FlamegraphTests(unittest.TestCase):
    def test_collapses_inclusive_sample_counts_into_exclusive_stacks(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "sample.txt"
            path.write_text(SAMPLE)
            root, samples, interval = cpu_flamegraph.parse_sample(path)
        self.assertEqual((samples, interval), (5, 2))
        folded = cpu_flamegraph.folded_stacks(root)
        self.assertEqual(sum(folded.values()), 5)
        self.assertEqual(sorted(folded.values()), [1, 2, 2])
        self.assertTrue(any("extract_snapshot" in ";".join(stack) for stack in folded))

        svg = cpu_flamegraph.render_svg(folded, interval)
        document = ET.fromstring(svg)
        self.assertEqual(document.tag, "{http://www.w3.org/2000/svg}svg")
        self.assertIn("click a frame to zoom", svg)
        self.assertIn("self 2 samples", svg)

    def test_rejects_missing_or_unrecognized_call_graph(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "sample.txt"
            path.write_text("no call graph")
            with self.assertRaisesRegex(ValueError, "no call graph"):
                cpu_flamegraph.parse_sample(path)
            path.write_text(SAMPLE.replace("profile_workload::main::habc", "other::main"))
            with self.assertRaisesRegex(ValueError, "no profile_workload::main"):
                cpu_flamegraph.parse_sample(path)


if __name__ == "__main__":
    unittest.main()
