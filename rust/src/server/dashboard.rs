/// Render the full single-page HTML dashboard.
///
/// Self-contained: all CSS and JS are inline (no external assets).
/// Matches the Elixir reference layout from `dashboard_live.ex`.
pub fn render_html_dashboard() -> String {
    r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Rusty Dashboard</title>
<style>
*,*::before,*::after{box-sizing:border-box;margin:0;padding:0}
:root{
  --bg:#0d1117;--surface:#161b22;--surface2:#1c2333;--border:#30363d;
  --text:#e6edf3;--muted:#8b949e;--accent:#58a6ff;
  --green:#3fb950;--yellow:#d29922;--orange:#db6d28;--red:#f85149;
  --mono:'SFMono-Regular','Consolas','Liberation Mono','Menlo',monospace;
}
html{font-size:14px}
body{font-family:var(--mono);background:var(--bg);color:var(--text);padding:1rem;min-height:100vh}
a{color:var(--accent);text-decoration:none}
a:hover{text-decoration:underline}

/* --- Hero header --- */
.hero-card{background:linear-gradient(135deg,#1c2333 0%,#161b22 100%);border:1px solid var(--border);border-radius:12px;padding:1.5rem 2rem;margin-bottom:1.5rem}
.hero-grid{display:flex;justify-content:space-between;align-items:flex-start;gap:1rem;flex-wrap:wrap}
.eyebrow{font-size:.75rem;text-transform:uppercase;letter-spacing:.08em;color:var(--muted);margin-bottom:.25rem}
.hero-title{font-size:1.5rem;font-weight:700;color:var(--text);margin-bottom:.35rem}
.hero-copy{font-size:.85rem;color:var(--muted);max-width:38rem;line-height:1.45}
.status-stack{display:flex;gap:.5rem;align-items:center}
.live-dot{width:8px;height:8px;border-radius:50%;display:inline-block;margin-right:4px}
.live-badge{display:inline-flex;align-items:center;font-size:.75rem;padding:3px 10px;border-radius:9999px;font-weight:600}
.live-badge-on{background:rgba(63,185,80,.15);color:var(--green);border:1px solid rgba(63,185,80,.3)}
.live-badge-on .live-dot{background:var(--green);animation:pulse 2s infinite}
.live-badge-off{background:rgba(248,81,73,.12);color:var(--red);border:1px solid rgba(248,81,73,.25)}
.live-badge-off .live-dot{background:var(--red)}
@keyframes pulse{0%,100%{opacity:1}50%{opacity:.4}}

/* --- Metric cards --- */
.metric-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:1rem;margin-bottom:1.5rem}
.metric-card{background:var(--surface);border:1px solid var(--border);border-radius:10px;padding:1.1rem 1.25rem}
.metric-label{font-size:.7rem;text-transform:uppercase;letter-spacing:.06em;color:var(--muted);margin-bottom:.3rem}
.metric-value{font-size:1.6rem;font-weight:700;line-height:1.2}
.metric-detail{font-size:.75rem;color:var(--muted);margin-top:.3rem}

/* --- Section cards --- */
.section-card{background:var(--surface);border:1px solid var(--border);border-radius:10px;padding:1.25rem;margin-bottom:1.5rem}
.section-header{margin-bottom:1rem}
.section-title{font-size:1rem;font-weight:600;margin-bottom:.2rem}
.section-copy{font-size:.8rem;color:var(--muted)}

/* --- Tables --- */
.table-wrap{overflow-x:auto;-webkit-overflow-scrolling:touch}
.data-table{width:100%;border-collapse:collapse;font-size:.8rem;min-width:700px}
.data-table th{text-align:left;padding:.55rem .65rem;background:var(--surface2);color:var(--muted);font-weight:600;font-size:.7rem;text-transform:uppercase;letter-spacing:.04em;border-bottom:1px solid var(--border);white-space:nowrap}
.data-table td{padding:.55rem .65rem;border-bottom:1px solid var(--border);vertical-align:top}
.data-table tbody tr:hover{background:rgba(88,166,255,.04)}

/* --- State badges --- */
.state-badge{display:inline-block;padding:2px 9px;border-radius:9999px;font-size:.72rem;font-weight:600;white-space:nowrap}
.state-badge-active{background:rgba(63,185,80,.15);color:var(--green);border:1px solid rgba(63,185,80,.3)}
.state-badge-warning{background:rgba(210,153,34,.14);color:var(--yellow);border:1px solid rgba(210,153,34,.28)}
.state-badge-rework{background:rgba(219,109,40,.14);color:var(--orange);border:1px solid rgba(219,109,40,.28)}
.state-badge-danger{background:rgba(248,81,73,.12);color:var(--red);border:1px solid rgba(248,81,73,.25)}

/* --- Utilities --- */
.issue-stack{display:flex;flex-direction:column;gap:2px}
.issue-id{font-weight:600}
.issue-link{font-size:.7rem;color:var(--accent)}
.session-stack{display:flex;flex-direction:column;gap:2px}
.copy-btn{background:var(--surface2);color:var(--muted);border:1px solid var(--border);border-radius:6px;padding:2px 8px;font-size:.72rem;cursor:pointer;font-family:var(--mono);transition:all .15s}
.copy-btn:hover{color:var(--text);border-color:var(--accent)}
.detail-stack{display:flex;flex-direction:column;gap:2px}
.event-text{max-width:22rem;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;display:block}
.event-meta{font-size:.7rem;color:var(--muted)}
.token-stack{display:flex;flex-direction:column;gap:1px}
.numeric{font-variant-numeric:tabular-nums}
.muted{color:var(--muted)}
.empty-state{color:var(--muted);font-style:italic;padding:.75rem 0;font-size:.85rem}
.code-panel{background:var(--bg);border:1px solid var(--border);border-radius:8px;padding:.85rem 1rem;font-size:.78rem;color:var(--muted);overflow-x:auto;white-space:pre-wrap;word-break:break-word}
.error-card{background:rgba(248,81,73,.08);border:1px solid rgba(248,81,73,.25);border-radius:10px;padding:1.25rem;margin-bottom:1.5rem}
.error-title{color:var(--red);font-size:1rem;margin-bottom:.35rem}
.error-copy{color:var(--muted);font-size:.85rem}

/* --- Responsive --- */
@media(max-width:640px){
  body{padding:.5rem}
  .hero-card{padding:1rem}
  .hero-title{font-size:1.15rem}
  .metric-grid{grid-template-columns:1fr 1fr}
  .data-table{font-size:.72rem}
  .section-card{padding:.85rem}
}
@media(max-width:400px){
  .metric-grid{grid-template-columns:1fr}
}
</style>
</head>
<body>

<section class="dashboard-shell">
  <header class="hero-card">
    <div class="hero-grid">
      <div>
        <p class="eyebrow">Rusty Observability</p>
        <h1 class="hero-title">Operations Dashboard</h1>
        <p class="hero-copy">Current state, retry pressure, token usage, and orchestration health. <a href="/api/v1/state">JSON API</a></p>
      </div>
      <div class="status-stack">
        <span id="live-badge" class="live-badge live-badge-on">
          <span class="live-dot"></span> Live
        </span>
      </div>
    </div>
  </header>

  <div id="error-region"></div>

  <section id="metrics-region" class="metric-grid" style="display:none">
    <article class="metric-card">
      <p class="metric-label">Running</p>
      <p class="metric-value numeric" id="m-running">—</p>
      <p class="metric-detail">Active issue sessions.</p>
    </article>
    <article class="metric-card">
      <p class="metric-label">Retrying</p>
      <p class="metric-value numeric" id="m-retrying">—</p>
      <p class="metric-detail">Issues waiting for retry window.</p>
    </article>
    <article class="metric-card">
      <p class="metric-label">Total tokens</p>
      <p class="metric-value numeric" id="m-tokens">—</p>
      <p class="metric-detail numeric" id="m-tokens-detail">In — / Out —</p>
    </article>
    <article class="metric-card">
      <p class="metric-label">Runtime</p>
      <p class="metric-value numeric" id="m-runtime">—</p>
      <p class="metric-detail">Aggregate Codex runtime.</p>
    </article>
  </section>

  <section id="rate-section" class="section-card" style="display:none">
    <div class="section-header">
      <h2 class="section-title">Rate limits</h2>
      <p class="section-copy">Latest upstream rate-limit snapshot.</p>
    </div>
    <pre class="code-panel" id="rate-panel">n/a</pre>
  </section>

  <section id="running-section" class="section-card" style="display:none">
    <div class="section-header">
      <h2 class="section-title">Running sessions</h2>
      <p class="section-copy">Active issues, agent activity, and token usage.</p>
    </div>
    <div id="running-body"></div>
  </section>

  <section id="retry-section" class="section-card" style="display:none">
    <div class="section-header">
      <h2 class="section-title">Backoff queue</h2>
      <p class="section-copy">Issues waiting for the next retry window.</p>
    </div>
    <div id="retry-body"></div>
  </section>
</section>

<script>
(function(){
  "use strict";

  // ---- helpers ----
  function fmtInt(n){
    if(n==null)return"n/a";
    return n.toLocaleString("en-US");
  }

  function fmtRuntime(sec){
    var s=Math.max(Math.floor(sec),0);
    var m=Math.floor(s/60); s=s%60;
    if(m>=60){var h=Math.floor(m/60);m=m%60;return h+"h "+m+"m "+s+"s";}
    return m+"m "+s+"s";
  }

  function elapsedSec(iso){
    if(!iso)return 0;
    var t=new Date(iso).getTime();
    if(isNaN(t))return 0;
    return Math.max((Date.now()-t)/1000,0);
  }

  function esc(s){
    if(s==null)return"";
    return String(s).replace(/&/g,"&amp;").replace(/</g,"&lt;").replace(/>/g,"&gt;").replace(/"/g,"&quot;");
  }

  function badgeClass(state){
    if(!state)return"state-badge";
    var s=state.toLowerCase();
    if(s.indexOf("progress")>=0||s.indexOf("running")>=0||s.indexOf("active")>=0)return"state-badge state-badge-active";
    if(s.indexOf("rework")>=0)return"state-badge state-badge-rework";
    if(s.indexOf("blocked")>=0||s.indexOf("error")>=0||s.indexOf("failed")>=0)return"state-badge state-badge-danger";
    if(s.indexOf("todo")>=0||s.indexOf("queued")>=0||s.indexOf("pending")>=0||s.indexOf("retry")>=0)return"state-badge state-badge-warning";
    return"state-badge";
  }

  function truncSid(sid){
    if(!sid)return null;
    return sid.length>12?sid.slice(0,8)+"…"+sid.slice(-4):sid;
  }

  function formatAge(iso){
    if(!iso)return"-";
    var t=new Date(iso).getTime();
    if(isNaN(t))return"-";
    var sec=Math.max(Math.floor((Date.now()-t)/1000),0);
    if(sec<60)return sec+"s";
    return Math.floor(sec/60)+"m "+sec%60+"s";
  }

  function truncSession(sid){
    if(!sid)return"-";
    if(sid.length>10)return sid.slice(0,4)+"..."+sid.slice(-6);
    return sid;
  }

  function fmtTokens(n){
    if(n==null)return"0";
    return n.toLocaleString("en-US");
  }

  // ---- state ----
  var lastData=null;
  var badge=document.getElementById("live-badge");

  // ---- render ----
  function render(d){
    lastData=d;

    // error
    var errR=document.getElementById("error-region");
    if(d.error){
      errR.innerHTML='<div class="error-card"><h2 class="error-title">Snapshot unavailable</h2><p class="error-copy"><strong>'+esc(d.error.code)+':</strong> '+esc(d.error.message)+'</p></div>';
      document.getElementById("metrics-region").style.display="none";
      document.getElementById("rate-section").style.display="none";
      document.getElementById("running-section").style.display="none";
      document.getElementById("retry-section").style.display="none";
      badge.className="live-badge live-badge-off";
      badge.innerHTML='<span class="live-dot"></span> Offline';
      return;
    }
    errR.innerHTML="";
    badge.className="live-badge live-badge-on";
    badge.innerHTML='<span class="live-dot"></span> Live';

    // metrics
    var mr=document.getElementById("metrics-region");mr.style.display="";
    document.getElementById("m-running").textContent=fmtInt(d.counts.running);
    document.getElementById("m-retrying").textContent=fmtInt(d.counts.retrying);
    var ct=d.codex_totals||{};
    document.getElementById("m-tokens").textContent=fmtInt(ct.total_tokens||0);
    document.getElementById("m-tokens-detail").textContent="In "+fmtInt(ct.input_tokens||0)+" / Out "+fmtInt(ct.output_tokens||0);
    updateRuntime();

    // rate limits
    var rs=document.getElementById("rate-section");
    if(d.rate_limits!=null){rs.style.display="";document.getElementById("rate-panel").textContent=JSON.stringify(d.rate_limits,null,2);}
    else{rs.style.display="none";}

    // running table
    var runS=document.getElementById("running-section");runS.style.display="";
    var rb=document.getElementById("running-body");
    if(!d.running||d.running.length===0){
      rb.innerHTML='<p class="empty-state">No active sessions.</p>';
    }else{
      var h='<div class="table-wrap"><table class="data-table"><thead><tr>';
      h+='<th>Issue</th><th>State</th><th>PID</th><th>Age / Turn</th><th>Tokens</th><th>Session</th><th>Event</th>';
      h+='</tr></thead><tbody>';
      d.running.forEach(function(r){
        var pid=r.pid!=null?String(r.pid):"-";
        var age=formatAge(r.started_at);
        var ageTurn=age+(r.turn_count>0?" / "+r.turn_count:"");
        var sid=r.session_id;
        var session=truncSession(sid);
        var event=r.last_message||r.last_event||"-";

        h+=`<tr>`;
        h+=`<td><div class="issue-stack"><span class="issue-id">${esc(r.identifier)}</span><a class="issue-link" href="/api/v1/${encodeURIComponent(r.identifier)}">JSON</a></div></td>`;
        h+=`<td><span class="${badgeClass(r.state)}">${esc(r.state)}</span></td>`;
        h+=`<td class="numeric">${esc(pid)}</td>`;
        h+=`<td class="numeric runtime-cell" data-started="${esc(r.started_at)}" data-turns="${r.turn_count||0}">${esc(ageTurn)}</td>`;
        h+=`<td><div class="token-stack numeric"><span>${fmtTokens(r.total_tokens)}</span><span class="muted">In ${fmtTokens(r.input_tokens)} / Out ${fmtTokens(r.output_tokens)}</span></div></td>`;
        h+=`<td>${esc(session)}</td>`;
        h+=`<td class="event"><span class="event-text" title="${esc(event)}">${esc(event)}</span></td>`;
        h+=`</tr>`;
      });
      h+='</tbody></table></div>';
      rb.innerHTML=h;
    }

    // retry table
    var retS=document.getElementById("retry-section");retS.style.display="";
    var retB=document.getElementById("retry-body");
    if(!d.retrying||d.retrying.length===0){
      retB.innerHTML='<p class="empty-state">No issues are currently backing off.</p>';
    }else{
      var rh='<div class="table-wrap"><table class="data-table"><thead><tr>';
      rh+='<th>Issue</th><th>Attempt</th><th>Due at</th><th>Error</th>';
      rh+='</tr></thead><tbody>';
      d.retrying.forEach(function(e){
        rh+='<tr>';
        rh+='<td><div class="issue-stack"><span class="issue-id">'+esc(e.identifier)+'</span><a class="issue-link" href="/api/v1/'+encodeURIComponent(e.identifier)+'">JSON</a></div></td>';
        rh+='<td>'+esc(e.attempt)+'</td>';
        rh+='<td class="numeric">'+esc(e.due_at||"n/a")+'</td>';
        rh+='<td>'+esc(e.error||"n/a")+'</td>';
        rh+='</tr>';
      });
      rh+='</tbody></table></div>';
      retB.innerHTML=rh;
    }
  }

  // ---- live age tick (1s) ----
  function updateRuntime(){
    if(!lastData)return;
    // aggregate runtime metric
    var ct=lastData.codex_totals||{};
    var completedSec=ct.seconds_running||0;
    var liveSec=0;
    if(lastData.running){
      lastData.running.forEach(function(e){liveSec+=elapsedSec(e.started_at);});
    }
    document.getElementById("m-runtime").textContent=fmtRuntime(completedSec+liveSec);

    // per-row runtime cells
    var cells=document.querySelectorAll(".runtime-cell");
    cells.forEach(function(c){
      var started=c.getAttribute("data-started");
      var turns=parseInt(c.getAttribute("data-turns"),10)||0;
      var txt=fmtRuntime(elapsedSec(started));
      if(turns>0)txt+=" / "+turns;
      c.textContent=txt;
    });
  }

  // ---- fetch loop (2s) ----
  function fetchState(){
    fetch("/api/v1/state").then(function(r){
      if(!r.ok)throw new Error("HTTP "+r.status);
      return r.json();
    }).then(function(d){render(d);}).catch(function(err){
      render({error:{code:"fetch_error",message:err.message}});
    });
  }

  fetchState();
  setInterval(fetchState,2000);
  setInterval(updateRuntime,1000);
})();
</script>
</body>
</html>"##
        .to_string()
}
