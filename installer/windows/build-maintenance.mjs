import {spawnSync} from 'node:child_process';
import {copyFileSync, existsSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import path from 'node:path';
const directory = path.dirname(fileURLToPath(import.meta.url));
const result = spawnSync('cargo', ['build', '--release', '--locked', '--manifest-path', path.join(directory, 'maintenance/Cargo.toml')], {stdio:'inherit'});
if (result.status !== 0) process.exit(result.status ?? 1);
const root = path.resolve(directory, '../../ui/src-tauri/target');
let copied = 0;
for (const profile of ['release','debug']) {
  const binary = path.join(root, profile, 'mineshare-app.exe');
  if (existsSync(binary)) {
    copyFileSync(path.join(directory, 'maintenance/target/release/mineshare-installer.exe'), binary + '.installer-helper.exe');
    copied++;
  }
}
if (!copied) throw new Error('No application binary found for MSI helper');
