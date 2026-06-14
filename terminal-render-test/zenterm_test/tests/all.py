"""Test registration — import all modules to populate the test registry."""

from . import sgr
from . import colors
from . import cursor
from . import unicode
from . import osc
from . import modes
from . import queries
from . import stress
from . import graphics
from . import keyboard


def register_all():
    """Register all test cases from all modules."""
    sgr.register_sgr_tests()
    colors.register_color_tests()
    cursor.register_cursor_tests()
    unicode.register_unicode_tests()
    osc.register_osc_tests()
    modes.register_modes_tests()
    queries.register_queries_tests()
    stress.register_stress_tests()
    graphics.register_graphics_tests()
    keyboard.register_keyboard_tests()
