"""Archive the working source, including uncommitted files, without build output."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import zipfile

root = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser()
parser.add_argument('output', type=Path)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
prefixes = ('.cargo/', '.github/', 'crates/', 'ui/', 'scripts/', 'installer/', 'tools/performance/')
names = subprocess.check_output(['git', 'ls-files', '--cached', '--others', '--exclude-standard', '-z'], cwd=root).decode().split('\0')
names = sorted({name for name in names if name and (name.startswith(prefixes) or name in ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml']) and (root/name).is_file()})
with zipfile.ZipFile(args.output/'source.zip', 'w', zipfile.ZIP_DEFLATED) as archive:
    for name in names:
        archive.write(root/name, name)
(args.output/'sources.json').write_text(json.dumps({name: hashlib.sha256((root/name).read_bytes()).hexdigest() for name in names}, indent=2))
(args.output/'git-status.txt').write_bytes(subprocess.check_output(['git', 'status', '--short'], cwd=root))
(args.output/'working-tree.patch').write_bytes(subprocess.check_output(['git', 'diff', '--binary'], cwd=root))
print(json.dumps({'files': len(names), 'archive': str(args.output/'source.zip')}))
