//! The dashboard: one self-contained HTML page served at `/`.
//!
//! No build step, no bundler, no external asset — the page is vanilla HTML, CSS
//! and JS embedded in the binary, matching the daemon's dependency-light stance.
//! It talks to the same `/api` routes documented in [`crate::app`]; if you add a
//! route there, wire it in here.

/// The complete dashboard page. Embedded verbatim; served as `text/html`.
pub const INDEX_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Cameo Console</title>
<style>
  :root{
    --bg:#0e1116; --panel:#161b22; --panel-2:#1c232d; --border:#2a3038;
    --ink:#e6edf3; --muted:#8b949e; --accent:#ff7f5c; --accent-dim:#3a2a24;
    --t1:#3fb950; --t2:#d29922; --t3:#39c5cf; --danger:#f85149;
  }
  *{box-sizing:border-box}
  body{margin:0;background:var(--bg);color:var(--ink);
    font:14px/1.5 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif;}
  a{color:var(--accent)}
  header{display:flex;align-items:baseline;gap:16px;padding:20px 28px;
    border-bottom:1px solid var(--border);background:var(--panel);}
  .wordmark{font-weight:800;letter-spacing:.5px;font-size:20px;color:var(--accent)}
  .tagline{color:var(--muted);font-size:13px}
  .status{margin-left:auto;font-size:12px;color:var(--muted)}
  .dot{display:inline-block;width:8px;height:8px;border-radius:50%;background:var(--muted);margin-right:6px;vertical-align:middle}
  .dot.ok{background:var(--t1)} .dot.bad{background:var(--danger)}
  main{max-width:1100px;margin:0 auto;padding:24px 28px;display:grid;gap:24px}
  section{background:var(--panel);border:1px solid var(--border);border-radius:10px;overflow:hidden}
  section > h2{margin:0;padding:14px 18px;font-size:13px;text-transform:uppercase;
    letter-spacing:.6px;color:var(--muted);border-bottom:1px solid var(--border);background:var(--panel-2)}
  .body{padding:18px}
  .grid{display:grid;gap:14px;grid-template-columns:repeat(auto-fill,minmax(260px,1fr))}
  .card{background:var(--panel-2);border:1px solid var(--border);border-radius:8px;padding:14px}
  .card h3{margin:0 0 8px;font-size:15px;display:flex;align-items:center;gap:8px}
  .kv{display:grid;grid-template-columns:auto 1fr;gap:2px 10px;font-size:13px;color:var(--muted)}
  .kv b{color:var(--ink);font-weight:500}
  .badge{font-size:11px;font-weight:700;padding:2px 8px;border-radius:20px;white-space:nowrap}
  .tier1{background:rgba(63,185,80,.15);color:var(--t1)}
  .tier2{background:rgba(210,153,34,.15);color:var(--t2)}
  .tier3{background:rgba(57,197,207,.15);color:var(--t3)}
  .st-running{background:rgba(63,185,80,.15);color:var(--t1)}
  .st-exited{background:rgba(139,148,158,.15);color:var(--muted)}
  .st-failed{background:rgba(248,81,73,.15);color:var(--danger)}
  table{width:100%;border-collapse:collapse}
  th,td{text-align:left;padding:9px 10px;border-bottom:1px solid var(--border);font-size:13px;vertical-align:top}
  th{color:var(--muted);font-weight:500;font-size:12px}
  td code{font-size:12px;color:var(--muted);word-break:break-all}
  .muted{color:var(--muted)}
  form{display:grid;grid-template-columns:repeat(auto-fit,minmax(150px,1fr));gap:12px;align-items:end}
  label{display:block;font-size:12px;color:var(--muted);margin-bottom:4px}
  input,select{width:100%;background:var(--bg);border:1px solid var(--border);color:var(--ink);
    border-radius:6px;padding:8px 10px;font-size:13px}
  input:focus,select:focus{outline:none;border-color:var(--accent)}
  .check{display:flex;align-items:center;gap:8px}
  .check input{width:auto}
  button{background:var(--accent);color:#1a1008;border:none;border-radius:6px;padding:9px 16px;
    font-weight:600;font-size:13px;cursor:pointer}
  button:hover{filter:brightness(1.08)} button:active{transform:translateY(1px)}
  button.ghost{background:transparent;color:var(--muted);border:1px solid var(--border)}
  button.stop{background:transparent;color:var(--danger);border:1px solid var(--danger);padding:5px 12px;font-size:12px}
  .notes{margin:6px 0 0;padding-left:16px;color:var(--muted);font-size:12px}
  .flash{padding:10px 14px;border-radius:6px;font-size:13px;margin-bottom:14px;display:none}
  .flash.err{display:block;background:var(--accent-dim);color:var(--accent);border:1px solid var(--accent)}
  .flash.ok{display:block;background:rgba(63,185,80,.1);color:var(--t1);border:1px solid var(--t1)}
  .empty{color:var(--muted);font-style:italic;padding:8px 0}
</style>
</head>
<body>
<header>
  <span class="wordmark">CAMEO</span>
  <span class="tagline">any AMD card → a working LLM box</span>
  <span class="status" id="status"><span class="dot" id="dot"></span><span id="status-text">connecting…</span></span>
</header>
<main>
  <section>
    <h2>GPUs &amp; tiers</h2>
    <div class="body"><div id="gpus" class="grid"><div class="empty">loading…</div></div>
      <div id="topo" class="muted" style="margin-top:12px"></div></div>
  </section>

  <section>
    <h2>Endpoints</h2>
    <div class="body">
      <div id="flash" class="flash"></div>
      <table id="servers-table" style="display:none">
        <thead><tr><th>Model</th><th>State</th><th>Endpoint</th><th>Backend</th><th>Uptime</th><th></th></tr></thead>
        <tbody id="servers"></tbody>
      </table>
      <div id="servers-empty" class="empty">No endpoints yet. Start one below.</div>
    </div>
  </section>

  <section>
    <h2>Start an endpoint</h2>
    <div class="body">
      <form id="start-form">
        <div><label for="f-model">Model</label>
          <input id="f-model" list="model-list" placeholder="tinyllama or /path/to.gguf" required>
          <datalist id="model-list"></datalist></div>
        <div><label for="f-port">Port</label><input id="f-port" type="number" value="8080" min="1" max="65535"></div>
        <div><label for="f-host">Bind host</label><input id="f-host" value="127.0.0.1"></div>
        <div><label for="f-params">Params (B)</label><input id="f-params" type="number" step="0.1" value="7"></div>
        <div><label for="f-quant">Quant</label>
          <select id="f-quant"><option>Q4_K_M</option><option>Q5_K_M</option><option>Q6_K</option>
            <option>Q8_0</option><option>Q4_0</option><option>F16</option></select></div>
        <div><label for="f-backend">Backend</label>
          <select id="f-backend"><option value="auto">auto (by tier)</option>
            <option value="vulkan">vulkan</option><option value="rocm">rocm</option></select></div>
        <div class="check"><input id="f-moe" type="checkbox"><label for="f-moe" style="margin:0">Mixture-of-Experts</label></div>
        <div><button type="submit">Start endpoint</button></div>
      </form>
    </div>
  </section>

  <section>
    <h2>Playground</h2>
    <div class="body">
      <div style="display:flex;gap:8px;margin-bottom:10px;flex-wrap:wrap">
        <select id="pg-model" style="max-width:280px"></select>
        <input id="pg-input" placeholder="Ask a running model something…" style="flex:1;min-width:180px"
          onkeydown="if(event.key==='Enter')pgSend()">
        <button onclick="pgSend()">Send</button>
      </div>
      <div id="pg-out" class="muted" style="white-space:pre-wrap;min-height:1.5em">Start an endpoint, then chat with it here through the /v1 gateway.</div>
    </div>
  </section>

  <section>
    <h2>Model cache</h2>
    <div class="body"><div id="models"><div class="empty">loading…</div></div></div>
  </section>
</main>

<script>
const KEY_STORE='cameo_console_key';
const getKey=()=>localStorage.getItem(KEY_STORE)||'';

async function api(path,opts={}){
  opts.headers=Object.assign({'Content-Type':'application/json'},opts.headers||{});
  const k=getKey(); if(k) opts.headers['Authorization']='Bearer '+k;
  let r=await fetch(path,opts);
  if(r.status===401){
    const entered=prompt('This console requires a key. Enter the console key:');
    if(entered){localStorage.setItem(KEY_STORE,entered); return api(path,opts);}
  }
  return r;
}

function setStatus(ok,text){
  document.getElementById('dot').className='dot '+(ok?'ok':'bad');
  document.getElementById('status-text').textContent=text;
}
function flash(kind,msg){
  const f=document.getElementById('flash');
  f.className='flash '+kind; f.textContent=msg;
  if(kind==='ok') setTimeout(()=>{f.className='flash';},4000);
}
function esc(s){const d=document.createElement('div');d.textContent=s==null?'':String(s);return d.innerHTML;}

function tierNum(t){return t?Number(String(t).replace('Tier','')):3;}

async function loadGpus(){
  try{
    const r=await api('/api/gpus');
    const el=document.getElementById('gpus');
    if(!r.ok){const e=await r.json().catch(()=>({error:r.statusText}));
      el.innerHTML='<div class="empty">'+esc(e.error||'detection unavailable')+'</div>';
      document.getElementById('topo').textContent=''; return;}
    const d=await r.json();
    if(!d.gpus||!d.gpus.length){el.innerHTML='<div class="empty">no AMD GPU detected</div>';return;}
    el.innerHTML=d.gpus.map(a=>{
      const n=tierNum(a.tier), g=a.gpu||{};
      const vram=g.vram_mb!=null?g.vram_mb+' MiB':'unknown';
      const arch=g.gfx_arch||'unknown (no ROCm stack)';
      return `<div class="card"><h3>${esc(g.model||'GPU')} <span class="badge tier${n}">Tier ${n}</span></h3>
        <div class="kv">
          <span>pci</span><b>${esc(g.pci_id||'?')}</b>
          <span>vram</span><b>${esc(vram)}</b>
          <span>arch</span><b>${esc(arch)}</b>
          <span>training</span><b>${a.training_supported?'supported':'inference only'}</b>
          ${a.hsa_override?`<span>hsa</span><b>${esc(a.hsa_override)}</b>`:''}
        </div>
        <p class="notes">${esc(a.rationale||'')}</p></div>`;
    }).join('');
    let topo='';
    if(d.host_mem){const gb=b=>(b/1073741824).toFixed(1);
      topo+=`Host RAM: ${gb(d.host_mem.total_bytes)} GiB total, ${gb(d.host_mem.available_bytes)} GiB available. `;}
    if(d.links&&d.links.length){topo+='Links: '+d.links.map(l=>`GPU${l.a}↔GPU${l.b} ${l.kind}`).join(', ')+'. ';}
    if(d.bottleneck) topo+='Bottleneck: '+d.bottleneck+'.';
    document.getElementById('topo').textContent=topo;
    setStatus(true,'connected');
  }catch(e){setStatus(false,'offline'); }
}

async function loadModels(){
  const r=await api('/api/models'); if(!r.ok)return;
  const d=await r.json();
  const dl=document.getElementById('model-list');
  const names=[...new Set([...(d.cached||[]).map(f=>f.replace(/\.gguf$/,'')),...(d.aliases||[]).map(a=>a.name)])];
  dl.innerHTML=names.map(n=>`<option value="${esc(n)}">`).join('');
  const cached=(d.cached||[]);
  const el=document.getElementById('models');
  el.innerHTML=`<div class="kv" style="grid-template-columns:auto 1fr">
      <span>cache dir</span><b>${esc(d.models_dir||'')}</b></div>
    <p style="margin:12px 0 4px" class="muted">Cached (${cached.length}):</p>
    ${cached.length?cached.map(f=>`<div style="display:flex;justify-content:space-between;align-items:center;padding:3px 0">
        <b>${esc(f)}</b><button class="stop del" data-name="${esc(f)}">Delete</button></div>`).join('')
      :'<div class="empty">none — pull one with the CLI: <code>cameo pull tinyllama</code></div>'}
    <p style="margin:14px 0 4px" class="muted">Aliases:</p>
    <div class="kv">${(d.aliases||[]).map(a=>`<span>${esc(a.name)}</span><b class="muted">${esc(a.repo)}</b>`).join('')}</div>`;
  el.querySelectorAll('button.del').forEach(b=>b.onclick=()=>delModel(b.dataset.name));
}

async function delModel(name){
  if(!confirm('Delete cached model '+name+'?'))return;
  const r=await api('/api/models/'+encodeURIComponent(name),{method:'DELETE'});
  if(r.ok){flash('ok','Deleted '+name);}else{const e=await r.json().catch(()=>({}));flash('err',e.error||'delete failed');}
  loadModels();
}

async function loadPlayground(){
  const r=await api('/v1/models'); if(!r.ok)return;
  const d=await r.json(); const models=(d.data||[]).map(m=>m.id);
  const sel=document.getElementById('pg-model');
  const prev=sel.value;
  sel.innerHTML=models.length?models.map(m=>`<option>${esc(m)}</option>`).join('')
    :'<option value="">(no running endpoint)</option>';
  if(models.includes(prev))sel.value=prev;
}

async function pgSend(){
  const model=document.getElementById('pg-model').value;
  const input=document.getElementById('pg-input');
  const out=document.getElementById('pg-out');
  if(!model){out.textContent='Start an endpoint first, then pick its model here.';return;}
  const text=input.value.trim(); if(!text)return;
  out.textContent='…'; input.value='';
  try{
    const r=await api('/v1/chat/completions',{method:'POST',
      body:JSON.stringify({model,messages:[{role:'user',content:text}]})});
    const d=await r.json().catch(()=>({}));
    out.textContent=r.ok
      ?((d.choices&&d.choices[0]&&d.choices[0].message&&d.choices[0].message.content)||JSON.stringify(d))
      :('Error: '+(d.error||r.status));
  }catch(e){out.textContent='Error: '+e;}
}

function uptime(s){if(s<60)return s+'s'; if(s<3600)return Math.floor(s/60)+'m'; return Math.floor(s/3600)+'h'+Math.floor((s%3600)/60)+'m';}

async function loadServers(){
  const r=await api('/api/servers'); if(!r.ok)return;
  const d=await r.json(); const rows=d.servers||[];
  const tb=document.getElementById('servers'), tbl=document.getElementById('servers-table'), empty=document.getElementById('servers-empty');
  if(!rows.length){tbl.style.display='none'; empty.style.display='block'; return;}
  tbl.style.display=''; empty.style.display='none';
  tb.innerHTML=rows.map(s=>{
    const detail=s.state==='failed'?`<div class="muted" style="margin-top:4px">${esc(s.error||'')}</div>`
      :s.state==='exited'?`<div class="muted" style="margin-top:4px">exit ${s.exit_code==null?'?':s.exit_code}</div>`:'';
    const link=s.state==='running'?`<a href="${esc(s.endpoint)}" target="_blank">${esc(s.endpoint)}</a>`:esc(s.endpoint);
    return `<tr>
      <td><b>${esc(s.model)}</b><br><code>${esc(s.command)}</code>
        ${s.notes&&s.notes.length?'<ul class="notes">'+s.notes.map(n=>`<li>${esc(n)}</li>`).join('')+'</ul>':''}</td>
      <td><span class="badge st-${esc(s.state)}">${esc(s.state)}</span>${detail}</td>
      <td>${link}</td>
      <td>${esc(s.backend)}${s.fits_vram?'':' <span class="muted">(spills)</span>'}</td>
      <td>${s.state==='running'?uptime(s.uptime_secs||0):'—'}</td>
      <td><button class="stop" data-id="${esc(s.id)}">Stop</button></td>
    </tr>`;
  }).join('');
  tb.querySelectorAll('button.stop').forEach(b=>b.onclick=()=>stopServer(b.dataset.id));
}

async function stopServer(id){
  const r=await api('/api/servers/'+encodeURIComponent(id),{method:'DELETE'});
  if(r.ok){flash('ok','Stopped '+id);} else {const e=await r.json().catch(()=>({}));flash('err',e.error||'stop failed');}
  loadServers();
}

document.getElementById('start-form').addEventListener('submit',async ev=>{
  ev.preventDefault();
  const body={
    model:document.getElementById('f-model').value.trim(),
    host:document.getElementById('f-host').value.trim()||'127.0.0.1',
    port:Number(document.getElementById('f-port').value),
    params:Number(document.getElementById('f-params').value),
    quant:document.getElementById('f-quant').value,
    backend:document.getElementById('f-backend').value,
    moe:document.getElementById('f-moe').checked,
  };
  const r=await api('/api/servers',{method:'POST',body:JSON.stringify(body)});
  const d=await r.json().catch(()=>({}));
  if(r.ok){
    if(d.state==='failed') flash('err','Started but process failed: '+(d.error||'unknown'));
    else flash('ok','Endpoint '+d.id+' started');
  }else{ flash('err',d.error||('start failed ('+r.status+')')); }
  loadServers();
});

function refresh(){loadGpus();loadServers();loadPlayground();}
loadModels(); refresh();
setInterval(()=>{loadServers();loadPlayground();},4000);
</script>
</body>
</html>
"##;
