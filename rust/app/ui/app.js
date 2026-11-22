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
      deckPath: $('deck').value.trim(),
      deckPrefix: $('prefix').value.trim() || null,
      embeddings: $('embeddings').value,
      postgres: $('postgres').checked,
      rebuild: $('rebuild').checked,
    } });
    status('index-status', `${info.source}: ${info.notes} notes, ${info.suspendedCards} suspended cards, embeddings ${info.embeddings ? 'on' : 'off'}.`);
  } catch (e) { status('index-status', String(e), true); }
}));

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
