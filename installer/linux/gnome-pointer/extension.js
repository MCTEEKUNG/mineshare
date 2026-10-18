import Gio from 'gi://Gio';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

const iface = `<node><interface name="org.gnome.Shell.Extensions.MineSharePointer">
<method name="GetPointer">
<arg type="i" direction="out" name="x"/><arg type="i" direction="out" name="y"/>
<arg type="i" direction="out" name="width"/><arg type="i" direction="out" name="height"/>
</method></interface></node>`;

export default class MineSharePointer extends Extension {
    enable() {
        this._export = Gio.DBusExportedObject.wrapJSObject(iface, {
            GetPointer() {
                // No input injection, clipboard/window data, eval or shell commands.
                if (Main.sessionMode.isLocked)
                    return [0, 0, 0, 0];
                const [x, y] = global.get_pointer();
                return [x, y, global.screen_width, global.screen_height];
            },
        });
        this._export.export(Gio.DBus.session, '/org/gnome/Shell/Extensions/MineSharePointer');
    }

    disable() {
        this._export?.unexport();
        this._export = null;
    }
}
