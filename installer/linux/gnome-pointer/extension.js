import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import {AppSwitcherPopup, WindowSwitcherPopup} from 'resource:///org/gnome/shell/ui/altTab.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

const iface = `<node><interface name="org.gnome.Shell.Extensions.MineSharePointer">
<method name="GetPointer">
<arg type="i" direction="out" name="x"/><arg type="i" direction="out" name="y"/>
<arg type="i" direction="out" name="width"/><arg type="i" direction="out" name="height"/>
</method>
<method name="GetPasteWindow"><arg type="t" direction="out" name="id"/></method>
<method name="RevealWindowSwitcher"/>
<method name="StepWindowSwitcher"><arg type="u" direction="in" name="backward"/><arg type="b" direction="out" name="handled"/></method>
</interface></node>`;

export default class MineSharePointer extends Extension {
    enable() {
        this._revealSource = 0;
        this._stepRequest = null;
        this._export = Gio.DBusExportedObject.wrapJSObject(iface, {
            GetPointer() {
                // No input injection, clipboard data, eval or shell commands.
                if (Main.sessionMode.isLocked)
                    return [0, 0, 0, 0];
                const [x, y] = global.get_pointer();
                return [x, y, global.screen_width, global.screen_height];
            },
            GetPasteWindow() {
                if (Main.sessionMode.isLocked || Main.overview.visible)
                    return 0;
                // Opaque identifier only, so a delayed paste can check focus.
                return global.display.focus_window?.get_stable_sequence() ?? 0;
            },
            RevealWindowSwitcher: () => this._revealWindowSwitcher(),
            StepWindowSwitcherAsync: ([count], invocation) => this._stepWindowSwitcher(count, invocation),
        });
        this._export.export(Gio.DBus.session, '/org/gnome/Shell/Extensions/MineSharePointer');
    }

    _windowSwitcher() {
        if (Main.sessionMode.isLocked || Main.overview.visible)
            return null;
        return Main.uiGroup.get_children().find(child =>
            (child instanceof AppSwitcherPopup || child instanceof WindowSwitcherPopup) && child._haveModal);
    }

    _revealWindowSwitcher() {
        if (this._revealSource || Main.sessionMode.isLocked || Main.overview.visible)
            return;
        // Mutter can dispatch the injected Tab after the D-Bus request. Briefly
        // wait for its existing popup without creating one or changing selection.
        let attempts = 5;
        this._revealSource = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 8, () => {
            if (!Main.sessionMode.isLocked && !Main.overview.visible) {
                const popup = this._windowSwitcher();
                if (popup?._initialDelayTimeoutId > 0 && typeof popup._showImmediately === 'function')
                    popup._showImmediately();
                else if (--attempts > 0)
                    return GLib.SOURCE_CONTINUE;
            }
            this._revealSource = 0;
            return GLib.SOURCE_REMOVE;
        });
    }

    _stepWindowSwitcher(count, invocation) {
        const reply = handled => invocation.return_value(new GLib.Variant('(b)', [handled]));
        if (count === 0) {
            reply(true);
            return;
        }
        if (count > 4 || this._stepRequest || Main.sessionMode.isLocked || Main.overview.visible) {
            reply(false);
            return;
        }
        const step = () => {
            const popup = this._windowSwitcher();
            if (!popup?._items?.length)
                return false;
            for (let i = 0; i < count; i++)
                popup._select(popup._previous());
            popup._showImmediately?.();
            return true;
        };
        if (step()) {
            reply(true);
            return;
        }
        // The initial Tab may still be in Mutter's input queue. Hold the
        // reply until the popup is ready so MineShare can release Alt after it.
        let attempts = 3;
        const id = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 8, () => {
            const handled = step();
            if (!handled && !Main.sessionMode.isLocked && !Main.overview.visible && --attempts > 0)
                return GLib.SOURCE_CONTINUE;
            this._stepRequest = null;
            reply(handled);
            return GLib.SOURCE_REMOVE;
        });
        this._stepRequest = {id, reply};
    }

    disable() {
        if (this._revealSource)
            GLib.source_remove(this._revealSource);
        this._revealSource = 0;
        if (this._stepRequest) {
            GLib.source_remove(this._stepRequest.id);
            this._stepRequest.reply(false);
            this._stepRequest = null;
        }
        this._export?.unexport();
        this._export = null;
    }
}
