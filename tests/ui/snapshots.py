"""Compare the rendered GPUI panels on an isolated Ubuntu X11 display."""

import argparse
from contextlib import contextmanager
import io
import json
import os
from pathlib import Path
import struct
import subprocess
import tempfile
import time

from PIL import Image, ImageChops, ImageGrab, ImageOps


HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
BASELINES = HERE / "snapshots"
OUTPUT = ROOT / "target/ui-snapshots"
SCENES = (
    "mixer", "mixer-full", "mixer-bypass", "devices", "devices-picker", "preferences",
    "preferences-checking", "preferences-error", "preferences-current", "preferences-available",
    "log", "mic", "mic-offline", "mic-error", "mic-standalone", "mic-recording", "mic-recorded",
    "mic-playing", "adjust-room", "adjust-measuring", "adjust-read", "adjust-recording",
    "adjust-analyzing", "adjust-opening", "adjust-preview", "adjust-restoring", "adjust-ready",
    "adjust-failed",
)


def test_binary():
    """Find the executable Cargo actually built, including custom target directories."""
    build = subprocess.run(
        ["cargo", "test", "--locked", "-p", "orvia", "--bin", "orvia", "--no-run",
         "--message-format=json-render-diagnostics"], cwd=ROOT, stdout=subprocess.PIPE,
        text=True, check=True,
    )
    for line in build.stdout.splitlines():
        item = json.loads(line)
        if (item.get("reason") == "compiler-artifact" and item.get("executable")
                and item["target"]["name"] == "orvia" and item["profile"]["test"]):
            return item["executable"]
    raise RuntimeError("Cargo did not produce the mixer test executable")


@contextmanager
def window_manager():
    """GPUI's X11 surface starts presenting when a window manager maps the client."""
    manager = subprocess.Popen(["openbox", "--sm-disable"])
    try:
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            prop = subprocess.check_output(["xprop", "-root", "_NET_SUPPORTING_WM_CHECK"], text=True)
            if "window id #" in prop:
                break
            if manager.poll() is not None:
                raise RuntimeError("Openbox exited before managing the display")
            time.sleep(0.1)
        else:
            raise RuntimeError("Openbox did not acquire the display")
        yield
    finally:
        manager.terminate()
        manager.wait(timeout=10)


def capture(executable, directory, scene, language):
    env = dict(os.environ, ORVIA_SNAPSHOT_SCENE=scene, ORVIA_SNAPSHOT_LANGUAGE=language,
               ORVIA_SNAPSHOT_DIR=str(directory), FONTCONFIG_FILE=str(HERE / "fonts.conf"),
               XDG_DATA_HOME=str(directory), XDG_RUNTIME_DIR=str(directory), XDG_SESSION_TYPE="x11", LANG="C.UTF-8",
               LC_ALL="C.UTF-8")
    env.pop("WAYLAND_DISPLAY", None)
    env.pop("ZED_HEADLESS", None)
    process = subprocess.Popen([executable, "--ignored", "--exact", "snapshots::render_window",
                                "--nocapture"], cwd=directory, env=env)
    try:
        image = None
        previous = None
        stable_since = None
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise RuntimeError(f"{scene}: window exited with status {process.returncode}")
            windows = subprocess.run(["xdotool", "search", "--onlyvisible", "--name",
                                      "^Orvia snapshot$"], capture_output=True, text=True)
            if windows.stdout.strip():
                wid = windows.stdout.splitlines()[-1]
                pixels = subprocess.check_output(["import", "-window", wid, "png:-"], timeout=10)
                with Image.open(io.BytesIO(pixels)) as frame:
                    image = frame.convert("RGB")
                if image.size != (830, 900 if scene == "mixer-full" else 610):
                    raise RuntimeError(f"{scene}: unexpected window size {image.size}")
                # Wait for the initial paint, asynchronous icons and the popup anchor layout.
                if image.getcolors(64) is None:
                    if previous is not None and not ImageChops.difference(previous, image).getbbox():
                        if time.monotonic() - stable_since >= 0.6:
                            return image
                    else:
                        previous = image
                        stable_since = time.monotonic()
            time.sleep(0.1)
        # Preserve the display and window tree if the runner cannot capture a settled frame.
        save(ImageGrab.grab(xdisplay=env["DISPLAY"]), OUTPUT / "diagnostics" / f"{scene}.png")
        subprocess.run(["xwininfo", "-root", "-tree"], check=False)
        detail = "no visible window" if image is None else f"last frame extrema: {image.getextrema()}"
        raise RuntimeError(f"{scene}: window did not settle ({detail})")
    finally:
        process.terminate()
        process.wait(timeout=10)


def save(image, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    image.save(destination)


def compare(image, relative):
    baseline = BASELINES / relative
    if not baseline.exists():
        print(f"Missing baseline: {relative}", flush=True)
        return False
    with Image.open(baseline) as previous:
        expected = previous.convert("RGB")
    if expected.size == image.size:
        delta = ImageChops.difference(expected, image)
        if not delta.getbbox():
            return True
        mask = ImageChops.lighter(ImageChops.lighter(delta.getchannel(0), delta.getchannel(1)),
                                 delta.getchannel(2)).point(lambda value: 255 if value else 0)
        overlay = ImageOps.grayscale(image).convert("RGB")
        overlay.paste((255, 0, 105), mask=mask)
        save(overlay, OUTPUT / "diff" / relative)
    save(expected, OUTPUT / "expected" / relative)
    print(f"Changed: {relative}", flush=True)
    return False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--update", action="store_true", help="replace baselines after UI review")
    args = parser.parse_args()
    executable = test_binary()
    failures = []
    cases = {Path(lang, scene + ".png") for lang in ("ja", "en") for scene in SCENES}
    obsolete = {p.relative_to(BASELINES) for p in BASELINES.rglob("*.png")} - cases
    if obsolete:
        raise SystemExit(f"Remove obsolete baselines: {sorted(str(p) for p in obsolete)}")
    with tempfile.TemporaryDirectory() as tmp, window_manager():
        directory = Path(tmp)
        # A one-second stereo IEEE-float take, in the engine's on-disk format.
        data = bytes(8000 * 2 * 4)
        fmt = struct.pack("<HHIIHH", 3, 2, 8000, 64000, 8, 32)
        (directory / "take.wav").write_bytes(b"RIFF" + struct.pack("<I", 36 + len(data))
                                              + b"WAVEfmt " + struct.pack("<I", len(fmt)) + fmt
                                              + b"data" + struct.pack("<I", len(data)) + data)
        subprocess.run(["xdotool", "mousemove", "0", "0"], check=True)
        for relative in sorted(cases):
            image = capture(executable, directory, relative.stem, relative.parent.name)
            save(image, OUTPUT / "actual" / relative)
            if args.update:
                save(image, BASELINES / relative)
            elif not compare(image, relative):
                failures.append(relative)
    if failures:
        raise SystemExit(f"{len(failures)} of {len(cases)} snapshots differ")
    print(f"{len(cases)} snapshots {'updated' if args.update else 'matched'}")


if __name__ == "__main__":
    main()
