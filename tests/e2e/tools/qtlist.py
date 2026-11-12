"""A Qt search box whose results open in a Qt pop-up over a list, as
WeCom's search does. Qt's own pop-up code decides when the results close,
so a test sees what a Qt app does, not an imitation of it.

The window title says the state: "qtlist open=1|0 got=<what was picked>".
Typing "item" opens the results item1 to item4 over rows 1 to 4, in the
window QTLIST_POPUP names: "completer" (Qt's pop-up, the default), "keyed"
or "frameless". A pick
in the results sets got=itemN; a press on a row sets got=rowN.
QTLIST_STATE names a file the app rewrites on each change with the window,
pop-up and item frames in global top-left points, as JSON.
QTLIST_LOG names a file each press, open and close is appended to.

    python qtlist.py
"""
import json
import os
import sys
import time

from PySide6.QtCore import QEvent, QObject, QStringListModel, Qt
from PySide6.QtWidgets import (
    QApplication,
    QCompleter,
    QLineEdit,
    QListWidget,
    QListWidgetItem,
    QMainWindow,
    QVBoxLayout,
    QWidget,
)

ROW = 50
ITEMS = ["item1", "item2", "item3", "item4"]


def log(line):
    path = os.environ.get("QTLIST_LOG")
    if path:
        with open(path, "a") as f:
            f.write(f"{time.time():.3f} {line}\n")


class Window(QMainWindow):
    def __init__(self):
        super().__init__()
        self.got = "none"
        self.picked = False
        central = QWidget()
        layout = QVBoxLayout(central)
        layout.setContentsMargins(20, 20, 20, 20)
        self.search = QLineEdit()
        self.search.setPlaceholderText("search")
        self.search.setFixedHeight(ROW)
        self.search.textEdited.connect(lambda text: log(f"text {text!r}"))
        layout.addWidget(self.search)
        self.rows = QListWidget()
        for i in range(8):
            self.rows.addItem(QListWidgetItem(f"row{i}"))
        self.rows.setStyleSheet(f"QListWidget::item {{ height: {ROW}px; }}")
        self.rows.itemPressed.connect(self.row_pressed)
        layout.addWidget(self.rows)
        self.setCentralWidget(central)
        # QTLIST_POPUP picks the kind of window the results open in. WeCom's
        # is a window of its own, not Qt's pop-up menu.
        self.mode = os.environ.get("QTLIST_POPUP", "completer")
        if self.mode == "completer":
            # Parented, or Python frees the model and the results stay empty.
            self.model = QStringListModel(ITEMS, self)
            self.completer = QCompleter(self.model, self)
            self.completer.setCompletionMode(QCompleter.PopupCompletion)
            self.completer.setCaseSensitivity(Qt.CaseInsensitive)
            self.completer.setMaxVisibleItems(len(ITEMS))
            self.results = self.completer.popup()
            self.completer.activated.connect(self.item_picked)
            self.search.setCompleter(self.completer)
        else:
            # "keyed": a borderless window of its own that takes the keys
            # once they go to it and closes when it loses key, as WeCom's
            # search results do. "frameless": one that never takes the keys
            # and closes when its app or main window loses active.
            flags = Qt.Window | Qt.FramelessWindowHint
            self.results = QListWidget()
            if self.mode == "frameless":
                flags |= Qt.WindowDoesNotAcceptFocus
                self.results.setAttribute(Qt.WA_ShowWithoutActivating)
                self.results.setFocusPolicy(Qt.NoFocus)
            self.results.setWindowFlags(flags)
            self.keyed = False
            for item in ITEMS:
                self.results.addItem(QListWidgetItem(item))
            self.results.itemClicked.connect(
                lambda item: (self.item_picked(item.text()), self.results.hide())
            )
            self.search.textEdited.connect(self.show_results)
            QApplication.instance().applicationStateChanged.connect(
                lambda state: state != Qt.ApplicationActive
                and self.mode == "frameless"
                and self.results.hide()
            )
        self.results.setStyleSheet(f"QListView::item {{ height: {ROW}px; }}")
        self.results.installEventFilter(self)
        self.resize(600, 560)
        self.update_title()

    # The frames change as Agent Screen places the window.
    def moveEvent(self, event):
        super().moveEvent(event)
        # A list in a window of its own goes with the search box.
        if self.mode != "completer" and self.results.isVisible():
            self.place_results()
        self.update_title()

    def resizeEvent(self, event):
        super().resizeEvent(event)
        self.update_title()

    def showEvent(self, event):
        super().showEvent(event)
        from PySide6.QtCore import QTimer
        QTimer.singleShot(100, self.update_title)

    def show_results(self, text):
        if not text or not any(i.startswith(text.lower()) for i in ITEMS):
            self.results.hide()
            return
        self.place_results()
        self.results.show()

    def place_results(self):
        under = self.search.mapToGlobal(self.search.rect().bottomLeft())
        self.results.setGeometry(
            under.x(), under.y() + 1, self.search.width(), ROW * len(ITEMS) + 6
        )

    def changeEvent(self, event):
        super().changeEvent(event)
        if (
            event.type() == QEvent.ActivationChange
            and self.mode == "frameless"
            and not self.isActiveWindow()
            and QApplication.activeWindow() is not self.results
        ):
            log("main window not active")
            self.results.hide()

    def row_pressed(self, item):
        log(f"press row {item.text()}")
        self.got = item.text()
        self.update_title()
        self.search.clear()

    def item_picked(self, text):
        log(f"pick {text}")
        self.got = text
        self.picked = True
        self.update_title()
        # A new search starts empty, as after a pick in WeCom.
        from PySide6.QtCore import QTimer
        QTimer.singleShot(0, self.search.clear)

    def eventFilter(self, obj, event):
        kind = event.type()
        if kind == QEvent.Show:
            self.picked = False
            log("open")
            # The frames once the pop-up has its place.
            from PySide6.QtCore import QTimer
            QTimer.singleShot(50, self.update_title)
        elif kind == QEvent.Hide:
            log("close" + (" after a pick" if self.picked else ""))
            self.update_title()
        elif kind == QEvent.WindowActivate and obj is self.results:
            self.keyed = True
            log("list key")
        elif kind == QEvent.WindowDeactivate and obj is self.results and self.keyed:
            self.keyed = False
            if self.mode == "keyed" and self.results.isVisible():
                log("list lost key")
                self.results.hide()
        elif kind == QEvent.KeyPress and obj is self.results:
            # The keys go on into the search box.
            QApplication.sendEvent(self.search, event)
            return True
        elif kind == QEvent.MouseButtonPress:
            log(f"press list at {event.position().y():.0f}")
        return False

    def update_title(self):
        popup = self.results
        is_open = popup.isVisible()
        self.setWindowTitle(f"qtlist open={int(is_open)} got={self.got}")
        path = os.environ.get("QTLIST_STATE")
        if not path:
            return

        def frame(widget):
            g = widget.mapToGlobal(widget.rect().topLeft())
            return [g.x(), g.y(), widget.width(), widget.height()]

        items = []
        if is_open:
            for i in range(popup.model().rowCount()):
                r = popup.visualRect(popup.model().index(i, 0))
                g = popup.viewport().mapToGlobal(r.topLeft())
                items.append([g.x(), g.y(), r.width(), r.height()])
        rows = []
        for i in range(self.rows.count()):
            r = self.rows.visualItemRect(self.rows.item(i))
            g = self.rows.viewport().mapToGlobal(r.topLeft())
            rows.append([g.x(), g.y(), r.width(), r.height()])
        state = {
            "open": is_open,
            "got": self.got,
            "window": [
                self.frameGeometry().x(),
                self.frameGeometry().y(),
                self.frameGeometry().width(),
                self.frameGeometry().height(),
            ],
            "search": frame(self.search),
            "popup": frame(popup) if is_open else None,
            "items": items,
            "rows": rows,
        }
        tmp = path + ".tmp"
        with open(tmp, "w") as f:
            json.dump(state, f)
        os.replace(tmp, path)


app = QApplication(sys.argv)
app.focusChanged.connect(
    lambda old, new: log(f"focus {type(new).__name__ if new else None}")
)
app.applicationStateChanged.connect(lambda state: log(f"state {state.name}"))
window = Window()
window.show()
window.raise_()
sys.exit(app.exec())
