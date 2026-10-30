"""Check both workspace policies; optionally audit actual cargo -vv invocations."""
import argparse
import json
import re
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
EXPECTED = {"opt-level": 3, "lto": "thin", "codegen-units": 1,
            "debug": 1, "split-debuginfo": "packed", "strip": "none"}
GUI_EXPECTED = {**EXPECTED, "lto": False, "codegen-units": 16}

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--log", type=Path)
    parser.add_argument("--crate", choices=["mineshare_app", "mineshare_daemon"])
    parser.add_argument("--symbols-dir", type=Path)
    args = parser.parse_args()
    for relative in ["Cargo.toml", "ui/src-tauri/Cargo.toml"]:
        manifest = tomllib.loads((ROOT / relative).read_text(encoding="utf-8"))
        expected = GUI_EXPECTED if relative.startswith('ui/') else EXPECTED
        assert manifest["profile"]["release"] == expected, f"Release profile drift: {relative}"
    if args.log:
        assert args.crate, "--crate is required with --log"
        lines = args.log.read_text(encoding="utf-8-sig", errors="replace").splitlines()
        invocation = [line for line in lines if re.search(r"--crate-name " + args.crate + r"\b", line) and "--crate-type bin" in line]
        assert invocation, "No fresh binary compiler invocation; capture a verbose rebuild"
        for line in invocation:
            units = 16 if args.crate == 'mineshare_app' else 1
            for flag in ["-C opt-level=3", f"-C codegen-units={units}", "-C debuginfo=1", "-C split-debuginfo=packed"]:
                assert flag in line, f"Missing effective flag: {flag}"
            if args.crate == 'mineshare_app':
                assert '-C lto=' not in line or '-C lto=off' in line, 'GUI LTO policy changed without acceptance'
            else:
                assert '-C lto=thin' in line, 'Missing daemon ThinLTO'
            assert "-C strip=" not in line or "-C strip=none" in line, "Symbols were stripped"
    if args.symbols_dir:
        assert args.crate, '--crate is required with --symbols-dir'
        symbols = [path for path in args.symbols_dir.iterdir()
                   if path.is_file() and path.suffix in ['.pdb', '.dwp']
                   and path.stem.replace('-', '_') == args.crate and path.stat().st_size > 0]
        assert symbols, 'Matching standalone debug symbols are missing'
    print(json.dumps({"status": "passed", "daemon_policy": EXPECTED, "gui_policy": GUI_EXPECTED, "effective_log": str(args.log) if args.log else None}))

if __name__ == "__main__":
    main()
