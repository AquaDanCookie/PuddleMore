#!/usr/bin/env python3
"""PuddleMore entry point.

Runs the puddletag PyQt6 interface with PuddleMore's extra format support.
"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
if HERE not in sys.path:
    sys.path.insert(0, HERE)

import puddlestuff.resource  # noqa: F401,E402
import puddlestuff.constants  # noqa: F401,E402


def main():
    args = sys.argv[1:]
    if '-v' in args or '--version' in args:
        from puddlestuff import version_string, changeset
        print('PuddleMore Version: %s%s' % (
            version_string,
            ', Changeset: %s' % changeset if changeset else ''))
        return
    if '-h' in args or '--help' in args:
        print('Usage: PuddleMore [options] [files or folders]\n'
              '  -v, --version   show version\n'
              '  -h, --help      show this help')
        return

    from PyQt6.QtWidgets import QApplication

    from puddlestuff.constants import CONFIGDIR, SAVEDIR
    for directory in (CONFIGDIR, SAVEDIR):
        os.makedirs(directory, exist_ok=True)

    app = QApplication(sys.argv)
    app.setOrganizationName("Puddle Inc.")
    app.setApplicationName("PuddleMore")
    app.setDesktopFileName("PuddleMore")
    from PyQt6.QtGui import QIcon
    icon_path = os.path.join(HERE, "PuddleMore.png")
    if os.path.exists(icon_path):
        app.setWindowIcon(QIcon(icon_path))

    try:
        from puddlestuff import theme
        theme.apply_theme(app)
    except Exception:
        pass

    from puddlestuff.puddletag import MainWin
    from puddlestuff.puddlesettings import load_gen_settings

    win = MainWin()
    if os.path.exists(icon_path):
        from PyQt6.QtGui import QIcon
        win.setWindowIcon(QIcon(icon_path))
    win.setVisible(True)
    app.processEvents()

    paths = [path for path in args if os.path.exists(path)]
    if paths:
        win.openFiles(paths, False)
    elif load_gen_settings([("&Load last folder at startup", False)])[0][1]:
        if win._lastdir and os.path.exists(win._lastdir[0]):
            win.openDir(win._lastdir[0], False)

    sys.exit(app.exec())


if __name__ == "__main__":
    main()
