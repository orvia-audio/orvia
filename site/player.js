"use strict";

const methods = [...document.querySelectorAll("[data-method]")];
const sceneSelect = document.getElementById("scene");
const canvas = document.getElementById("waveform");
const seek = document.getElementById("seek");
const play = document.getElementById("play");
const notice = document.getElementById("notice");
const tracks = new Map();
const sources = new Map();
const seconds = Number(seek.max);

let scene = "air-conditioner";
let method = "deepfilternet3";
let decoder;
let context;
let offset = 0;
let started = 0;
let playing = false;
let revision = 0;
let animation = 0;

function envelope(buffer) {
  const samples = buffer.getChannelData(0);
  const columns = 560;
  return Array.from({length: columns}, (_, i) => {
    let low = 0, high = 0;
    const end = Math.floor((i + 1) * samples.length / columns);
    for (let j = Math.floor(i * samples.length / columns); j < end; j++) {
      low = Math.min(low, samples[j]);
      high = Math.max(high, samples[j]);
    }
    return [low, high];
  });
}

function track(key) {
  if (!tracks.has(key)) {
    const entry = {};
    entry.ready = (async () => {
      const response = await fetch("media/" + key + ".flac");
      if (!response.ok) throw new Error("Audio unavailable");
      decoder ||= new OfflineAudioContext(1, 1, 48000);
      entry.buffer = await decoder.decodeAudioData(await response.arrayBuffer());
      entry.peaks = envelope(entry.buffer);
      return entry;
    })().catch(error => { tracks.delete(key); throw error; });
    tracks.set(key, entry);
  }
  return tracks.get(key).ready;
}

function position() {
  return sources.size ? (offset + Math.max(0, context.currentTime - started)) % seconds : offset;
}

function draw() {
  const ratio = Math.min(2, window.devicePixelRatio || 1);
  const w = Math.round(canvas.clientWidth * ratio);
  const h = Math.round(canvas.clientHeight * ratio);
  if (w === 0 || h === 0) return;
  if (canvas.width !== w || canvas.height !== h) { canvas.width = w; canvas.height = h; }
  const ctx = canvas.getContext("2d");
  const t = position();
  const cursor = Math.min(w - 1, Math.round(w * t / seconds));
  ctx.clearRect(0, 0, w, h);
  ctx.fillStyle = "#cbd4ce";
  ctx.fillRect(0, Math.floor(h / 2), w, ratio);
  const peaks = tracks.get(scene + "-" + method)?.peaks || [];
  const dx = w / peaks.length;
  peaks.forEach(([low, high], i) => {
    const x = i * dx;
    ctx.fillStyle = x <= cursor ? "#a7492b" : "#566e67";
    ctx.fillRect(x, h / 2 - high * h * .57, Math.max(1, dx), Math.max(ratio, (high - low) * h * .57));
  });
  ctx.fillStyle = "#a7492b";
  ctx.fillRect(cursor, 0, Math.max(2, ratio), h);
  seek.value = String(t);
  seek.setAttribute("aria-valuetext", t.toFixed(1) + " seconds");
  document.getElementById("clock").textContent = "0:" + String(Math.floor(t)).padStart(2, "0") + " / 0:14";
}

function tick() {
  draw();
  if (sources.size) animation = requestAnimationFrame(tick);
}

function disconnect() {
  offset = position();
  cancelAnimationFrame(animation);
  for (const {source, gain} of sources.values()) {
    source.stop(); source.disconnect(); gain.disconnect();
  }
  sources.clear();
}

function updateButton() {
  document.getElementById("play-label").textContent = playing ? "Pause" : "Play";
  document.getElementById("play-icon").setAttribute("d", playing ? "M4 3h4v14H4z M12 3h4v14h-4z" : "m6 3 11 7-11 7Z");
}

function start() {
  started = context.currentTime + .025;
  offset %= seconds;
  for (const button of methods) {
    const id = button.dataset.method;
    const source = context.createBufferSource();
    const gain = context.createGain();
    source.buffer = tracks.get(scene + "-" + id).buffer;
    source.loop = true;
    gain.gain.value = id === method ? 1 : 0;
    source.connect(gain).connect(context.destination);
    source.start(started, offset);
    sources.set(id, {source, gain});
  }
  tick();
}

function failure() {
  notice.textContent = "Audio could not load. ";
  const link = document.createElement("a");
  link.href = "media/compare-16s.mp4";
  link.textContent = "Play the 16-second video";
  notice.append(link);
}

async function loadScene() {
  const own = ++revision;
  try {
    const files = methods.map(b => track(scene + "-" + b.dataset.method));
    await Promise.all(files);
    if (revision !== own) return;
    notice.textContent = "";
    if (playing) start(); else draw();
  } catch {
    if (revision !== own) return;
    playing = false;
    updateButton();
    failure();
  }
}

play.addEventListener("click", async () => {
  if (playing) {
    revision++;
    disconnect();
    playing = false;
    updateButton();
    draw();
    return;
  }
  playing = true;
  updateButton();
  const own = ++revision;
  try {
    context ||= new AudioContext();
    await context.resume();
    if (own === revision) await loadScene();
  } catch {
    if (own !== revision) return;
    playing = false;
    updateButton();
    failure();
  }
});

sceneSelect.addEventListener("change", () => {
  if (scene === sceneSelect.value) return;
  disconnect();
  scene = sceneSelect.value;
  draw();
  void loadScene();
});

for (const button of methods) button.addEventListener("click", () => {
  method = button.dataset.method;
  methods.forEach(b => b.setAttribute("aria-pressed", String(b === button)));
  for (const [id, {gain}] of sources) {
    const now = context.currentTime;
    gain.gain.cancelScheduledValues(now);
    gain.gain.setValueAtTime(gain.gain.value, now);
    gain.gain.linearRampToValueAtTime(id === method ? 1 : 0, now + .008);
  }
  draw();
});

function jump(to) {
  const live = sources.size > 0;
  disconnect();
  offset = Math.max(0, Math.min(seconds, to));
  if (live) start(); else draw();
}
seek.addEventListener("input", () => jump(Number(seek.value)));
document.getElementById("replay").addEventListener("click", () => jump(position() - 3));
new ResizeObserver(draw).observe(canvas);
void loadScene();
