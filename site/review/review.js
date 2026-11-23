import {matchingPulls, pullMergeBase} from "./pulls.mjs";

const $ = id => document.getElementById(id);
const repo = "orvia-audio/orvia";
const prefix = "orvia-review-v1/";
const params = new URLSearchParams(location.search);
const touchPreview = matchMedia("(pointer: coarse), (max-width: 830px)");
const words = {
  review: ["Snapshot review", "画面レビュー"], report: ["Review notes", "指摘をまとめる"], latest: ["Latest screens", "最新の画面"],
  compare: ["Compare", "差分を比較"], screen: ["Screen", "画面"],
  twoCommits: ["Two commits", "コミット間"], pullHead: ["PR head commit", "PR 先端のコミットハッシュ"],
  choosePull: ["Choose a pull request", "比較する PR を選択"], checks: ["CI checks", "CI の結果"],
  noPull: ["No Orvia PR has this head commit.", "このハッシュを先端に持つ Orvia の PR は見つかりませんでした。"],
  noPullDiff: ["No committed snapshot changes in this PR.", "この PR で変更された基準画像はありません。"],
  missingBase: ["Select the original base and head in Two commits.", "「コミット間」で元の比較元と先端を指定してください。"],
  comments: ["Comments", "コメント"], hint: ["Click a point in the image to leave a comment.", "画像の気になる位置をクリックして、コメントを残せます。"],
  touchHint: ["Tap a screenshot to enlarge it or mark a point.", "画像をタップすると、拡大して気になる位置にコメントできます。"],
  enlarge: ["Enlarge image", "画像を拡大"], fit: ["Fit to width", "幅に合わせる"],
  actualSize: ["Actual size", "原寸で表示"], markPoint: ["Mark a point", "位置にコメント"],
  general: ["Comment on the whole screen", "画面全体にコメント"], whole: ["Whole screen", "画面全体"],
  base: ["Before", "変更前"], head: ["After", "変更後"], side: ["Side by side", "並べて表示"],
  diff: ["Pixel differences", "画像の差分"], showDiff: ["Show differences", "差分を表示"],
  single: ["End comparison", "比較を終了"], changedOnly: ["Changed screens only", "変更のある画面のみ"],
  prev: ["Previous screen", "前の画面"], next: ["Next screen", "次の画面"],
  accept: ["Reviewed · Next", "確認して次へ"], finish: ["Finish review", "確認を完了"],
  reviewed: ["reviewed", "確認済み"], count: ["comments", "件"], close: ["Close", "閉じる"],
  copy: ["Copy", "コピー"], download: ["Save .md", "Markdown を保存"], copied: ["Copied", "コピーしました"],
  selectText: ["Select and copy the report.", "本文を選択してコピーしてください。"],
  emptyNotes: ["No comments.", "指摘はありません。"], addNote: ["Add a comment", "コメントを入力"],
  remove: ["Delete", "削除"], point: ["Comment on this image", "この画像にコメント"],
  noChanges: ["No snapshot changes between these commits.", "この2つのコミットに画像の差分はありません。"],
  noImages: ["No snapshots in this revision.", "このコミットにスナップショットはありません。"],
  added: ["Added", "追加"], removed: ["Removed", "削除"], absent: ["No image at this revision", "このコミットには画像がありません"],
  pixels: ["changed pixels", "変更ピクセル"], loading: ["Loading…", "読み込み中…"],
  badRef: ["Enter a 7–40 digit commit hash.", "7～40桁のコミットハッシュを入力してください。"],
  unknown: ["Could not resolve this commit", "コミットを取得できません"], ambiguous: ["Use a longer commit hash", "より長いコミットハッシュを入力してください"],
  retry: ["Retry", "再試行"], imageError: ["Could not load the image.", "画像を読み込めませんでした。"],
  offline: ["Browser storage is unavailable. Save your notes before closing.", "ブラウザーに保存できません。閉じる前に指摘を保存してください。"],
  local: ["Notes are saved in this browser.", "コメントはこのブラウザーに保存されます。"],
};
// Unknown screens from other revisions remain reviewable under their filenames.
const names = {
  mixer: ["Mixer", "ミキサー"], "mixer-full": ["Mixer · Full view", "ミキサー · 全体"],
  "tone-setup": ["Tone setup", "音質調整 · 自動設定"], "tone-opening": ["Tone setup · Opening", "音質調整 · 準備"],
  "tone-room": ["Tone setup · Room", "音質調整 · 環境音"], "tone-read": ["Tone setup · Reading", "音質調整 · 読み上げ"],
  "tone-measuring": ["Tone setup · Measuring", "音質調整 · 入力測定"], "tone-analyzing": ["Tone setup · Analysis", "音質調整 · 解析"],
  "tone-ready": ["Tone setup · Result", "音質調整 · 結果"], "tone-playing": ["Tone setup · Playback", "音質調整 · 試聴"],
  "tone-failed": ["Tone setup · Error", "音質調整 · エラー"],
  "mixer-bypass": ["Mixer · Suppression off", "ミキサー · 抑制オフ"],
  "mixer-spectrum": ["Mixer · Spectrum", "ミキサー · スペクトル"],
  "mixer-spectrum-inspect": ["Mixer · Frequency band", "ミキサー · 帯域の確認"],
  "mixer-spectrum-silent": ["Mixer · Silent spectrum", "ミキサー · スペクトル（無音）"],
  devices: ["Devices", "デバイス"], "devices-picker": ["Devices · Device list", "デバイス · 選択リスト"],
  mic: ["Mic test", "マイクテスト"], "mic-recording": ["Mic test · Recording", "マイクテスト · 録音中"],
  "mic-recorded": ["Mic test · Recorded", "マイクテスト · 録音済み"], "mic-playing": ["Mic test · Playback", "マイクテスト · 再生中"],
  "mic-standalone": ["Mic test · Standalone", "マイクテスト · 単独ウィンドウ"],
  "mic-offline": ["Mic test · Disconnected", "マイクテスト · 未接続"], "mic-error": ["Mic test · Error", "マイクテスト · エラー"],
  "adjust-room": ["Auto adjust · Room sample", "自動調節 · 環境音"], "adjust-measuring": ["Auto adjust · Measuring", "自動調節 · 入力測定"],
  "adjust-read": ["Auto adjust · Read aloud", "自動調節 · 読み上げ"], "adjust-recording": ["Auto adjust · Recording", "自動調節 · 録音中"],
  "adjust-analyzing": ["Auto adjust · Analysis", "自動調節 · 解析"], "adjust-opening": ["Auto adjust · Open preview", "自動調節 · プレビュー準備"],
  "adjust-preview": ["Auto adjust · Preview", "自動調節 · プレビュー"], "adjust-restoring": ["Auto adjust · Restore mix", "自動調節 · 設定復元"],
  "adjust-ready": ["Auto adjust · Result", "自動調節 · 結果"], "adjust-failed": ["Auto adjust · Error", "自動調節 · エラー"],
  preferences: ["Preferences", "設定"], "preferences-checking": ["Preferences · Checking updates", "設定 · 更新確認中"],
  "preferences-current": ["Preferences · Up to date", "設定 · 更新なし"], "preferences-available": ["Preferences · Update available", "設定 · 更新あり"],
  "preferences-error": ["Preferences · Update error", "設定 · 更新エラー"], log: ["Engine log", "エンジンログ"],
};

const notes = new Map();
const memory = new Map();
let storageFailed = false;
function read(key) {
  try { return JSON.parse(localStorage.getItem(prefix + key)) ?? memory.get(key); }
  catch { return memory.get(key); }
}
function write(key, value) {
  memory.set(key, value);
  try {
    if (value === null) localStorage.removeItem(prefix + key);
    else localStorage.setItem(prefix + key, JSON.stringify(value));
  } catch {
    storageFailed = true;
    $("message").textContent = t("offline");
  }
}
try {
  for (const key of Object.keys(localStorage).filter(key => key.startsWith(prefix + "note/"))) {
    const note = read(key.slice(prefix.length));
    if (note && typeof note.text === "string") notes.set(note.id, note);
  }
} catch { storageFailed = true; }
const saved = read("session") || {};
let lang = params.get("lang") || saved.lang || (navigator.language.startsWith("ja") ? "ja" : "en");
if (lang !== "ja") lang = "en";
const t = key => words[key][lang === "ja" ? 1 : 0];
const title = scene => names[scene]?.[lang === "ja" ? 1 : 0] ?? scene;
const short = sha => sha?.slice(0, 8);
const pathFor = scene => lang + "/" + scene + ".png";
const rootPath = "tests/ui/snapshots/";
let catalog, bundled, head, base, scene, scenes = [], view = "diff", frameVersion = 0;
let changedOnly = true;
let linkedPoint = null;
let headRepo = repo, pullNumber = null, candidates = [];
const remote = new Map(), images = new Map();
function repository(value) {
  return typeof value === "string" && /^[\w.-]+\/[\w.-]+$/.test(value) && !value.split("/").some(part => part === "." || part === "..") ? value : repo;
}
function originOf(sha) { return sha === head ? headRepo : repo; }
function pullId(value) { return /^[1-9]\d{0,9}$/.test(value ?? "") ? Number(value) : null; }
function contextParams() {
  return {...(headRepo !== repo ? {source: headRepo} : {}), ...(base && pullNumber ? {pull: pullNumber} : {})};
}
function files(sha) { return remote.get(sha) ?? catalog.trees[catalog.revisions[sha]] ?? {}; }
function doneKey(item) { return ["seen", lang, base || "-", head, item].join("/"); }
function savedNote(note) { notes.set(note.id, note); write("note/" + note.id, note); }
function sessionKey() { return ["cursor", lang, base || "-", head].join("/"); }
function matchingNotes(item) {
  return [...notes.values()].filter(n => (n.sha === head || n.sha === base) && n.lang === lang && n.scene === item)
    .sort((a, b) => a.created - b.created || a.id.localeCompare(b.id));
}
function element(tag, text, className) {
  const node = document.createElement(tag);
  if (text !== undefined) node.textContent = text;
  if (className) node.className = className;
  return node;
}
function button(text, action) {
  const node = element("button", text);
  node.type = "button";
  node.onclick = action;
  return node;
}

async function api(path, source = repo) {
  const response = await fetch("https://api.github.com/repos/" + source + "/" + path);
  if (!response.ok) throw new Error(t("unknown") + " (GitHub HTTP " + response.status + ")");
  return response.json();
}
function hash(input) {
  const value = input.trim().toLowerCase();
  if (!/^[0-9a-f]{7,40}$/.test(value)) throw new Error(t("badRef"));
  return value;
}
async function resolveRef(input, source = repo) {
  const value = hash(input);
  const matches = [...Object.keys(catalog.revisions), ...remote.keys()].filter(sha => sha.startsWith(value));
  if (matches.length > 1) throw new Error(t("ambiguous") + " · " + value);
  if (matches.length === 1) return matches[0];
  const commit = await api("commits/" + value, source);
  let tree = commit.commit.tree.sha;
  for (const name of ["tests", "ui", "snapshots"]) {
    const listing = await api("git/trees/" + tree, source);
    tree = listing.tree.find(entry => entry.type === "tree" && entry.path === name)?.sha;
    if (!tree) break;
  }
  const listing = tree ? await api("git/trees/" + tree + "?recursive=1", source) : {tree: []};
  if (listing.truncated) throw new Error(t("unknown") + " · " + value);
  remote.set(commit.sha, Object.fromEntries(listing.tree.filter(entry =>
    entry.type === "blob" && /^(ja|en)\/[a-z0-9-]+\.png$/.test(entry.path)).map(entry => [entry.path, entry.sha])));
  return commit.sha;
}
async function findPulls(input) {
  const matches = await matchingPulls(api, hash(input));
  if (!matches.length) throw new Error(t("noPull"));
  return matches;
}
async function pullRange(pr, tip = pr.head.sha, target = pr.base.sha) {
  const source = repository(pr.head.repo?.full_name);
  const tipSha = await resolveRef(tip, source);
  const common = await pullMergeBase(api, pr, tipSha, hash(target));
  if (!common) throw new Error(t("missingBase"));
  return {base: await resolveRef(common), head: tipSha, source, pull: pr.number};
}
function useRange(range) {
  base = range.base; head = range.head; headRepo = range.source; pullNumber = range.pull;
}
function loadImage(sha, item) {
  const path = pathFor(item), blob = files(sha)[path];
  if (!blob) return Promise.resolve(null);
  if (!images.has(blob)) {
    if (images.size >= 12) images.delete(images.keys().next().value);
    const image = new Image();
    image.crossOrigin = "anonymous";
    image.src = bundled.has(blob) ? "images/" + blob + ".png" :
      "https://raw.githubusercontent.com/" + originOf(sha) + "/" + sha + "/" + rootPath + path;
    images.set(blob, image.decode().then(() => image).catch(error => { images.delete(blob); throw error; }));
  }
  return images.get(blob);
}
function listScenes() {
  const old = base ? files(base) : {}, current = files(head);
  const paths = new Set([...Object.keys(old), ...Object.keys(current)]);
  const available = [...paths].filter(path => path.startsWith(lang + "/") &&
    (!base || !changedOnly || old[path] !== current[path])).map(path => path.slice(3, -4));
  return [...Object.keys(names).filter(name => available.includes(name)),
    ...available.filter(name => !Object.hasOwn(names, name)).sort()];
}
function localize() {
  document.documentElement.lang = lang;
  document.title = t("review") + " · Orvia";
  document.querySelectorAll("[data-t]").forEach(node => { node.textContent = t(node.dataset.t); });
  $("previous").ariaLabel = t("prev"); $("next").ariaLabel = t("next");
  $("language").value = lang;
  $("general").title = t("local");
  updateImageLabels();
}
function updateImageLabels() {
  $("hint").textContent = t(touchPreview.matches ? "touchHint" : "hint");
  document.querySelectorAll(".surface").forEach(surface => {
    surface.ariaLabel = t(touchPreview.matches ? "enlarge" : "point") + " · " + surface.dataset.label;
  });
}
touchPreview.addEventListener("change", updateImageLabels);
function savePosition() {
  if (scene) write(sessionKey(), scene);
  write("session", {lang, head, base, view, changedOnly, ...contextParams()});
  const url = new URL(location.href);
  url.search = new URLSearchParams({lang, head, ...contextParams(), ...(base ? {base, view, only: changedOnly ? "1" : "0"} : {}),
    ...(scene ? {screen: scene} : {})});
  if (linkedPoint?.scene === scene && linkedPoint.lang === lang && (linkedPoint.sha === head || linkedPoint.sha === base)) {
    url.searchParams.set("point", linkedPoint.point.x + "," + linkedPoint.point.y);
    url.searchParams.set("on", linkedPoint.sha);
  }
  history.replaceState(null, "", url);
}
function updateProgress() {
  const total = scenes.length, index = scenes.indexOf(scene);
  const done = scenes.filter(name => read(doneKey(name))).length;
  $("scene").replaceChildren(...scenes.map(name => {
    const path = pathFor(name);
    const change = !base ? "" : !files(base)[path] ? t("added") : !files(head)[path] ? t("removed") : "";
    return new Option((read(doneKey(name)) ? "✓ " : "") + title(name) + (change ? " · " + change : ""), name);
  }));
  if (scene) $("scene").value = scene;
  $("scene").disabled = !total;
  $("progress").max = total || 1; $("progress").value = done;
  $("progress-label").textContent = done + " / " + total + " " + t("reviewed");
  $("progress").ariaLabel = $("progress-label").textContent;
  $("position").textContent = (index + 1) + " / " + total;
  $("previous").disabled = index <= 0; $("next").disabled = index >= total - 1;
  $("accept").disabled = !total; $("general").disabled = !total;
  $("accept").textContent = done === total || (total - done === 1 && !read(doneKey(scene))) ? t("finish") : t("accept");
}
function navigate(to) {
  scenes = listScenes();
  scene = scenes.includes(to) ? to : scenes[0] ?? "";
  $("current-commit").textContent = short(head);
  $("current-commit").href = "https://github.com/" + headRepo + "/commit/" + head;
  $("pull-link").hidden = !pullNumber;
  $("pull-checks").hidden = !pullNumber;
  if (pullNumber) {
    $("pull-link").href = "https://github.com/" + repo + "/pull/" + pullNumber;
    $("pull-link").textContent = "PR #" + pullNumber;
    $("pull-checks").href = $("pull-link").href + "/checks";
  }
  $("latest").hidden = head === catalog.current;
  $("comparison").hidden = !base; $("views").hidden = !base || !scene;
  $("revisions").textContent = base ? short(base) + " → " + short(head) : "";
  $("changes-only").checked = changedOnly;
  updateProgress(); savePosition(); drawNotes(); drawFrames();
}

// Composite decoded pixels in native coordinates. A larger or smaller canvas is itself a change.
function pixelDiff(before, after) {
  const width = Math.max(before.naturalWidth, after.naturalWidth), height = Math.max(before.naturalHeight, after.naturalHeight);
  const canvas = element("canvas");
  canvas.width = width; canvas.height = height;
  const ctx = canvas.getContext("2d", {willReadFrequently: true});
  ctx.drawImage(before, 0, 0);
  const a = ctx.getImageData(0, 0, width, height).data;
  ctx.clearRect(0, 0, width, height); ctx.drawImage(after, 0, 0);
  const output = ctx.getImageData(0, 0, width, height), b = output.data;
  let count = 0;
  for (let y = 0, i = 0; y < height; y++) for (let x = 0; x < width; x++, i += 4) {
    const inA = x < before.naturalWidth && y < before.naturalHeight;
    const inB = x < after.naturalWidth && y < after.naturalHeight;
    const changed = inA !== inB || a[i] !== b[i] || a[i+1] !== b[i+1] || a[i+2] !== b[i+2] || a[i+3] !== b[i+3];
    if (changed) {
      b.set([204, 38, 93, 255], i); count++;
    } else {
      const grey = 110 + Math.round((b[i] * .30 + b[i+1] * .59 + b[i+2] * .11) * .54);
      b.set([grey, grey, grey, 255], i);
    }
  }
  ctx.putImageData(output, 0, 0);
  return {canvas, count};
}
function addNote(sha, point) {
  const note = {id: crypto.randomUUID(), created: Date.now(), sha, source: originOf(sha), lang, scene, point, text: ""};
  savedNote(note); drawNotes(note.id); drawPins();
}
function drawPins() {
  const entries = matchingNotes(scene);
  for (const wrap of $("frames").querySelectorAll(".image-wrap")) {
    wrap.querySelectorAll(".pin").forEach(pin => pin.remove());
    const pins = entries.map((note, index) => ({...note, number: String(index + 1)}));
    if (linkedPoint?.scene === scene && linkedPoint.lang === lang &&
        !entries.some(n => n.sha === linkedPoint.sha && n.point?.x === linkedPoint.point.x && n.point?.y === linkedPoint.point.y)) {
      pins.push({...linkedPoint, number: "+"});
    }
    for (const note of pins) {
      if (!note.point || !wrap.dataset.refs.split(",").includes(note.sha) ||
          note.point.x >= Number(wrap.dataset.width) || note.point.y >= Number(wrap.dataset.height)) continue;
      const pin = button(note.number, () => {
        if (note.id) document.getElementById("note-" + note.id)?.focus();
        else { linkedPoint = null; addNote(note.sha, note.point); savePosition(); }
      });
      pin.className = "pin"; pin.ariaLabel = note.number + " · " + (note.text || t("addNote"));
      pin.style.left = ((note.point.x + .5) / Number(wrap.dataset.width) * 100) + "%";
      pin.style.top = ((note.point.y + .5) / Number(wrap.dataset.height) * 100) + "%";
      wrap.append(pin);
    }
  }
}
function imagePoint(event, surface, width, height) {
  const rect = surface.getBoundingClientRect();
  const x = event.detail ? (event.clientX - rect.left) / rect.width : .5;
  const y = event.detail ? (event.clientY - rect.top) / rect.height : .5;
  return {x: Math.max(0, Math.min(width - 1, Math.floor(x * width))),
    y: Math.max(0, Math.min(height - 1, Math.floor(y * height)))};
}
function enlargeImage(node, caption, width, height, point, owner) {
  const dialog = $("image-dialog"), scroll = $("image-scroll"), detail = $("image-detail");
  const scale = $("image-scale"), mark = $("image-point");
  const copy = node.cloneNode();
  if (node instanceof HTMLCanvasElement) copy.getContext("2d").drawImage(node, 0, 0);
  const label = [title(scene), caption].filter(Boolean).join(" · ");
  $("image-title").textContent = label;
  detail.replaceChildren(copy); detail.style.width = width + "px"; detail.className = "";
  mark.setAttribute("aria-pressed", "false");
  function resize(fit, center) {
    detail.classList.toggle("fit", fit);
    scale.textContent = t(fit ? "actualSize" : "fit");
    const action = detail.classList.contains("marking") ? t("markPoint") : scale.textContent;
    detail.ariaLabel = action + " · " + label;
    scroll.scrollLeft = center.x * detail.offsetWidth / width - scroll.clientWidth / 2;
    scroll.scrollTop = center.y * detail.offsetHeight / height - scroll.clientHeight / 2;
  }
  scale.onclick = () => {
    const area = scroll.getBoundingClientRect(), image = detail.getBoundingClientRect();
    resize(!detail.classList.contains("fit"), {
      x: (area.left + scroll.clientWidth / 2 - image.left) * width / image.width,
      y: (area.top + scroll.clientHeight / 2 - image.top) * height / image.height,
    });
  };
  mark.onclick = () => {
    const marking = detail.classList.toggle("marking");
    mark.setAttribute("aria-pressed", String(marking));
    detail.ariaLabel = (marking ? t("markPoint") : scale.textContent) + " · " + label;
    if (marking) detail.focus({preventScroll: true});
  };
  detail.onclick = event => {
    const selected = imagePoint(event, detail, width, height);
    if (detail.classList.contains("marking")) {
      const sha = owner(selected);
      if (sha) { dialog.close(); addNote(sha, selected); }
    } else resize(!detail.classList.contains("fit"), selected);
  };
  dialog.showModal(); resize(false, point);
}
$("image-dialog").addEventListener("close", () => {
  if ($("image-dialog").open) return;
  $("image-detail").replaceChildren();
  for (const id of ["image-detail", "image-scale", "image-point"]) $(id).onclick = null;
});
function frame(node, caption, width, height, refs, owner) {
  const block = element("div", undefined, "frame");
  block.style.width = width + "px";
  if (caption) block.append(element("div", caption, "caption"));
  const wrap = element("div", undefined, "image-wrap");
  Object.assign(wrap.dataset, {width, height, refs: refs.join(",")});
  const surface = element("button", undefined, "surface");
  surface.type = "button"; surface.dataset.label = caption || title(scene);
  surface.ariaLabel = t(touchPreview.matches ? "enlarge" : "point") + " · " + surface.dataset.label;
  surface.append(node);
  surface.onclick = event => {
    const point = imagePoint(event, surface, width, height);
    if (touchPreview.matches) { enlargeImage(node, caption, width, height, point, owner); return; }
    const sha = owner(point);
    if (sha) addNote(sha, point);
  };
  wrap.append(surface); block.append(wrap);
  return block;
}
function original(image, sha, label) {
  if (!image) return element("p", label + " · " + t("absent"), "empty");
  const node = image.cloneNode(); node.alt = title(scene);
  return frame(node, label, image.naturalWidth, image.naturalHeight, [sha], () => sha);
}
async function drawFrames() {
  const version = ++frameVersion, area = $("frames");
  area.classList.toggle("pair", Boolean(base && view === "side" && scene));
  $("views").replaceChildren(...["base", "head", "side", "diff"].map(mode => {
    const tab = button(t(mode), () => { view = mode; savePosition(); drawFrames(); });
    tab.setAttribute("aria-pressed", String(view === mode)); return tab;
  }));
  const empty = base && changedOnly ? (pullNumber ? "noPullDiff" : "noChanges") : "noImages";
  area.replaceChildren(element("p", t(scene ? "loading" : empty), "empty"));
  if (!scene) return;
  try {
    const [before, after] = await Promise.all([base && view !== "head" ? loadImage(base, scene) : null,
      !base || view !== "base" ? loadImage(head, scene) : null]);
    if (version !== frameVersion) return;
    const oldLabel = t("base") + " · " + short(base), newLabel = t("head") + " · " + short(head);
    if (!base) area.replaceChildren(original(after, head, ""));
    else if (view === "side") area.replaceChildren(original(before, base, oldLabel), original(after, head, newLabel));
    else if (view === "base") area.replaceChildren(original(before, base, oldLabel));
    else if (view === "head") area.replaceChildren(original(after, head, newLabel));
    else if (!before || !after) area.replaceChildren(original(after || before, after ? head : base, t(after ? "added" : "removed")));
    else {
      const {canvas, count} = pixelDiff(before, after);
      area.replaceChildren(frame(canvas, count.toLocaleString(lang) + " " + t("pixels"), canvas.width, canvas.height,
        [base, head], ({x, y}) => x < after.naturalWidth && y < after.naturalHeight ? head :
          x < before.naturalWidth && y < before.naturalHeight ? base : null));
    }
    drawPins();
  } catch {
    if (version === frameVersion) area.replaceChildren(element("p", t("imageError"), "empty"), button(t("retry"), drawFrames));
  }
}
function drawNotes(focusId) {
  const entries = matchingNotes(scene);
  $("count").textContent = String(entries.length);
  $("notes").replaceChildren(...entries.map((note, index) => {
    const row = element("div", undefined, "note");
    const label = (index + 1) + " · " + (base ? t(note.sha === head ? "head" : "base") + " · " : "") +
      (note.point ? note.point.x + ", " + note.point.y + " px" : t("whole"));
    const bar = element("div", undefined, "note-header");
    const caption = element("label", label); caption.htmlFor = "note-" + note.id;
    bar.append(caption, button(t("remove"), () => {
      write("note/" + note.id, null); notes.delete(note.id); drawNotes(); drawPins(); $("general").focus();
    }));
    const input = element("textarea");
    input.id = caption.htmlFor; input.value = note.text; input.placeholder = t("addNote");
    input.oninput = () => { note.text = input.value; savedNote(note); };
    row.append(bar, input); return row;
  }));
  if (focusId) document.getElementById("note-" + focusId)?.focus();
}
function openReport() {
  const out = ["# Orvia · " + t("review"), "", base ? base + " → " + head : head, lang, ""];
  if (pullNumber) out.push("[PR #" + pullNumber + "](https://github.com/" + repo + "/pull/" + pullNumber + ")", "");
  const items = [...new Set([...scenes, ...[...notes.values()].map(n => n.scene)])];
  let count = 0;
  for (const item of items) {
    const entries = matchingNotes(item).filter(note => note.text.trim());
    if (!entries.length) continue;
    out.push("## " + title(item), "");
    for (const note of entries) {
      const path = rootPath + pathFor(item);
      const link = "https://github.com/" + (note.source || originOf(note.sha)) + "/blob/" + note.sha + "/" + path;
      let where = t("whole");
      if (note.point) {
        const url = new URL(location.href);
        url.search = new URLSearchParams({lang, head, ...contextParams(), screen: item, on: note.sha,
          point: note.point.x + "," + note.point.y,
          ...(base ? {base, view: note.sha === base ? "base" : "head", only: "0"} : {})});
        where = "[x=" + note.point.x + ", y=" + note.point.y + " px](" + url.href + ")";
      }
      out.push("- [" + pathFor(item) + " @ " + short(note.sha) + "](" + link + ") · " + where,
        ...note.text.trim().split(/\r?\n/).map(line => "  " + line), "");
      count++;
    }
  }
  if (!count) out.push(t("emptyNotes"));
  $("markdown").value = out.join("\n"); $("report-status").textContent = t("local");
  $("report-dialog").showModal();
}

$("scene").onchange = () => navigate($("scene").value);
$("previous").onclick = () => navigate(scenes[scenes.indexOf(scene) - 1]);
$("next").onclick = () => navigate(scenes[scenes.indexOf(scene) + 1]);
$("accept").onclick = () => {
  write(doneKey(scene), true); updateProgress();
  const index = scenes.indexOf(scene);
  const next = [...scenes.slice(index + 1), ...scenes.slice(0, index)].find(item => !read(doneKey(item)));
  if (next) navigate(next); else openReport();
};
$("general").onclick = () => {
  const path = pathFor(scene);
  const before = base && ((!files(head)[path]) || (view === "base" && files(base)[path]));
  addNote(before ? base : head, null);
};
$("language").onchange = () => { lang = $("language").value; localize(); navigate(scene); };
$("changes-only").onchange = () => { changedOnly = $("changes-only").checked; navigate(scene); };
$("exit-compare").onclick = () => { base = null; pullNumber = null; navigate(scene); };
$("latest").onclick = () => {
  useRange({head: catalog.current, base: null, source: repo, pull: null}); navigate(read(sessionKey()));
};
const modes = [...document.querySelectorAll("input[name=comparison-kind]")];
let compareAttempt = 0;
function clearCandidates() {
  candidates = []; $("pull-choice").hidden = true;
  compareAttempt++; $("apply").disabled = false; $("compare-error").textContent = "";
}
function setMode(mode) {
  clearCandidates();
  modes.forEach(input => { input.checked = input.value === mode; });
  const pr = mode === "pr";
  $("base-field").hidden = pr; $("base").disabled = pr;
  $("head").labels[0].textContent = t(pr ? "pullHead" : "head");
  if (pr) $("head").removeAttribute("list"); else $("head").setAttribute("list", "history");
}
function selectPull(matches) {
  candidates = matches;
  $("pull-number").replaceChildren(...matches.map(pr => new Option("#" + pr.number + " → " + pr.base.ref + " · " + pr.title, pr.number)));
  $("pull-choice").hidden = false; $("compare-error").textContent = t("choosePull"); $("pull-number").focus();
}
modes.forEach(input => input.onchange = () => {
  setMode(input.value);
  if (input.value === "pr") {
    if (!pullNumber && $("head").value === head) $("head").value = "";
    $("head").focus(); $("head").select();
  } else if (!$("head").value) $("head").value = head;
});
$("head").oninput = clearCandidates; $("base").oninput = clearCandidates;
$("compare-dialog").addEventListener("close", clearCandidates);
$("compare").onclick = () => {
  $("base").value = base || catalog.base || ""; $("head").value = head;
  setMode(pullNumber ? "pr" : "commits"); $("compare-dialog").showModal();
};
$("compare-form").onsubmit = async event => {
  event.preventDefault();
  const attempt = ++compareAttempt;
  $("apply").disabled = true; $("compare-error").textContent = t("loading");
  try {
    let range;
    if (modes.find(input => input.checked).value === "pr") {
      const choice = candidates.find(pr => pr.number === Number($("pull-number").value));
      const matches = choice ? [choice] : await findPulls($("head").value);
      if (attempt !== compareAttempt) return;
      if (matches.length > 1) { selectPull(matches); return; }
      range = await pullRange(matches[0]);
    } else {
      const [left, right] = await Promise.all([resolveRef($("base").value), resolveRef($("head").value)]);
      range = {base: left, head: right, source: repo, pull: null};
    }
    if (attempt !== compareAttempt || !$("compare-dialog").open) return;
    useRange(range); view = "diff"; changedOnly = true; $("message").textContent = storageFailed ? t("offline") : "";
    $("compare-dialog").close(); navigate(read(sessionKey()));
  } catch (error) { if (attempt === compareAttempt) $("compare-error").textContent = error.message; }
  finally { if (attempt === compareAttempt) $("apply").disabled = false; }
};
document.querySelectorAll(".close-dialog").forEach(btn => btn.onclick = () => btn.closest("dialog").close());
$("export").onclick = openReport;
$("copy").onclick = async () => {
  try { await navigator.clipboard.writeText($("markdown").value); $("report-status").textContent = t("copied"); }
  catch { $("markdown").focus(); $("markdown").select(); $("report-status").textContent = t("selectText"); }
};
$("download").onclick = () => {
  const url = URL.createObjectURL(new Blob([$("markdown").value], {type: "text/markdown;charset=utf-8"}));
  const link = element("a"); link.href = url; link.download = "orvia-review-" + short(head) + "-" + lang + ".md";
  link.click(); setTimeout(() => URL.revokeObjectURL(url), 1000);
};
async function start() {
  localize(); $("language").disabled = true; $("message").textContent = t("loading");
  const response = await fetch("catalog.json", {cache: "no-cache"});
  if (!response.ok) throw new Error("HTTP " + response.status);
  catalog = await response.json(); bundled = new Set(catalog.bundled);
  head = catalog.current; base = null;
  // An explicit revision link takes precedence over the last local review.
  const explicit = params.has("head") || params.has("base") || params.has("pr");
  const headRef = explicit ? params.get("head") : saved.head, baseRef = explicit ? params.get("base") : saved.base;
  const source = repository(explicit ? params.get("source") : saved.source);
  const number = pullId(explicit ? params.get("pull") : saved.pull);
  let error, choices;
  try {
    if (params.has("pr")) {
      const value = hash(params.get("pr"));
      const matches = number ? [await api("pulls/" + number)] : await findPulls(value);
      if (matches.length > 1) choices = matches;
      else {
        const pr = matches[0], target = params.get("target");
        // CI permalinks retain both event SHAs, even after a PR is updated.
        const pinned = number && /^[0-9a-f]{40}$/.test(target || "") && value.length === 40;
        if (!pinned && !pr.head.sha.startsWith(value)) throw new Error(t("noPull"));
        useRange(await pullRange(pr, pinned ? value : pr.head.sha, pinned ? target : pr.base.sha));
      }
    } else {
      const [right, left] = await Promise.all([headRef ? resolveRef(headRef, source) : head, baseRef ? resolveRef(baseRef) : null]);
      useRange({head: right, base: left, source, pull: left ? number : null});
    }
  } catch (cause) { error = cause.message; }
  const mode = explicit ? params.get("view") : saved.view;
  view = ["base", "head", "side", "diff"].includes(mode) ? mode : "diff";
  changedOnly = explicit ? params.get("only") !== "0" : saved.changedOnly !== false;
  const point = /^(\d{1,6}),(\d{1,6})$/.exec(params.get("point") || "");
  const on = params.get("on");
  if (point && on && (on === head || on === base)) {
    linkedPoint = {lang, sha: on, scene: params.get("screen"), point: {x: Number(point[1]), y: Number(point[2])}};
  }
  $("history").replaceChildren(...catalog.recent.map(([sha, subject]) => new Option(short(sha) + " · " + subject, sha)));
  $("message").textContent = error || (storageFailed ? t("offline") : "");
  $("language").disabled = false; $("compare").disabled = false; $("export").disabled = false;
  $("content").hidden = false;
  navigate(params.get("screen") || read(sessionKey()));
  if (params.has("pr") && (choices || error)) {
    $("head").value = params.get("pr"); setMode("pr"); $("compare-dialog").showModal();
    if (choices) selectPull(choices); else $("compare-error").textContent = error;
  } else if (params.get("mode") === "pr") {
    $("base").value = base || catalog.base || ""; $("head").value = pullNumber ? head : "";
    setMode("pr"); $("compare-dialog").showModal(); $("head").focus();
  }
}
start().catch(error => { $("message").textContent = t("imageError") + " " + error.message; });
