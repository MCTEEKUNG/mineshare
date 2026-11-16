"""Allow only Tauri's documented bundle-type marker patch in the MSI copy."""
import hashlib
import json
from pathlib import Path
import sys

payload=Path(sys.argv[1]).read_bytes()
unbundled=Path(sys.argv[2]).read_bytes()
source=sys.argv[3]
assert len(payload)==len(unbundled), 'Executable lengths differ'
differences=[i for i,(a,b) in enumerate(zip(payload,unbundled)) if a!=b]
if differences:
    at=differences[0]
    tag=b'__TAURI_BUNDLE_TYPE_VAR_'
    assert differences==list(range(at,at+3)), 'Unexpected executable differences'
    assert payload[at:at+3]==b'MSI' and unbundled[at:at+3]==b'UNK'
    assert payload[at-len(tag):at]==unbundled[at-len(tag):at]==tag
assert source.encode() in payload and source.encode() in unbundled
report={'source_id':source,'payload_sha256':hashlib.sha256(payload).hexdigest(),
        'unbundled_sha256':hashlib.sha256(unbundled).hexdigest(),
        'only_bundle_type_marker_differs':True,'different_bytes':len(differences)}
Path(sys.argv[4]).write_text(json.dumps(report,indent=2),encoding='utf-8')
print(json.dumps(report))
