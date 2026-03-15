/// Render a minimal HTML dashboard page.
///
/// All dynamic API values are escaped via a client-side `esc()` helper
/// before insertion into the DOM.  Link targets are validated with
/// `safeUrl()` which only allows `http:` / `https:` protocols.
pub fn render_html_dashboard() -> String {
    r#"<!DOCTYPE html>
<html>
<head><title>Rusty Dashboard</title>
<meta http-equiv="refresh" content="5">
<style>
body { font-family: monospace; background: #1a1a2e; color: #eee; padding: 20px; }
h1 { color: #e94560; }
table { border-collapse: collapse; width: 100%; }
th, td { text-align: left; padding: 8px; border-bottom: 1px solid #333; }
th { color: #0f3460; background: #16213e; }
.status { padding: 2px 8px; border-radius: 4px; }
.stats { background: #16213e; padding: 12px 16px; border-radius: 6px; margin-bottom: 16px; line-height: 1.8; }
.stats a { color: #6ea8fe; }
</style>
</head>
<body>
<h1>Rusty Dashboard</h1>
<div id="state">Loading...</div>
<script>
function esc(s){let d=document.createElement('div');d.textContent=String(s);return d.innerHTML}
function safeUrl(u){try{let p=new URL(u);return(p.protocol==='http:'||p.protocol==='https:')?p.href:null}catch(e){return null}}
function fmt(n){return esc(n.toString().replace(/\B(?=(\d{3})+(?!\d))/g,','))}
fetch('/api/v1/state').then(r=>r.json()).then(d=>{
  let h='<div class="stats">';
  h+=`<b>Agents:</b> ${esc(d.counts.running)}/${esc(d.max_agents||'?')} &nbsp;|&nbsp; `;
  h+=`<b>Throughput:</b> ${fmt(Math.round(d.throughput_tps||0))} tps &nbsp;|&nbsp; `;
  let s=d.codex_totals.seconds_running||0;
  if(s<60)h+=`<b>Runtime:</b> ${esc(Math.floor(s))}s<br>`;
  else if(s<3600)h+=`<b>Runtime:</b> ${esc(Math.floor(s/60))}m ${esc(Math.floor(s%60))}s<br>`;
  else h+=`<b>Runtime:</b> ${esc(Math.floor(s/3600))}h ${esc(Math.floor((s%3600)/60))}m<br>`;
  h+=`<b>Tokens:</b> in ${fmt(d.codex_totals.input_tokens)} | out ${fmt(d.codex_totals.output_tokens)} | total ${fmt(d.codex_totals.total_tokens)}<br>`;
  let rl=d.rate_limits?JSON.stringify(d.rate_limits):'n/a';
  h+=`<b>Rate Limits:</b> ${esc(rl)}<br>`;
  if(d.project_url){let u=safeUrl(d.project_url);if(u)h+=`<b>Project:</b> <a href="${u}" target="_blank">${esc(d.project_url)}</a><br>`}
  if(d.next_tick_at)h+=`<b>Next refresh:</b> ${esc(new Date(d.next_tick_at).toLocaleTimeString())}<br>`;
  h+='</div>';
  h+=`<p>Running: ${esc(d.counts.running)} | Retrying: ${esc(d.counts.retrying)}</p>`;
  if(d.running.length){h+=`<h2>Running</h2><table><tr><th>Issue</th><th>State</th><th>Turns</th><th>Tokens</th></tr>`;
  d.running.forEach(r=>{h+=`<tr><td>${esc(r.identifier)}</td><td>${esc(r.state)}</td><td>${esc(r.turn_count)}</td><td>${fmt(r.total_tokens)}</td></tr>`});h+=`</table>`}
  document.getElementById('state').innerHTML=h;
}).catch(e=>{document.getElementById('state').textContent='Error loading state'});
</script>
</body></html>"#
        .to_string()
}
