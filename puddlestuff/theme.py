"""PuddleMore high-contrast System/Light/Dark themes."""
from PyQt6.QtCore import Qt
from PyQt6.QtGui import QColor, QIcon, QImage, QPainter, QPalette, QPixmap
from PyQt6.QtWidgets import QApplication, QStyleFactory

SYSTEM = "system"
LIGHT = "light"
DARK = "dark"
THEMES = [("System", SYSTEM), ("Light", LIGHT), ("Dark", DARK)]

_ACTIVE_DARK = False
_STYLE_SOURCES = {}


LIGHT_COLORS = {
    "Window": "#f2f4f7",
    "WindowText": "#171a1e",
    "Base": "#ffffff",
    "AlternateBase": "#eef1f5",
    "ToolTipBase": "#ffffe1",
    "ToolTipText": "#171a1e",
    "Text": "#171a1e",
    "Button": "#e6e9ee",
    "ButtonText": "#171a1e",
    "BrightText": "#ffffff",
    "Light": "#ffffff",
    "Midlight": "#d8dde4",
    "Mid": "#c2c9d1",
    "Dark": "#9aa3ad",
    "Shadow": "#6f7882",
    "Highlight": "#2f7ec7",
    "HighlightedText": "#ffffff",
    "Link": "#155d9e",
    "LinkVisited": "#6a3d9a",
    "PlaceholderText": "#66707a",
}

DARK_COLORS = {
    "Window": "#20242a",
    "WindowText": "#eef2f6",
    "Base": "#15191e",
    "AlternateBase": "#1d2228",
    "ToolTipBase": "#2b3139",
    "ToolTipText": "#eef2f6",
    "Text": "#eef2f6",
    "Button": "#2a3038",
    "ButtonText": "#eef2f6",
    "BrightText": "#ffffff",
    "Light": "#5a6572",
    "Midlight": "#333b44",
    "Mid": "#3c454f",
    "Dark": "#11151a",
    "Shadow": "#07090b",
    "Highlight": "#2f81c7",
    "HighlightedText": "#ffffff",
    "Link": "#74b9ff",
    "LinkVisited": "#c39ae8",
    "PlaceholderText": "#9ba6b2",
}


def _config():
    from .puddleobjects import PuddleConfig
    return PuddleConfig()


def load_theme():
    try:
        value = _config().get("appearance", "theme", SYSTEM)
    except Exception:
        return SYSTEM
    value = str(value).lower()
    return value if value in (SYSTEM, LIGHT, DARK) else SYSTEM


def save_theme(value):
    value = str(value).lower()
    if value not in (SYSTEM, LIGHT, DARK):
        value = SYSTEM
    try:
        _config().set("appearance", "theme", value)
    except Exception:
        pass


def system_is_dark(app=None):
    app = app or QApplication.instance()
    if app is None:
        return False
    try:
        scheme = app.styleHints().colorScheme()
        if scheme == Qt.ColorScheme.Dark:
            return True
        if scheme == Qt.ColorScheme.Light:
            return False
    except Exception:
        pass
    return app.palette().color(QPalette.ColorRole.Window).lightness() < 128


def resolved_theme(theme=None, app=None):
    theme = (theme or load_theme()).lower()
    if theme == SYSTEM:
        return DARK if system_is_dark(app) else LIGHT
    return theme if theme in (LIGHT, DARK) else LIGHT


def _palette(colors):
    palette = QPalette()
    for name, value in colors.items():
        role = getattr(QPalette.ColorRole, name, None)
        if role is not None:
            palette.setColor(role, QColor(value))
    disabled = QColor(colors["PlaceholderText"])
    for role in (QPalette.ColorRole.WindowText, QPalette.ColorRole.Text,
                 QPalette.ColorRole.ButtonText, QPalette.ColorRole.PlaceholderText):
        palette.setColor(QPalette.ColorGroup.Disabled, role, disabled)
    palette.setColor(QPalette.ColorGroup.Disabled, QPalette.ColorRole.Base,
                     QColor(colors["Base"]).darker(105))
    palette.setColor(QPalette.ColorGroup.Disabled, QPalette.ColorRole.Button,
                     QColor(colors["Button"]).darker(105))
    palette.setColor(QPalette.ColorGroup.Disabled, QPalette.ColorRole.Highlight,
                     QColor(colors["Mid"]))
    palette.setColor(QPalette.ColorGroup.Disabled, QPalette.ColorRole.HighlightedText,
                     QColor(colors["Text"]))
    return palette


def palette_for(theme=None, app=None):
    return _palette(DARK_COLORS if resolved_theme(theme, app) == DARK else LIGHT_COLORS)


def selection_color(app=None):
    app = app or QApplication.instance()
    if app is None:
        return QColor(LIGHT_COLORS["Highlight"])
    return app.palette().color(QPalette.ColorRole.Highlight)


def selection_text_color(app=None):
    app = app or QApplication.instance()
    if app is None:
        return QColor(LIGHT_COLORS["HighlightedText"])
    return app.palette().color(QPalette.ColorRole.HighlightedText)


def style_icon(icon, dark=None):
    if icon is None or icon.isNull():
        return icon
    if dark is None:
        dark = _ACTIVE_DARK
    sizes = icon.availableSizes()
    size = max((s.width() for s in sizes), default=64)
    size = max(24, min(128, size))
    pixmap = icon.pixmap(size, size)
    if pixmap.isNull():
        return icon
    image = pixmap.toImage().convertToFormat(QImage.Format.Format_ARGB32)
    for y in range(image.height()):
        for x in range(image.width()):
            pixel = image.pixelColor(x, y)
            alpha = pixel.alpha()
            if alpha == 0:
                continue
            r, g, b = pixel.red(), pixel.green(), pixel.blue()
            saturation = max(r, g, b) - min(r, g, b)
            luminance = (299 * r + 587 * g + 114 * b) // 1000
            if saturation <= 28:
                if dark:
                    value = 60 + int((255 - luminance) / 255.0 * 160)
                else:
                    value = int(luminance * 0.28) + 8
                pixel.setRgb(max(0, min(255, value)),
                             max(0, min(255, value)),
                             max(0, min(255, value)), alpha)
            elif dark and luminance < 70:
                pixel.setRgb(int(r + (255 - r) * 0.55),
                             int(g + (255 - g) * 0.55),
                             int(b + (255 - b) * 0.55), alpha)
            elif not dark and luminance > 205:
                pixel.setRgb(int(r * 0.55), int(g * 0.55), int(b * 0.55), alpha)
            image.setPixelColor(x, y, pixel)
    styled = QIcon(QPixmap.fromImage(image))
    _STYLE_SOURCES[styled.cacheKey()] = icon
    return styled


def refresh_icons(app=None):
    app = app or QApplication.instance()
    if app is None:
        return
    dark = _ACTIVE_DARK
    seen = set()
    for widget in app.allWidgets():
        actions = getattr(widget, "actions", None)
        if not callable(actions):
            continue
        for action in actions():
            if id(action) in seen:
                continue
            seen.add(id(action))
            icon = action.icon()
            if icon.isNull():
                continue
            source = _STYLE_SOURCES.get(icon.cacheKey(), icon)
            action.setIcon(style_icon(source, dark))


def apply_theme(app=None, theme=None):
    global _ACTIVE_DARK
    app = app or QApplication.instance()
    if app is None:
        return LIGHT
    resolved = resolved_theme(theme, app)
    _ACTIVE_DARK = resolved == DARK
    try:
        if app.style().objectName().lower() != "fusion":
            app.setStyle(QStyleFactory.create("Fusion"))
    except Exception:
        pass
    app.setPalette(_palette(DARK_COLORS if _ACTIVE_DARK else LIGHT_COLORS))
    app.setProperty("puddlemore_theme", resolved)
    if _ACTIVE_DARK:
        app.setStyleSheet(
            "QToolTip{background-color:#2b3139;color:#eef2f6;border:1px solid #4a545f;padding:3px;}"
            "QMenuBar,QMenu,QToolBar,QStatusBar,QDockWidget{color:#eef2f6;}"
        )
    else:
        app.setStyleSheet(
            "QToolTip{background-color:#ffffe1;color:#171a1e;border:1px solid #9aa3ad;padding:3px;}"
            "QMenuBar,QMenu,QToolBar,QStatusBar,QDockWidget{color:#171a1e;}"
        )
    refresh_icons(app)
    return resolved
