# File clipboard replay prevention

## Failure and evidence

A single six-file copy was repeatedly materialized under Downloads/MineShare
on both Windows peers. Repeated transfer filenames accumulated numeric suffixes.
The Laptop initially contained 30 files but only six unique SHA-256 hashes.
Log entries publishing six received clipboard files aligned with daemon restarts.

The in-memory echo fingerprint disappeared when the process restarted, while the
native file clipboard survived. The watcher's initial poll therefore treated the
old file clipboard as a fresh copy. Windows revision changes could also bypass
fingerprint equality. Separately, a failed file batch was retried every poll,
potentially re-transferring files already delivered before the failure.

## Corrected behavior

- Establish a file-clipboard baseline at startup; do not replay files already
  present. Copy files again after launching/connecting to request a new send.
- On Windows, atomically publish incoming CF_HDROP with arboard's
  ExcludeClipboardContentFromMonitorProcessing format. Honor that format when
  watching files, including across process restarts. A genuine new native Copy
  replaces the format, so subsequent user copies still work.
- Record a file snapshot as attempted before starting side-effecting transfers.
  A failure requires a new Copy rather than an unbounded automatic retry.
- Apply startup baselining to Linux/content-polling paths too. No protocol
  change or blanket disabling of file transfer, text or image clipboard sync.

Regression tests cover the original six-file/four-restart case, peer-origin
revision changes, failed-send repeat polls, and fresh copies after baseline.
No existing user files or duplicate files are automatically deleted.
