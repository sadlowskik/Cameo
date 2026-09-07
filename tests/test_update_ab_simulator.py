import json, shutil, unittest
from pathlib import Path
from unittest.mock import patch
from scripts import update_ab_simulator as ab

class TransactionTests(unittest.TestCase):
    def setUp(self):
        self.root=Path(__file__).parent/"fixtures/update-ab"; self.journal=self.root/"journal.json"
        self.layout={"schema":"cameo-update-layout/v1","firmware":"uefi","active_slot":"A","slots":{"A":{"partuuid":"a","device_id":"disk","root":"/a","mounted":True},"B":{"partuuid":"b","device_id":"disk","root":"/b","mounted":True}},"persistent":{"mounted":True,"partuuid":"state","root":"/state","state_sha256":"a"*64}}
        self.layout_bytes=json.dumps(self.layout,sort_keys=True).encode(); self.manifest=b'{"release":"2"}'
        self.tx=ab.Transaction(self.root,self.layout_bytes,self.manifest,self.journal)
        for item in (self.journal,self.journal.with_suffix(".lock"),self.journal.with_name(self.journal.name+".next")):
            if item.exists(): item.unlink()
        generations=self.root/"b/cameo-generations"
        if generations.exists(): shutil.rmtree(generations)
    def tearDown(self):
        for item in (self.journal,self.journal.with_suffix(".lock"),self.journal.with_name(self.journal.name+".next")):
            if item.exists(): item.unlink()
        generations=self.root/"b/cameo-generations"
        if generations.exists(): shutil.rmtree(generations)
    def phase(self,name):
        self.tx.begin()
        for phase in ab.PHASES[1:ab.PHASES.index(name)+1]: self.tx.advance(phase)
    def test_bound_exact_layout_and_manifest(self):
        record=self.tx.begin(); self.assertEqual(record["inactive_slot"],"B")
        changed=ab.Transaction(self.root,self.layout_bytes+b" ",self.manifest,self.journal)
        with self.assertRaises(ValueError): changed.advance("preflighted")
        changed=ab.Transaction(self.root,self.layout_bytes,self.manifest+b" ",self.journal)
        with self.assertRaises(ValueError): changed.advance("preflighted")
    def test_layout_rejects_missing_alias_nested_escape_and_persistence_alias(self):
        mutations=[lambda x:x["slots"]["A"].update(partuuid=""),lambda x:x["persistent"].update(state_sha256="x"),lambda x:x["slots"]["B"].update(root="/../outside"),lambda x:x["slots"]["B"].update(root="/a"),lambda x:x["persistent"].update(partuuid="a"),lambda x:x["persistent"].update(root="/a/child")]
        for mutate in mutations:
            data=json.loads(self.layout_bytes); mutate(data)
            with self.subTest(data=data),self.assertRaises((ValueError,KeyError)): ab.Transaction(self.root,json.dumps(data).encode(),self.manifest,self.journal).begin()
    def test_integrated_stage_and_immutable_generation(self):
        self.phase("state_saved"); payload=b"new"; promoted=self.tx.stage([("usr/bin/cameo",payload,ab.digest(payload))])
        self.assertEqual((promoted/"usr/bin/cameo").read_bytes(),payload); self.assertEqual(json.loads(self.journal.read_text())["phase"],"inactive_written")
        with self.assertRaises(ValueError): self.tx.stage([("other",payload,ab.digest(payload))])
    def test_fault_leaves_partial_then_recovery_removes_only_partial(self):
        self.phase("state_saved"); payload=b"new"
        with self.assertRaises(RuntimeError): self.tx.stage([("cameo",payload,ab.digest(payload))],lambda _:(_ for _ in ()).throw(RuntimeError("power loss")))
        partial=self.root/"b/cameo-generations"/(ab.digest(self.manifest)+".staging"); self.assertTrue(partial.exists()); self.assertTrue((self.root/"a/.keep").exists())
        result=self.tx.recover("state","a"*64); self.assertEqual(result["boot_slot"],"A"); self.assertFalse(partial.exists())
    def test_every_phase_recovers_expected_slot_and_persists_counters(self):
        for phase in ab.PHASES:
            with self.subTest(phase=phase):
                self.tearDown(); self.phase(phase)
                if phase=="boot_trial":
                    result=self.tx.recover("state","a"*64,False); self.assertEqual(result["boot_slot"],"B"); self.assertEqual(json.loads(self.journal.read_text())["trial_tries_left"],2)
                elif phase=="health_committed": self.assertEqual(self.tx.recover("state","a"*64)["boot_slot"],"B")
                else: self.assertEqual(self.tx.recover("state","a"*64)["boot_slot"],"A")
    def test_three_failed_trials_fallback_and_success_bless(self):
        self.phase("boot_trial")
        for expected in (2,1,0): result=self.tx.recover("state","a"*64,False); self.assertEqual(result["journal"]["trial_tries_left"],expected)
        self.assertEqual(result["boot_slot"],"A")
        self.tearDown(); self.phase("boot_trial"); self.assertEqual(self.tx.recover("state","a"*64,True)["action"],"bless_trial")
    def test_torn_phase_file_is_ignored_and_lock_is_exclusive(self):
        self.tx.begin(); self.journal.with_name(self.journal.name+".next").write_text('{"phase":')
        self.assertEqual(json.loads(self.journal.read_text())["phase"],"verified")
        self.journal.with_name(self.journal.name+".next").unlink()
        with ab.writer_lock(self.tx.lock_path):
            with self.assertRaises((BlockingIOError,OSError)):
                with ab.writer_lock(self.tx.lock_path): pass
    def test_validation_precedes_recovery_mutation(self):
        self.phase("state_saved"); partial=self.root/"b/cameo-generations"/(ab.digest(self.manifest)+".staging"); partial.mkdir(parents=True)
        bad=ab.Transaction(self.root,self.layout_bytes,self.manifest+b"bad",self.journal)
        with self.assertRaises(ValueError): bad.recover("state","a"*64)
        self.assertTrue(partial.exists())
    def test_stage_bound_and_digest_fail_without_promotion(self):
        self.phase("state_saved")
        with patch.object(ab,"MAX_STAGE_BYTES",2),self.assertRaises(ValueError): self.tx.stage([("x",b"big",ab.digest(b"big"))])
        self.assertFalse((self.root/"b/cameo-generations"/ab.digest(self.manifest)).exists())
    def test_begin_refuses_to_clobber_an_existing_journal(self):
        self.tx.begin()
        with self.assertRaises(ValueError): self.tx.begin()
        self.assertEqual(json.loads(self.journal.read_text())["phase"],"verified")
    def test_bios_layout_is_accepted_at_this_layer(self):
        data=json.loads(self.layout_bytes); data["firmware"]="bios"
        record=ab.Transaction(self.root,json.dumps(data,sort_keys=True).encode(),self.manifest,self.journal).begin()
        self.assertEqual(record["inactive_slot"],"B")
    def test_mutated_layout_object_cannot_diverge_from_bound_bytes(self):
        self.tx.begin()
        leaked=self.tx._layout(); leaked["slots"]["B"]["root"]="/a"
        self.assertEqual(self.tx.advance("preflighted")["inactive_slot"],"B")
        self.assertEqual(self.tx._layout()["slots"]["B"]["root"],"/b")
    def test_promoted_generation_without_journal_commit_is_discarded(self):
        self.phase("state_saved"); payload=b"new"
        with self.assertRaises(RuntimeError): self.tx.stage([("cameo",payload,ab.digest(payload))],"after_promote")
        promoted=self.root/"b/cameo-generations"/ab.digest(self.manifest)
        self.assertTrue(promoted.exists()); self.assertEqual(json.loads(self.journal.read_text())["phase"],"state_saved")
        result=self.tx.recover("state","a"*64)
        self.assertEqual(result["boot_slot"],"A"); self.assertFalse(promoted.exists())
        retry=self.tx.stage([("cameo",payload,ab.digest(payload))])
        self.assertEqual((retry/"cameo").read_bytes(),payload)

if __name__=="__main__": unittest.main()
