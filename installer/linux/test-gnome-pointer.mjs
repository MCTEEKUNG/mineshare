import {readFileSync} from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';

const source = readFileSync(new URL('./gnome-pointer/extension.js', import.meta.url), 'utf8')
    .replace(/^import .*;\r?\n/gm, '').replace('export default class', 'globalThis.ExtensionClass = class');
let handler, exported, unexported = 0;
const context = vm.createContext({
    Extension: class {}, Main: {sessionMode: {isLocked: false}},
    global: {get_pointer: () => [800, 350, 0], screen_width: 1280, screen_height: 720},
    Gio: {DBus: {session: {}}, DBusExportedObject: {wrapJSObject(iface, implementation) {
        assert.match(iface, /name="GetPointer"/);
        handler = implementation;
        return {export(_bus, path) { exported = path; }, unexport() { unexported++; }};
    }}},
});
vm.runInContext(source, context);
const extension = new context.ExtensionClass();
extension.enable();
assert.equal(exported, '/org/gnome/Shell/Extensions/MineSharePointer');
assert.deepEqual(Array.from(handler.GetPointer()), [800, 350, 1280, 720]);
context.Main.sessionMode.isLocked = true;
assert.deepEqual(Array.from(handler.GetPointer()), [0, 0, 0, 0]);
extension.disable();
extension.disable();
assert.equal(unexported, 1);
const metadata = JSON.parse(readFileSync(new URL('./gnome-pointer/metadata.json', import.meta.url)));
assert(metadata['shell-version'].includes('50'));
console.log('PASS: read-only pointer coordinates, logical dimensions, lock guard and extension cleanup');
