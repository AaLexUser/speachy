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
  // The pill shows one line; the whole text is there on hover.
  label.title = label.textContent;
}

// --- The bars -------------------------------------------------------------
//
// They show how loud the speech is, as it is heard: the newest moment on the
// right, moving left, so what you see is the last half second of the voice.
// The engine sends each chunk's loudness over time with `chunk_ready`.
// While a voice loads or the next chunk is prepared there is nothing to
// hear, and a slow ripple runs through instead.

const bars = [...document.querySelectorAll(".bars i")];
const LOWEST = 3; // px
const TALLEST = 20; // px
const STEP = 55; // ms of speech from one bar to the next
const RISE = 30; // ms for a bar to follow the speech up…
const FALL = 95; // …and down again
// Speech is rarely quiet, so drawn as it is the bars would all stand tall.
// Raising loudness to this power keeps the peaks and lowers the rest.
const CONTRAST = 1.8;
const stillBars = matchMedia("(prefers-reduced-motion: reduce)").matches;

const wave = {
  levels: new Map(), // chunk index → { values, windowMs }
  heard: null, // the chunk being heard, and how far into it we are
  into: 0, // ms
  clock: 0, // ms; stands still while paused
  trail: [], // [clock, loudness 0–1], oldest first
  shown: bars.map(() => 0),
  frame: 0,
  last: 0,
};

/** Loudness right now, from 0 to 1. */
function loudnessNow(now) {
  if (view.state === "loading" || view.state === "preparing") {
    return 0.3 + 0.22 * Math.sin(now / 150);
  }
  if (view.state !== "speaking" || !wave.heard) return 0;
  const { values, windowMs } = wave.heard;
  const position = wave.into / windowMs;
  const index = Math.floor(position);
  // Past the end is the pause before the next chunk.
  const from = values[index] ?? 0;
  const to = values[index + 1] ?? 0;
  return ((from + (to - from) * (position - index)) / 100) ** CONTRAST;
}

/** What the trail held at `time`. */
function trailAt(time) {
  const trail = wave.trail;
  for (let i = trail.length - 1; i > 0; i--) {
    const [t0, v0] = trail[i - 1];
    const [t1, v1] = trail[i];
    if (time >= t0) return t1 === t0 ? v1 : v0 + ((v1 - v0) * Math.min(1, (time - t0) / (t1 - t0)));
  }
  return trail.length && time >= trail[0][0] ? trail[0][1] : 0;
}

function draw(now) {
  // A frame can be long after the window was hidden; do not leap.
  const elapsed = Math.min(100, now - wave.last);
  wave.last = now;
  if (view.state !== "paused") {
    wave.clock += elapsed;
    if (view.state === "speaking") wave.into += elapsed;
    wave.trail.push([wave.clock, loudnessNow(now)]);
    const oldest = wave.clock - STEP * (bars.length + 1);
    while (wave.trail.length > 2 && wave.trail[1][0] < oldest) wave.trail.shift();
  }
  let showing = false;
  bars.forEach((bar, i) => {
    // Paused: every bar stays where it is.
    const target = view.state === "paused" ? wave.shown[i] : trailAt(wave.clock - STEP * (bars.length - 1 - i));
    const pace = target > wave.shown[i] ? RISE : FALL;
    wave.shown[i] += (target - wave.shown[i]) * (1 - Math.exp(-elapsed / pace));
    showing ||= wave.shown[i] > 0.004;
    bar.style.height = `${(LOWEST + (TALLEST - LOWEST) * wave.shown[i]).toFixed(2)}px`;
  });
  // Keep going while there is something to follow, or bars left to settle.
  const following = view.state !== "idle" && view.state !== "paused";
  wave.frame = following || (view.state === "idle" && showing) ? requestAnimationFrame(draw) : 0;
}

function animate() {
  if (stillBars) {
    // No motion wanted: a fixed shape that only says "speaking" or not.
    const shape = [0.3, 0.55, 0.8, 0.45, 0.7, 0.35, 0.6, 0.4, 0.25];
    bars.forEach((bar, i) => {
      const level = view.state === "idle" ? 0 : shape[i % shape.length];
      bar.style.height = `${LOWEST + (TALLEST - LOWEST) * level}px`;
    });
    return;
  }
  if (!wave.frame) {
    wave.last = performance.now();
    wave.frame = requestAnimationFrame(draw);
  }
}

listen("speech", ({ payload: e }) => {
  if (e.type === "state") view.state = e.state;
  else if (e.type === "started") {
    Object.assign(view, { session: e.session, chunks: e.chunks, playing: -1, notice: null });
    wave.levels.clear();
    wave.heard = null;
  } else if (e.session !== view.session) return;
  else if (e.type === "chunk_ready" && e.levels?.length) {
    wave.levels.set(e.index, { values: e.levels, windowMs: (e.audio_secs * 1000) / e.levels.length });
  } else if (e.type === "chunk_playing") {
    view.playing = e.index;
    wave.heard = wave.levels.get(e.index) ?? null;
    wave.into = 0;
  } else if (e.type === "finished") {
    Object.assign(view, { chunks: [], playing: -1 });
    wave.heard = null;
  }
  render();
  animate();
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

const engine = (cmd) => invoke("engine", { command: { cmd } });
document.getElementById("pause").addEventListener("click", () => engine("toggle_pause"));
document.getElementById("stop").addEventListener("click", () => engine("stop"));
render();
