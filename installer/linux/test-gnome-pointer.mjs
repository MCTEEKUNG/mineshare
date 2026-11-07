import {readFileSync} from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';

const source = readFileSync(new URL('./gnome-pointer/extension.js', import.meta.url), 'utf8')
    .replace(/^import .*;\r?\n/gm, '').replace('export default class', 'globalThis.ExtensionClass = class');
let handler, exported, unexported = 0;
let revealTick, children = [];
class AppSwitcherPopup {
    _haveModal = true;
    _items = [0, 1, 2];
    _selectedIndex = 1;
    _initialDelayTimeoutId = 1;
    _previous() { return (this._selectedIndex + 2) % 3; }
    _select(index) { this._selectedIndex = index; }
    _showImmediately() { this._initialDelayTimeoutId = 0; }
}
const context = vm.createContext({
    Extension: class {}, AppSwitcherPopup, WindowSwitcherPopup: class {},
    Main: {sessionMode: {isLocked: false}, overview: {visible: false}, uiGroup: {get_children: () => children}},
    GLib: {
        SOURCE_CONTINUE: true, SOURCE_REMOVE: false,
        Variant: class { constructor(signature, values) { assert.equal(signature, '(b)'); this.values = values; } },
        timeout_add(_priority, delay, callback) {
            assert.equal(delay, 8);
            revealTick = callback;
            return 1;
        },
        source_remove(id) { assert.equal(id, 1); revealTick = undefined; },
    },
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
handler.RevealWindowSwitcher();
assert.equal(revealTick(), true, 'wait briefly if Mutter has not received Tab yet');
const popup = new AppSwitcherPopup();
children.push(popup);
assert.equal(revealTick(), false);
assert.equal(popup._initialDelayTimeoutId, 0, 'reveal the existing switcher');
handler.RevealWindowSwitcher();
for (let i = 0; i < 4; i++)
    assert.equal(revealTick(), true);
assert.equal(revealTick(), false, 'stop waiting if the shortcut never opens a switcher');
handler.RevealWindowSwitcher();
context.Main.sessionMode.isLocked = true;
assert.deepEqual(Array.from(handler.GetPointer()), [0, 0, 0, 0]);
popup._initialDelayTimeoutId = 1;
assert.equal(revealTick(), false);
assert.equal(popup._initialDelayTimeoutId, 1, 'stop a pending reveal when the session locks');
context.Main.sessionMode.isLocked = false;
let handled;
const invokeStep = count => handler.StepWindowSwitcherAsync([count], {
    return_value(value) { handled = value.values[0]; },
});
invokeStep(0);
assert.equal(handled, true, 'advertise reverse support without changing selection');
assert.equal(popup._selectedIndex, 1);
children = [];
invokeStep(2);
assert.equal(revealTick(), true, 'wait for the initial Alt+Tab');
children = [popup];
assert.equal(revealTick(), false);
assert.equal(handled, true);
assert.equal(popup._selectedIndex, 2, 'a reverse start wraps back past the focused window');
invokeStep(1);
assert.equal(popup._selectedIndex, 1, 'move one window back when the popup already exists');
context.Main.sessionMode.isLocked = true;
invokeStep(1);
assert.equal(handled, false);
assert.equal(popup._selectedIndex, 1);
context.Main.sessionMode.isLocked = false;
handler.RevealWindowSwitcher();
extension.disable();
assert.equal(revealTick, undefined, 'cancel pending reveal when disabled');
extension.disable();
assert.equal(unexported, 1);
const metadata = JSON.parse(readFileSync(new URL('./gnome-pointer/metadata.json', import.meta.url)));
assert(metadata['shell-version'].includes('50'));
console.log('PASS: pointer coordinates, lock guard, bounded switcher reveal and backward selection');
