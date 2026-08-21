//! The dashboard: one self-contained HTML page served at `/`.
//!
//! Console (this node's GPUs, endpoints, playground) plus a fleet map (every
//! node, GPU, VRAM, loaded model, and who is using it). A hub sets
//! `healthz.hub`; the HTML is the same. No build step, no bundler.

/// The complete dashboard page. Embedded verbatim; served as `text/html`.
pub const INDEX_HTML: &str = r##"
<!doctype html>
<html lang="en" class="dark">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>CAMEO Console</title>
<style>
  :root{
    --bg:#0A0A0A; --panel:#1A1812; --panel-2:#201f1f; --panel-3:#161311;
    --line:#3F3F46; --line-2:#5d4038;
    --ink:#e5e2e1; --muted:#e7bdb2; --faint:#8b857c;
    --ember:#FF5625; --ember-soft:#ffb5a0;
    --t1:#4ADE80; --t2:#FACC15; --t3:#FF4500; --danger:#ffb4ab; --ok:#4ADE80;
    --disp:"Space Grotesk",system-ui,-apple-system,"Segoe UI",sans-serif;
    --mono:"JetBrains Mono",ui-monospace,"Cascadia Code",Consolas,monospace;
    --label:"IBM Plex Sans",system-ui,sans-serif;
  }
  *{box-sizing:border-box}
  html{-webkit-text-size-adjust:100%}
  body{margin:0;background:var(--bg);color:var(--ink);font-family:var(--mono);font-size:14px;line-height:1.6;
    -webkit-font-smoothing:antialiased;overflow-x:hidden;
    background-image:linear-gradient(rgba(255,255,255,.02) 1px,transparent 1px),linear-gradient(90deg,rgba(255,255,255,.02) 1px,transparent 1px);
    background-size:16px 16px}
  /* faint scanline over everything */
  body::after{content:"";position:fixed;inset:0;z-index:60;pointer-events:none;opacity:.5;
    background:repeating-linear-gradient(to bottom,transparent 0,transparent 3px,rgba(0,0,0,.10) 3px,rgba(0,0,0,.10) 4px)}
  a{color:var(--ember-soft);text-decoration:none}
  code{font-family:var(--mono)}
  ::selection{background:rgba(255,86,37,.35);color:#fff}
  ::-webkit-scrollbar{width:8px;height:8px}
  ::-webkit-scrollbar-track{background:var(--panel)}
  ::-webkit-scrollbar-thumb{background:var(--line)}
  ::-webkit-scrollbar-thumb:hover{background:var(--ember)}

  /* header */
  header{position:sticky;top:0;z-index:40;display:flex;align-items:center;gap:20px;height:60px;
    padding:0 28px;background:rgba(10,10,10,.9);backdrop-filter:blur(8px);border-bottom:1px solid var(--line)}
  .wordmark{font-family:var(--disp);font-weight:700;letter-spacing:.02em;font-size:20px;color:var(--ember-soft)}
  .tagline{font-family:var(--mono);font-size:12px;color:var(--faint);letter-spacing:.04em}
  .status{margin-left:auto;display:flex;align-items:center;gap:8px;font-size:11px;font-weight:700;
    letter-spacing:.1em;text-transform:uppercase;color:var(--muted);border:1px solid var(--line);
    background:var(--panel);padding:5px 11px}
  .dot{width:8px;height:8px;border-radius:50%;background:var(--faint)}
  .dot.ok{background:var(--ok);box-shadow:0 0 8px rgba(74,222,128,.7);animation:pulse 2.2s ease-in-out infinite}
  .dot.bad{background:var(--danger);box-shadow:0 0 8px rgba(255,180,171,.6)}
  @keyframes pulse{50%{opacity:.5}}

  /* tabs */
  nav.tabs{display:flex;gap:2px;padding:0 24px;background:var(--bg);border-bottom:1px solid var(--line);position:sticky;top:60px;z-index:39}
  nav.tabs button{background:transparent;color:var(--faint);border:none;border-bottom:2px solid transparent;
    font-family:var(--label);font-weight:600;font-size:12px;letter-spacing:.08em;text-transform:uppercase;
    padding:12px 16px;cursor:pointer;transition:color .15s}
  nav.tabs button:hover{color:var(--muted)}
  nav.tabs button.on{color:var(--ember-soft);border-bottom-color:var(--ember)}

  main{max-width:1280px;margin-inline:auto;padding:28px}
  #view-console.hide{display:none}
  #view-deck{display:none}
  #view-deck.show{display:grid}

  /* panels */
  section{background:var(--panel);border:1px solid var(--line);position:relative;margin-bottom:20px}
  section::before,section::after{content:"";position:absolute;width:9px;height:9px;border:2px solid var(--ember);opacity:.5;pointer-events:none}
  section::before{top:-1px;left:-1px;border-width:2px 0 0 2px}
  section::after{bottom:-1px;right:-1px;border-width:0 2px 2px 0}
  section>h2{margin:0;padding:12px 18px;font-family:var(--label);font-weight:600;font-size:12px;letter-spacing:.1em;
    text-transform:uppercase;color:var(--muted);border-bottom:1px solid var(--line);background:var(--panel-3);
    display:flex;align-items:center;gap:10px}
  section>h2 .hash{color:var(--ember)}
  .body{padding:18px}

  /* GPU tanks */
  .gpus{display:grid;gap:16px;grid-template-columns:repeat(auto-fill,minmax(220px,1fr))}
  .gpu{background:var(--panel-2);border:1px solid var(--line);padding:14px}
  .gpu .hd{display:flex;justify-content:space-between;align-items:center;margin-bottom:12px;gap:8px}
  .gpu .name{font-size:13px;color:var(--ink);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
  .gpu .name b{color:var(--faint);font-weight:400}
  .tierbadge{flex:none;font-size:10px;font-weight:700;letter-spacing:.06em;text-transform:uppercase;padding:2px 7px;border:1px solid}
  .tier1{color:var(--t1);border-color:rgba(74,222,128,.4);background:rgba(74,222,128,.1)}
  .tier2{color:var(--t2);border-color:rgba(250,204,21,.4);background:rgba(250,204,21,.1)}
  .tier3{color:var(--t3);border-color:rgba(255,69,0,.4);background:rgba(255,69,0,.1)}
  .cup{width:100%;height:auto;display:block}
  .fillrise{transform:scaleX(0);transform-box:fill-box;transform-origin:0% 50%;transition:transform 1.1s cubic-bezier(.2,.8,.2,1)}
  .gpu.on .fillrise{transform:scaleX(1)}
  .gpu .rd{display:flex;justify-content:space-between;align-items:baseline;margin-top:12px;border-top:1px solid var(--line);padding-top:10px}
  .gpu .rd .lab{font-size:10px;letter-spacing:.1em;color:var(--faint);text-transform:uppercase}
  .gpu .rd .gb{font-size:12.5px;color:var(--muted)}
  .gpu .rd .pct{font-family:var(--disp);font-weight:700;font-size:22px;line-height:1}
  .gpu .rd .pct.none{font-family:var(--mono);font-size:11px;font-weight:400;color:var(--faint)}
  .gpu .why{margin:10px 0 0;font-size:11.5px;color:var(--faint);line-height:1.5;
    border-left:2px solid var(--line);padding-left:8px}
  .topo{margin-top:14px;font-size:12px;color:var(--faint)}

  /* table */
  table{width:100%;border-collapse:collapse}
  th,td{text-align:left;padding:10px 12px;border-bottom:1px solid var(--line);font-size:13px;vertical-align:top}
  th{font-family:var(--label);font-weight:600;font-size:11px;letter-spacing:.06em;text-transform:uppercase;color:var(--faint);background:var(--panel-3)}
  tbody tr:hover{background:rgba(255,255,255,.02)}
  td code{font-size:11.5px;color:var(--faint);word-break:break-all}
  .badge{font-size:10px;font-weight:700;letter-spacing:.04em;text-transform:uppercase;padding:2px 7px;border:1px solid}
  .st-running{color:var(--ok);border-color:rgba(74,222,128,.4);background:rgba(74,222,128,.1)}
  .st-exited{color:var(--faint);border-color:var(--line);background:var(--panel-3)}
  .st-failed{color:var(--danger);border-color:rgba(255,180,171,.4);background:rgba(255,180,171,.1)}
  .muted{color:var(--faint)}
  .empty{color:var(--faint);font-style:italic;padding:10px 0}
  .notes{margin:6px 0 0;padding-left:16px;color:var(--faint);font-size:12px}

  /* forms */
  form{display:grid;grid-template-columns:repeat(auto-fit,minmax(150px,1fr));gap:14px;align-items:end}
  label{display:block;font-family:var(--label);font-size:11px;font-weight:600;letter-spacing:.06em;text-transform:uppercase;color:var(--faint);margin-bottom:5px}
  input,select{width:100%;background:var(--panel-3);border:1px solid var(--line);color:var(--ink);
    font-family:var(--mono);font-size:13px;padding:9px 10px;border-radius:0}
  input:focus,select:focus{outline:none;border-color:var(--ember)}
  .check{display:flex;align-items:center;gap:8px}
  .check input{width:auto}
  .check label{margin:0;text-transform:none;letter-spacing:0;font-family:var(--mono);font-size:13px;color:var(--muted)}
  button{font-family:var(--label);font-weight:600;font-size:12px;letter-spacing:.06em;text-transform:uppercase;
    background:var(--ember);color:#180a05;border:none;padding:10px 18px;cursor:pointer;transition:filter .15s}
  button:hover{filter:brightness(1.08)}
  button.ghost{background:transparent;color:var(--muted);border:1px solid var(--line)}
  button.ghost:hover{border-color:var(--ember);color:var(--ember-soft);filter:none}
  button.stop{background:transparent;color:var(--danger);border:1px solid rgba(255,180,171,.4);padding:5px 12px;font-size:11px}
  button.stop:hover{background:rgba(255,180,171,.1);filter:none}
  .flash{display:none;padding:10px 14px;margin-bottom:14px;font-size:13px;border:1px solid}
  .flash.err{display:block;background:rgba(255,86,37,.08);color:var(--ember-soft);border-color:var(--ember)}
  .flash.ok{display:block;background:rgba(74,222,128,.08);color:var(--ok);border-color:rgba(74,222,128,.4)}
  #key-bar{display:none;gap:10px;align-items:center;flex-wrap:wrap;padding:10px 28px;background:var(--panel);border-bottom:1px solid var(--line)}
  #key-bar.show{display:flex}
  #key-bar span{color:var(--muted);font-size:13px}
  #key-bar input{max-width:280px}

  /* playground */
  .pg-row{display:flex;gap:8px;margin-bottom:10px;flex-wrap:wrap}
  .pg-row select{max-width:280px}
  .pg-row input{flex:1;min-width:180px}
  #pg-out{white-space:pre-wrap;min-height:1.5em;color:var(--muted);background:var(--panel-3);border:1px solid var(--line);padding:12px 14px;font-size:13px}

  /* model cache */
  .kv{display:grid;grid-template-columns:auto 1fr;gap:3px 12px;font-size:12.5px;color:var(--faint)}
  .kv b{color:var(--ink);font-weight:500}
  .mrow{display:flex;justify-content:space-between;align-items:center;padding:5px 0;border-bottom:1px solid var(--line)}

  /* deck view */
  #view-deck{grid-template-columns:240px 1fr 260px;gap:0;height:calc(100vh - 122px);margin-top:0}
  .rail{background:var(--panel);border-right:1px solid var(--line);overflow:auto;padding:16px}
  .rail.r{border-right:none;border-left:1px solid var(--line)}
  .rail h3{margin:0 0 10px;font-family:var(--label);font-size:11px;font-weight:600;letter-spacing:.08em;text-transform:uppercase;color:var(--faint)}
  #field{position:relative;overflow:auto;background:radial-gradient(ellipse at 50% 30%,#1b1712 0%,var(--bg) 72%)}
  #field .res,#field .unit{position:absolute;min-width:120px;border:1px solid var(--line);background:var(--panel);padding:10px 12px;cursor:pointer;font-size:12px}
  #field .unit{border-color:var(--line-2)}
  #field .unit.write{box-shadow:0 0 0 1px var(--ember)}
  #field .res b,#field .unit b{color:var(--ink)}
  .meter{height:6px;background:var(--panel-3);margin-top:6px;overflow:hidden}
  .meter>i{display:block;height:100%;background:var(--ember);width:0}
  .dcard{background:var(--panel-2);border:1px solid var(--line);padding:10px;margin-bottom:8px;font-size:12px}
  .nmap{display:grid;gap:14px;grid-template-columns:repeat(auto-fill,minmax(280px,1fr));padding:18px;align-content:start}
  .ncard{background:var(--panel);border:1px solid var(--line);padding:14px;cursor:pointer}
  .ncard:hover,.ncard.on{border-color:var(--ember)}
  .ncard.offline{opacity:.55}
  .ncard h3{margin:0 0 8px;font-size:14px;display:flex;align-items:center;gap:8px}
  .ncard h3 .addr{margin-left:auto;font-size:11px;color:var(--faint);font-weight:400}
  .nchips{display:flex;flex-wrap:wrap;gap:6px;margin:8px 0}
  .nchip{background:var(--panel-2);border:1px solid var(--line);padding:3px 8px;font-size:11px}
  .ncard .who{font-size:12px;color:var(--muted);margin-top:8px}
  #view-deck .serve{display:flex;gap:8px;margin-top:12px;flex-wrap:wrap}
  #view-deck .serve input{flex:1;min-width:80px}
  @media(max-width:820px){#view-deck.show{grid-template-columns:1fr;height:auto}.rail{border:1px solid var(--line);border-top:none}}
</style>
</head>
<body>
<header>
  <span class="wordmark">CAMEO</span>
  <span class="tagline">any AMD card → a working LLM box</span>
  <span class="status"><span class="dot" id="dot"></span><span id="status-text">connecting…</span></span>
</header>
<div id="key-bar">
  <span>This console needs the key printed at login (<code>cameo-hello</code>).</span>
  <input id="key-input" type="password" placeholder="console key" autocomplete="off">
  <button type="button" id="key-save">Unlock</button>
</div>
<nav class="tabs">
  <button id="tab-console" class="on" onclick="showView('console')">Console</button>
  <button id="tab-deck" onclick="showView('deck')">Fleet</button>
</nav>

<main>
<div id="view-console">
  <section>
    <h2><span class="hash">01</span> Compute · GPUs &amp; tiers</h2>
    <div class="body">
      <div id="gpus" class="gpus"><div class="empty">loading…</div></div>
      <div id="topo" class="topo"></div>
    </div>
  </section>

  <section>
    <h2><span class="hash">02</span> Endpoints</h2>
    <div class="body">
      <div id="flash" class="flash"></div>
      <table id="servers-table" style="display:none">
        <thead><tr><th>Model</th><th>State</th><th>Endpoint</th><th>Backend</th><th>Uptime</th><th></th></tr></thead>
        <tbody id="servers"></tbody>
      </table>
      <div id="servers-empty" class="empty">
        No model running yet.
        <div style="margin-top:12px"><button type="button" id="start-starter">Start qwen2.5-0.5b and chat</button></div>
        <div class="muted" style="margin-top:8px">Smoke-test model (~0.5B). Pull a larger GGUF when you have a network.</div>
      </div>
    </div>
  </section>

  <section>
    <h2><span class="hash">03</span> Start an endpoint</h2>
    <div class="body">
      <form id="start-form">
        <div><label for="f-model">Model</label>
          <input id="f-model" list="model-list" placeholder="qwen2.5-0.5b" value="qwen2.5-0.5b" required>
          <datalist id="model-list"></datalist></div>
        <div><label for="f-port">Port</label><input id="f-port" type="number" value="8080" min="1" max="65535"></div>
        <div><label for="f-host">Bind host</label><input id="f-host" value="127.0.0.1"></div>
        <div><label for="f-params">Params (B)</label><input id="f-params" type="number" step="0.1" value="0.5"></div>
        <div><label for="f-quant">Quant</label>
          <select id="f-quant"><option>Q4_K_M</option><option>Q5_K_M</option><option>Q6_K</option>
            <option>Q8_0</option><option>Q4_0</option><option>F16</option></select></div>
        <div><label for="f-backend">Backend</label>
          <select id="f-backend"><option value="auto">auto (by tier)</option>
            <option value="vulkan">vulkan</option><option value="rocm">rocm</option>
            <option value="cpu">cpu (system RAM)</option></select></div>
        <div class="check"><input id="f-moe" type="checkbox"><label for="f-moe">Mixture-of-Experts</label></div>
        <div><button type="submit">Start endpoint</button></div>
      </form>
    </div>
  </section>

  <section>
    <h2><span class="hash">04</span> Playground</h2>
    <div class="body">
      <div class="pg-row">
        <select id="pg-model"></select>
        <input id="pg-input" placeholder="Ask a running model something…" onkeydown="if(event.key==='Enter')pgSend()">
        <button onclick="pgSend()">Send</button>
      </div>
      <div id="pg-out">Start the starter model (button above), then type here.</div>
    </div>
  </section>

  <section>
    <h2><span class="hash">05</span> Model cache</h2>
    <div class="body"><div id="models"><div class="empty">loading…</div></div></div>
  </section>
</div>

<div id="view-deck">
  <aside class="rail">
    <h3>Nodes</h3>
    <div id="deck-nodes"></div>
  </aside>
  <div id="field" title="Every node, GPU, VRAM, loaded model, and who is using it"></div>
  <aside class="rail r">
    <div id="deck-flash" class="flash"></div>
    <h3>Selected</h3>
    <div id="detail"><div class="empty">Click a node, GPU, or session.</div></div>
    <h3 style="margin-top:18px">Serve on this node</h3>
    <div class="serve">
      <input id="deck-model" placeholder="model" list="model-list">
      <input id="deck-port" value="8080" style="flex:0 0 70px">
      <button id="deck-serve" type="button">Serve</button>
    </div>
    <h3 style="margin-top:18px">Who</h3>
    <div id="deck-sessions" class="muted">No harness heartbeats yet.</div>
  </aside>
</div>
</main>

<script>
const KEY_STORE='cameo_console_key';
const getKey=()=>localStorage.getItem(KEY_STORE)||'';

async function api(path,opts={}){
  opts.headers=Object.assign({'Content-Type':'application/json'},opts.headers||{});
  const k=getKey(); if(k) opts.headers['Authorization']='Bearer '+k;
  let r=await fetch(path,opts);
  if(r.status===401){
    document.getElementById('key-bar').classList.add('show');
    document.getElementById('key-input').focus();
    return r;
  }
  return r;
}
document.getElementById('key-save').onclick=()=>{
  const v=document.getElementById('key-input').value.trim();
  if(!v)return;
  localStorage.setItem(KEY_STORE,v);
  document.getElementById('key-bar').classList.remove('show');
  boot();
};
document.getElementById('key-input').addEventListener('keydown',e=>{
  if(e.key==='Enter') document.getElementById('key-save').click();
});

function setStatus(ok,text){
  document.getElementById('dot').className='dot '+(ok?'ok':'bad');
  document.getElementById('status-text').textContent=text;
}
function flash(kind,msg){
  const deckOn=document.getElementById('view-deck').classList.contains('show');
  const f=document.getElementById(deckOn?'deck-flash':'flash')||document.getElementById('flash');
  f.className='flash '+kind; f.textContent=msg;
  if(kind==='ok') setTimeout(()=>{f.className='flash';},4000);
}
function esc(s){return String(s==null?'':s)
  .replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;')
  .replace(/"/g,'&quot;').replace(/'/g,'&#39;');}
function tierNum(t){return t?Number(String(t).replace('Tier','')):3;}

/* The GPU glyph from the landing page, turned into a VRAM tank: the card body
   fills from the bottom to the fraction of VRAM in use, tier-coloured, with the
   fans sitting over the fill like submerged hardware. Fed by real sysfs data
   (vram_used_mb / vram_mb); when used-VRAM is unavailable it renders an empty
   tank labelled "no telemetry" rather than inventing a level. */
const TC={1:'#4ADE80',2:'#FACC15',3:'#FF4500'};
function gpuCup(idx,tier,usedMb,totalMb){
  const col=TC[tier]||TC[3], id='g'+idx;
  const known = usedMb!=null && totalMb!=null && totalMb>0;
  const pct = known ? Math.max(0,Math.min(1,usedMb/totalMb)) : 0;
  const X=12,Y=12,W=176,H=96,R=8,BOT=Y+H;
  const fillW=W*pct, fillRight=X+fillW;
  const cy=Y+H/2, fr=24;
  const fan=cx=>('<circle cx="'+cx+'" cy="'+cy+'" r="'+fr+'" fill="none" stroke="rgba(255,255,255,.26)" stroke-width="1.6"/>'
    +'<path d="M'+cx+' '+(cy-fr)+'V'+(cy+fr)+' M'+(cx-fr*0.7)+' '+(cy-fr*0.7)+' '+(cx+fr*0.7)+' '+(cy+fr*0.7)+' M'+(cx-fr*0.7)+' '+(cy+fr*0.7)+' '+(cx+fr*0.7)+' '+(cy-fr*0.7)+'" stroke="rgba(255,255,255,.22)" stroke-width="1.5" fill="none"/>'
    +'<circle cx="'+cx+'" cy="'+cy+'" r="4.5" fill="rgba(255,255,255,.5)"/>');
  return '<svg class="cup" viewBox="0 0 200 130" role="img" aria-label="VRAM '+Math.round(pct*100)+'%">'
    +'<defs><clipPath id="clip'+id+'"><rect x="'+X+'" y="'+Y+'" width="'+W+'" height="'+H+'" rx="'+R+'"/></clipPath>'
    +'<linearGradient id="grad'+id+'" x1="0" y1="0" x2="1" y2="0">'
    +'<stop offset="0" stop-color="'+col+'" stop-opacity="0.92"/><stop offset="1" stop-color="'+col+'" stop-opacity="0.42"/></linearGradient></defs>'
    +'<rect x="'+X+'" y="'+Y+'" width="'+W+'" height="'+H+'" rx="'+R+'" fill="#161311"/>'
    +(known?('<g clip-path="url(#clip'+id+')"><g class="fillrise">'
      +'<rect x="'+X+'" y="'+Y+'" width="'+fillW.toFixed(1)+'" height="'+H+'" fill="url(#grad'+id+')"/>'
      +'<rect x="'+(fillRight-1.5).toFixed(1)+'" y="'+Y+'" width="3" height="'+H+'" fill="rgba(255,255,255,.8)"/>'
      +'</g></g>'):'')
    +'<line x1="'+(X+13)+'" y1="'+Y+'" x2="'+(X+13)+'" y2="'+BOT+'" stroke="rgba(255,255,255,.13)" stroke-width="1.4"/>'
    +fan(76)+fan(148)
    +'<rect x="'+X+'" y="'+Y+'" width="'+W+'" height="'+H+'" rx="'+R+'" fill="none" stroke="'+col+'" stroke-width="2"/>'
    +'</svg>';
}

async function loadGpus(){
  try{
    const r=await api('/api/gpus');
    const el=document.getElementById('gpus');
    if(!r.ok){const e=await r.json().catch(()=>({error:r.statusText}));
      el.innerHTML='<div class="empty">'+esc(e.error||'detection unavailable')+'</div>';
      document.getElementById('topo').textContent=''; return;}
    const d=await r.json();
    if(!d.gpus||!d.gpus.length){el.innerHTML='<div class="empty">no AMD GPU detected</div>';return;}
    el.innerHTML=d.gpus.map((a,i)=>{
      const n=tierNum(a.tier), g=a.gpu||{};
      const total=g.vram_mb, used=g.vram_used_mb;
      const col=TC[n]||TC[3];
      const known = used!=null && total!=null && total>0;
      const pct = known ? Math.round(100*used/total) : null;
      const gbUsed = used!=null ? (used/1024).toFixed(1) : '?';
      const gbTot  = total!=null ? (total/1024).toFixed(1) : '?';
      const rd = known
        ? '<div><div class="lab">VRAM used</div><div class="gb">'+gbUsed+' / '+gbTot+' GB</div></div><div class="pct" style="color:'+col+'">'+pct+'<span style="font-size:13px">%</span></div>'
        : '<div><div class="lab">VRAM total</div><div class="gb">'+gbTot+' GB</div></div><div class="pct none">no telemetry</div>';
      return '<div class="gpu" data-i="'+i+'">'
        +'<div class="hd"><span class="name">'+esc(g.model||'GPU')+' <b>·</b> '+esc(g.gfx_arch||'gfx?')+'</span>'
        +'<span class="tierbadge tier'+n+'">Tier '+n+'</span></div>'
        +gpuCup(i,n,used,total)
        +'<div class="rd">'+rd+'</div>'
        +(a.rationale?'<p class="why">'+esc(a.rationale)+'</p>':'')
        +'</div>';
    }).join('');
    // trigger the fill animations
    requestAnimationFrame(()=>requestAnimationFrame(()=>{
      el.querySelectorAll('.gpu').forEach((c,i)=>setTimeout(()=>c.classList.add('on'),80+i*120));
    }));
    let topo='';
    if(d.host_mem){const gb=b=>(b/1073741824).toFixed(1);
      topo+='Host RAM: '+gb(d.host_mem.total_bytes)+' GiB total, '+gb(d.host_mem.available_bytes)+' GiB available. ';}
    if(d.links&&d.links.length){topo+='Links: '+d.links.map(l=>'GPU'+l.a+'↔GPU'+l.b+' '+l.kind).join(', ')+'. ';}
    if(d.bottleneck) topo+='Bottleneck: '+d.bottleneck+'.';
    document.getElementById('topo').textContent=topo;
    setStatus(true,'connected');
  }catch(e){setStatus(false,'offline');}
}

async function loadModels(){
  const r=await api('/api/models'); if(!r.ok)return;
  const d=await r.json();
  const dl=document.getElementById('model-list');
  const names=[...new Set([...(d.cached||[]).map(f=>f.replace(/\.gguf$/,'')),...(d.aliases||[]).map(a=>a.name)])];
  dl.innerHTML=names.map(n=>'<option value="'+esc(n)+'">').join('');
  const cached=(d.cached||[]);
  const el=document.getElementById('models');
  el.innerHTML='<div class="kv" style="grid-template-columns:auto 1fr"><span>cache dir</span><b>'+esc(d.models_dir||'')+'</b></div>'
    +'<p style="margin:12px 0 4px" class="muted">Cached ('+cached.length+'):</p>'
    +(cached.length?cached.map(f=>'<div class="mrow"><b>'+esc(f)+'</b><button class="stop del" data-name="'+esc(f)+'">Delete</button></div>').join('')
      :'<div class="empty">none yet — a starter should appear after boot. Or drop a .gguf into the cache dir.</div>')
    +'<p style="margin:14px 0 4px" class="muted">Aliases:</p>'
    +'<div class="kv">'+(d.aliases||[]).map(a=>'<span>'+esc(a.name)+'</span><b class="muted">'+esc(a.repo)+'</b>').join('')+'</div>';
  el.querySelectorAll('button.del').forEach(b=>b.onclick=()=>delModel(b.dataset.name));
  const modelEl=document.getElementById('f-model');
  const paramEl=document.getElementById('f-params');
  const byName={};
  (d.aliases||[]).forEach(a=>{if(a.params_b!=null)byName[a.name]=a.params_b;});
  function syncParams(){
    const key=(modelEl.value||'').replace(/\.gguf$/,'');
    if(byName[key]!=null) paramEl.value=byName[key];
  }
  modelEl.onchange=syncParams;
  modelEl.oninput=syncParams;
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
  const sel=document.getElementById('pg-model'); const prev=sel.value;
  sel.innerHTML=models.length?models.map(m=>'<option>'+esc(m)+'</option>').join(''):'<option value="">(no running endpoint)</option>';
  if(models.includes(prev))sel.value=prev;
}
async function pgSend(){
  const model=document.getElementById('pg-model').value;
  const input=document.getElementById('pg-input'); const out=document.getElementById('pg-out');
  if(!model){out.textContent='Start an endpoint first, then pick its model here.';return;}
  const text=input.value.trim(); if(!text)return;
  out.textContent='…'; input.value='';
  try{
    const r=await api('/v1/chat/completions',{method:'POST',body:JSON.stringify({model,messages:[{role:'user',content:text}]})});
    const d=await r.json().catch(()=>({}));
    out.textContent=r.ok?((d.choices&&d.choices[0]&&d.choices[0].message&&d.choices[0].message.content)||JSON.stringify(d)):('Error: '+((d.error&&(d.error.message||d.error))||r.status));
  }catch(e){out.textContent='Error: '+e;}
}
function uptime(s){if(s<60)return s+'s'; if(s<3600)return Math.floor(s/60)+'m'; return Math.floor(s/3600)+'h'+Math.floor((s%3600)/60)+'m';}
async function loadServers(){
  const r=await api('/api/servers'); if(!r.ok)return;
  const d=await r.json(); const rows=d.servers||[];
  const tb=document.getElementById('servers'),tbl=document.getElementById('servers-table'),empty=document.getElementById('servers-empty');
  if(!rows.length){tbl.style.display='none'; empty.style.display='block'; return;}
  tbl.style.display=''; empty.style.display='none';
  tb.innerHTML=rows.map(s=>{
    const detail=s.state==='failed'?'<div class="muted" style="margin-top:4px">'+esc(s.error||'')+'</div>'
      :s.state==='exited'?'<div class="muted" style="margin-top:4px">exit '+(s.exit_code==null?'?':s.exit_code)+'</div>':'';
    const link=s.state==='running'?'<a href="'+esc(s.endpoint)+'" target="_blank">'+esc(s.endpoint)+'</a>':esc(s.endpoint);
    return '<tr><td><b>'+esc(s.model)+'</b><br><code>'+esc(s.command)+'</code>'
      +(s.notes&&s.notes.length?'<ul class="notes">'+s.notes.map(n=>'<li>'+esc(n)+'</li>').join('')+'</ul>':'')+'</td>'
      +'<td><span class="badge st-'+esc(s.state)+'">'+esc(s.state)+'</span>'+detail+'</td>'
      +'<td>'+link+'</td>'
      +'<td>'+esc(s.backend)+(s.fits_vram?'':' <span class="muted">(spills)</span>')+'</td>'
      +'<td>'+(s.state==='running'?uptime(s.uptime_secs||0):'—')+'</td>'
      +'<td><button class="stop" data-id="'+esc(s.id)+'">Stop</button></td></tr>';
  }).join('');
  tb.querySelectorAll('button.stop').forEach(b=>b.onclick=()=>stopServer(b.dataset.id));
}
async function stopServer(id){
  const r=await api('/api/servers/'+encodeURIComponent(id),{method:'DELETE'});
  if(r.ok){flash('ok','Stopped '+id);}else{const e=await r.json().catch(()=>({}));flash('err',e.error||'stop failed');}
  loadServers();
}
document.getElementById('start-form').addEventListener('submit',async ev=>{
  ev.preventDefault();
  const body={model:document.getElementById('f-model').value.trim(),host:document.getElementById('f-host').value.trim()||'127.0.0.1',
    port:Number(document.getElementById('f-port').value),params:Number(document.getElementById('f-params').value),
    quant:document.getElementById('f-quant').value,backend:document.getElementById('f-backend').value,moe:document.getElementById('f-moe').checked};
  const r=await api('/api/servers',{method:'POST',body:JSON.stringify(body)});
  const d=await r.json().catch(()=>({}));
  if(r.ok){if(d.state==='failed') flash('err','Started but process failed: '+(d.error||'unknown')); else flash('ok','Endpoint '+d.id+' started');}
  else{flash('err',d.error||('start failed ('+r.status+')'));}
  loadServers(); loadPlayground();
});
async function startStarter(){
  document.getElementById('f-model').value='qwen2.5-0.5b';
  document.getElementById('f-params').value='0.5';
  const r=await api('/api/servers',{method:'POST',body:JSON.stringify({
    model:'qwen2.5-0.5b',host:'127.0.0.1',port:8080,params:0.5,quant:'Q4_K_M',backend:'auto',moe:false
  })});
  const d=await r.json().catch(()=>({}));
  if(r.status===401) return;
  if(r.ok){
    if(d.state==='failed') flash('err','Started but process failed: '+(d.error||'unknown'));
    else flash('ok','Starter running — type below');
  } else flash('err',d.error||('start failed ('+r.status+')'));
  await loadServers(); await loadPlayground();
  const pg=document.getElementById('pg-input'); pg.focus();
  document.getElementById('pg-out').scrollIntoView({behavior:'smooth',block:'center'});
}
document.getElementById('start-starter').onclick=startStarter;

function showView(name){
  document.getElementById('view-deck').classList.toggle('show', name==='deck');
  document.getElementById('view-console').classList.toggle('hide', name==='deck');
  document.getElementById('tab-deck').classList.toggle('on', name==='deck');
  document.getElementById('tab-console').classList.toggle('on', name==='console');
}

let IS_HUB=false;
let selected=null;
let selectedNode=null;
let fleetNodes=[];

function vramLabel(g){
  const tot=g.vram_mb!=null?g.vram_mb:(g.vram!=null?g.vram:null);
  const used=g.vram_used_mb!=null?g.vram_used_mb:(g.vram_used!=null?g.vram_used:null);
  if(tot==null) return '? MiB';
  return (used!=null?used+'/':'')+tot+' MiB';
}
function gpuChips(gpus){
  if(!gpus||!gpus.length) return '<span class="nchip muted">CPU-only</span>';
  return gpus.map(g=>{
    const t=g.tier!=null?tierNum(g.tier):'';
    const tot=g.vram_mb!=null?g.vram_mb:g.vram;
    const used=g.vram_used_mb!=null?g.vram_used_mb:g.vram_used;
    const pct=(used!=null&&tot)?Math.min(100,100*used/tot):0;
    return '<span class="nchip">'+(esc(g.model||g.label||'GPU'))
      +(t?(' T'+t):'')+' · '+esc(vramLabel(g))
      +'<div class="meter"><i style="width:'+pct+'%"></i></div></span>';
  }).join('');
}
function whoLine(sessions, endpoints){
  const who=(sessions||[]).map(s=>s.name||s.label||s.id).filter(Boolean);
  const models=(endpoints||[]).map(e=>e.model||e.label).filter(Boolean);
  const bits=[];
  if(models.length) bits.push(models.join(', '));
  if(who.length) bits.push('who: '+who.join(', '));
  return bits.join(' · ');
}
function pick(ent){
  selected=ent; const el=document.getElementById('detail');
  if(!ent){el.innerHTML='<div class="empty">Click a node, GPU, or session.</div>';return;}
  if(ent.kind==='node'||ent.node_id) selectedNode=ent;
  const rows=[];
  for(const [k,v] of Object.entries(ent)){
    if(k==='plugin'||v==null||v===''||typeof v==='object') continue;
    rows.push('<span>'+esc(k)+'</span><b>'+esc(v)+'</b>');
  }
  el.innerHTML='<div class="muted">'+esc(ent.kind||'node')+'</div><h3 style="margin:6px 0">'+esc(ent.label||ent.name||ent.id)+'</h3><div class="kv">'+rows.join('')+'</div>';
}

async function localNode(){
  const n={node_id:'local',name:'this-node',online:true,local:true,kind:'node',gpus:[],endpoints:[],sessions:[],cameo_version:''};
  try{const r=await api('/api/node');if(r.ok){const d=await r.json();
    n.name=d.name||n.name; n.cameo_version=d.cameo_version||'';
    n.endpoints=d.endpoints||[]; n.sessions=d.sessions||[];
    n.gpus=(d.gpus||[]).map(a=>{const g=a.gpu||{};
      return {model:g.model||'GPU',vram_mb:g.vram_mb,vram_used_mb:g.vram_used_mb,tier:tierNum(a.tier)};});
  }}catch(e){}
  return n;
}
async function remoteNodes(){
  if(!IS_HUB) return [];
  try{const r=await api('/hub/nodes'); if(!r.ok) return []; const d=await r.json();
    return (d.nodes||[]).map(x=>({...x, kind:'node', local:false, label:x.name}));
  }catch(e){return [];}
}
function mergeFleet(local, remotes){
  const out=[local];
  for(const r of remotes){
    if(r.name && r.name===local.name) continue;
    out.push(r);
  }
  return out;
}
function nodeCard(n, i){
  const on=selectedNode && selectedNode.node_id===n.node_id;
  const eps=n.endpoints||[];
  const sess=n.sessions||[];
  const who=whoLine(sess, eps);
  const models=eps.length?eps.map(e=>'<span class="nchip"><code>'+esc(e.model||e.label||'')+'</code> <span class="badge st-'+esc(e.state||'')+'">'+esc(e.state||'')+'</span></span>').join(''):'<span class="nchip muted">no model loaded</span>';
  return '<div class="ncard '+(n.online===false?'offline':'')+(on?' on':'')+'" data-i="'+i+'">'
    +'<h3><span class="dot '+(n.online===false?'bad':'ok')+'"></span>'+esc(n.name||n.node_id)
    +(n.local?'<span class="badge">this box</span>':'')
    +'<span class="addr">'+esc(n.address||'')+'</span></h3>'
    +'<div class="nchips">'+gpuChips(n.gpus)+'</div>'
    +'<div class="nchips">'+models+'</div>'
    +(who?'<div class="who">'+esc(who)+'</div>':'')
    +'</div>';
}
function layoutFleet(nodes){
  fleetNodes=nodes;
  const field=document.getElementById('field');
  if(!nodes.length){
    field.innerHTML='<div class="empty" style="padding:24px">No nodes yet. Load a model on Console, or boot another Cameo box with CAMEO_HUB_URL.</div>';
  } else {
    field.innerHTML='<div class="nmap">'+nodes.map(nodeCard).join('')+'</div>';
    field.querySelectorAll('.ncard').forEach(c=>c.onclick=()=>pick(nodes[Number(c.dataset.i)]));
  }
  const rail=document.getElementById('deck-nodes');
  rail.innerHTML=nodes.map((n,i)=>'<div class="dcard" data-i="'+i+'"><b>'+esc(n.name||n.node_id)+'</b><div class="muted">'+(n.online===false?'offline':'online')+(n.local?' · this box':'')+'</div></div>').join('');
  rail.querySelectorAll('.dcard').forEach(c=>c.onclick=()=>pick(nodes[Number(c.dataset.i)]));
  const focus=selectedNode?nodes.find(n=>n.node_id===selectedNode.node_id):nodes[0];
  const units=focus?(focus.sessions||[]):[];
  const sEl=document.getElementById('deck-sessions');
  sEl.innerHTML=units.length?units.map(u=>'<div class="dcard">'+esc(u.name||u.label||u.id)+' <span class="muted">'+esc(u.mode||'')+' · '+esc(u.model||'')+'</span></div>').join(''):'<div class="empty">No harness heartbeats yet. Point Knossos at this node.</div>';
  if(focus) pick(focus);
}
async function tickDeck(){
  const local=await localNode();
  const remotes=await remoteNodes();
  layoutFleet(mergeFleet(local, remotes));
}
document.getElementById('deck-serve').onclick=async()=>{
  const model=document.getElementById('deck-model').value.trim();
  const port=Number(document.getElementById('deck-port').value)||8080;
  if(!model){flash('err','Enter a model name to serve.');return;}
  const n=selectedNode||fleetNodes[0];
  let r;
  if(!n||n.local||n.node_id==='local'||!IS_HUB){
    r=await api('/api/servers',{method:'POST',body:JSON.stringify({model,host:'127.0.0.1',port})});
  } else {
    r=await api('/hub/nodes/'+encodeURIComponent(n.node_id)+'/servers',{method:'POST',body:JSON.stringify({model,host:'127.0.0.1',port})});
  }
  const d=await r.json().catch(()=>({}));
  if(r.ok) flash('ok','Serving '+model);
  else flash('err',d.error||('serve failed ('+r.status+')'));
  tickDeck(); loadServers();
};
function refresh(){loadGpus();loadServers();loadPlayground();tickDeck();}
let booted=false;
async function boot(){
  try{const r=await fetch('/healthz'); const d=await r.json(); IS_HUB=!!d.hub;
    if(IS_HUB){document.querySelector('.tagline').textContent='every node · GPU · VRAM · who is using it';
      document.title='Cameo Fleet'; showView('deck');}
  }catch(e){}
  loadModels(); refresh();
  if(!booted){ booted=true; setInterval(()=>{loadServers();loadPlayground();tickDeck();},4000); }
}
boot();
</script>
</body>
</html>
"##;

/// Hub and node serve the same page. The JS reads `healthz.hub`.
#[allow(dead_code)]
pub const HUB_HTML: &str = INDEX_HTML;
