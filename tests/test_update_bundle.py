import importlib.machinery
import importlib.util
import unittest
import os
import shutil
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).parents[1]
SCRIPT = ROOT / "archiso/airootfs/usr/local/lib/cameo/update-verify"
FIXTURE = ROOT / "tests/fixtures/update-valid"
loader = importlib.machinery.SourceFileLoader("update_verify", str(SCRIPT))
spec = importlib.util.spec_from_loader(loader.name, loader)
update = importlib.util.module_from_spec(spec); loader.exec_module(update)

class UpdateBundleTests(unittest.TestCase):
    def manifest(self): return update.load(FIXTURE / "manifest.json")
    def test_valid_bundle(self): self.assertEqual(update.verify(FIXTURE)["release_id"], "1.2.3")
    def test_digest_mismatch_fails(self):
        data = self.manifest(); data["files"][0]["sha256"] = "0" * 64
        with self.assertRaises(SystemExit): list(update.entries(FIXTURE, data))
    def test_target_escape_fails(self):
        data = self.manifest(); data["files"][0]["target"] = "/etc/passwd"
        with self.assertRaises(SystemExit): list(update.entries(FIXTURE, data))
    def test_duplicate_target_fails(self):
        data = self.manifest(); data["files"].append(dict(data["files"][0]))
        with self.assertRaises(SystemExit): list(update.entries(FIXTURE, data))
    def test_normalized_target_alias_fails(self):
        data = self.manifest(); alias = dict(data["files"][0]); alias["target"] = "/usr/local/bin/./cameo"; data["files"].append(alias)
        with self.assertRaises(SystemExit): list(update.entries(FIXTURE, data))
    @unittest.skipIf(os.name == "nt", "OpenSSL fixture runs in Linux CI")
    def test_actual_ed25519_signature_and_tamper(self):
        self.assertIsNotNone(shutil.which("openssl"), "openssl is required for the Linux update gate")
        with tempfile.TemporaryDirectory() as directory:
            d=Path(directory); key=d/"key.pem"; public=d/"public.pem"; sig=d/"manifest.sig"; manifest=FIXTURE/"manifest.json"
            subprocess.run(["openssl","genpkey","-algorithm","ED25519","-out",key],check=True)
            subprocess.run(["openssl","pkey","-in",key,"-pubout","-out",public],check=True)
            subprocess.run(["openssl","pkeyutl","-sign","-inkey",key,"-rawin","-in",manifest,"-out",sig],check=True)
            verify=["openssl","pkeyutl","-verify","-pubin","-inkey",str(public),"-rawin","-in",str(manifest),"-sigfile",str(sig)]
            self.assertEqual(subprocess.run(verify).returncode,0)
            tampered=d/"tampered"; tampered.write_bytes(manifest.read_bytes()+b" ")
            verify[verify.index(str(manifest))]=str(tampered)
            self.assertNotEqual(subprocess.run(verify).returncode,0)
            transaction=ROOT/"archiso/airootfs/usr/local/lib/cameo/update-transaction"
            verifier=d/"verifier"; verifier.write_text(f"#!/bin/sh\nexec python3 '{SCRIPT}' \"$@\"\n"); verifier.chmod(0o700)
            env=dict(os.environ,CAMEO_UPDATE_TEST_KEY=str(public),CAMEO_UPDATE_TEST_VERIFIER=str(verifier))
            shutil.copy2(sig,FIXTURE/"manifest.sig")
            try:
                result=subprocess.run(["bash",str(transaction),"verify",str(FIXTURE)],env=env,capture_output=True,text=True)
                self.assertEqual(result.returncode,0,result.stderr)
                (FIXTURE/"manifest.sig").write_bytes(b"tampered")
                self.assertNotEqual(subprocess.run(["bash",str(transaction),"verify",str(FIXTURE)],env=env,capture_output=True).returncode,0)
            finally: (FIXTURE/"manifest.sig").unlink(missing_ok=True)

if __name__ == "__main__": unittest.main()
