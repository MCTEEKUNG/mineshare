import Gio from 'gi://Gio';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

const iface = `<node><interface name="org.gnome.Shell.Extensions.MineSharePointer">
<method name="GetPointer">
<arg type="i" direction="out" name="x"/><arg type="i" direction="out" name="y"/>
<arg type="i" direction="out" name="width"/><arg type="i" direction="out" name="height"/>
</method>
<method name="GetPasteWindow"><arg type="t" direction="out" name="id"/></method>
</interface></node>`;

export default class MineSharePointer extends Extension {
    enable() {
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
        });
        this._export.export(Gio.DBus.session, '/org/gnome/Shell/Extensions/MineSharePointer');
    }

    disable() {
        this._export?.unexport();
        this._export = null;
    }
}
