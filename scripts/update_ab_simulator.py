"""Locked regular-filesystem transaction engine for Cameo A/B design fixtures."""
import contextlib, hashlib, json, os, shutil
from pathlib import Path

PHASES=("verified","preflighted","drained","state_saved","inactive_written","boot_trial","health_committed")
TRIAL_ATTEMPTS=3
MAX_STAGE_BYTES=64*1024*1024
FIRMWARES={"uefi","bios"}

def digest(data): return hashlib.sha256(data).hexdigest()
def required(value,name):
    if not isinstance(value,str) or not value.strip(): raise ValueError(f"missing {name}")
    return value
def beneath(root,value):
    lexical=root/value.lstrip("/")
    cursor=lexical
    while cursor!=root:
        if cursor.is_symlink(): raise ValueError("linked layout path")
        cursor=cursor.parent
    resolved=lexical.resolve(); resolved.relative_to(root.resolve()); return resolved
def overlaps(a,b):
    return a==b or a in b.parents or b in a.parents

def validate_layout(layout,root):
    root=root.resolve()
    if layout.get("schema")!="cameo-update-layout/v1" or layout.get("firmware") not in FIRMWARES: raise ValueError("unsupported layout")
    slots=layout.get("slots",{}); active=layout.get("active_slot")
    if set(slots)!={"A","B"} or active not in slots: raise ValueError("invalid slots")
    identities=[]; paths=[]
    for name in ("A","B"):
        slot=slots[name]; identities.append(required(slot.get("partuuid"),f"slot {name} partuuid")); required(slot.get("device_id"),f"slot {name} device")
        if slot.get("mounted") is not True: raise ValueError("slot unavailable")
        path=beneath(root,required(slot.get("root"),f"slot {name} root"))
        if not path.is_dir(): raise ValueError("slot unavailable")
        paths.append(path)
    persistent=layout.get("persistent",{}); identities.append(required(persistent.get("partuuid"),"persistent partuuid"))
    state_hash=required(persistent.get("state_sha256"),"persistent state hash")
    if len(state_hash)!=64 or any(c not in "0123456789abcdef" for c in state_hash.lower()): raise ValueError("invalid persistent state hash")
    if persistent.get("mounted") is not True: raise ValueError("persistent state unavailable")
    persistent_path=beneath(root,required(persistent.get("root"),"persistent root"))
    if not persistent_path.is_dir(): raise ValueError("persistent state unavailable")
    if len(set(identities))!=3 or slots["A"]["device_id"]!=slots["B"]["device_id"]: raise ValueError("partition identities alias or slots span devices")
    if any(overlaps(a,b) for index,a in enumerate(paths+[persistent_path]) for b in (paths+[persistent_path])[index+1:]): raise ValueError("layout paths overlap")
    return "B" if active=="A" else "A"

@contextlib.contextmanager
def writer_lock(path):
    path.parent.mkdir(parents=True,exist_ok=True); stream=path.open("a+b"); acquired=False
    try:
        if os.name=="nt":
            import msvcrt
            if stream.tell()==0: stream.write(b"0"); stream.flush()
            stream.seek(0); msvcrt.locking(stream.fileno(),msvcrt.LK_NBLCK,1)
        else:
            import fcntl; fcntl.flock(stream.fileno(),fcntl.LOCK_EX|fcntl.LOCK_NB)
        acquired=True; yield
    finally:
        if acquired and os.name=="nt":
            import msvcrt; stream.seek(0); msvcrt.locking(stream.fileno(),msvcrt.LK_UNLCK,1)
        elif acquired:
            import fcntl; fcntl.flock(stream.fileno(),fcntl.LOCK_UN)
        stream.close()

def sync_directory(path):
    if os.name!="nt":
        descriptor=os.open(path,os.O_RDONLY|os.O_DIRECTORY)
        try: os.fsync(descriptor)
        finally: os.close(descriptor)
def write_journal(path,journal):
    temporary=path.with_name(path.name+".next")
    with temporary.open("x",encoding="utf-8") as stream:
        json.dump(journal,stream,sort_keys=True,separators=(",",":")); stream.flush(); os.fsync(stream.fileno())
    os.replace(temporary,path); sync_directory(path.parent)

class Transaction:
    """All mutation entry points take the exclusive writer lock and re-parse bound bytes."""
    def __init__(self,root,layout_bytes,manifest_bytes,journal_path):
        self.root=Path(root).resolve(); self.layout_bytes=bytes(layout_bytes); self.manifest_bytes=bytes(manifest_bytes)
        self.journal_path=Path(journal_path); self.lock_path=self.journal_path.with_suffix(".lock")
        self._layout()
    def _layout(self):
        try: layout=json.loads(self.layout_bytes)
        except (UnicodeError,json.JSONDecodeError,TypeError,ValueError): raise ValueError("invalid bound layout")
        if not isinstance(layout,dict): raise ValueError("invalid bound layout")
        return layout
    def _validate(self): return validate_layout(self._layout(),self.root)
    def _locked(self,work):
        with writer_lock(self.lock_path): return work()
    def _load(self):
        journal=json.loads(self.journal_path.read_text(encoding="utf-8")); layout=self._layout(); inactive=self._validate()
        if journal.get("schema")!="cameo-update-journal/v1" or journal.get("phase") not in PHASES: raise ValueError("invalid journal")
        if journal.get("layout_sha256")!=digest(self.layout_bytes) or journal.get("manifest_sha256")!=digest(self.manifest_bytes): raise ValueError("journal input mismatch")
        if journal.get("prior_healthy_slot")==journal.get("inactive_slot") or journal.get("inactive_slot")!=inactive: raise ValueError("journal slot mismatch")
        return journal, layout
    def _discard_incomplete(self,journal,layout):
        inactive=beneath(self.root,layout["slots"][journal["inactive_slot"]]["root"]); generations=inactive/"cameo-generations"
        if not generations.is_dir(): return
        generation=journal["manifest_sha256"]
        for name in (generation+".staging", generation):
            path=generations/name
            if not path.exists(): continue
            if path.is_symlink() or path.parent.resolve()!=generations.resolve(): raise ValueError("unsafe partial generation")
            if path.is_dir(): shutil.rmtree(path)
            else: path.unlink()
        sync_directory(generations)
    def begin(self):
        def work():
            if self.journal_path.exists(): raise ValueError("transaction already begun")
            inactive=self._validate(); persistent=self._layout()["persistent"]
            record={"schema":"cameo-update-journal/v1","phase":"verified","active_slot":self._layout()["active_slot"],"inactive_slot":inactive,"prior_healthy_slot":self._layout()["active_slot"],"trial_tries_left":TRIAL_ATTEMPTS,"trial_tries_done":0,"persistent_id":persistent["partuuid"],"persistent_state_sha256":persistent["state_sha256"],"layout_sha256":digest(self.layout_bytes),"manifest_sha256":digest(self.manifest_bytes)}
            write_journal(self.journal_path,record); return record
        return self._locked(work)
    def advance(self,phase):
        def work():
            data, _layout=self._load()
            if PHASES.index(phase)!=PHASES.index(data["phase"])+1: raise ValueError("non-sequential phase")
            data["phase"]=phase; write_journal(self.journal_path,data); return data
        return self._locked(work)
    def stage(self,files,fault=None):
        def work():
            journal, layout=self._load()
            if journal["phase"]!="state_saved": raise ValueError("stage requires saved state")
            inactive=beneath(self.root,layout["slots"][journal["inactive_slot"]]["root"])
            generations=inactive/"cameo-generations"; generations.mkdir(exist_ok=True)
            generation=journal["manifest_sha256"]; staging=generations/(generation+".staging"); promoted=generations/generation
            if staging.exists() or promoted.exists(): raise ValueError("generation already exists")
            staging.mkdir(); total=0
            for index,(relative,data,expected) in enumerate(files):
                if Path(relative).is_absolute() or ".." in Path(relative).parts: raise ValueError("unsafe staged path")
                total+=len(data)
                if total>MAX_STAGE_BYTES or digest(data)!=expected: raise ValueError("stage bound or digest failure")
                target=staging/relative; target.parent.mkdir(parents=True,exist_ok=True)
                with target.open("xb") as stream: stream.write(data); stream.flush(); os.fsync(stream.fileno())
                if callable(fault): fault(index)
            sync_directory(staging); os.replace(staging,promoted); sync_directory(generations)
            if fault=="after_promote": raise RuntimeError("power loss after promote")
            journal["phase"]="inactive_written"; journal["generation_path"]=str(promoted.relative_to(self.root)); write_journal(self.journal_path,journal)
            return promoted
        return self._locked(work)
    def recover(self,observed_persistent_id,observed_state_sha256,boot_succeeded=None):
        def work():
            journal, layout=self._load()
            if observed_persistent_id!=journal["persistent_id"] or observed_state_sha256!=journal["persistent_state_sha256"]: raise ValueError("persistent state changed")
            phase=journal["phase"]
            if PHASES.index(phase)<PHASES.index("boot_trial"):
                if PHASES.index(phase)<PHASES.index("inactive_written"): self._discard_incomplete(journal,layout)
                return {"boot_slot":journal["prior_healthy_slot"],"action":"discard_inactive","journal":journal}
            if phase=="health_committed": return {"boot_slot":journal["inactive_slot"],"action":"committed","journal":journal}
            updated=dict(journal)
            if updated["trial_tries_left"]<=0: return {"boot_slot":updated["prior_healthy_slot"],"action":"automatic_fallback","journal":updated}
            updated["trial_tries_left"]-=1; updated["trial_tries_done"]+=1
            if boot_succeeded is True: updated["phase"]="health_committed"; write_journal(self.journal_path,updated); return {"boot_slot":updated["inactive_slot"],"action":"bless_trial","journal":updated}
            write_journal(self.journal_path,updated); action="retry_trial" if updated["trial_tries_left"] else "automatic_fallback"; slot=updated["inactive_slot"] if updated["trial_tries_left"] else updated["prior_healthy_slot"]
            return {"boot_slot":slot,"action":action,"journal":updated}
        return self._locked(work)
