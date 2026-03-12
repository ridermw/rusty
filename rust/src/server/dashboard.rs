/// Render a minimal HTML dashboard page.
pub fn render_html_dashboard() -> String {
    r#"<!DOCTYPE html>
<html>
<head><title>Symphony Dashboard</title>
<meta http-equiv="refresh" content="5">
<style>
body { font-family: monospace; background: #1a1a2e; color: #eee; padding: 20px; }
h1 { color: #e94560; }
table { border-collapse: collapse; width: 100%; }
th, td { text-align: left; padding: 8px; border-bottom: 1px solid #333; }
th { color: #0f3460; background: #16213e; }
.status { padding: 2px 8px; border-radius: 4px; }
</style>
</head>
<body>
<h1>Symphony Dashboard</h1>
<p>Auto-refreshes every 5 seconds. For JSON data, use <a href="/api/v1/state">/api/v1/state</a>.</p>
<div id="state">Loading...</div>
<script>
fetch('/api/v1/state').then(r=>r.json()).then(d=>{
  let h = `<p>Running: ${d.counts.running} | Retrying: ${d.counts.retrying}</p>`;
  if(d.running.length){h+=`<h2>Running</h2><table><tr><th>Issue</th><th>State</th><th>Turns</th><th>Tokens</th></tr>`;
  d.running.forEach(r=>{h+=`<tr><td>${r.identifier}</td><td>${r.state}</td><td>${r.turn_count}</td><td>${r.total_tokens}</td></tr>`});h+=`</table>`}
  document.getElementById('state').innerHTML=h;
}).catch(e=>{document.getElementById('state').innerHTML='Error loading state'});
</script>
</body></html>"#
        .to_string()
}
