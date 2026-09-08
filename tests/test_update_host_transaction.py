import json
import tempfile
import unittest
from pathlib import Path

from scripts.update_host_transaction import (
    HostTransaction,
    UpdateError,
    json_bytes,
    sha256_bytes,
    sha256_file,
)


class FakeHost:
    def __init__(self):
        self.active = "A"
        self.calls = []
        self.fail_bless = False
        self.fail_restore = False
        self.installed = {"writes": 3, "reads": [1, 2, 3]}
        self.persistent = {"writes": 3, "reads": [3]}

    def observed_active_slot(self, layout):
        return self.active

    def preflight(self, layout, manifest, inactive):
        self.calls.append(("preflight", inactive))

    def drain(self):
        self.calls.append(("drain",))

    def snapshot_state(self, destination):
        destination.write_text("verified state\n", encoding="utf-8")
        self.calls.append(("snapshot", destination.name))
        return sha256_file(destination)

    def write_inactive(self, layout, manifest, bundle, inactive):
        self.calls.append(("write", inactive, manifest["release_id"]))
        return "generation-2"

    def stage_boot_trial(self, layout, manifest, inactive, generation):
        self.calls.append(("trial", inactive, generation))
        return "cameo-B+3"

    def health_check(self, journal):
        self.calls.append(("health", journal["inactive_slot"]))

    def bless(self, journal):
        self.calls.append(("bless", journal["boot_entry"]))
        if self.fail_bless:
            self.fail_bless = False
            raise RuntimeError("simulated power loss before layout commit")

    def select_prior(self, journal):
        self.calls.append(("prior", journal["prior_healthy_slot"]))

    def discard_inactive(self, journal):
        self.calls.append(("discard", journal["inactive_slot"]))

    def installed_state_support(self):
        return dict(self.installed)

    def persistent_state_schema(self):
        return dict(self.persistent)

    def restore_state(self, backup):
        self.calls.append(("restore", Path(backup).name))
        if self.fail_restore:
            raise UpdateError("cannot restore persistent state")
        self.persistent = {
            "writes": self.installed["writes"],
            "reads": [self.installed["writes"]],
        }

    def record_persistent_schema(self, writes):
        self.persistent = {"writes": writes, "reads": [writes]}
        self.calls.append(("record", writes))


class HostTransactionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.layout = self.root / "layout.json"
        self.journal = self.root / "state/transaction.json"
        self.state = self.root / "state"
        self.bundle = self.root / "bundle"
        self.bundle.mkdir()
        self.manifest = self.bundle / "manifest.json"
        layout = {
            "schema": "cameo-update-layout/v2",
            "firmware": "uefi",
            "bootloader": "systemd-boot",
            "active_slot": "A",
            "boot": {"partuuid": "esp", "device_id": "disk-1"},
            "slots": {
                "A": {"partuuid": "slot-a", "device_id": "disk-1", "boot_entry": "cameo-A"},
                "B": {"partuuid": "slot-b", "device_id": "disk-1", "boot_entry": None},
            },
            "persistent": {"partuuid": "state", "device_id": "disk-1"},
        }
        manifest = {
            "schema": "cameo-update/v1",
            "release_id": "2.0.0",
            "compatibility": {"id": "amd-v1", "state": {"writes": 3, "reads": [2, 3]}},
            "identities": {
                name: {"id": name if name != "rocm" else "none"}
                for name in (
                    "os",
                    "kernel",
                    "firmware",
                    "mesa_vulkan",
                    "rocm",
                    "llama",
                    "cameo",
                    "knossos",
                    "starter_model",
                    "schemas",
                )
            },
            "files": [{"source": "payload", "target": "/usr/local/bin/cameo", "sha256": "0" * 64}],
        }
        self.layout.write_text(json.dumps(layout), encoding="utf-8")
        self.manifest.write_text(json.dumps(manifest), encoding="utf-8")
        (self.bundle / "payload").write_text("binary", encoding="utf-8")
        self.host = FakeHost()
        self.tx = HostTransaction(self.layout, self.journal, self.state, self.host)

    def tearDown(self):
        self.temp.cleanup()

    def test_apply_records_every_irreversible_boundary_before_trial_boot(self):
        result = self.tx.apply(self.bundle, self.manifest)
        self.assertEqual(result["phase"], "boot_trial")
        self.assertEqual(result["inactive_slot"], "B")
        self.assertEqual(
            [call[0] for call in self.host.calls],
            ["preflight", "drain", "snapshot", "write", "trial"],
        )
        persisted = json.loads(self.journal.read_text(encoding="utf-8"))
        self.assertEqual(persisted["boot_entry"], "cameo-B+3")
        self.assertEqual(persisted["state_backup_sha256"], sha256_file(Path(persisted["state_backup"])))

    def test_commit_requires_the_trial_slot_and_moves_layout_atomically(self):
        self.tx.apply(self.bundle, self.manifest)
        with self.assertRaisesRegex(UpdateError, "trial slot"):
            self.tx.commit()
        self.host.active = "B"
        result = self.tx.commit()
        self.assertEqual(result["phase"], "health_committed")
        self.assertEqual(json.loads(self.layout.read_text())["active_slot"], "B")
        self.assertEqual(
            json.loads(self.layout.read_text())["slots"]["B"]["boot_entry"],
            "cameo-B+3",
        )
        self.assertIn(("health", "B"), self.host.calls)
        self.assertIn(("bless", "cameo-B+3"), self.host.calls)

    def test_interrupted_health_commit_can_resume_before_or_after_layout_write(self):
        self.tx.apply(self.bundle, self.manifest)
        self.host.active = "B"
        self.host.fail_bless = True
        with self.assertRaisesRegex(RuntimeError, "power loss"):
            self.tx.commit()
        journal = json.loads(self.journal.read_text())
        self.assertEqual(journal["phase"], "health_checked")
        self.assertEqual(json.loads(self.layout.read_text())["active_slot"], "A")

        # Model a crash after the layout rename but before the final journal
        # rename. The prepared next-layout hash makes this state recoverable.
        layout = journal["next_layout"]
        self.assertEqual(
            sha256_bytes(json_bytes(layout)), journal["next_layout_sha256"]
        )
        self.layout.write_bytes(json_bytes(layout))
        result = self.tx.commit()
        self.assertEqual(result["phase"], "health_committed")

    def test_recover_before_trial_selects_prior_and_discards_inactive(self):
        self.tx.apply(self.bundle, self.manifest)
        journal = json.loads(self.journal.read_text())
        journal["phase"] = "state_saved"
        self.journal.write_text(json.dumps(journal), encoding="utf-8")
        result = self.tx.recover()
        self.assertEqual(result["phase"], "rolled_back")
        self.assertEqual(self.host.calls[-2:], [("prior", "A"), ("discard", "B")])

    def test_recover_leaves_a_boot_trial_for_boot_assessment(self):
        self.tx.apply(self.bundle, self.manifest)
        result = self.tx.recover()
        self.assertEqual(result["phase"], "boot_trial")
        self.assertNotIn(("discard", "B"), self.host.calls)

    def test_changed_state_backup_blocks_commit_and_recovery(self):
        result = self.tx.apply(self.bundle, self.manifest)
        Path(result["state_backup"]).write_text("tampered\n", encoding="utf-8")
        with self.assertRaisesRegex(UpdateError, "backup evidence changed"):
            self.tx.recover()

    def test_layout_alias_drift_and_unfinished_overwrite_fail_closed(self):
        self.tx.apply(self.bundle, self.manifest)
        with self.assertRaisesRegex(UpdateError, "unfinished update"):
            self.tx.apply(self.bundle, self.manifest)
        changed = json.loads(self.layout.read_text())
        changed["slots"]["B"]["partuuid"] = "slot-a"
        self.layout.write_text(json.dumps(changed), encoding="utf-8")
        with self.assertRaises(UpdateError):
            self.tx.status()

    def test_commit_records_the_new_persistent_schema(self):
        self.tx.apply(self.bundle, self.manifest)
        self.host.active = "B"
        self.tx.commit()
        self.assertEqual(self.host.persistent["writes"], 3)
        self.assertIn(("record", 3), self.host.calls)

    def test_incomplete_component_identities_are_refused(self):
        manifest = json.loads(self.manifest.read_text(encoding="utf-8"))
        manifest["identities"].pop("kernel")
        self.manifest.write_text(json.dumps(manifest), encoding="utf-8")
        with self.assertRaisesRegex(UpdateError, "component identities"):
            self.tx.apply(self.bundle, self.manifest)

    def test_incompatible_downgrade_is_refused_before_mutation(self):
        self.host.persistent = {"writes": 4, "reads": [4]}
        with self.assertRaisesRegex(UpdateError, "cannot be read"):
            self.tx.apply(self.bundle, self.manifest)
        self.assertFalse(self.journal.exists())
        self.assertEqual(self.host.calls, [])

    def test_breaking_forward_migration_restores_state_before_prior_slot(self):
        manifest = json.loads(self.manifest.read_text(encoding="utf-8"))
        manifest["compatibility"]["state"] = {"writes": 4, "reads": [3, 4]}
        self.manifest.write_text(json.dumps(manifest), encoding="utf-8")
        result = self.tx.apply(self.bundle, self.manifest)
        self.assertTrue(result["rollback_requires_state_restore"])
        self.host.active = "B"
        rolled = self.tx.rollback()
        self.assertEqual(rolled["phase"], "rolled_back")
        self.assertEqual(self.host.calls[-4:], [
            ("restore", Path(result["state_backup"]).name),
            ("record", 3),
            ("prior", "A"),
            ("discard", "B"),
        ])
        self.assertEqual(self.host.persistent["writes"], 3)

    def test_failed_state_restore_does_not_select_the_prior_slot(self):
        manifest = json.loads(self.manifest.read_text(encoding="utf-8"))
        manifest["compatibility"]["state"] = {"writes": 4, "reads": [3, 4]}
        self.manifest.write_text(json.dumps(manifest), encoding="utf-8")
        self.tx.apply(self.bundle, self.manifest)
        self.host.active = "B"
        self.host.fail_restore = True
        with self.assertRaisesRegex(UpdateError, "cannot restore"):
            self.tx.rollback()
        self.assertNotIn(("prior", "A"), self.host.calls)
        self.assertEqual(json.loads(self.journal.read_text())["phase"], "boot_trial")

    def test_assessment_fallback_restores_before_old_slot_keeps_running(self):
        manifest = json.loads(self.manifest.read_text(encoding="utf-8"))
        manifest["compatibility"]["state"] = {"writes": 4, "reads": [3, 4]}
        self.manifest.write_text(json.dumps(manifest), encoding="utf-8")
        self.tx.apply(self.bundle, self.manifest)
        self.host.active = "A"
        recovered = self.tx.recover()
        self.assertEqual(recovered["phase"], "boot_trial")
        self.assertTrue(any(call[0] == "restore" for call in self.host.calls))
        self.assertEqual(self.host.persistent["writes"], 3)

    def test_compatible_rollback_does_not_rewrite_readable_state(self):
        self.tx.apply(self.bundle, self.manifest)
        self.host.active = "B"
        self.tx.rollback()
        self.assertFalse(any(call[0] == "restore" for call in self.host.calls))

    def test_bios_requires_the_explicit_grub_adapter(self):
        layout = json.loads(self.layout.read_text())
        layout["firmware"] = "bios"
        self.layout.write_text(json.dumps(layout), encoding="utf-8")
        with self.assertRaisesRegex(UpdateError, "GRUB"):
            self.tx.status()


if __name__ == "__main__":
    unittest.main()
