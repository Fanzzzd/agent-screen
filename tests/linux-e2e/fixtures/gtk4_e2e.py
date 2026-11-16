#!/usr/bin/env python3
"""GTK4 fixture for Linux E2E tests (supports both GDK_BACKEND=wayland and GDK_BACKEND=x11)."""

import json
import os
import sys
import tempfile

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, GLib, Gtk  # noqa: E402


class E2EWindow(Gtk.ApplicationWindow):
    def __init__(self, app: Gtk.Application, state_path: str, title: str) -> None:
        super().__init__(application=app, title=title)
        self.state_path = state_path
        self.win_title = title
        self.set_default_size(640, 440)
        self.clicks = 0
        self.drag_delta = [0.0, 0.0]

        root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        root.set_margin_top(16)
        root.set_margin_bottom(16)
        root.set_margin_start(16)
        root.set_margin_end(16)
        self.set_child(root)

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

        self.button = Gtk.Button(label="Action Button: 0")
        self.button.add_css_class("hit-btn")
        self.button.update_property([Gtk.AccessibleProperty.LABEL], ["Action Button"])
        self.button.connect("clicked", self.on_clicked)
        root.append(self.button)

        self.drag_label = Gtk.Label(label="Drag Area")
        self.drag_label.add_css_class("drag-zone")
        drag = Gtk.GestureDrag.new()
        drag.connect("drag-update", self.on_drag)
        drag.connect("drag-end", self.on_drag_end)
        self.drag_label.add_controller(drag)
        root.append(self.drag_label)

        self.entry = Gtk.Entry()
        self.entry.set_placeholder_text("Text Input")
        self.entry.update_property([Gtk.AccessibleProperty.LABEL], ["Text Input"])
        self.entry.connect("changed", lambda *_: self.dump_state())
        root.append(self.entry)

        self.password = Gtk.PasswordEntry()
        self.password.set_show_peek_icon(False)
        self.password.update_property([Gtk.AccessibleProperty.LABEL], ["Password Input"])
        self.password.connect("changed", lambda *_: self.dump_state())
        root.append(self.password)

        self.async_btn = Gtk.Button(label="Spawn Async Status")
        self.async_btn.update_property([Gtk.AccessibleProperty.LABEL], ["Spawn Async Status"])
        self.async_btn.connect("clicked", self.on_spawn_async)
        root.append(self.async_btn)
        self.root_box = root
        self.banner_added = False

        init_clip = os.environ.get("E2E_INIT_CLIPBOARD")
        if init_clip:
            Gdk.Display.get_default().get_clipboard().set(init_clip)

        GLib.timeout_add(120, self.initial_dump)

    def on_spawn_async(self, _btn: Gtk.Button) -> None:
        def add_banner() -> bool:
            if not self.banner_added:
                self.banner_added = True
                lbl = Gtk.Label(label="Async Banner Ready")
                lbl.update_property([Gtk.AccessibleProperty.LABEL], ["Async Banner Ready"])
                self.root_box.append(lbl)
                self.dump_state()
            return False

        GLib.timeout_add(280, add_banner)

    def initial_dump(self) -> bool:
        self.dump_state()
        return False

    def on_clicked(self, _btn: Gtk.Button) -> None:
        self.clicks += 1
        self.button.set_label(f"Action Button: {self.clicks}")
        self.button.remove_css_class("hit-btn")
        self.button.add_css_class("clicked-btn")
        self.dump_state()

    def on_drag(self, _gesture: Gtk.GestureDrag, dx: float, dy: float) -> None:
        self.drag_delta = [round(dx, 1), round(dy, 1)]

    def on_drag_end(self, _gesture: Gtk.GestureDrag, dx: float, dy: float) -> None:
        self.drag_delta = [round(dx, 1), round(dy, 1)]
        self.drag_label.set_label(f"Dragged {dx:.0f},{dy:.0f}")
        self.dump_state()

    def dump_state(self) -> None:
        display_type = type(Gdk.Display.get_default()).__name__
        payload = {
            "pid": os.getpid(),
            "title": self.win_title,
            "backend": display_type,
            "clicks": self.clicks,
            "drag_delta": self.drag_delta,
            "entry_text": self.entry.get_text(),
            "password_len": len(self.password.get_text()),
        }
        parent = os.path.dirname(self.state_path)
        os.makedirs(parent, exist_ok=True)
        fd, tmp = tempfile.mkstemp(dir=parent, prefix=".e2e-gtk-")
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            json.dump(payload, f)
        os.replace(tmp, self.state_path)


def main() -> None:
    state_path = sys.argv[1]
    title = sys.argv[2] if len(sys.argv) > 2 else "GTK4 E2E Window"
    slug = "".join(c if c.isalnum() else "" for c in title) or "App"
    app = Gtk.Application(application_id=f"app.agentscreen.E2E{slug}{os.getpid()}")
    app.connect("activate", lambda a: E2EWindow(a, state_path, title).present())
    app.run([])


if __name__ == "__main__":
    main()
