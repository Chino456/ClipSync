import St from 'gi://St';
import Meta from 'gi://Meta';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const DAEMON_BUS_NAME = 'org.clipsync.Daemon';
const DAEMON_OBJECT_PATH = '/org/clipsync/Daemon';
const MAX_TEXT_BYTES = 1024 * 1024;
const MAX_IMAGE_BYTES = 10 * 1024 * 1024;
const PNG_SIGNATURE = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

export default class ClipSyncExtension extends Extension {
    enable() {
        this._selection = global.display.get_selection();
        this._clipboard = St.Clipboard.get_default();
        this._readTimeout = null;
        this._ownerChangedId = this._selection.connect(
            'owner-changed', this._onOwnerChanged.bind(this));
    }

    disable() {
        if (this._readTimeout) {
            GLib.source_remove(this._readTimeout);
            this._readTimeout = null;
        }
        if (this._ownerChangedId) {
            this._selection.disconnect(this._ownerChangedId);
            this._ownerChangedId = null;
        }
        this._selection = null;
        this._clipboard = null;
    }

    _onOwnerChanged(_selection, selectionType) {
        if (selectionType !== Meta.SelectionType.SELECTION_CLIPBOARD)
            return;
        if (this._readTimeout)
            GLib.source_remove(this._readTimeout);

        // Give the new owner a moment to publish its offered data.
        this._readTimeout = GLib.timeout_add(
            GLib.PRIORITY_DEFAULT, 120, () => {
                this._readTimeout = null;
                this._clipboard.get_text(St.ClipboardType.CLIPBOARD,
                    (_clipboard, text) => this._sendText(text));
                this._clipboard.get_content(St.ClipboardType.CLIPBOARD, 'image/png',
                    (_clipboard, bytes) => this._sendImage(bytes));
                return GLib.SOURCE_REMOVE;
            });
    }

    _sendText(text) {
        if (!text || new TextEncoder().encode(text).length > MAX_TEXT_BYTES)
            return;
        Gio.DBus.session.call(
            DAEMON_BUS_NAME,
            DAEMON_OBJECT_PATH,
            DAEMON_BUS_NAME,
            'ClipboardText',
            new GLib.Variant('(s)', [text]),
            null,
            Gio.DBusCallFlags.NO_AUTO_START,
            1000,
            null,
            (connection, result) => {
                try {
                    connection.call_finish(result);
                } catch (error) {
                    // ClipSync is not running or is shutting down.
                }
            });
    }

    _sendImage(bytes) {
        if (!bytes)
            return;
        const data = bytes.get_data();
        if (!data || data.length === 0 || data.length > MAX_IMAGE_BYTES)
            return;
        if (!PNG_SIGNATURE.every((value, index) => data[index] === value))
            return;
        Gio.DBus.session.call(
            DAEMON_BUS_NAME,
            DAEMON_OBJECT_PATH,
            DAEMON_BUS_NAME,
            'ClipboardImage',
            new GLib.Variant('(ay)', [data]),
            null,
            Gio.DBusCallFlags.NO_AUTO_START,
            3000,
            null,
            (connection, result) => {
                try {
                    connection.call_finish(result);
                } catch (error) {
                    // ClipSync is not running or is shutting down.
                }
            });
    }
}
