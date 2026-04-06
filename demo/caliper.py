#!/usr/bin/env python3
"""EAF caliper — draws bright borders and corner markers to debug geometry alignment."""
import sys

from PyQt6.QtCore import Qt
from PyQt6.QtGui import QColor, QFont, QPainter, QPen
from PyQt6.QtWidgets import QApplication, QMainWindow, QWidget


BORDER = 4  # px thickness of edge rulers


class CaliperWidget(QWidget):
    """Draws coloured edge rulers and corner labels."""

    def paintEvent(self, _event):
        w, h = self.width(), self.height()
        p = QPainter(self)
        p.setRenderHint(QPainter.RenderHint.Antialiasing)

        # Background
        p.fillRect(0, 0, w, h, QColor(30, 30, 30))

        # Edge rulers (red=top, green=bottom, blue=left, yellow=right)
        p.fillRect(0, 0, w, BORDER, QColor(255, 60, 60))       # top
        p.fillRect(0, h - BORDER, w, BORDER, QColor(60, 220, 60))  # bottom
        p.fillRect(0, 0, BORDER, h, QColor(60, 120, 255))      # left
        p.fillRect(w - BORDER, 0, BORDER, h, QColor(255, 220, 40))  # right

        # Labels
        p.setPen(QPen(QColor(255, 255, 255)))
        font = QFont("monospace", 11)
        p.setFont(font)

        margin = BORDER + 6
        p.drawText(margin, margin + 14, f"top-left (0, 0)")
        p.drawText(w - 180, margin + 14, f"top-right ({w}, 0)")
        p.drawText(margin, h - margin, f"bottom-left (0, {h})")
        p.drawText(w - 200, h - margin, f"bottom-right ({w}, {h})")

        # Center crosshair and size
        cx, cy = w // 2, h // 2
        p.setPen(QPen(QColor(255, 255, 255, 80), 1, Qt.PenStyle.DashLine))
        p.drawLine(cx, 0, cx, h)
        p.drawLine(0, cy, w, cy)

        p.setPen(QPen(QColor(255, 255, 255)))
        font.setPointSize(14)
        p.setFont(font)
        p.drawText(cx - 60, cy - 10, f"{w} x {h} px")

        p.end()


def main() -> None:
    app = QApplication(sys.argv)
    app.setApplicationName("eaf-caliper")

    window = QMainWindow()
    window.setWindowTitle("EAF Caliper")
    window.setCentralWidget(CaliperWidget())
    window.show()
    sys.exit(app.exec())


if __name__ == "__main__":
    main()
