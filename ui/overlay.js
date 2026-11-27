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

const view = { state: "idle", session: null, chunks: [], playing: -1, notice: null, speed: null, captions: null };
let noticeTimer;

function render() {
  pill.dataset.state = view.state;
  pill.classList.toggle("compact", Boolean(view.captions));
  pauseIcon.innerHTML = view.state === "paused" ? PLAY : PAUSE;
  document.getElementById("pause").title = view.state === "paused" ? "Resume" : "Pause";
  if (view.notice) {
    label.textContent = view.notice;
  } else if (view.state === "speaking" && view.captions) {
    // The captions show the words; the pill does not repeat them.
    label.textContent = "";
  } else if (view.state === "speaking" && view.chunks[view.playing]) {
    // The words being read give a better sense of position than a counter.
    label.textContent = view.chunks[view.playing].text;
  } else {
    label.textContent = TEXT[view.state] || "Speachy";
  }
  // The pill shows one line; the whole text is there on hover.
  label.title = label.textContent;
  renderSpeed();
}

/** 1.0×, 1.2×, 1.25× */
function speedText(speed) {
  const hundredths = Math.round(speed * 100);
  return (hundredths / 100).toFixed(hundredths % 10 === 0 ? 1 : 2) + "×";
}

// The speed, which is also the reset button: at 1.0, it goes back.
function renderSpeed() {
  const button = document.getElementById("speed");
  const settings = view.speed;
  button.hidden = !settings;
  if (!settings) return;
  const normal = Math.round(settings.speed * 100) === 100;
  const target = normal ? settings.speed_before_reset : 1;
  button.textContent = speedText(settings.speed);
  const back = target === settings.speed ? "" : `Back to ${speedText(target)}`;
  button.title = back || "Speed";
  button.setAttribute("aria-label", `Speed ${speedText(settings.speed)}. ${back}`.trim());
}

// --- The bars -------------------------------------------------------------
//
// They show how loud the speech is, as it is heard. The middle bar is this
// moment; each bar further out is a moment earlier, so every syllable runs
// from the middle to both ends, and the outer bars are kept shorter. The
// engine sends each chunk's loudness over time with `chunk_ready`.
// While a voice loads or the next chunk is prepared there is nothing to
// hear, and a small ripple passes through instead. Otherwise: dots.

const bars = [...document.querySelectorAll(".bars i")];
const MIDDLE = (bars.length - 1) / 2;
const LOWEST = 2.5; // px: a dot
const TALLEST = 18; // px
const STEP = 45; // ms of speech from one bar to the next one out
const RISE = 40; // ms for a bar to follow the speech up…
const FALL = 150; // …and down again
// Speech is rarely quiet, so drawn as it is the bars would all stand tall.
// Raising loudness to this power keeps the peaks and lowers the rest; the
// gain then lets ordinary syllables, not only the loudest, reach the top.
const CONTRAST = 1.5;
const GAIN = 1.3;
const stillBars = matchMedia("(prefers-reduced-motion: reduce)").matches;
/** How much of the loudness a bar shows: all of it in the middle. */
const share = (i) => 1 - 0.6 * (Math.abs(i - MIDDLE) / MIDDLE) ** 1.4;

const wave = {
  levels: new Map(), // chunk index → { values, windowMs }
  heard: null, // the chunk being heard, and how far into it we are
  into: 0, // ms
  trail: [], // [time, loudness 0–1] of the last moments, oldest first
  shown: bars.map(() => 0),
  frame: 0,
  last: 0,
};

/** Loudness of the speech right now, from 0 to 1. */
function loudnessNow() {
  if (view.state !== "speaking" || !wave.heard) return 0;
  const { values, windowMs } = wave.heard;
  const position = wave.into / windowMs;
  const index = Math.floor(position);
  // Past the end is the pause before the next chunk.
  const from = values[index] ?? 0;
  const to = values[index + 1] ?? 0;
  return Math.min(1, GAIN * ((from + (to - from) * (position - index)) / 100) ** CONTRAST);
}

/** What the trail held at `time`. */
function trailAt(time) {
  const trail = wave.trail;
  for (let i = trail.length - 1; i > 0; i--) {
    const [t0, v0] = trail[i - 1];
    const [t1, v1] = trail[i];
    if (time >= t0) return t1 === t0 ? v1 : v0 + (v1 - v0) * Math.min(1, (time - t0) / (t1 - t0));
  }
  return trail.length && time >= trail[0][0] ? trail[0][1] : 0;
}

function draw(now) {
  // A frame can be long after the window was hidden; do not leap.
  const elapsed = Math.min(100, now - wave.last);
  wave.last = now;
  // Levels are for natural pace; the voice is heard `speed` times faster.
  if (view.state === "speaking") wave.into += elapsed * (view.speed?.speed ?? 1);
  wave.trail.push([now, loudnessNow()]);
  const oldest = now - STEP * (MIDDLE + 2);
  while (wave.trail.length > 2 && wave.trail[1][0] < oldest) wave.trail.shift();

  const waiting = view.state === "loading" || view.state === "preparing";
  let showing = false;
  bars.forEach((bar, i) => {
    let target = 0;
    if (view.state === "speaking") target = share(i) * trailAt(now - STEP * Math.abs(i - MIDDLE));
    else if (waiting) target = 0.1 + 0.2 * (0.5 + 0.5 * Math.sin(now / 190 - i * 0.8));
    const pace = target > wave.shown[i] ? RISE : FALL;
    wave.shown[i] += (target - wave.shown[i]) * (1 - Math.exp(-elapsed / pace));
    showing ||= wave.shown[i] > 0.004;
    bar.style.height = `${(LOWEST + (TALLEST - LOWEST) * wave.shown[i]).toFixed(2)}px`;
  });
  // Keep going while there is something to follow, or bars left to settle.
  wave.frame = view.state === "speaking" || waiting || showing ? requestAnimationFrame(draw) : 0;
}

function animate() {
  if (stillBars) {
    // No motion wanted: a fixed shape that only says "speaking" or not.
    bars.forEach((bar, i) => {
      const level = view.state === "speaking" ? 0.75 * share(i) : 0;
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

// Puts `text` on the label for a moment.
function flash(text) {
  view.notice = text;
  clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => {
    view.notice = null;
    render();
  }, 2400);
  render();
}

listen("notice", ({ payload }) => flash(payload));

listen("settings", ({ payload }) => {
  const changed = view.speed && view.speed.speed !== payload.speed;
  view.speed = { speed: payload.speed, speed_before_reset: payload.speed_before_reset };
  if (view.captions !== payload.captions) {
    view.captions = payload.captions;
    render();
  }
  // The readout has the number; the label says what changed.
  if (changed) flash("Speed");
  else renderSpeed();
});

const engine = (cmd) => invoke("engine", { command: { cmd } });
document.getElementById("pause").addEventListener("click", () => engine("toggle_pause"));
document.getElementById("stop").addEventListener("click", () => engine("stop"));
document.getElementById("speed").addEventListener("click", () => engine("reset_speed"));
render();
engine("snapshot").then(({ settings }) => {
  // A settings event may already have brought a newer speed.
  view.speed ??= { speed: settings.speed, speed_before_reset: settings.speed_before_reset };
  view.captions ??= settings.captions;
  render();
});
