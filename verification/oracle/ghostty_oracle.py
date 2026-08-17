#!/usr/bin/env python3
"""Dev-only normalized snapshot runner for the frozen libghostty-vt oracle."""

import argparse
import ctypes
import json
import os
from pathlib import Path

SOURCE = Path(os.environ.get("MR_CRABS_GHOSTTY_ORACLE", "/Users/jamie/Documents/Projects/active/mr-crabs"))
LIBRARY = SOURCE / "zig-out/lib/libghostty-vt.0.1.0.dylib"

Handle = ctypes.c_void_p


class RGB(ctypes.Structure):
    _fields_ = [("r", ctypes.c_uint8), ("g", ctypes.c_uint8), ("b", ctypes.c_uint8)]


class ColorValue(ctypes.Union):
    _fields_ = [("palette", ctypes.c_uint8), ("rgb", RGB), ("_padding", ctypes.c_uint64)]


class StyleColor(ctypes.Structure):
    _fields_ = [("tag", ctypes.c_int), ("value", ColorValue)]


class Style(ctypes.Structure):
    _fields_ = [
        ("size", ctypes.c_size_t),
        ("fg", StyleColor),
        ("bg", StyleColor),
        ("underline_color", StyleColor),
        ("bold", ctypes.c_bool),
        ("italic", ctypes.c_bool),
        ("faint", ctypes.c_bool),
        ("blink", ctypes.c_bool),
        ("inverse", ctypes.c_bool),
        ("invisible", ctypes.c_bool),
        ("strikethrough", ctypes.c_bool),
        ("overline", ctypes.c_bool),
        ("underline", ctypes.c_int),
    ]


class ModeConfig(ctypes.Structure):
    _fields_ = [("mode", ctypes.c_uint16), ("value", ctypes.c_bool)]


NAMED = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    "bright_black", "bright_red", "bright_green", "bright_yellow", "bright_blue",
    "bright_magenta", "bright_cyan", "bright_white",
]


def bind(library):
    library.ghostty_terminal_new.argtypes = [ctypes.c_void_p, ctypes.POINTER(Handle), ctypes.c_uint16, ctypes.c_uint16]
    library.ghostty_terminal_new.restype = ctypes.c_int
    library.ghostty_terminal_free.argtypes = [Handle]
    library.ghostty_terminal_vt_write.argtypes = [Handle, ctypes.POINTER(ctypes.c_uint8), ctypes.c_size_t]
    library.ghostty_terminal_get.argtypes = [Handle, ctypes.c_int, ctypes.c_void_p]
    library.ghostty_terminal_get.restype = ctypes.c_int
    library.ghostty_render_state_new.argtypes = [ctypes.c_void_p, ctypes.POINTER(Handle)]
    library.ghostty_render_state_new.restype = ctypes.c_int
    library.ghostty_render_state_free.argtypes = [Handle]
    library.ghostty_render_state_update.argtypes = [Handle, Handle]
    library.ghostty_render_state_update.restype = ctypes.c_int
    library.ghostty_render_state_get.argtypes = [Handle, ctypes.c_int, ctypes.c_void_p]
    library.ghostty_render_state_get.restype = ctypes.c_int
    library.ghostty_render_state_row_iterator_new.argtypes = [ctypes.c_void_p, ctypes.POINTER(Handle)]
    library.ghostty_render_state_row_iterator_new.restype = ctypes.c_int
    library.ghostty_render_state_row_iterator_free.argtypes = [Handle]
    library.ghostty_render_state_row_iterator_next.argtypes = [Handle]
    library.ghostty_render_state_row_iterator_next.restype = ctypes.c_bool
    library.ghostty_render_state_row_get.argtypes = [Handle, ctypes.c_int, ctypes.c_void_p]
    library.ghostty_render_state_row_get.restype = ctypes.c_int
    library.ghostty_render_state_row_cells_new.argtypes = [ctypes.c_void_p, ctypes.POINTER(Handle)]
    library.ghostty_render_state_row_cells_new.restype = ctypes.c_int
    library.ghostty_render_state_row_cells_free.argtypes = [Handle]
    library.ghostty_render_state_row_cells_next.argtypes = [Handle]
    library.ghostty_render_state_row_cells_next.restype = ctypes.c_bool
    library.ghostty_render_state_row_cells_get.argtypes = [Handle, ctypes.c_int, ctypes.c_void_p]
    library.ghostty_render_state_row_cells_get.restype = ctypes.c_int
    library.ghostty_cell_get.argtypes = [ctypes.c_uint64, ctypes.c_int, ctypes.c_void_p]
    library.ghostty_cell_get.restype = ctypes.c_int
    library.ghostty_row_get.argtypes = [ctypes.c_uint64, ctypes.c_int, ctypes.c_void_p]
    library.ghostty_row_get.restype = ctypes.c_int


def require(result, operation):
    if result != 0:
        raise RuntimeError(f"{operation} failed with GhosttyResult {result}")


def color(value, default):
    if value.tag == 0:
        return {"kind": "named", "value": default}
    if value.tag == 1:
        index = int(value.value.palette)
        if index < len(NAMED):
            return {"kind": "named", "value": NAMED[index]}
        return {"kind": "indexed", "value": index}
    if value.tag == 2:
        rgb = value.value.rgb
        return {"kind": "rgb", "value": [rgb.r, rgb.g, rgb.b]}
    raise RuntimeError(f"unknown Ghostty style color tag {value.tag}")


def flags(style, wide):
    result = 0
    if style.inverse: result |= 0x0001
    if style.bold: result |= 0x0002
    if style.italic: result |= 0x0004
    result |= {0: 0, 1: 0x0008, 2: 0x0800, 3: 0x1000, 4: 0x2000, 5: 0x4000}[style.underline]
    if wide == 1: result |= 0x0020
    elif wide == 2: result |= 0x0040
    elif wide == 3: result |= 0x0400
    if style.faint: result |= 0x0080
    if style.invisible: result |= 0x0100
    if style.strikethrough: result |= 0x0200
    return result


def packed_mode(value, ansi=False):
    return value | (0x8000 if ansi else 0)


def mode(library, terminal, value, ansi=False):
    config = ModeConfig(packed_mode(value, ansi), False)
    require(library.ghostty_terminal_get(terminal, 37, ctypes.byref(config)), f"query mode {value}")
    return bool(config.value)


def snapshot(cols, rows, payload):
    library = ctypes.CDLL(str(LIBRARY))
    bind(library)
    terminal = Handle()
    render = Handle()
    rows_iterator = Handle()
    row_cells = Handle()
    require(library.ghostty_terminal_new(None, ctypes.byref(terminal), cols, rows), "terminal_new")
    try:
        if payload:
            source = (ctypes.c_uint8 * len(payload)).from_buffer_copy(payload)
            library.ghostty_terminal_vt_write(terminal, source, len(payload))
        cursor_x, cursor_y = ctypes.c_uint16(), ctypes.c_uint16()
        pending_wrap = ctypes.c_bool()
        require(library.ghostty_terminal_get(terminal, 3, ctypes.byref(cursor_x)), "cursor_x")
        require(library.ghostty_terminal_get(terminal, 4, ctypes.byref(cursor_y)), "cursor_y")
        require(library.ghostty_terminal_get(terminal, 5, ctypes.byref(pending_wrap)), "pending_wrap")
        require(library.ghostty_render_state_new(None, ctypes.byref(render)), "render_state_new")
        require(library.ghostty_render_state_update(render, terminal), "render_state_update")
        require(library.ghostty_render_state_row_iterator_new(None, ctypes.byref(rows_iterator)), "row_iterator_new")
        require(library.ghostty_render_state_row_cells_new(None, ctypes.byref(row_cells)), "row_cells_new")
        require(library.ghostty_render_state_get(render, 4, ctypes.byref(rows_iterator)), "row_iterator")
        cells, styles, combining = [], [{"foreground": {"kind": "named", "value": "foreground"}, "background": {"kind": "named", "value": "background"}, "underline": None}], []
        style_keys = {(json.dumps(styles[0], sort_keys=True), 0): 0}
        cell_index = 0
        while library.ghostty_render_state_row_iterator_next(rows_iterator):
            raw_row, wrapped = ctypes.c_uint64(), ctypes.c_bool()
            require(library.ghostty_render_state_row_get(rows_iterator, 2, ctypes.byref(raw_row)), "raw row")
            require(library.ghostty_row_get(raw_row.value, 1, ctypes.byref(wrapped)), "row wrap")
            require(library.ghostty_render_state_row_get(rows_iterator, 3, ctypes.byref(row_cells)), "row cells")
            while library.ghostty_render_state_row_cells_next(row_cells):
                raw, codepoint, wide = ctypes.c_uint64(), ctypes.c_uint32(), ctypes.c_int()
                style = Style(); style.size = ctypes.sizeof(Style)
                require(library.ghostty_render_state_row_cells_get(row_cells, 1, ctypes.byref(raw)), "raw cell")
                require(library.ghostty_render_state_row_cells_get(row_cells, 2, ctypes.byref(style)), "cell style")
                require(library.ghostty_cell_get(raw.value, 1, ctypes.byref(codepoint)), "cell codepoint")
                require(library.ghostty_cell_get(raw.value, 3, ctypes.byref(wide)), "cell width")
                normalized_style = {"foreground": color(style.fg, "foreground"), "background": color(style.bg, "background"), "underline": None if style.underline_color.tag == 0 else color(style.underline_color, "foreground")}
                style_key = (json.dumps(normalized_style, sort_keys=True), flags(style, 0) & ~(0x0020 | 0x0040 | 0x0400))
                if style_key not in style_keys:
                    style_keys[style_key] = len(styles); styles.append(normalized_style)
                grapheme_len = ctypes.c_uint32()
                require(library.ghostty_render_state_row_cells_get(row_cells, 3, ctypes.byref(grapheme_len)), "grapheme length")
                if grapheme_len.value > 1:
                    graphemes = (ctypes.c_uint32 * grapheme_len.value)()
                    require(library.ghostty_render_state_row_cells_get(row_cells, 4, graphemes), "graphemes")
                    combining.append({"cell_index": cell_index, "codepoints": list(graphemes)[1:]})
                cells.append({"content": codepoint.value or 32, "style": style_keys[style_key], "flags": flags(style, wide.value)})
                cell_index += 1
            if wrapped.value:
                cells[-1]["flags"] |= 0x0010
        modes = []
        for active, name in [
            (mode(library, terminal, 25), "show_cursor"), (mode(library, terminal, 1), "app_cursor"),
            (mode(library, terminal, 66), "app_keypad"), (mode(library, terminal, 1000), "mouse_report_click"),
            (mode(library, terminal, 2004), "bracketed_paste"), (mode(library, terminal, 1006), "sgr_mouse"),
            (mode(library, terminal, 1003), "mouse_motion"), (mode(library, terminal, 7), "line_wrap"),
            (mode(library, terminal, 20, True), "line_feed_new_line"), (mode(library, terminal, 6), "origin"),
            (mode(library, terminal, 4, True), "insert"), (mode(library, terminal, 1004), "focus_in_out"),
            (mode(library, terminal, 1049), "alt_screen"), (mode(library, terminal, 1002), "mouse_drag"),
            (mode(library, terminal, 1005), "utf8_mouse"),
        ]:
            if active: modes.append(name)
        # Alacritty exposes these application defaults as mode bits; include
        # them in the shared normalized schema rather than pretending Ghostty
        # has matching DEC mode numbers.
        modes.extend(["alternate_scroll", "urgency_hints"])
        return {"size": {"cols": cols, "rows": rows}, "cursor": {"row": cursor_y.value, "col": cursor_x.value, "wrap_pending": bool(pending_wrap.value)}, "cells": cells, "styles": styles, "combining_marks": combining, "modes": modes}
    finally:
        if row_cells: library.ghostty_render_state_row_cells_free(row_cells)
        if rows_iterator: library.ghostty_render_state_row_iterator_free(rows_iterator)
        if render: library.ghostty_render_state_free(render)
        if terminal: library.ghostty_terminal_free(terminal)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=["snapshot"])
    parser.add_argument("--cols", type=int, required=True)
    parser.add_argument("--rows", type=int, required=True)
    parser.add_argument("--input-hex", required=True)
    args = parser.parse_args()
    print(json.dumps(snapshot(args.cols, args.rows, bytes.fromhex(args.input_hex)), separators=(",", ":")))


if __name__ == "__main__":
    main()
