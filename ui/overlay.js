// The floating status pill shown while Speachy is speaking.
"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const pill = document.getElementById("pill");
const label = document.getElementById("label");
const pauseIcon = document.getElementById("pause-icon");
const PAUSE = '<path d="M2.5 1.5h2.4v9H2.5zm4.6 0h2.4v9H7.1z" />';
const PLAY = '<path d="M3 1.5v9L10.5 6z" />';
const TEXT = { loading: "Loading voice…", preparing: "Preparing…", paused: "Paused" };

const view = { state: "idle", session: null, chunks: [], playing: -1, notice: null };
let noticeTimer;

function render() {
  pill.dataset.state = view.state;
  pauseIcon.innerHTML = view.state === "paused" ? PLAY : PAUSE;
  document.getElementById("pause").title = view.state === "paused" ? "Resume" : "Pause";
  if (view.notice) {
    label.textContent = view.notice;
  } else if (view.state === "speaking" && view.chunks[view.playing]) {
    // The words being read give a better sense of position than a counter.
    label.textContent = view.chunks[view.playing].text;
  } else {
    label.textContent = TEXT[view.state] || "Speachy";
  }
}

listen("speech", ({ payload: e }) => {
  if (e.type === "state") view.state = e.state;
  else if (e.type === "started") Object.assign(view, { session: e.session, chunks: e.chunks, playing: -1, notice: null });
  else if (e.type === "chunk_playing" && e.session === view.session) view.playing = e.index;
  else if (e.type === "finished" && e.session === view.session) Object.assign(view, { chunks: [], playing: -1 });
  render();
});

listen("notice", ({ payload }) => {
  view.notice = payload;
  clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => {
    view.notice = null;
    render();
  }, 2400);
  render();
});

document.getElementById("pause").addEventListener("click", () => invoke("toggle_pause"));
document.getElementById("stop").addEventListener("click", () => invoke("stop"));
render();
