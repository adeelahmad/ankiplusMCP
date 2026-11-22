const invoke = window.__TAURI__ ? window.__TAURI__.core.invoke : async () => { throw new Error('Run inside the AnkiPlus app.'); };
const $ = (id) => document.getElementById(id);

function status(id, text, error = false) {
  const el = $(id);
  el.textContent = text;
  el.classList.toggle('error', error);
}

async function busy(button, fn) {
  button.disabled = true;
  try { await fn(); } finally { button.disabled = false; }
}

$('index').addEventListener('click', () => busy($('index'), async () => {
  status('index-status', 'Indexing… a large deck with embeddings takes a few minutes the first time.');
  try {
    const info = await invoke('index_deck', { args: {
      deckPath: $('deck').value.trim() || null,
      profile: $('deck').value.trim() ? null : ($('profile').value || null),
      deckPrefix: $('prefix').value.trim() || null,
      embeddings: $('embeddings').value,
      postgres: $('postgres').checked,
      rebuild: $('rebuild').checked,
    } });
    status('index-status', `${info.source}: ${info.notes} notes, ${info.suspendedCards} suspended cards, embeddings ${info.embeddings ? 'on' : 'off'}.`);
  } catch (e) { status('index-status', String(e), true); }
}));

invoke('list_anki_profiles').then(({ profiles }) => {
  for (const p of profiles) {
    const o = document.createElement('option');
    o.value = o.textContent = p.profile;
    $('profile').appendChild(o);
  }
}).catch(() => {});

let lastCards = [];

$('record').addEventListener('click', () => busy($('record'), async () => {
  try {
    await invoke('record_unsuspend', { cardIds: lastCards, status: 'success' });
    status('record-status', `Logged ${lastCards.length} cards; later lectures will not propose them again.`);
  } catch (e) { status('record-status', String(e), true); }
}));

function cell(row, text, cls) {
  const td = row.insertCell();
  td.textContent = text;
  if (cls) td.className = cls;
}

$('match').addEventListener('click', () => busy($('match'), async () => {
  status('match-status', 'Matching…');
  const pdf = $('pdf').value.trim();
  try {
    const { report, prompt } = await invoke('match_source', { args: {
      source: pdf ? { pdfPath: pdf } : { text: $('text').value },
      sourceName: pdf ? pdf.split(/[\\/]/).pop() : 'pasted text',
      covered: $('covered').value ? parseFloat($('covered').value) : null,
      related: $('related').value ? parseFloat($('related').value) : null,
      unsuspendConcepts: $('concepts').checked,
    } });
    const s = report.summary;
    $('summary').textContent = `${s.chunks} chunks: ${s.covered} covered, ${s.related} related, ${s.gap} gaps. ` +
      `${s.cards_to_unsuspend} suspended cards on ${s.notes_to_unsuspend} notes to unsuspend.`;
    $('unsuspend').value = report.unsuspend.card_ids.join(', ');
    const body = $('chunks');
    body.replaceChildren();
    for (const r of report.chunks) {
      const row = body.insertRow();
      cell(row, `p${r.page}`);
      cell(row, r.status, r.status);
      cell(row, r.best_score.toFixed(2), 'num');
      cell(row, r.text);
      cell(row, r.matches.length ? `${r.matches[0].note_id}: ${r.matches[0].preview}` : '');
    }
    $('prompt').value = prompt;
    lastCards = report.unsuspend.card_ids;
    $('results').hidden = false;
    status('match-status', 'Done. Unsuspend the listed CARD ids through the Anki MCP after review.');
  } catch (e) { status('match-status', String(e), true); }
}));

// ---- first-run wizard -------------------------------------------------------------

const listen = window.__TAURI__ ? window.__TAURI__.event.listen : async () => () => {};

function fillDecks(decks) {
  const sel = $('w-deck');
  sel.length = 1;
  for (const d of decks.filter((d) => !d.name.includes('::') || d.name.split('::').length <= 2)) {
    const o = document.createElement('option');
    o.value = d.name;
    o.textContent = `${d.name.replaceAll('::', ' › ')} (${d.notes.toLocaleString()} notes, ${d.suspended_cards.toLocaleString()} suspended)`;
    sel.appendChild(o);
  }
  const anking = decks.find((d) => /^anking/i.test(d.name) && !d.name.includes('::'));
  if (anking) sel.value = anking.name;
}

async function loadDecks() {
  status('anki-status', 'Reading your decks…');
  try {
    const decks = await invoke('list_decks', { profile: $('w-profile').value || null });
    fillDecks(decks);
    status('anki-status', `Found ${decks.filter((d) => !d.name.includes('::')).length} decks in profile "${$('w-profile').value}".`);
  } catch (e) { status('anki-status', String(e), true); }
}

async function setup() {
  let s;
  try { s = await invoke('setup_status'); } catch (e) { status('anki-status', String(e), true); return; }
  if (!s.profiles.length) {
    status('anki-status', `Anki not found in ${s.ankiBase}. Install Anki and open it once, then reopen this app.`, true);
  } else {
    const sel = $('w-profile');
    for (const p of s.profiles) {
      const o = document.createElement('option');
      o.value = o.textContent = p.profile;
      sel.appendChild(o);
    }
    sel.value = (s.profiles.find((p) => p.profile === 'User 1') || s.profiles[0]).profile;
    sel.addEventListener('change', loadDecks);
    loadDecks();
  }
  if (s.index) status('index-wizard-status', `Already indexed: ${s.index.notes} notes${s.index.deckPrefix ? ` from ${s.index.deckPrefix}` : ''}. Run again after big deck updates (Claude also updates it by itself).`);
  $('w-roots').value = s.sourceRoots.join('\n');
  if (!s.claudeInstalled) status('claude-status', 'Claude Desktop not found. Install it from claude.ai/download, sign in, then connect.', true);
  else if (s.registered) status('claude-status', 'Claude Desktop is already connected. Connect again to update it.');
  if (!s.uvBundled) status('claude-status', 'This build cannot connect Claude (no bundled runtime). Use the release download.', true);
}

$('w-index').addEventListener('click', async () => {
  $('w-index').disabled = true;
  $('w-progress').hidden = false;
  $('w-progress').removeAttribute('value');
  status('index-wizard-status', 'Starting…');
  try {
    await invoke('start_index', { profile: $('w-profile').value || null, deckPrefix: $('w-deck').value || null });
  } catch (e) { status('index-wizard-status', String(e), true); $('w-index').disabled = false; $('w-progress').hidden = true; }
});

listen('index-progress', ({ payload: p }) => {
  if (p.total) {
    $('w-progress').max = p.total;
    $('w-progress').value = p.done;
    status('index-wizard-status', `Embedding ${p.done.toLocaleString()} of ${p.total.toLocaleString()} notes…`);
  } else {
    $('w-progress').removeAttribute('value');
    status('index-wizard-status', p.stage.charAt(0).toUpperCase() + p.stage.slice(1) + '…');
  }
});

listen('index-done', ({ payload: d }) => {
  $('w-index').disabled = false;
  $('w-progress').hidden = true;
  if (!d.ok) { status('index-wizard-status', d.error, true); return; }
  const r = d.result;
  status('index-wizard-status', `Indexed ${r.notes.toLocaleString()} notes (${r.suspended_cards.toLocaleString()} suspended cards)` +
    (r.reused ? `; ${r.embedded} new or changed, ${r.reused} reused.` : '.') + (r.embeddings ? '' : ' Keyword search only: the search model could not load.'));
  $('w-index').textContent = 'Index again';
});

$('w-claude').addEventListener('click', () => busy($('w-claude'), async () => {
  $('warmup').textContent = '';
  try {
    const roots = $('w-roots').value.split('\n').map((r) => r.trim()).filter(Boolean);
    const r = await invoke('register_claude', { sourceRoots: roots });
    status('claude-status', `Claude Desktop ${r.status === 'unchanged' ? 'was already set up' : 'is set up'}` +
      (r.backup ? ` (old settings saved as ${r.backup})` : '') + '. Preparing the tools once…');
    $('warmup').hidden = false;
  } catch (e) { status('claude-status', String(e), true); }
}));

listen('warmup-progress', ({ payload: line }) => {
  const log = $('warmup');
  log.textContent += line + '\n';
  log.scrollTop = log.scrollHeight;
});

listen('warmup-done', ({ payload: d }) => {
  const missing = (d.result?.checks || []).filter((c) => c.status === 'missing' && c.required);
  if (d.ok || (d.result && !missing.length)) {
    status('claude-status', 'Claude Desktop is ready.');
    $('step-done').hidden = false;
  } else {
    status('claude-status', `Set up, but the first start failed: ${d.error || missing.map((c) => c.name).join(', ')}. ` +
      'Check your internet connection and press Connect again.', true);
  }
});

setup();
