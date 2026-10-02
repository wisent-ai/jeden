const refreshButton = document.getElementById('refresh-stats');
const feedback = document.getElementById('feedback');
const failure = document.getElementById('failure');
let reading = false;
let snapshot = null;

function escapeText(value) {
  const element = document.createElement('span');
  element.textContent = String(value);
  return element.innerHTML;
}

async function refresh() {
  if (reading) return;
  reading = true;
  refreshButton.disabled = true;
  feedback.textContent = 'Reading snapshot...';
  try {
    const response = await fetch('/api/stats', { cache: 'no-store' });
    if (!response.ok) {
      const detail = await response.text();
      throw new Error(`/api/stats returned HTTP ${response.status}: ${detail}`);
    }
    const stats = await response.json();
    const generatedAt = new Date(stats.generatedAtMs).toISOString();
    const quotaHtml = stats.quota.available
      ? stats.quota.providers.map(provider => '<div class="card"><b>' + escapeText(provider.provider) + '</b>' + provider.entries.map(entry => {
          if (entry.error != null) {
            return '<div class="row"><span>' + escapeText(entry.label) + '</span><span class="error">quota unavailable: ' + escapeText(entry.error) + '</span></div>';
          }
          const amount = 'state ' + escapeText(entry.state) + ' · remaining ' + escapeText(entry.remaining) + ' · limit ' + escapeText(entry.limit) + ' · percent free ' + escapeText(entry.percentFree);
          let bar = '';
          if (typeof entry.percentFree === 'number') {
            bar = '<div class="bar"><div style="width:' + entry.percentFree + '%"></div></div>';
          }
          return '<div class="row"><span>' + escapeText(entry.label) + '</span><span class="num dim">' + amount + '</span></div>' + bar;
        }).join('') + '</div>').join('')
      : '<div class="card dim">quota unavailable: ' + escapeText(stats.quota.reason) + '</div>';
    const usageHtml = Object.entries(stats.usage).map(([scope, usage]) => {
      return '<div class="card"><b>' + escapeText(scope) + '</b><div class="row"><span>' + usage.events + ' events</span><span class="num">' + Math.round(usage.tokens) + ' tokens</span><span class="num dim">cost ' + usage.cost.toFixed(4) + '</span></div></div>';
    }).join('');
    const sessionsHtml = stats.sessions.count + ' sessions' + (stats.sessions.recent.length ? '<br><span class="dim">latest: ' + escapeText(stats.sessions.recent.join(', ')) + '</span>' : '');
    document.getElementById('ver').textContent = 'v' + stats.version;
    document.getElementById('quota').innerHTML = quotaHtml;
    document.getElementById('usage').innerHTML = usageHtml;
    document.getElementById('sessions').innerHTML = sessionsHtml;
    snapshot = generatedAt;
    feedback.textContent = 'Snapshot generated at ' + snapshot;
    failure.textContent = '';
  } catch (error) {
    failure.textContent = 'GET /api/stats failed: ' + (error instanceof Error ? error.message : String(error));
    feedback.textContent = snapshot ? 'Still showing the snapshot generated at ' + snapshot : 'No snapshot available.';
  } finally {
    reading = false;
    refreshButton.disabled = false;
  }
}

refreshButton.addEventListener('click', refresh);
void refresh();
