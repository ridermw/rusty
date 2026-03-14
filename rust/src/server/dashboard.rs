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
a { color: #6ea8fe; text-decoration: none; }
a:hover { text-decoration: underline; }
#conn-badge { display: inline-block; padding: 2px 8px; border-radius: 4px; font-size: 0.85em; margin-left: 12px; }
.connected { background: #198754; color: #fff; }
.disconnected { background: #dc3545; color: #fff; }
</style>
</head>
<body>
<h1>Rusty Dashboard <span id="conn-badge" class="disconnected">connecting…</span></h1>
<p>Live via SSE. JSON: <a href="/api/v1/state">/api/v1/state</a></p>
<div id="state">Loading...</div>
<script>
function issueCell(identifier, url) {
  if (url) return '<a href="' + url + '" target="_blank" rel="noopener">' + identifier + '</a>';
  return identifier;
}
function renderState(d) {
  let h = '<p>Running: ' + d.counts.running + ' | Retrying: ' + d.counts.retrying + '</p>';
  if (d.running.length) {
    h += '<h2>Running</h2><table><tr><th>Issue</th><th>State</th><th>Turns</th><th>Tokens</th></tr>';
    d.running.forEach(function(r) {
      h += '<tr><td>' + issueCell(r.identifier, r.issue_url) + '</td><td>' + r.state + '</td><td>' + r.turn_count + '</td><td>' + r.total_tokens + '</td></tr>';
    });
    h += '</table>';
  }
  if (d.retrying && d.retrying.length) {
    h += '<h2>Retry Queue</h2><table><tr><th>Issue</th><th>Attempt</th><th>Due</th><th>Error</th></tr>';
    d.retrying.forEach(function(r) {
      h += '<tr><td>' + issueCell(r.identifier, r.issue_url) + '</td><td>' + r.attempt + '</td><td>' + r.due_at + '</td><td>' + (r.error || '-') + '</td></tr>';
    });
    h += '</table>';
  }
  document.getElementById('state').innerHTML = h;
}
var es;
function connectSSE() {
  es = new EventSource('/api/v1/events');
  var badge = document.getElementById('conn-badge');
  es.onopen = function() { badge.className = 'connected'; badge.textContent = 'live'; };
  es.addEventListener('snapshot', function(e) {
    try { renderState(JSON.parse(e.data)); } catch(err) { console.error('SSE parse error', err); }
  });
  es.onerror = function() {
    badge.className = 'disconnected'; badge.textContent = 'reconnecting…';
  };
}
// Initial fetch then switch to SSE
fetch('/api/v1/state').then(function(r){return r.json()}).then(renderState).catch(function(){
  document.getElementById('state').innerHTML = 'Error loading state';
});
connectSSE();
</script>
</body></html>"#
        .to_string()
}
