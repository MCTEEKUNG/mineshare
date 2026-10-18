import { readFileSync, mkdirSync, writeFileSync, chmodSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const directory = path.dirname(fileURLToPath(import.meta.url));
const target = path.resolve(directory, '../../ui/src-tauri/target/deb-hooks');
mkdirSync(target, { recursive: true });
const read = name => readFileSync(path.join(directory, name), 'utf8').replaceAll('\r\n', '\n');
const lifecycle = read('deb-lifecycle.sh');
for (const name of ['preinst', 'postinst']) {
  const entry = read(`deb-${name}`).replace(/^#![^\n]*\n/, '');
  const output = path.join(target, name);
  writeFileSync(output, `${lifecycle}\n${entry}`);
  chmodSync(output, 0o755);
}
