import json
import tempfile
import unittest
from pathlib import Path

from scripts.build_update_bundle import BundleError, build_bundle
from scripts.update_host_transaction import REQUIRED_COMPONENT_IDENTITIES


def identities(**overrides):
    records = {name: {"id": name} for name in REQUIRED_COMPONENT_IDENTITIES}
    records["rocm"] = {"id": "none"}
    records.update(overrides)
    return records


class BuildUpdateBundleTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.one = self.root / "cameo"
        self.two = self.root / "cameod"
        self.one.write_bytes(b"cameo-cli\n")
        self.two.write_bytes(b"cameo-daemon\n")

    def tearDown(self):
        self.temporary.cleanup()

    def build(self, name="bundle", components=None):
        output = self.root / name
        manifest = build_bundle(
            output,
            release_id="1.2.3",
            compatibility_id="amd-v1",
            state_writes=3,
            state_reads=[3, 2, 3],
            identities=identities(),
            components=components
            or [
                (self.two, "/usr/local/bin/cameod", 0o755),
                (self.one, "/usr/local/bin/cameo", 0o755),
            ],
        )
        return output, json.loads(manifest.read_text(encoding="utf-8"))

    def test_output_is_canonical_content_bound_and_target_sorted(self):
        first, manifest = self.build("one")
        second, _ = self.build("two")
        self.assertEqual(
            (first / "manifest.json").read_bytes(),
            (second / "manifest.json").read_bytes(),
        )
        self.assertEqual(
            [item["target"] for item in manifest["files"]],
            ["/usr/local/bin/cameo", "/usr/local/bin/cameod"],
        )
        self.assertEqual(manifest["compatibility"]["state"]["reads"], [2, 3])
        self.assertEqual(set(manifest["identities"]), set(REQUIRED_COMPONENT_IDENTITIES))

    def test_existing_output_is_never_overwritten(self):
        output, _ = self.build()
        marker = output / "marker"
        marker.write_text("keep", encoding="utf-8")
        with self.assertRaisesRegex(BundleError, "refusing to replace"):
            self.build()
        self.assertEqual(marker.read_text(encoding="utf-8"), "keep")

    def test_link_duplicate_unsafe_target_and_state_contract_fail_closed(self):
        linked = self.root / "linked"
        try:
            linked.symlink_to(self.one)
        except OSError:
            linked = None
        if linked is not None:
            with self.assertRaisesRegex(BundleError, "linked"):
                self.build("linked-output", [(linked, "/usr/local/bin/cameo", 0o755)])
        with self.assertRaisesRegex(BundleError, "duplicate"):
            self.build(
                "duplicate",
                [
                    (self.one, "/usr/local/bin/cameo", 0o755),
                    (self.two, "/usr/local/bin/cameo", 0o755),
                ],
            )
        with self.assertRaisesRegex(BundleError, "allowlist"):
            self.build("unsafe", [(self.one, "/etc/passwd", 0o644)])
        with self.assertRaisesRegex(BundleError, "allowlist"):
            self.build(
                "persistent-config",
                [(self.one, "/etc/cameo/cameod.env", 0o644)],
            )
        with self.assertRaisesRegex(BundleError, "state_reads"):
            build_bundle(
                self.root / "bad-state",
                release_id="1",
                compatibility_id="amd-v1",
                state_writes=4,
                state_reads=[3],
                identities=identities(),
                components=[(self.one, "/usr/local/bin/cameo", 0o755)],
            )
        with self.assertRaisesRegex(BundleError, "component identities"):
            build_bundle(
                self.root / "bad-identities",
                release_id="1",
                compatibility_id="amd-v1",
                state_writes=3,
                state_reads=[3],
                identities={"cameo": {"id": "cameo"}},
                components=[(self.one, "/usr/local/bin/cameo", 0o755)],
            )


if __name__ == "__main__":
    unittest.main()
