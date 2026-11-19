"""Record a generated tone through the installed Orvia PipeWire microphone."""

import array
import contextlib
import json
import math
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time
import wave


def wait_for(check, message, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.1)
    raise RuntimeError(message)


with tempfile.TemporaryDirectory() as tmp, contextlib.ExitStack() as processes:
    env = {**os.environ, "XDG_RUNTIME_DIR": tmp, "XDG_DATA_HOME": tmp,
           "PULSE_SERVER": f"unix:{tmp}/pulse/native"}

    def run(*args):
        return subprocess.run(args, env=env, check=True, capture_output=True, text=True, timeout=15).stdout

    def start(*args, **kwargs):
        process = subprocess.Popen(args, env=env, **kwargs)

        def stop():
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)

        processes.callback(stop)
        return process

    start("pipewire")
    wait_for(lambda: Path(tmp, "pipewire-0").is_socket(), "PipeWire did not start")
    start("wireplumber")
    start("pipewire-pulse")
    wait_for(lambda: Path(tmp, "pulse/native").is_socket(), "Pulse socket unavailable")
    wait_for(lambda: "orvia_microphone" in run("pactl", "list", "short", "sources"),
             "The packaged source was not loaded")
    for kind, name, description in [("sinks", "orvia_output", "Orvia Output"),
                                    ("sources", "orvia_microphone", "Orvia Microphone")]:
        devices = json.loads(run("pactl", "--format=json", "list", kind))
        assert next(d["description"] for d in devices if d["name"] == name) == description
    run("pactl", "load-module", "module-null-sink", "sink_name=test_input", "rate=48000")
    run("pactl", "load-module", "module-remap-source", "master=test_input.monitor",
        "source_name=test_microphone")
    run("pactl", "load-module", "module-null-sink", "sink_name=test_headphones", "rate=48000")
    state = Path(tmp, "orvia")
    state.mkdir()
    (state / "settings.json").write_text(json.dumps({
        "input": "pulseaudio:test_microphone", "output": "pulseaudio:orvia_output",
        "gain_percent": 100, "suppress_noise": False, "monitor": "pulseaudio:test_headphones",
    }))
    (state / "observe").touch()
    engine = start("orvia-engine")
    status = state / "status.json"
    wait_for(lambda: status.exists() and "Streaming" in json.loads(status.read_text())["message"],
             "Orvia did not start streaming")
    samples = array.array("h", (int(8000 * math.sin(2 * math.pi * 440 * i / 48000))
                                for i in range(48000 * 2)))
    raw = Path(tmp, "recording.raw")
    with raw.open("wb") as output:
        recorder = start("parec", "-d", "orvia_microphone", "--latency-msec=20", "--raw", "--format=s16le",
                         "--channels=1", "--rate=48000", stdout=output)
        time.sleep(0.5)
        subprocess.run(["pacat", "-d", "test_input", "--latency-msec=20", "--raw", "--format=s16le",
                        "--channels=1", "--rate=48000"], env=env, input=samples.tobytes(), check=True)
        time.sleep(0.5)
        recorder.send_signal(signal.SIGINT)
        recorder.wait(timeout=5)
    captured = array.array("h", raw.read_bytes())
    # A 100 ms window contains exactly 44 periods. Require a coherent tone.
    size = 4800
    reference = [complex(math.cos(i * 2 * math.pi * 440 / 48000),
                    math.sin(i * 2 * math.pi * 440 / 48000)) for i in range(size)]
    amplitude = max((2 * abs(sum(x * w for x, w in zip(captured[n:n+size], reference))) / size
                     for n in range(0, len(captured) - size, size)), default=0)
    if not 3000 < amplitude < 11000:
        snapshot = json.loads(status.read_text()) if status.exists() else {}
        snapshot.pop("signal", None)
        raise AssertionError(f"440 Hz amplitude {amplitude:.0f} in {len(captured)} samples; engine {snapshot}")
    mixer = start("orvia")
    def window_visible():
        if (code := mixer.poll()) is not None:
            raise RuntimeError(f"The installed mixer exited before showing a window (status {code})")
        return subprocess.run(["xdotool", "search", "--onlyvisible", "--name", "^Orvia$"],
                              env=env, capture_output=True).returncode == 0
    wait_for(window_visible, "The installed mixer window did not appear", timeout=45)
    (state / "close-window").touch()
    assert mixer.wait(timeout=10) == 0

    def pulse_devices(kind):
        return json.loads(run("pactl", "--format=json", "list", kind))

    def device_index(kind, name):
        return next(d["index"] for d in pulse_devices(kind) if d["name"] == name)

    source = device_index("sources", "orvia_microphone")
    headphones = device_index("sinks", "test_headphones")

    def recording():
        return any(s["source"] == source for s in pulse_devices("source-outputs"))

    def playing():
        return any(s["sink"] == headphones for s in pulse_devices("sink-inputs"))

    speech_path = Path(tmp, "passage.wav")
    run("espeak-ng", "-v", "en-us", "-s", "148", "-w", str(speech_path), "I opened the window and felt a cool breeze.")
    with wave.open(str(speech_path)) as speech_file:
        assert speech_file.getsampwidth() == 2 and speech_file.getnchannels() == 1
        speech_rate = speech_file.getframerate()
        speech = array.array("h", speech_file.readframes(speech_file.getnframes()))
    def read_passage(sink, samples=speech):
        # Lower the voice to exercise normalization and keep the cable-only marker distinct.
        pcm = array.array("h", (s // 32 for s in samples))
        subprocess.run(["pacat", "-d", sink, "--latency-msec=20", "--raw", "--format=s16le",
                        "--channels=1", f"--rate={speech_rate}"], env=env, input=pcm.tobytes(), check=True)
    test_panel = start("orvia", "--mic-test")
    window = run("xdotool", "search", "--sync", "--onlyvisible", "--name", "Orvia .*Mic test").splitlines()[0]
    run("xdotool", "windowfocus", "--sync", window)
    # With an empty take, the primary button is the last control in the panel.
    run("xdotool", "key", "--delay", "250", "shift+Tab", "Return")
    wait_for(recording, "The primary button did not start recording")
    time.sleep(1)
    run("xdotool", "key", "Return")
    take = state / "mic-test" / "recording.wav"
    wait_for(take.is_file, "Stopping the recording did not save a take")
    first = take.read_bytes()
    run("xdotool", "key", "Return")
    wait_for(playing, "The next activation of the primary button did not play the take")
    wait_for(lambda: not playing(), "Playback did not finish")
    assert take.read_bytes() == first and not recording()

    # Retake is a separate control. It returns focus to the primary button.
    run("xdotool", "key", "--delay", "250", "Tab", "Return")
    wait_for(recording, "Retake did not start a new recording")
    mid_sentence = int(speech_rate * 1.34)
    read_passage("orvia_output", speech[:mid_sentence])
    time.sleep(11)
    assert recording() and take.read_bytes() == first, "A pause mid-passage must not end the take"
    read_passage("orvia_output", speech[mid_sentence:])
    wait_for(lambda: take.stat().st_size > len(first), "Reading the passage did not finish the retake", timeout=45)
    second = take.read_bytes()
    run("xdotool", "key", "Return")
    wait_for(playing, "The primary button did not play the automatically stopped retake")
    assert take.read_bytes() == second and not recording()
    test_panel.terminate()
    test_panel.wait(timeout=10)

    # The marker exists only on the cable. Hearing it distinguishes a receiver
    # capture from an internal mix tap, even when the physical input is silent.
    def set_controls(**values):
        path = state / "settings.json"
        settings = json.loads(path.read_text())
        stage = state / "test-settings.tmp"
        stage.write_text(json.dumps({**settings, **values}))
        stage.replace(path)

    def play_pcm(sink, pcm):
        subprocess.run(["pacat", "-d", sink, "--latency-msec=20", "--raw", "--format=s16le",
                        "--channels=1", "--rate=48000"], env=env, input=pcm.tobytes(), check=True, timeout=18)

    marker = array.array("h", (int(1300 * math.sin(2 * math.pi * 750 * i / 48000))
                               for i in range(48000)))

    def record_headphones(action):
        path = Path(tmp, "headphones.raw")
        with path.open("wb") as output:
            capture = start("parec", "-d", "test_headphones.monitor", "--latency-msec=20", "--raw",
                            "--format=s16le", "--channels=1", "--rate=48000", stdout=output)
            time.sleep(0.3)
            action()
            time.sleep(0.4)
            capture.terminate()
            capture.wait(timeout=5)
        pcm = array.array("h", path.read_bytes())
        # Measure RMS in the loudest 100 ms, excluding stream startup and silence.
        return max((math.sqrt(sum(x*x for x in pcm[i:i+4800]) / 4800)
                    for i in range(0, len(pcm)-4800, 4800)), default=0)

    set_controls(monitor_enabled=True)
    wait_for(playing, "Receiver monitor did not open headphones")
    unity = record_headphones(lambda: play_pcm("orvia_output", marker))
    assert unity > 150, unity
    set_controls(monitor_enabled=False, suppress_noise=True,
                 noise_method="deepfilternet3", deep_filter_limit_db=12, mixing="average",
                 check_updates_automatically=False)
    wait_for(lambda: not playing(), "Headphone monitor did not stop")

    mixer = start("orvia")
    wait_for(window_visible, "Mixer did not reopen", timeout=45)
    window = run("xdotool", "search", "--onlyvisible", "--name", "^Orvia$").splitlines()[0]
    run("xdotool", "windowfocus", "--sync", window)
    physical = device_index("sources", "test_microphone")

    def mic_count():
        return sum(s["source"] == physical for s in pulse_devices("source-outputs"))

    # Four sidebar controls, four suppression choices, then Auto adjust.
    time.sleep(1)
    run("xdotool", "key", "--delay", "100", *["Tab"]*9, "Return")
    wait_for(lambda: mic_count() >= 2, "Auto adjust did not start room capture")
    wait_for(lambda: mic_count() == 1, "Room capture did not finish")
    for phrase_number in range(2):
        time.sleep(0.6)
        run("xdotool", "key", "Return")
        wait_for(lambda: mic_count() >= 2, "The reading button did not open the microphone")
        read_passage("test_input")
        wait_for(lambda: mic_count() == 1, "The passage was not recognized", timeout=45)
    # Wait for analysis. The reading button only becomes active when the suggestion is ready.
    def start_preview():
        if recording():
            return True
        run("xdotool", "key", "Return")
        time.sleep(0.4)
        return recording()
    wait_for(start_preview, "The suggested mix was not recorded on the virtual microphone", timeout=45)
    trial = state / "suppression-audition.json"
    assert trial.is_file()
    assert json.loads((state / "settings.json").read_text())["deep_filter_limit_db"] == 12
    play_pcm("orvia_output", marker)
    def hear_preview():
        read_passage("test_input")
        wait_for(playing, "The virtual microphone take did not play automatically", timeout=45)
        wait_for(lambda: not playing(), "The preview did not finish", timeout=45)
    preview = record_headphones(hear_preview)
    assert 0.8 < preview/unity < 1.25, (preview, unity)
    assert not trial.exists() and take.read_bytes() == second
    assert json.loads((state / "settings.json").read_text())["deep_filter_limit_db"] == 12
    (state / "close-window").touch()
    assert mixer.wait(timeout=10) == 0
    assert engine.poll() is None
    (state / f"stop-engine-{engine.pid}").touch()
    assert engine.wait(timeout=10) == 0
    print(f"Recorded {len(captured)/48000:.2f} seconds, 440 Hz amplitude {amplitude:.0f}.")
