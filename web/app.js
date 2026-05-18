/* DCTRL YVR seed monitor — frontend */

const PALETTE = {
  orange:  '#f7931a',
  ochre:   '#c9a564',
  rust:    '#b8482c',
  moss:    '#7c9a4c',
  mustard: '#c19a3a',
  violet:  '#8a73b8',
  ink:     '#f1e7d2',
  inkDim:  '#cabea3',
  muted:   '#978770',
  rule:    '#3d342a',
  surface: '#1d1812',
};

const NETWORK_KEY = 'seed-monitor-network';

const state = {
  page: 'seeds',
  rangeSecs: 30 * 86400,
  network: (typeof localStorage !== 'undefined' && localStorage.getItem(NETWORK_KEY)) || 'mainnet',
  seeds: [],
};

function fmtDate(ts) {
  const d = new Date(ts * 1000);
  return d.toISOString().slice(0, 16).replace('T', ' ');
}

async function api(path, params = {}) {
  const now = Math.floor(Date.now() / 1000);
  const from = now - state.rangeSecs;
  const url = new URL(path, location.origin);
  url.searchParams.set('from', String(from));
  url.searchParams.set('to', String(now));
  url.searchParams.set('network', state.network);
  for (const [k, v] of Object.entries(params)) url.searchParams.set(k, v);
  const r = await fetch(url);
  if (!r.ok) throw new Error(`${path} → ${r.status}`);
  return r.json();
}

function seedsInNetwork() {
  return (state.seeds || []).filter(s => (s.network || 'mainnet') === state.network);
}

function lineChart(el, series, options = {}) {
  el.innerHTML = '';
  if (!series || series[0].length === 0) {
    el.innerHTML = '<p style="color:var(--muted);">No data yet — collector hasn\'t completed a round.</p>';
    return;
  }
  const opts = {
    width: el.clientWidth || 800,
    height: 280,
    series: options.series || [{}, { stroke: PALETTE.orange, width: 1.4 }],
    axes: [
      { stroke: PALETTE.muted, grid: { stroke: PALETTE.rule, width: 0.5 }, ticks: { stroke: PALETTE.rule } },
      { stroke: PALETTE.muted, grid: { stroke: PALETTE.rule, width: 0.5 }, ticks: { stroke: PALETTE.rule } },
    ],
  };
  new uPlot(opts, series, el);
}

const pages = {
  async seeds(main) {
    main.innerHTML = `
      <h2>DNS seeds</h2>
      <p class="lede">All mainnet seeds from Bitcoin Core's <code>chainparams.cpp</code>. Counts cover the last 24 hours.</p>
      <div class="card">
        <h3>Seed roster</h3>
        <div id="seed-table"></div>
      </div>
      <div class="card">
        <h3>Reachable / unreachable per seed (selected range)</h3>
        <div id="seed-chart" style="width:100%"></div>
        <select id="seed-pick" class="picker"></select>
      </div>
    `;
    const data = await api('/api/seeds');
    state.seeds = data.seeds;
    const summary = Object.fromEntries((data.latest || []).map(l => [l.seed_id, l]));
    const shown = data.seeds.filter(s => (s.network || 'mainnet') === state.network);
    const rows = shown.map(s => {
      const l = summary[s.id] || { reachable: 0, unreachable: 0, last_round: 0 };
      const total = l.reachable + l.unreachable;
      const rate = total > 0 ? (100 * l.reachable / total).toFixed(1) + '%' : '—';
      const net = (s.network || 'mainnet').toLowerCase();
      return `<tr>
        <td class="hostname">${s.hostname}<span class="net-chip ${net}">${net}</span></td>
        <td class="op">${s.operator ?? ''}</td>
        <td class="num">${l.reachable}</td>
        <td class="num">${l.unreachable}</td>
        <td class="num" style="color:${total > 0 && l.reachable/total < 0.5 ? PALETTE.rust : PALETTE.orange}">${rate}</td>
        <td class="num">${l.last_round ? fmtDate(l.last_round) : '—'}</td>
      </tr>`;
    }).join('');
    main.querySelector('#seed-table').innerHTML = `
      <table>
        <thead><tr>
          <th>Hostname</th>
          <th>Operator</th>
          <th class="num">Reachable</th>
          <th class="num">Unreachable</th>
          <th class="num">Success</th>
          <th class="num">Last round</th>
        </tr></thead>
        <tbody>${rows}</tbody>
      </table>`;
    const pick = main.querySelector('#seed-pick');
    pick.innerHTML = shown.map(s => `<option value="${s.id}">${s.hostname}</option>`).join('');
    const renderChart = async () => {
      const id = pick.value;
      const ts = await api(`/api/seeds/${id}/timeseries`);
      const xs = ts.points.map(p => p.bucket);
      const r  = ts.points.map(p => p.reachable);
      const u  = ts.points.map(p => p.unreachable);
      lineChart(main.querySelector('#seed-chart'), [xs, r, u], {
        series: [
          {},
          { label: 'reachable',   stroke: PALETTE.orange, width: 1.4 },
          { label: 'unreachable', stroke: PALETTE.rust,   width: 1.4 },
        ],
      });
    };
    pick.addEventListener('change', renderChart);
    if (shown[0]) { pick.value = shown[0].id; await renderChart(); }
    else { main.querySelector('#seed-chart').innerHTML = '<p style="color:var(--muted)">No seeds in this network yet.</p>'; }
  },

  async reachable(main) {
    main.innerHTML = `
      <h2>Reachable nodes</h2>
      <p class="lede">Unique reachable addresses per hour, split by network type.</p>
      <div class="card"><h3>IPv4</h3><div id="c4" style="width:100%"></div></div>
      <div class="card"><h3>IPv6</h3><div id="c6" style="width:100%"></div></div>
    `;
    const data = await api('/api/nodes/reachable');
    const buckets = [...new Set(data.points.map(p => p.bucket))].sort((a,b)=>a-b);
    const idx = Object.fromEntries(buckets.map((b,i)=>[b,i]));
    const v4 = new Array(buckets.length).fill(0);
    const v6 = new Array(buckets.length).fill(0);
    for (const p of data.points) {
      if (p.network_type === 'ipv4') v4[idx[p.bucket]] = p.count;
      else v6[idx[p.bucket]] = p.count;
    }
    lineChart(main.querySelector('#c4'), [buckets, v4], { series: [{}, { label: 'ipv4', stroke: PALETTE.orange, width: 1.4 }] });
    lineChart(main.querySelector('#c6'), [buckets, v6], { series: [{}, { label: 'ipv6', stroke: PALETTE.ochre,  width: 1.4 }] });
  },

  async unique(main) {
    main.innerHTML = `
      <h2>Unique reachable nodes</h2>
      <p class="lede">Distinct reachable addresses per day across all seeds.</p>
      <div class="card"><h3>Per day</h3><div id="cu" style="width:100%"></div></div>
    `;
    const data = await api('/api/nodes/unique');
    const xs = data.points.map(p => p.bucket);
    const ys = data.points.map(p => p.count);
    lineChart(main.querySelector('#cu'), [xs, ys], { series: [{}, { label: 'unique reachable', stroke: PALETTE.orange, width: 1.4 }] });
  },

  async agents(main) {
    main.innerHTML = `
      <h2>User agents</h2>
      <p class="lede">Top advertised user agents over the selected range. Each row is one distinct UA string returned by a successful handshake.</p>
      <div class="card"><h3>Top 30</h3><div id="ua"></div></div>
    `;
    const data = await api('/api/user-agents');
    const rows = (data.rows || []).map(r => `<tr><td>${escapeHtml(r.user_agent)}</td><td class="num">${r.count}</td></tr>`).join('');
    main.querySelector('#ua').innerHTML = `
      <table>
        <thead><tr><th>User agent</th><th class="num">Count</th></tr></thead>
        <tbody>${rows || '<tr><td colspan=2 style="color:var(--muted)">No data yet.</td></tr>'}</tbody>
      </table>`;
  },

  async versions(main) {
    main.innerHTML = `
      <h2>Version share</h2>
      <p class="lede">Per-seed share of successful handshakes by Bitcoin Core major version. Cells show <b>count (share)</b>; orange intensity tracks share. Dark cells &mdash; the seed served zero nodes of that version in the selected range.</p>
      <div class="card">
        <h3>Seeds × Core major version</h3>
        <div id="vs-table" style="overflow-x:auto"></div>
        <p style="color:var(--muted);font-size:12px;margin-top:14px">User agents not matching <code>/Satoshi:N.x.y/</code> (e.g. alternative clients) are bucketed as <b>other</b>.</p>
      </div>
    `;
    const data = await api('/api/version-share');
    const rows = data.rows || [];
    if (!state.seeds || !state.seeds.length) state.seeds = (await api('/api/seeds')).seeds;
    const seedList = seedsInNetwork();
    const seedById = Object.fromEntries(seedList.map(s => [s.id, s]));
    const seedIds = seedList.map(s => s.id);

    const majorSet = new Set();
    for (const r of rows) majorSet.add(r.major == null ? 'other' : r.major);
    const majors = [...majorSet].filter(m => m !== 'other').sort((a,b)=>a-b);
    if (majorSet.has('other')) majors.push('other');

    const cell = {};
    const total = {};
    for (const sid of seedIds) { total[sid] = 0; for (const m of majors) cell[`${sid}|${m}`] = 0; }
    for (const r of rows) {
      const m = r.major == null ? 'other' : r.major;
      const key = `${r.seed_id}|${m}`;
      cell[key] = (cell[key] || 0) + r.count;
      total[r.seed_id] = (total[r.seed_id] || 0) + r.count;
    }

    let maxShare = 0;
    for (const sid of seedIds) {
      if (!total[sid]) continue;
      for (const m of majors) {
        const s = cell[`${sid}|${m}`] / total[sid];
        if (s > maxShare) maxShare = s;
      }
    }
    const clipAt = Math.min(0.6, Math.max(0.25, maxShare));

    const cellBg = (share) => {
      if (share === 0) return 'transparent';
      const t = Math.min(1, share / clipAt);
      const l = 16 + Math.round(t * 36);
      const s = 25 + Math.round(t * 65);
      return `hsl(33 ${s}% ${l}%)`;
    };

    const fmtPct = (n, t) => t ? (100*n/t).toFixed(1) + '%' : '—';
    const headCells = [
      '<th style="text-align:left">Seed</th>',
      ...majors.map(m => `<th>${m === 'other' ? 'other' : 'v' + m}</th>`),
      '<th>total</th>',
    ].join('');

    const bodyRows = seedIds.map(sid => {
      const s = seedById[sid];
      const t = total[sid] || 0;
      const cells = majors.map(m => {
        const n = cell[`${sid}|${m}`] || 0;
        const share = t ? n/t : 0;
        const bg = cellBg(share);
        const isPale = share > clipAt * 0.55;
        const fg = isPale ? '#1a1d22' : PALETTE.ink;
        const sub = isPale ? 'rgba(26,29,34,0.7)' : PALETTE.inkDim;
        if (n === 0) {
          return `<td style="background:${bg};text-align:center;min-width:54px;color:${PALETTE.muted}">·</td>`;
        }
        return `<td style="background:${bg};color:${fg};text-align:center;min-width:54px">
          <div style="font-weight:500">${n}</div>
          <div style="font-size:10px;color:${sub};margin-top:2px">${fmtPct(n,t)}</div>
        </td>`;
      }).join('');
      return `<tr>
        <th>
          <div>${s.hostname}</div>
          <div style="font-family:var(--sans);font-size:11px;color:var(--muted);margin-top:2px">${s.operator ?? ''}</div>
        </th>
        ${cells}
        <td class="num" style="color:var(--muted);padding-left:14px">${t}</td>
      </tr>`;
    }).join('');

    main.querySelector('#vs-table').innerHTML = `
      <table class="heatmap">
        <thead><tr>${headCells}</tr></thead>
        <tbody>${bodyRows}</tbody>
      </table>`;
  },

  async classes(main) {
    main.innerHTML = `
      <h2>Classification mix</h2>
      <p class="lede">For each seed, the share of advertised addresses falling into each daily classification. A bar dominated by <b style="color:${PALETTE.moss}">pristine</b> means the seed is actively crawling new addresses; <b style="color:${PALETTE.mustard}">stale</b> means it's recycling yesterday's list; <b style="color:${PALETTE.rust}">unreachable</b> is junk it served us. Definitions on <a href="#methodology">Methodology</a>.</p>
      <div class="card"><h3>Per-seed share, selected range</h3><div id="cls"></div></div>
    `;
    const data = await api('/api/class-shares');
    if (!state.seeds || !state.seeds.length) state.seeds = (await api('/api/seeds')).seeds;
    const seedList = seedsInNetwork();

    const ORDER = ['pristine','reachable','stale','duplicate','unreachable'];
    const COLOR = {
      pristine:    PALETTE.moss,
      reachable:   '#9ab66a',
      stale:       PALETTE.mustard,
      duplicate:   PALETTE.violet,
      unreachable: PALETTE.rust,
    };

    const bySeed = {};
    for (const r of data.rows || []) {
      (bySeed[r.seed_id] ||= {})[r.class] = r.count;
    }

    const fmtPct = (n, t) => t ? (100*n/t).toFixed(1) + '%' : '—';
    const rows = seedList.map(s => {
      const counts = bySeed[s.id] || {};
      const total = ORDER.reduce((a,c)=> a + (counts[c]||0), 0);
      if (total === 0) {
        return `<div class="classbar">
          <div class="classbar-head">
            <span class="seed">${s.hostname}<span class="op">${s.operator ?? ''}</span></span>
            <span class="total classbar-empty">no data in range</span>
          </div>
          <div class="classbar-track"></div>
        </div>`;
      }
      const segs = ORDER.map(c => {
        const n = counts[c] || 0;
        if (n === 0) return '';
        const pct = (100*n/total).toFixed(1) + '%';
        return `<div title="${c}: ${n} (${pct})" style="flex:${n} 0 0;background:${COLOR[c]}"></div>`;
      }).join('');
      const legend = ORDER.map(c => {
        const n = counts[c] || 0;
        return `<span style="color:${COLOR[c]}">${c} ${fmtPct(n,total)}</span>`;
      }).join('');
      return `<div class="classbar">
        <div class="classbar-head">
          <span class="seed">${s.hostname}<span class="op">${s.operator ?? ''}</span></span>
          <span class="total">${total} addr</span>
        </div>
        <div class="classbar-track">${segs}</div>
        <div class="classbar-legend">${legend}</div>
      </div>`;
    }).join('');
    main.querySelector('#cls').innerHTML = rows || '<p style="color:var(--muted)">No data yet — needs at least one full day of probing.</p>';
  },

  async overlap(main) {
    main.innerHTML = `
      <h2>Seed overlap</h2>
      <p class="lede">Pairwise Jaccard similarity between the address sets each seed returned in the selected range. 100% would mean two seeds return identical pools; 0% means fully disjoint. Cells show <b>share</b> and <b>shared / union</b>.</p>
      <div class="card"><h3>Pairwise Jaccard, selected range</h3><div id="ov" style="overflow-x:auto"></div></div>
    `;
    if (!state.seeds || !state.seeds.length) state.seeds = (await api('/api/seeds')).seeds;
    const seedList = seedsInNetwork();
    const data = await api('/api/seed-overlap');
    const totals = Object.fromEntries((data.totals || []).map(t => [t.seed_id, t.addresses]));
    const pairKey = (a,b) => a < b ? `${a}|${b}` : `${b}|${a}`;
    const shared = {};
    for (const p of data.pairs || []) shared[pairKey(p.a, p.b)] = p.shared;

    const seeds = [...seedList].sort((x,y) => (totals[y.id]||0) - (totals[x.id]||0));
    const shortName = h => h.replace(/^(seed|dnsseed)\./,'').replace(/\.[^.]+$/,'');

    const head = ['<th></th>', ...seeds.map(s => `<th title="${s.hostname}">${shortName(s.hostname)}</th>`), '<th></th>'].join('');
    const body = seeds.map(r => {
      const cells = seeds.map(c => {
        if (r.id === c.id) {
          const t = totals[r.id] || 0;
          return `<td style="background:var(--surface-2);color:var(--muted);text-align:center;font-size:11px;padding:6px 8px">${t}</td>`;
        }
        const sh = shared[pairKey(r.id, c.id)] || 0;
        const tr = totals[r.id] || 0, tc = totals[c.id] || 0;
        const un = tr + tc - sh;
        const j = un ? sh/un : 0;
        const t = Math.min(1, j / 0.3);
        const l = 14 + Math.round(t * 36);
        const s = 25 + Math.round(t * 65);
        const bg = sh === 0 ? 'transparent' : `hsl(33 ${s}% ${l}%)`;
        const isPale = t > 0.55;
        const fg = isPale ? '#1a1d22' : PALETTE.ink;
        const sub = isPale ? 'rgba(26,29,34,0.7)' : PALETTE.muted;
        if (sh === 0) {
          return `<td style="background:${bg};color:${PALETTE.muted};text-align:center;padding:6px 8px;min-width:68px">·</td>`;
        }
        return `<td style="background:${bg};color:${fg};text-align:center;padding:6px 8px;min-width:68px">
          <div style="font-weight:500">${(100*j).toFixed(1)}%</div>
          <div style="font-size:10px;color:${sub};margin-top:2px">${sh}/${un}</div>
        </td>`;
      }).join('');
      return `<tr>
        <th title="${r.hostname}">${shortName(r.hostname)}</th>
        ${cells}
        <th style="text-align:right;color:var(--muted);font-family:var(--mono);font-size:11px;font-style:normal">${totals[r.id] || 0}</th>
      </tr>`;
    }).join('');

    main.querySelector('#ov').innerHTML = `
      <table class="heatmap">
        <thead><tr>${head}</tr></thead>
        <tbody>${body}</tbody>
      </table>
      <p style="color:var(--muted);font-size:12px;margin-top:14px">Diagonal &amp; right edge show that seed's total unique addresses in range.</p>`;
  },

  async alerts(main) {
    main.innerHTML = `
      <h2>Alerts</h2>
      <p class="lede">Seeds currently in a degraded or dead state, computed live from the last 24 hours of probes. <b>critical</b> — fewer than 10% reachable with at least 5 probes. <b>warning</b> — under 50% over 24h with at least 30 probes, or stale data (no recent rounds).</p>
      <div class="card"><h3>Current alerts</h3><div id="alert-list"></div></div>
    `;
    const data = await api('/api/alerts');
    const list = data.alerts || [];
    const root = main.querySelector('#alert-list');
    if (list.length === 0) {
      root.innerHTML = `<div class="alerts-empty">All seeds nominal.</div>`;
    } else {
      root.innerHTML = list
        .sort((a, b) => (a.level === 'critical' ? 0 : 1) - (b.level === 'critical' ? 0 : 1) || a.hostname.localeCompare(b.hostname))
        .map(a => `
          <div class="alert-row">
            <div class="level ${a.level}">${a.level}</div>
            <div class="body">
              <div class="host">${a.hostname}<span class="net-chip ${a.network}">${a.network}</span></div>
              <div class="op">${a.operator ?? ''}</div>
              <div class="reason">${a.reason}</div>
            </div>
            <div class="stats">
              ${a.reachable} / ${a.reachable + a.unreachable} reachable<br/>
              ${a.last_round ? 'last ' + fmtDate(a.last_round) : 'no recent rounds'}
            </div>
          </div>
        `).join('');
    }
  },

  async fixed(main) {
    main.innerHTML = `
      <h2>Fixed-seed decay</h2>
      <p class="lede">Bitcoin Core ships a compiled-in fallback list of peer addresses (the "fixed seeds") that get used when DNS seeds fail. We parse those lists from each Core release tag and probe the clearnet (IPv4/IPv6) entries. <b>Recent reachable</b> means a successful handshake in the last 24h; <b>age</b> is time since release. As releases age, their fixed seeds gradually go dark — that's the decay.</p>
      <div class="card">
        <h3>Per-release reachability — <span style="color:var(--orange)">${state.network}</span></h3>
        <div id="fs-table"></div>
        <p style="color:var(--muted);font-size:12px;margin-top:14px">Onion / i2p / cjdns entries are excluded — probing them needs Tor / I2P infrastructure not present here. Counts reflect only the clearnet portion of each release's fixed seed list.</p>
      </div>
    `;
    const data = await api('/api/fixed-seeds/decay');
    const releases = data.releases || [];
    if (releases.length === 0) {
      main.querySelector('#fs-table').innerHTML = '<p style="color:var(--muted)">No fixed-seed data yet — ingestion runs at startup and probes every 6 hours. Check back shortly.</p>';
      return;
    }
    const now = Math.floor(Date.now() / 1000);
    const fmtAge = (secs) => {
      const d = Math.floor(secs / 86400);
      if (d < 60)  return `${d}d`;
      if (d < 700) return `${Math.floor(d/30)}mo`;
      return `${(d/365).toFixed(1)}y`;
    };
    const fmtDateShort = (ts) => new Date(ts * 1000).toISOString().slice(0, 10);
    const rows = releases.map(r => {
      const pct = r.total > 0 ? (100 * r.reachable_recent / r.total) : 0;
      const ageSecs = now - r.released_at;
      const colour = pct >= 50 ? PALETTE.orange : (pct >= 25 ? PALETTE.mustard : PALETTE.rust);
      return `<tr>
        <td class="hostname">${r.tag}</td>
        <td>${fmtDateShort(r.released_at)}</td>
        <td class="num">${fmtAge(ageSecs)}</td>
        <td class="num">${r.total}</td>
        <td class="num">${r.probed_recent}</td>
        <td class="num">${r.reachable_recent}</td>
        <td class="num" style="color:${colour}">${r.total > 0 ? pct.toFixed(1) + '%' : '—'}</td>
      </tr>`;
    }).join('');
    main.querySelector('#fs-table').innerHTML = `
      <table>
        <thead><tr>
          <th>Release</th><th>Released</th><th class="num">Age</th>
          <th class="num">Total seeds</th><th class="num">Probed 24h</th>
          <th class="num">Reachable 24h</th><th class="num">Reachable %</th>
        </tr></thead>
        <tbody>${rows}</tbody>
      </table>`;
  },

  async methodology(main) {
    main.innerHTML = `
      <h2>Methodology</h2>
      <p class="lede">How this dashboard measures Bitcoin DNS seeds.</p>
      <div class="card">
        <h3>Collection</h3>
        <p>Every hour, we query each mainnet seed listed in Bitcoin Core's <code>chainparams.cpp</code> 20 times over a 5-minute window, mixing A and AAAA lookups. We dedupe the returned addresses and attempt a Bitcoin P2P handshake (version/verack) against each on port 8333 with a 10-second timeout. The exchange is closed immediately after verack — no data beyond the peer's advertised <code>version</code> message is collected.</p>
      </div>
      <div class="card">
        <h3>Classifications</h3>
        <ul>
          <li><b>pristine</b> — handshake succeeded; address was not in this seed's advertised set yesterday.</li>
          <li><b>stale</b> — handshake succeeded; address was already advertised by this seed yesterday.</li>
          <li><b>duplicate</b> — address was advertised by more than one seed in the same round.</li>
          <li><b>reachable</b> — handshake succeeded but the first day of data means we can't yet call it pristine or stale.</li>
          <li><b>unreachable</b> — connection or handshake failed.</li>
        </ul>
        <p>Definitions intentionally match <a href="https://21.ninja/dns-seeds/">21.ninja</a> so historical comparison is possible.</p>
      </div>
      <div class="card">
        <h3>Identification</h3>
        <p>Our probes identify themselves with the user agent <code>/DCTRL-YVR-seed-monitor:0.1.0/</code>. If you operate a node and would prefer to opt out, contact us.</p>
      </div>
    `;
  },
};

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, c => ({ '&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;','\'':'&#39;' })[c]);
}

async function refreshAlertsBadge() {
  try {
    const data = await api('/api/alerts');
    const n = (data.alerts || []).length;
    const badge = document.getElementById('alerts-badge');
    if (!badge) return;
    if (n > 0) {
      badge.textContent = String(n);
      badge.classList.add('shown');
    } else {
      badge.classList.remove('shown');
    }
  } catch (e) { /* ignore */ }
}

async function render() {
  const main = document.getElementById('main');
  main.innerHTML = '<p style="color:var(--muted);font-size:18px">loading…</p>';
  document.querySelectorAll('.sidebar nav a').forEach(a => {
    a.classList.toggle('active', a.dataset.page === state.page);
  });
  try {
    await pages[state.page](main);
    refreshAlertsBadge();
  } catch (e) {
    main.innerHTML = `<p style="color:${PALETTE.rust};">error: ${escapeHtml(e.message)}</p>`;
  }
}

const THEME_KEY = 'seed-monitor-theme';
const VALID_THEMES = new Set(['warm', 'cool', 'neutral']);

function applyTheme(name) {
  if (!VALID_THEMES.has(name)) name = 'warm';
  if (name === 'warm') document.documentElement.removeAttribute('data-theme');
  else document.documentElement.setAttribute('data-theme', name);
  localStorage.setItem(THEME_KEY, name);
  document.querySelectorAll('.theme-chip').forEach(c => {
    c.classList.toggle('active', c.dataset.theme === name);
  });
}

function init() {
  const hash = location.hash.slice(1);
  if (hash && pages[hash]) state.page = hash;
  document.querySelectorAll('.sidebar nav a').forEach(a => {
    a.addEventListener('click', e => {
      e.preventDefault();
      state.page = a.dataset.page;
      location.hash = state.page;
      render();
    });
  });
  document.getElementById('range').addEventListener('change', e => {
    state.rangeSecs = Number(e.target.value);
    render();
  });
  const netSel = document.getElementById('network');
  netSel.value = state.network;
  netSel.addEventListener('change', e => {
    state.network = e.target.value;
    try { localStorage.setItem(NETWORK_KEY, state.network); } catch (_) {}
    render();
  });
  document.querySelectorAll('.theme-chip').forEach(c => {
    c.addEventListener('click', () => applyTheme(c.dataset.theme));
  });
  applyTheme(localStorage.getItem(THEME_KEY) || 'warm');
  render();
}
init();
