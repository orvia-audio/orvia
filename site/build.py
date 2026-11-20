"""Assemble the public Pages site from the explicitly listed project assets."""

import hashlib
from pathlib import Path
import shutil
import subprocess
import sys

site = Path(__file__).resolve().parent
root = site.parent
out = root / "dist" / "pages"
media = out / "media"
media.mkdir(parents=True, exist_ok=True)
html = (site / "index.html").read_text(encoding="utf-8")
for name in ("style.css", "player.js"):
    content = (site / name).read_bytes()
    (out / name).write_bytes(content)
    version = hashlib.sha256(content).hexdigest()[:16]
    html = html.replace(f'"{name}"', f'"{name}?v={version}"')
(out / "index.html").write_text(html, encoding="utf-8")
shutil.copyfile(site / "brand-github.svg", media / "brand-github.svg")
shutil.copyfile(site / "social-card.png", media / "social-card.png")
for name in ("orvia.svg", "mixer.png"):
    shutil.copyfile(root / "assets" / name, media / name)
for name in ("microphone.svg", "chevron-down.svg", "activity.svg", "filter.svg", "volume.svg", "x.svg"):
    shutil.copyfile(root / "assets" / "tabler" / "outline" / name, media / name)
samples = root / "docs" / "samples"
for scene in ("air-conditioner", "keyboard"):
    for method in ("original", "deepfilternet3", "rnnoise", "webrtc"):
        clip = f"{scene}-{method}.flac"
        shutil.copyfile(samples / clip, media / clip)
shutil.copyfile(samples / "compare-16s.mp4", media / "compare-16s.mp4")
notices = [root / "LICENSE", root / "assets" / "tabler" / "LICENSE", site / "octicons-LICENSE.txt"]
(out / "licenses.txt").write_text("\n\n".join(p.read_text(encoding="utf-8") for p in notices), encoding="utf-8")
subprocess.run([sys.executable, site / "review/build.py"], check=True)
