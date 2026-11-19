"""Assemble the public Pages site from the explicitly listed project assets."""

from pathlib import Path
import shutil

site = Path(__file__).resolve().parent
root = site.parent
out = root / "dist" / "pages"
media = out / "media"
media.mkdir(parents=True, exist_ok=True)
for name in ("index.html", "sample-credits.html", "style.css", "player.js"):
    shutil.copyfile(site / name, out / name)
shutil.copyfile(site / "brand-github.svg", media / "brand-github.svg")
for name in ("orvia.svg", "mixer.png"):
    shutil.copyfile(root / "assets" / name, media / name)
for name in ("microphone.svg", "chevron-down.svg", "activity.svg", "filter.svg", "volume.svg", "refresh.svg"):
    shutil.copyfile(root / "assets" / "tabler" / "outline" / name, media / name)
samples = root / "docs" / "samples"
for scene in ("air-conditioner", "keyboard"):
    for method in ("original", "deepfilternet3", "rnnoise", "webrtc"):
        clip = f"{scene}-{method}.flac"
        shutil.copyfile(samples / clip, media / clip)
shutil.copyfile(samples / "compare-16s.mp4", media / "compare-16s.mp4")
notices = [root / "LICENSE", root / "assets" / "tabler" / "LICENSE", site / "octicons-LICENSE.txt"]
(out / "licenses.txt").write_text("\n\n".join(p.read_text(encoding="utf-8") for p in notices), encoding="utf-8")
