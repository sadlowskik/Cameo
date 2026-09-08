import unittest
from pathlib import Path


ROOT = Path(__file__).parents[1]
INSTALLER = ROOT / "archiso/airootfs/usr/local/bin/cameo-install"


class InstallerAbContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.script = INSTALLER.read_text(encoding="utf-8")

    def test_layout_has_two_fixed_system_slots_and_separate_state(self):
        for label in ("CAMEO-A", "CAMEO-B", "CAMEO-STATE"):
            self.assertIn(label, self.script)
        self.assertIn('ROOT_SLOT_GIB=24', self.script)
        self.assertIn('MIN_DISK_BYTES=$((54 * 1024 * 1024 * 1024))', self.script)
        self.assertNotIn('-c2:cameo-root', self.script)

    def test_identity_configuration_models_and_homes_are_persistent(self):
        for mapping in (
            '"etc/cameo:etc-cameo"',
            '"var/lib/cameo:var-lib-cameo"',
            '"home:home"',
        ):
            self.assertIn(mapping, self.script)
        self.assertIn("cameo-update-layout/v2", self.script)
        self.assertIn('"active_slot": "A"', self.script)

    def test_uefi_uses_assessment_aware_systemd_boot_without_a_forced_default(self):
        self.assertIn('BOOTLOADER="systemd-boot"', self.script)
        self.assertIn('bootctl --esp-path=/boot install', self.script)
        self.assertIn('options root=PARTUUID=${ROOT_A_PARTUUID} rw cameo.slot=A', self.script)
        loader = self.script.split("cat > /mnt/boot/loader/loader.conf <<'EOF'", 1)[1].split("EOF", 1)[0]
        self.assertNotIn("default", loader)

    def test_bios_has_a_stable_saved_entry_for_one_shot_fallback(self):
        self.assertIn('GRUB_DEFAULT=saved', self.script)
        self.assertIn("--id 'cameo-A'", self.script)
        self.assertIn('grub-set-default cameo-A', self.script)

    def test_compatibility_and_component_identities_are_declared(self):
        self.assertIn("compatibility-id", self.script)
        self.assertIn("component-identities.json", self.script)
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
        ):
            self.assertIn(f'"{name}"', self.script)

    def test_slot_and_persistent_state_schema_are_declared(self):
        self.assertIn("/usr/share/cameo/state-schema.json", self.script)
        self.assertIn("/var/lib/cameo/state-schema.json", self.script)
        self.assertIn('"writes": 3', self.script)
        self.assertIn('"reads": [1, 2, 3]', self.script)

    def test_recovery_and_health_commit_units_are_enabled(self):
        self.assertIn("cameo-update-recover.service", self.script)
        self.assertIn("cameo-update-health.service", self.script)


if __name__ == "__main__":
    unittest.main()
