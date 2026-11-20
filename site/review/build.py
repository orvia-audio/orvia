"""Index committed UI snapshots and package the two most recent revisions for Pages."""

import json
from pathlib import Path
import re
import shutil
import subprocess


HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
SNAPSHOTS = "tests/ui/snapshots"


def git(*args, data=None):
    return subprocess.run(["git", *args], cwd=ROOT, input=data, stdout=subprocess.PIPE,
                          check=True).stdout


def build(destination):
    commits = git("rev-list", "HEAD").decode().splitlines()
    requests = "".join(f"{commit}:{SNAPSHOTS}\n" for commit in commits)
    answers = git("cat-file", "--batch-check=%(objectname)", data=requests.encode()).decode().splitlines()
    revisions = {commit: tree if re.fullmatch("[0-9a-f]{40}", tree) else None
                 for commit, tree in zip(commits, answers, strict=True)}
    trees = {}
    for tree in dict.fromkeys(revisions.values()):
        if tree is None:
            continue
        files = {}
        for entry in git("ls-tree", "-r", tree).decode().splitlines():
            metadata, path = entry.split("\t", 1)
            if re.fullmatch(r"(?:en|ja)/[a-z0-9-]+\.png", path):
                files[path] = metadata.split()[2]
        trees[tree] = files

    history = git("log", "--first-parent", "--format=%H %s", "HEAD", "--",
                  f":(glob){SNAPSHOTS}/*/*.png")
    recent = [line.split(" ", 1) for line in history.decode().splitlines()]
    current_files = trees.get(revisions[commits[0]], {})
    current = next((commit for commit, _ in recent if trees.get(revisions[commit], {}) == current_files),
                   commits[0])
    base = next((commit for commit, _ in recent
                 if revisions[commit] and trees[revisions[commit]] != current_files), None)
    bundled = set()
    for commit in (current, base):
        if commit:
            bundled.update(trees.get(revisions[commit], {}).values())

    images = destination / "images"
    images.mkdir(parents=True, exist_ok=True)
    for blob in sorted(bundled):
        (images / f"{blob}.png").write_bytes(git("cat-file", "blob", blob))
    catalog = dict(current=current, base=base, recent=recent[:20], revisions=revisions,
                   trees=trees, bundled=sorted(bundled))
    (destination / "catalog.json").write_text(json.dumps(catalog, separators=(",", ":")), encoding="utf-8")
    for name in ("index.html", "review.css", "review.js"):
        shutil.copyfile(HERE / name, destination / name)


if __name__ == "__main__":
    build(ROOT / "dist/pages/review")
