/// Render a minimal HTML dashboard page.
pub fn render_html_dashboard() -> String {
    r#"<!DOCTYPE html>
<html>
<head><title>Rusty Dashboard</title>
<style>
body { font-family: monospace; background: #1a1a2e; color: #eee; padding: 20px; }
h1 { color: #e94560; }
table { border-collapse: collapse; width: 100%; }
th, td { text-align: left; padding: 8px; border-bottom: 1px solid #333; }
th { color: #0f3460; background: #16213e; }
.status { padding: 2px 8px; border-radius: 4px; }
td.event { max-width: 320px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
</head>
<body>
<h1>Rusty Dashboard</h1>
<p>For JSON data, use <a href="/api/v1/state">/api/v1/state</a>.</p>
<div id="state">Loading...</div>
<script>
function formatAge(startedAt){
  const ms=Date.now()-new Date(startedAt).getTime();
  if(ms<0)return '0s';
  const s=Math.floor(ms/1000);
  if(s<60)return s+'s';
  const m=Math.floor(s/60);
  return m+'m '+s%60+'s';
}
function truncSession(sid){
  if(!sid)return '-';
  if(sid.length<=10)return sid;
  return sid.slice(0,4)+'...'+sid.slice(-6);
}
function fmtTokens(n){return n.toLocaleString();}
function refresh(){
  fetch('/api/v1/state').then(r=>r.json()).then(d=>{
    let h=`<p>Running: ${d.counts.running} | Retrying: ${d.counts.retrying}</p>`;
    if(d.running.length){
      h+=`<h2>Running</h2><table><tr><th>Issue</th><th>State</th><th>PID</th><th>Age / Turn</th><th>Tokens</th><th>Session</th><th>Event</th></tr>`;
      d.running.forEach(r=>{
        const pid=r.pid||'-';
        const age=formatAge(r.started_at);
        const session=truncSession(r.session_id);
        const event=r.last_message||r.last_event||'-';
        h+=`<tr><td>${r.identifier}</td><td>${r.state}</td><td>${pid}</td><td>${age} / ${r.turn_count}</td><td>${fmtTokens(r.total_tokens)}</td><td>${session}</td><td class="event">${event}</td></tr>`;
      });
      h+=`</table>`;
    }
    document.getElementById('state').innerHTML=h;
  }).catch(e=>{document.getElementById('state').innerHTML='Error loading state'});
}
refresh();
setInterval(refresh,5000);
</script>
</body></html>"#
        .to_string()
}
