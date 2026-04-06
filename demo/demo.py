#!/usr/bin/env python3
"""Minimal EAF demo app — connects to the eafvil Wayland compositor."""
import sys

from PyQt6.QtWidgets import QApplication, QMainWindow, QLabel
from PyQt6.QtCore import Qt


def main() -> None:
    app = QApplication(sys.argv)
    app.setApplicationName("eaf-demo")

    window = QMainWindow()
    window.setWindowTitle("EAF Demo")

    window.setStyleSheet("background-color: black;")
    label = QLabel(
        "<h2 style='color:white'>EAF Demo</h2>"
        "<p style='color:white'>Running inside the eafvil compositor.</p>",
        window,
    )
    label.setAlignment(Qt.AlignmentFlag.AlignCenter)
    window.setCentralWidget(label)

    window.show()
    sys.exit(app.exec())


if __name__ == "__main__":
    main()
