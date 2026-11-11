// Speachy settings window. Plain DOM, no build step.
"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const S = {
  snap: null,
  page: "home",
  speech: { state: "idle", session: null, chunks: [], playing: -1, ready: -1 },
  /** Model whose voices the Voices page shows. */
  voiceModel: null,
  /** Shortcut field being recorded, or null. */
  recording: null,
  shortcutErrors: [],
  /** State of the app updater: { status: "idle" | "checking" | "available" | … }. */
  update: { status: "idle" },
  draft: "Speachy reads any text you select, in any app. The first sentence starts within a moment, and the rest is prepared while you listen, so there are no pauses between sentences.",
};

const main = document.getElementById("main");
const isMac = () => S.snap && S.snap.platform === "macos";

// ---------- helpers ----------

const esc = (s) =>
  String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

function fmtBytes(n) {
  if (n >= 1e9) return (n / 1e9).toFixed(1) + " GB";
  return Math.round(n / 1e6) + " MB";
}

function prettyKey(code) {
  const named = { Space: "Space", ArrowUp: "↑", ArrowDown: "↓", ArrowLeft: "←", ArrowRight: "→", Escape: "Esc", Enter: "↩", Backquote: "`", Minus: "-", Equal: "=", Comma: ",", Period: ".", Slash: "/", Semicolon: ";", Quote: "'", BracketLeft: "[", BracketRight: "]", Backslash: "\\" };
  if (named[code]) return named[code];
  return code.replace(/^Key/, "").replace(/^Digit/, "");
}

/** "Ctrl+Alt+KeyS" → keycap HTML. */
function keycaps(shortcut) {
  if (!shortcut) return '<span class="muted">Not set</span>';
  const mac = { ctrl: "⌃", control: "⌃", alt: "⌥", option: "⌥", shift: "⇧", super: "⌘", cmd: "⌘", command: "⌘" };
  const other = { ctrl: "Ctrl", control: "Ctrl", alt: "Alt", option: "Alt", shift: "Shift", super: "Win", cmd: "Win", command: "Win" };
  return shortcut
    .split("+")
    .map((token) => {
      const mod = (isMac() ? mac : other)[token.toLowerCase()];
      return `<kbd>${esc(mod || prettyKey(token))}</kbd>`;
    })
    .join("");
}

const models = () => S.snap.models;
const installed = () => models().filter((m) => m.installed);
const modelById = (id) => models().find((m) => m.id === id);
const langName = (code) => S.snap.languages[code] || code;

/** The voice used when nothing more specific applies. */
function defaultVoice() {
  const chosen = S.snap.settings.default_voice;
  const model = chosen && installed().find((m) => m.id === chosen.model_id);
  const voice = model && model.voices.find((v) => v.id === chosen.voice_id);
  if (voice) return { model, voice };
  const first = installed()[0];
  return first ? { model: first, voice: first.voices.find((v) => v.id === first.default_voice) } : null;
}

function voiceLabel(model, voice) {
  const accent = voice.accent ? ` (${voice.accent})` : "";
  return `${model.name} · ${voice.name}${accent}`;
}

let toastTimer;
function toast(message, kind = "info") {
  const el = document.getElementById("toast");
  el.textContent = message;
  el.dataset.kind = kind;
  el.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.remove("show"), 4200);
}

async function call(command, args) {
  try {
    return await invoke(command, args);
  } catch (err) {
    toast(String(err), "error");
    throw err;
  }
}

/** Runs an engine command: the same commands the CLI and `serve` take. */
const engine = (cmd, args = {}) => call("engine", { command: { cmd, ...args } });

async function refresh() {
  const [snapshot, shell] = await Promise.all([
    invoke("engine", { command: { cmd: "snapshot" } }),
    invoke("shell_info"),
  ]);
  S.snap = { ...snapshot, ...shell };
  if (shell.updates_supported) S.update = await invoke("update_status");
  S.speech.state = S.snap.speech_state;
  render();
}

async function saveSettings(patch, { rerender = true } = {}) {
  const result = await engine("update_settings", { patch });
  S.snap.settings = result.settings;
  S.shortcutErrors = result.shortcut_errors;
  if (result.shortcut_errors.length) toast(result.shortcut_errors[0], "error");
  if (rerender) render();
}

// ---------- pages ----------

function pageHome() {
  const voice = defaultVoice();
  const shortcut = S.snap.settings.shortcuts.speak_selection;
  const banners = [];
  if (!installed().length) {
    banners.push(`
      <div class="banner">
        <div><strong>Download a voice to get started.</strong>
        <span>Speachy speaks with models that run on this computer. Supertonic 3 is a good first choice.</span></div>
        <button class="primary" data-action="goto" data-page="models">Choose a model</button>
      </div>`);
  }
  if (isMac() && !S.snap.accessibility) {
    banners.push(`
      <div class="banner warn">
        <div><strong>Allow Accessibility access.</strong>
        <span>macOS requires it before Speachy can read the text you have selected in other apps.</span></div>
        <button data-action="grant">Open settings</button>
      </div>`);
  }
  if (S.update.status === "available") {
    banners.unshift(`
      <div class="banner" id="update-banner">
        <div><strong>Speachy ${esc(S.update.version)} is available.</strong>
        <span>You have ${esc(S.snap.version)}. Updating takes a moment and restarts the app.</span></div>
        <button class="primary" data-action="install-update">Update and restart</button>
      </div>`);
  }
  return `
    <header class="page-head">
      <h1>Hear any text read aloud</h1>
      <p class="lede">Select text in any app and press ${keycaps(shortcut)}. Press it again to stop.</p>
    </header>
    ${banners.join("")}
    <section class="card">
      <div class="card-head">
        <h2>Try it</h2>
        <span class="meta">${voice ? esc(voiceLabel(voice.model, voice.voice)) : "No voice installed"}</span>
      </div>
      <textarea id="try-text" spellcheck="false" placeholder="Type or paste text to hear it…">${esc(S.draft)}</textarea>
      <div id="reading" class="reading" hidden></div>
      <div class="row">
        <button class="primary" id="speak-btn" data-action="speak" ${voice ? "" : "disabled"}>Speak</button>
        <button id="pause-btn" data-action="pause" hidden>Pause</button>
        <span class="spacer"></span>
        <span class="hint" id="speech-hint"></span>
      </div>
    </section>
    <section class="steps">
      <div><span class="num">1</span><h3>Select</h3><p>Highlight text in a browser, a PDF, your editor, anywhere.</p></div>
      <div><span class="num">2</span><h3>Press ${keycaps(shortcut)}</h3><p>Speachy stays in the menu bar and listens for the shortcut.</p></div>
      <div><span class="num">3</span><h3>Listen</h3><p>Speech starts with the first sentence while the rest is prepared ahead, so playback never waits.</p></div>
    </section>`;
}

function modelCard(m) {
  const langs = m.languages.length > 4 ? `${m.languages.length} languages` : m.languages.map(langName).join(", ");
  const speed = ["", "Slower", "Fast", "Very fast"][m.speed_tier] || "";
  let action;
  if (m.installing) {
    action = `<div class="progress" data-model="${esc(m.id)}">${progressHtml(m.progress)}</div>
      <button data-action="cancel-install" data-id="${esc(m.id)}">Cancel</button>`;
  } else if (m.installed) {
    action = `<span class="installed">Installed</span>
      <button class="quiet danger" data-action="remove" data-id="${esc(m.id)}">Remove</button>`;
  } else {
    const resume = m.partial_bytes > 0 ? "Resume download" : "Download";
    action = `<button class="primary" data-action="install" data-id="${esc(m.id)}">${resume} · ${fmtBytes(m.size_bytes)}</button>`;
  }
  return `
    <article class="card model" data-installed="${m.installed}">
      <div class="model-info">
        <h2>${esc(m.name)} ${m.recommended ? '<span class="tag accent">Recommended</span>' : ""}</h2>
        <p>${esc(m.description)}</p>
        <div class="facts">
          <span>${esc(langs)}</span><span>${m.voices.length} voices</span><span>${esc(speed)}</span><span>${esc(m.license)}</span>
        </div>
      </div>
      <div class="model-action">${action}</div>
    </article>`;
}

function progressHtml(p) {
  if (!p) return `<div class="bar"><i style="width:0%"></i></div><span>Starting…</span>`;
  const pct = p.total_bytes ? Math.min(100, (p.done_bytes / p.total_bytes) * 100) : 0;
  const label = p.phase === "extracting" ? "Unpacking…" : `${fmtBytes(p.done_bytes)} of ${fmtBytes(p.total_bytes)}`;
  return `<div class="bar"><i style="width:${pct.toFixed(1)}%"></i></div><span>${label}</span>`;
}

function pageModels() {
  return `
    <header class="page-head">
      <h1>Models</h1>
      <p class="lede">Voices run on this computer; nothing you read leaves it. Download one or more models, then pick voices.</p>
    </header>
    ${models().map(modelCard).join("")}`;
}

function voiceOptions(lang, selected) {
  // Voices of installed models that can speak `lang`, grouped by model.
  return installed()
    .map((m) => {
      const options = m.voices
        .filter((v) => !lang || v.languages.includes(lang))
        .map((v) => {
          const value = `${m.id}|${v.id}`;
          const on = selected && selected.model_id === m.id && selected.voice_id === v.id;
          const accent = v.accent ? `, ${v.accent}` : "";
          return `<option value="${esc(value)}" ${on ? "selected" : ""}>${esc(v.name)} (${v.gender}${esc(accent)})</option>`;
        })
        .join("");
      return options ? `<optgroup label="${esc(m.name)}">${options}</optgroup>` : "";
    })
    .join("");
}

function pageVoices() {
  const s = S.snap.settings;
  const current = defaultVoice();
  if (!current) {
    return `
      <header class="page-head"><h1>Voices</h1></header>
      <div class="banner"><div><strong>No model installed yet.</strong><span>Voices come with the models you download.</span></div>
      <button class="primary" data-action="goto" data-page="models">Choose a model</button></div>`;
  }
  const shown = installed().find((m) => m.id === S.voiceModel) || current.model;
  const tabs = installed()
    .map((m) => `<button class="${m.id === shown.id ? "active" : ""}" data-action="voice-model" data-id="${esc(m.id)}">${esc(m.name)}</button>`)
    .join("");
  const cards = shown.voices
    .map((v) => {
      const on = current.model.id === shown.id && current.voice.id === v.id;
      const sub = [v.gender === "female" ? "Female" : "Male", v.accent].filter(Boolean).join(" · ");
      return `
        <div class="voice ${on ? "selected" : ""}" role="button" tabindex="0" data-action="pick-voice" data-model="${esc(shown.id)}" data-voice="${esc(v.id)}">
          <span class="voice-name">${esc(v.name)}</span><span class="voice-sub">${esc(sub)}</span>
          <button class="play" title="Preview" aria-label="Preview ${esc(v.name)}" data-action="preview" data-model="${esc(shown.id)}" data-voice="${esc(v.id)}">
            <svg viewBox="0 0 12 12"><path d="M3 1.8v8.4L10 6z"/></svg>
          </button>
        </div>`;
    })
    .join("");

  const allLangs = [...new Set(installed().flatMap((m) => m.languages))].sort((a, b) => langName(a).localeCompare(langName(b)));
  const langOptions = (selected) => allLangs.map((c) => `<option value="${c}" ${c === selected ? "selected" : ""}>${esc(langName(c))}</option>`).join("");
  const overrides = Object.entries(s.language_voices)
    .map(
      ([lang, ref]) => `
        <div class="override">
          <span class="lang">${esc(langName(lang))}</span>
          <select data-change="override-voice" data-lang="${lang}">${voiceOptions(lang, ref)}</select>
          <button class="play" title="Preview" data-action="preview" data-model="${esc(ref.model_id)}" data-voice="${esc(ref.voice_id)}" data-lang="${lang}"><svg viewBox="0 0 12 12"><path d="M3 1.8v8.4L10 6z"/></svg></button>
          <button class="quiet" data-action="remove-override" data-lang="${lang}" aria-label="Remove">✕</button>
        </div>`,
    )
    .join("");
  const unused = allLangs.filter((c) => !(c in s.language_voices));

  return `
    <header class="page-head">
      <h1>Voices</h1>
      <p class="lede">Pick the voice Speachy reads with, and optionally a different voice for specific languages.</p>
    </header>
    <section class="card">
      <div class="card-head"><h2>Default voice</h2><span class="meta">${esc(voiceLabel(current.model, current.voice))}</span></div>
      ${installed().length > 1 ? `<div class="tabs">${tabs}</div>` : ""}
      <div class="voices">${cards}</div>
    </section>
    <section class="card">
      <div class="card-head"><h2>Speed</h2><span class="meta" id="speed-label">${s.speed.toFixed(2)}×</span></div>
      <input type="range" min="0.5" max="2.5" step="0.05" value="${s.speed}" data-input="speed" aria-label="Speech speed" />
    </section>
    <section class="card">
      <div class="card-head"><h2>Language</h2></div>
      <label class="toggle"><input type="checkbox" data-change="auto-detect" ${s.auto_detect_language ? "checked" : ""} />
        <span><strong>Detect the language of the text</strong><small>Each part of the text is read in its own language, with a voice that speaks it.</small></span></label>
      <label class="field"><span>${s.auto_detect_language ? "When unsure, read as" : "Always read as"}</span>
        <select data-change="language">${langOptions(s.language)}</select></label>
    </section>
    <section class="card">
      <div class="card-head"><h2>Voices by language</h2></div>
      <p class="note">Choose a specific voice for a language. Languages without one use the default voice, or another installed model that speaks them.</p>
      ${overrides || '<p class="muted empty">No language-specific voices yet.</p>'}
      ${unused.length ? `<div class="row"><select id="add-lang">${unused.map((c) => `<option value="${c}">${esc(langName(c))}</option>`).join("")}</select><button data-action="add-override">Add language</button></div>` : ""}
    </section>`;
}

function shortcutRow(field, title, help) {
  const value = S.snap.settings.shortcuts[field];
  const recording = S.recording === field;
  return `
    <div class="setting">
      <div><strong>${title}</strong><small>${help}</small></div>
      <div class="shortcut">
        <button class="keys ${recording ? "recording" : ""}" data-action="record" data-field="${field}">
          ${recording ? "Press keys… (Esc to cancel)" : keycaps(value)}
        </button>
        ${value && !recording ? `<button class="quiet" data-action="clear-shortcut" data-field="${field}" aria-label="Clear">✕</button>` : ""}
      </div>
    </div>`;
}

function toggleRow(field, title, help) {
  return `
    <label class="setting">
      <div><strong>${title}</strong><small>${help}</small></div>
      <input type="checkbox" class="switch" data-change="toggle" data-field="${field}" ${S.snap.settings[field] ? "checked" : ""} />
    </label>`;
}

/** The inside of the Updates card, redrawn as an update progresses. */
function updatesBody() {
  const u = S.update;
  const auto = S.snap.settings.check_for_updates;
  let text = "";
  let detail = "";
  let button = `<button data-action="check-updates">Check for updates</button>`;
  if (u.status === "checking") {
    text = "Checking…";
    button = `<button disabled>Check for updates</button>`;
  } else if (u.status === "up_to_date") {
    text = "You have the newest version.";
  } else if (u.status === "available") {
    text = `Version ${esc(u.version)} is available.`;
    if (u.notes) detail = `<pre class="notes">${esc(u.notes.trim().slice(0, 1200))}</pre>`;
    button = `<button class="primary" data-action="install-update">Update and restart</button>`;
  } else if (u.status === "downloading") {
    const pct = u.total ? Math.min(100, (u.downloaded / u.total) * 100) : 0;
    text = u.total ? `Downloading version ${esc(u.version)}… ${Math.round(pct)}%` : `Downloading version ${esc(u.version)}…`;
    detail = `<div class="bar"><i style="width:${pct.toFixed(1)}%"></i></div>`;
    button = "";
  } else if (u.status === "installed") {
    text = `Version ${esc(u.version)} is installed. Restarting…`;
    button = "";
  } else if (u.status === "failed") {
    text = "";
    detail = `<p class="error">${esc(u.message)}</p>`;
    button = `<button data-action="check-updates">Check again</button>`;
  }
  return `
    <div class="setting">
      <div><strong>Speachy ${esc(S.snap.version)}</strong><small>${text}</small></div>
      ${button}
    </div>
    ${detail}
    <label class="setting">
      <div><strong>Check automatically</strong><small>Look for a new version now and then. Nothing is installed without asking.</small></div>
      <input type="checkbox" class="switch" data-change="toggle" data-field="check_for_updates" ${auto ? "checked" : ""} />
    </label>`;
}

function pageSettings() {
  const s = S.snap.settings;
  const errors = S.shortcutErrors.map((e) => `<p class="error">${esc(e)}</p>`).join("");
  return `
    <header class="page-head"><h1>Settings</h1></header>
    ${S.snap.updates_supported ? `<section class="card"><div class="card-head"><h2>Updates</h2></div><div id="updates-body">${updatesBody()}</div></section>` : ""}
    <section class="card">
      <div class="card-head"><h2>Shortcuts</h2></div>
      ${shortcutRow("speak_selection", "Speak selection", "Reads the selected text. Pressed again, stops.")}
      ${shortcutRow("pause_resume", "Pause / resume", "Holds playback where it is.")}
      ${shortcutRow("stop", "Stop", "Stops speaking immediately.")}
      ${errors}
    </section>
    <section class="card">
      <div class="card-head"><h2>General</h2></div>
      ${toggleRow("launch_at_login", "Launch at login", "Start Speachy in the menu bar when you log in.")}
      ${toggleRow("start_hidden", "Start in the menu bar", "Do not open this window when Speachy starts.")}
      ${toggleRow("show_overlay", "Show the floating pill", "A small control at the bottom of the screen while speaking.")}
      ${toggleRow("preload_model", "Keep the voice ready", "Load the default voice at launch so the first request starts sooner. Uses more memory.")}
    </section>
    <section class="card">
      <div class="card-head"><h2>Playback</h2></div>
      <div class="setting">
        <div><strong>Volume</strong></div>
        <div class="slider"><input type="range" min="0" max="1.5" step="0.05" value="${s.volume}" data-input="volume" aria-label="Volume" /><span id="volume-label">${Math.round(s.volume * 100)}%</span></div>
      </div>
      <div class="setting">
        <div><strong>Prepare ahead</strong><small>How much speech is synthesized ahead of playback. More protects against pauses on a busy machine; less wastes less work when you stop early.</small></div>
        <div class="slider"><input type="range" min="5" max="60" step="5" value="${s.lookahead_secs}" data-input="lookahead" aria-label="Seconds prepared ahead" /><span id="lookahead-label">${Math.round(s.lookahead_secs)} s</span></div>
      </div>
    </section>`;
}

const PAGES = { home: pageHome, models: pageModels, voices: pageVoices, settings: pageSettings };

function render() {
  if (!S.snap) return;
  document.querySelectorAll("#nav button").forEach((b) => b.classList.toggle("active", b.dataset.page === S.page));
  document.getElementById("version").textContent = "v" + S.snap.version;
  const scroll = main.scrollTop;
  main.innerHTML = `<div class="page">${PAGES[S.page]()}</div>`;
  main.scrollTop = scroll;
  updateSpeechUi();
}

// ---------- speech state ----------

const STATE_TEXT = { idle: "Ready", loading: "Loading voice…", preparing: "Preparing…", speaking: "Speaking", paused: "Paused" };

function updateSpeechUi() {
  const sp = S.speech;
  const status = document.getElementById("status");
  status.dataset.state = sp.state;
  document.getElementById("status-text").textContent = STATE_TEXT[sp.state] || sp.state;

  const speakBtn = document.getElementById("speak-btn");
  if (!speakBtn) return;
  const active = sp.state !== "idle";
  speakBtn.textContent = active ? "Stop" : "Speak";
  speakBtn.classList.toggle("stop", active);
  const pauseBtn = document.getElementById("pause-btn");
  pauseBtn.hidden = !active;
  pauseBtn.textContent = sp.state === "paused" ? "Resume" : "Pause";

  const reading = document.getElementById("reading");
  const textarea = document.getElementById("try-text");
  const show = active && sp.chunks.length > 0;
  reading.hidden = !show;
  textarea.hidden = show;
  if (show) {
    reading.innerHTML = sp.chunks
      .map((c, i) => {
        const cls = i === sp.playing ? "now" : i < sp.playing ? "done" : i <= sp.ready ? "ready" : "";
        const br = c.boundary === "paragraph" ? "<br><br>" : " ";
        return `<span class="${cls}">${esc(c.text)}</span>${br}`;
      })
      .join("");
    const now = reading.querySelector(".now");
    if (now) now.scrollIntoView({ block: "nearest" });
  }
  const hint = document.getElementById("speech-hint");
  if (show && sp.playing >= 0) {
    const c = sp.chunks[sp.playing];
    hint.textContent = `Part ${sp.playing + 1} of ${sp.chunks.length} · ${langName(c.lang)}`;
  } else {
    hint.textContent = active ? STATE_TEXT[sp.state] : "";
  }
}

function onSpeechEvent(e) {
  const sp = S.speech;
  if (e.type === "state") sp.state = e.state;
  else if (e.type === "started") Object.assign(sp, { session: e.session, chunks: e.chunks, playing: -1, ready: -1 });
  else if (e.type === "chunk_ready" && e.session === sp.session) sp.ready = Math.max(sp.ready, e.index);
  else if (e.type === "chunk_playing" && e.session === sp.session) sp.playing = e.index;
  else if (e.type === "finished" && e.session === sp.session) {
    Object.assign(sp, { chunks: [], playing: -1, ready: -1 });
    if (e.outcome === "failed" && e.message) toast(e.message, "error");
  }
  updateSpeechUi();
}

function onUpdateEvent(status) {
  const before = S.update.status;
  S.update = status;
  const body = document.getElementById("updates-body");
  if (body) body.innerHTML = updatesBody();
  // The Home banner appears and disappears with the "available" state.
  const flipped = (before === "available") !== (status.status === "available");
  if (S.page === "home" && flipped) render();
  if (status.status === "available" && before !== "available" && S.page !== "settings") {
    toast(`Speachy ${status.version} is available. See Settings → Updates.`);
  }
}

function onDownloadEvent(e) {
  const model = modelById(e.id);
  if (!model) return;
  if (e.finished) {
    if (e.error) toast(`${model.name}: ${e.error}`, "error");
    refresh();
    return;
  }
  model.installing = true;
  model.progress = e.progress;
  const el = document.querySelector(`.progress[data-model="${CSS.escape(e.id)}"]`);
  if (el) el.innerHTML = progressHtml(e.progress);
}

// ---------- actions ----------

function preview(modelId, voiceId, lang) {
  const model = modelById(modelId);
  const voice = model.voices.find((v) => v.id === voiceId);
  // Preview in the user's language when the voice speaks it.
  const preferred = lang || S.snap.settings.language;
  const code = voice.languages.includes(preferred) ? preferred : voice.languages[0];
  engine("speak", { text: window.SAMPLES[code] || window.SAMPLES.en, voice: { model_id: modelId, voice_id: voiceId }, lang: code });
}

const ACTIONS = {
  goto: (el) => {
    S.page = el.dataset.page;
    render();
  },
  grant: async () => {
    const granted = await call("request_accessibility");
    if (granted) refresh();
    else toast("Enable Speachy under Privacy & Security → Accessibility, then come back.");
  },
  speak: async () => {
    if (S.speech.state !== "idle") return engine("stop");
    const text = document.getElementById("try-text").value.trim();
    if (!text) return toast("Type some text first.");
    await engine("speak", { text });
  },
  pause: () => engine("toggle_pause"),
  "check-updates": () => call("check_for_updates"),
  "install-update": () => call("install_update"),
  install: async (el) => {
    const model = modelById(el.dataset.id);
    model.installing = true;
    render();
    await engine("install_model", { id: el.dataset.id }).catch(refresh);
  },
  "cancel-install": (el) => engine("cancel_install", { id: el.dataset.id }),
  remove: async (el) => {
    const model = modelById(el.dataset.id);
    if (!window.confirm(`Remove ${model.name} and its voices from this computer?`)) return;
    await engine("remove_model", { id: el.dataset.id });
    await refresh();
  },
  "voice-model": (el) => {
    S.voiceModel = el.dataset.id;
    render();
  },
  "pick-voice": (el) => saveSettings({ default_voice: { model_id: el.dataset.model, voice_id: el.dataset.voice } }),
  preview: (el) => preview(el.dataset.model, el.dataset.voice, el.dataset.lang),
  "add-override": () => {
    const lang = document.getElementById("add-lang").value;
    const model = installed().find((m) => m.voices.some((v) => v.languages.includes(lang)));
    const voice = model.voices.find((v) => v.languages.includes(lang));
    const next = { ...S.snap.settings.language_voices, [lang]: { model_id: model.id, voice_id: voice.id } };
    saveSettings({ language_voices: next });
  },
  "remove-override": (el) => {
    const next = { ...S.snap.settings.language_voices };
    delete next[el.dataset.lang];
    saveSettings({ language_voices: next });
  },
  record: async (el) => {
    if (S.recording) return stopRecording();
    S.recording = el.dataset.field;
    await call("set_shortcuts_suspended", { suspended: true });
    render();
  },
  "clear-shortcut": (el) => saveSettings({ shortcuts: { [el.dataset.field]: "" } }),
};

const CHANGES = {
  "auto-detect": (el) => saveSettings({ auto_detect_language: el.checked }),
  language: (el) => saveSettings({ language: el.value }),
  toggle: (el) => saveSettings({ [el.dataset.field]: el.checked }, { rerender: false }),
  "override-voice": (el) => {
    const [model_id, voice_id] = el.value.split("|");
    saveSettings({ language_voices: { ...S.snap.settings.language_voices, [el.dataset.lang]: { model_id, voice_id } } });
  },
};

// Sliders update their label while dragging and save when released.
const INPUTS = {
  speed: { label: "speed-label", text: (v) => v.toFixed(2) + "×", patch: (v) => ({ speed: v }) },
  volume: { label: "volume-label", text: (v) => Math.round(v * 100) + "%", patch: (v) => ({ volume: v }) },
  lookahead: { label: "lookahead-label", text: (v) => Math.round(v) + " s", patch: (v) => ({ lookahead_secs: v }) },
};

async function stopRecording() {
  S.recording = null;
  await call("set_shortcuts_suspended", { suspended: false });
  render();
}

function onKeyDown(e) {
  if (!S.recording) {
    // Enter/Space activate voice cards, which are not native buttons.
    if ((e.key === "Enter" || e.key === " ") && e.target.matches?.('[role="button"]')) {
      e.preventDefault();
      e.target.click();
    }
    return;
  }
  e.preventDefault();
  e.stopPropagation();
  if (e.code === "Escape") return stopRecording();
  if (["Control", "Alt", "Shift", "Meta"].includes(e.key)) return; // wait for the main key
  const mods = [];
  if (e.ctrlKey) mods.push("Ctrl");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  if (e.metaKey) mods.push("Super");
  const functionKey = /^F\d{1,2}$/.test(e.code);
  if (!mods.length && !functionKey) return toast("Use at least one modifier key (⌃, ⌥ or ⌘).");
  if (mods.length === 1 && mods[0] === "Shift" && !functionKey) return toast("Shift alone is not enough for a global shortcut.");
  const field = S.recording;
  S.recording = null;
  // Whether or not the save succeeds, the global shortcuts come back.
  saveSettings({ shortcuts: { [field]: [...mods, e.code].join("+") } })
    .catch(() => {})
    .then(stopRecording);
}

document.getElementById("nav").addEventListener("click", (e) => {
  const button = e.target.closest("button[data-page]");
  if (!button) return;
  if (S.recording) stopRecording();
  S.page = button.dataset.page;
  render();
  main.scrollTop = 0;
});

main.addEventListener("click", (e) => {
  const el = e.target.closest("[data-action]");
  if (el && ACTIONS[el.dataset.action]) ACTIONS[el.dataset.action](el);
});

main.addEventListener("change", (e) => {
  const el = e.target;
  if (el.dataset.change) CHANGES[el.dataset.change](el);
  const input = INPUTS[el.dataset.input];
  if (input) saveSettings(input.patch(Number(el.value)), { rerender: false });
});

main.addEventListener("input", (e) => {
  const el = e.target;
  if (el.id === "try-text") S.draft = el.value;
  const input = INPUTS[el.dataset.input];
  if (input) document.getElementById(input.label).textContent = input.text(Number(el.value));
});

window.addEventListener("keydown", onKeyDown, true);
// Never leave the global shortcuts released because the window lost focus
// in the middle of recording one.
window.addEventListener("blur", () => S.recording && stopRecording());
// The window may have been hidden while permissions changed in System Settings.
window.addEventListener("focus", () => S.snap && !S.recording && refresh());

listen("speech", (e) => onSpeechEvent(e.payload));
listen("download", (e) => onDownloadEvent(e.payload));
listen("notice", (e) => toast(e.payload));
listen("update", (e) => onUpdateEvent(e.payload));
listen("show-page", (e) => {
  if (PAGES[e.payload]) {
    S.page = e.payload;
    render();
  }
});
refresh();
