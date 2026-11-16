"""Clock probe and interactive-session audio stimulus for Windows pair tests."""
import ctypes
import json
from pathlib import Path
import subprocess
import sys
import time

def now_ns():
    value = ctypes.c_ulonglong()
    ctypes.windll.kernel32.GetSystemTimePreciseAsFileTime(ctypes.byref(value))
    return (value.value - 116444736000000000) * 100

if sys.argv[1] == "clock":
    for line in sys.stdin:
        received = now_ns()
        request = json.loads(line)
        print(json.dumps({"id": request["id"], "rx": received, "tx": now_ns()}), flush=True)
elif sys.argv[1] in ["capture", "record"]:
    import winsound
    folder, recorder = map(Path, sys.argv[2:4])
    stimulus = Path(sys.argv[4]) if sys.argv[1] == 'capture' else None
    folder.mkdir(parents=True, exist_ok=True)
    with (folder / "recorder.log").open("w") as log:
        process = subprocess.Popen([str(recorder), str(folder), "46"], stdout=log, stderr=subprocess.STDOUT,
                                   creationflags=subprocess.CREATE_NO_WINDOW)
        try:
            deadline = time.monotonic() + 10
            while not (folder / "windows-ready.json").exists():
                assert process.poll() is None, "Recorder exited before ready"
                assert time.monotonic() < deadline, "Recorder readiness timeout"
                time.sleep(.05)
            if stimulus is not None:
                time.sleep(3)
                (folder / "stimulus-start.json").write_text(json.dumps({"utc_ns": now_ns()}))
                # Bound playback explicitly; native endpoint timestamps are
                # the timing reference in either source direction.
                winsound.PlaySound(str(stimulus), winsound.SND_FILENAME | winsound.SND_ASYNC)
                time.sleep(36.25)
                winsound.PlaySound(None, 0)
            assert process.wait(timeout=55) == 0, "Recorder failed"
            (folder / "completed.json").write_text(json.dumps({"ok": True}))
        finally:
            if process.poll() is None:
                process.terminate()
