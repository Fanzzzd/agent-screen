#!/usr/bin/env python3
"""GTK4 Wayland fixture for Phase 0 compositor, input, clipboard, and AT-SPI2 spike."""

import json
import os
import sys
import tempfile

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, GLib, Gtk  # noqa: E402


class SpikeWindow(Gtk.ApplicationWindow):
    def __init__(self, app: Gtk.Application, state_path: str) -> None:
        super().__init__(application=app, title="Spike GTK4 Fixture")
        self.state_path = state_path
        self.set_default_size(640, 440)
        self.clicks = 0
        self.drag_end = [0.0, 0.0]

        root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        root.set_margin_top(16)
        root.set_margin_bottom(16)
        root.set_margin_start(16)
        root.set_margin_end(16)
        self.set_child(root)

        # Bright green target button at predictable top area for screencopy & click check.
        css = Gtk.CssProvider()
        css.load_from_string(
            """
            window { background-color: #1e222a; }
            .hit-btn { background-image: none; background-color: #22c55e; color: #000000; font-weight: bold; min-height: 64px; }
            .clicked-btn { background-image: none; background-color: #ef4444; color: #ffffff; font-weight: bold; min-height: 64px; }
            .drag-zone { background-color: #3b82f6; min-height: 96px; color: #ffffff; }
            """
        )
        Gtk.StyleContext.add_provider_for_display(
            Gdk.Display.get_default(), css, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
        )

        self.button = Gtk.Button(label="Click Counter: 0")
        self.button.add_css_class("hit-btn")
        self.button.connect("clicked", self.on_clicked)
        root.append(self.button)

        self.drag_label = Gtk.Label(label="Drag Zone (0, 0)")
        self.drag_label.add_css_class("drag-zone")
        drag = Gtk.GestureDrag.new()
        drag.connect("drag-update", self.on_drag_update)
        drag.connect("drag-end", self.on_drag_end)
        self.drag_label.add_controller(drag)
        root.append(self.drag_label)

        self.entry = Gtk.Entry()
        self.entry.set_placeholder_text("Normal Field")
        self.entry.update_property(
            [Gtk.AccessibleProperty.LABEL], ["Normal Field"]
        )
        self.entry.connect("changed", lambda *_: self.write_state())
        root.append(self.entry)

        self.password = Gtk.PasswordEntry()
        self.password.set_show_peek_icon(False)
        self.password.update_property(
            [Gtk.AccessibleProperty.LABEL], ["Secret Password"]
        )
        self.password.connect("changed", lambda *_: self.write_state())
        root.append(self.password)

        GLib.timeout_add(150, self.initial_dump)

    def initial_dump(self) -> bool:
        self.write_state()
        return False

    def on_clicked(self, _btn: Gtk.Button) -> None:
        self.clicks += 1
        self.button.set_label(f"Click Counter: {self.clicks}")
        self.button.remove_css_class("hit-btn")
        self.button.add_css_class("clicked-btn")
        self.write_state()

    def on_drag_update(self, _gesture: Gtk.GestureDrag, dx: float, dy: float) -> None:
        self.drag_end = [round(dx, 1), round(dy, 1)]
        self.drag_label.set_label(f"Drag Zone ({dx:.0f}, {dy:.0f})")

    def on_drag_end(self, _gesture: Gtk.GestureDrag, dx: float, dy: float) -> None:
        self.drag_end = [round(dx, 1), round(dy, 1)]
        self.drag_label.set_label(f"Drag Zone ({dx:.0f}, {dy:.0f})")
        self.write_state()

    def write_state(self) -> None:
        payload = {
            "pid": os.getpid(),
            "clicks": self.clicks,
            "drag_delta": self.drag_end,
            "entry_text": self.entry.get_text(),
            "password_len": len(self.password.get_text()),
            "gtk_use_portal": os.environ.get("GTK_USE_PORTAL"),
        }
        dir_name = os.path.dirname(self.state_path)
        fd, tmp = tempfile.mkstemp(dir=dir_name, prefix=".gtk4-state-")
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            json.dump(payload, f)
        os.replace(tmp, self.state_path)


def main() -> None:
    state_path = sys.argv[1]

    app = Gtk.Application(application_id="app.agentscreen.SpikeGtk4")
    app.connect("activate", lambda a: SpikeWindow(a, state_path).present())
    app.run([])


if __name__ == "__main__":
    main()
