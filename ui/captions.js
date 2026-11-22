// Captions: the line of the word being read, the word highlighted.
//
// The window keeps no clock. The engine sends each chunk's words with
// `chunk_ready` and says which word is playing with `word_playing`, from the
// audio thread, so speed, pause and the change of chunk need nothing here.
"use strict";

const { listen } = window.__TAURI__.event;

const body = document.body;
const card = document.getElementById("card");
const line = document.getElementById("line");
const measure = document.getElementById("measure");
const chip = document.getElementById("chip");
/** Room in the card for a line, in px. */
const ROOM = 664 - 2;
/** How far the highlight reaches past its word on each side, in px. */
const PAD = 6;

const words = new Map(); // chunk index → [{ text, start_secs, line }]
let session = null;
let shown = null; // { index, line, scale, left } on screen
let width = null; // { index, px }: the card's width for a chunk
let spans = [];

// Han, kana and their punctuation are written without spaces;
// `captions::spaced` in the engine follows the same rule.
const TIGHT = /[　-〿぀-ヿ㐀-䶿一-鿿豈-﫿＀-｠]/u;
const spaced = (a, b) => !(TIGHT.test(a.at(-1) ?? "") || TIGHT.test(b[0] ?? ""));

/** Fills `into` with the words of line `number` of chunk `index`. */
function fill(into, index, number) {
  const all = words.get(index) ?? [];
  const on = all.map((w, n) => ({ ...w, n })).filter((w) => w.line === number);
  into.replaceChildren(
    ...on.map((w, k) => {
      const span = document.createElement("span");
      span.className = "w";
      span.textContent = w.text;
      span.dataset.word = w.n;
      if (k + 1 < on.length && !spaced(w.text, on[k + 1].text)) span.classList.add("tight");
      return span;
    }),
  );
}

/** The card keeps one width for a whole chunk: its widest line's. */
function chunkWidth(index) {
  if (width?.index === index) return width.px;
  const lines = new Set((words.get(index) ?? []).map((w) => w.line));
  let px = 0;
  for (const number of lines) {
    fill(measure, index, number);
    px = Math.max(px, measure.getBoundingClientRect().width);
  }
  measure.replaceChildren();
  width = { index, px: Math.min(ROOM, px) };
  return width.px;
}

function showLine(index, number) {
  const inner = chunkWidth(index);
  fill(line, index, number);
  spans = [...line.children];
  // A line wider than the card (one very long word) is drawn smaller, never cut.
  const natural = line.getBoundingClientRect().width;
  const scale = Math.min(1, ROOM / natural);
  const left = (inner - natural * scale) / 2;
  line.style.transform = scale < 1 ? `scale(${scale})` : "";
  line.style.left = `${left}px`;
  card.style.width = `${Math.ceil(inner) + 2}px`;
  if (shown) {
    line.animate([{ opacity: 0, translate: "0 5px" }, { opacity: 1, translate: "0 0" }], {
      duration: 150,
      easing: "cubic-bezier(0.2, 0.8, 0.2, 1)",
    });
  }
  // The highlight does not glide in from the line before.
  chip.classList.add("jump");
  shown = { index, line: number, scale, left };
}

function wordPlaying(index, word) {
  const w = words.get(index)?.[word];
  if (!w) return;
  if (shown?.index !== index || shown.line !== w.line) showLine(index, w.line);
  for (const span of spans) {
    const n = Number(span.dataset.word);
    span.classList.toggle("read", n < word);
    span.classList.toggle("now", n === word);
    if (n !== word) continue;
    // 9 % larger, but no more than 5 px to each side: a long word must not
    // spill out of the card.
    span.style.setProperty("--grow", Math.min(1.09, 1 + 10 / span.offsetWidth));
    chip.style.width = `${span.offsetWidth * shown.scale + 2 * PAD}px`;
    chip.style.transform = `translateX(${shown.left + span.offsetLeft * shown.scale - PAD}px)`;
  }
  body.dataset.on = "true";
  void chip.offsetWidth;
  chip.classList.remove("jump");
}

function reset(next) {
  session = next;
  words.clear();
  shown = null;
  width = null;
  body.dataset.on = "false";
}

listen("speech", ({ payload: e }) => {
  if (e.type === "state") body.dataset.state = e.state;
  else if (e.type === "started") reset(e.session);
  else if (e.session !== session) return;
  else if (e.type === "chunk_ready") words.set(e.index, e.words ?? []);
  else if (e.type === "word_playing") wordPlaying(e.index, e.word);
  else if (e.type === "finished") reset(null);
});
