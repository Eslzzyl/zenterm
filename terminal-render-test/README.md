# Terminal Render Test Suite

A cross-platform Python tool for testing terminal emulator rendering capabilities.
Designed to verify **zenterm** during development, but works with **any** terminal
emulator (kitty, WezTerm, Alacritty, iTerm2, Windows Terminal, Ghostty, etc.).

## Quick Start

```bash
# Run with visual verification (recommended)
python3 zenterm-test.py

# Quick automated checks
python3 zenterm-test.py --quick

# CI mode: automatic, JSON report only
python3 zenterm-test.py --auto --json report.json --quiet

# Run specific categories
python3 zenterm-test.py --include sgr,colors

# List all tests
python3 zenterm-test.py --list-tests
```

## Test Coverage

| Category | Tests | What's Tested |
|----------|-------|---------------|
| `sgr` | 4 | Bold, dim, italic, underline, blink, reverse, conceal, strikethrough, double-underline, overline, underline colour (SGR 58), underline styles (4:1–4:5) |
| `colors` | 5 | 16 standard colours, 256-colour palette, truecolor gradients, colour resets (39/49), OSC colour queries |
| `cursor` | 4 | Cursor shapes (DECSCUSR 0–6), visibility (DECTCEM 25), movement (CUU/CUD/CUF/CUB/CHA/CUP/HVP), save/restore (DECSC/DECRC), erase operations (ED/EL/ECH) |
| `unicode` | 6 | UTF-8 encoding, CJK character width, combining characters, Devanagari/Thai/Arabic shaping, bidirectional text, emoji (ZWJ, skin tones, flags), programming ligatures, Powerline & Nerd Font glyphs |
| `osc` | 6 | Window title (0/2), hyperlinks (OSC 8), dynamic colours (4/10/11/12), clipboard (OSC 52), notifications (OSC 9/777), semantic prompts (OSC 133) |
| `modes` | 8 | Alternate screen buffer (1049), bracketed paste (2004), focus events (1004), synchronised output (2026), scroll regions (DECSTBM), mouse normal/motion/pixels (1000/1002/1016) |
| `queries` | 5 | Device Attributes (DA1/DA2), DSR cursor position, DECRQM mode queries, OSC colour queries |
| `graphics` | 9 | Kitty graphics protocol: basic display, positioned, transmit+display, chunked transfer, z-index, delete, delete all, query, screen clear interaction |
| `keyboard` | 5 | Legacy keyboard encoding, Kitty keyboard protocol: disambiguate (1), report events (3), report all keys (8), modifier reporting |
| `stress` | 5 | Scrolling throughput, full-screen colour barrage, rapid erase/repaint, mixed Unicode stress, window resize stability |

**Total: 57 test cases** covering ~250 individual checks across 10 categories.

## Modes

### Interactive (default)

Each test displays visual output and asks for a verdict:
```
  ◉ test-name   Description
  ── Visual check ──
  [p]ass [f]ail [s]kip ?
```

### Auto Mode (`--auto`)

Skips all interactive prompts, returning `SKIP` for visual-only tests.
Useful for automated runs where you only want automatic query tests.

### Quick Mode (`--quick`)

Only runs tests tagged `auto_verify` — cursor positioning, DSR queries,
DECRQM, colour queries. Faster but less thorough.

### CI Mode (`--auto --json report.json --quiet`)

Silent operation with JSON output for CI pipelines.

## Output Formats

### Terminal (default)
Colour-coded pass/fail/skip/error output with visual test patterns.

### JSON (`--json report.json`)
Machine-readable output:

```json
{
  "timestamp": 1718000000.0,
  "terminal_name": "WezTerm",
  "capabilities": {
    "truecolor": true,
    "kitty_graphics": true,
    "sync_output": true
  },
  "duration_seconds": 42.5,
  "results": [
    {
      "test_id": "sgr-bold",
      "category": "sgr",
      "status": "pass",
      "message": ""
    }
  ]
}
```

## Platform Support

| Platform | Status | Notes |
|----------|--------|-------|
| macOS | ✅ Tested | Full TTY support via termios |
| Linux | ✅ Should work | Same termios path |
| Windows | ✅ Should work | msvcrt + ctypes for console API |

Requires Python 3.8+. No third-party dependencies — only stdlib.

## Architecture

```
zenterm-test.py              Entry point / CLI argument parsing
zenterm_test/
├── __init__.py
├── terminal.py              Cross-platform Terminal I/O abstraction
│   ├── Terminal             Abstract base class
│   ├── _UnixTerminal        termios + tty + select implementation
│   └── _WindowsTerminal     msvcrt + ctypes implementation
├── capability.py            Auto-detection of terminal features
│                            (DA1/DA2, truecolor, protocols)
├── runner.py                TestRunner — filter, execute, report
├── reporter.py              TestResult, TestCase, Report, registry
│                            Terminal output helpers (heading, info, etc.)
└── tests/
    ├── sgr.py               SGR attribute tests
    ├── colors.py            Colour rendering tests
    ├── cursor.py            Cursor movement/shape/erase tests
    ├── unicode.py           UTF-8, CJK, emoji, ligature tests
    ├── osc.py               OSC sequence tests
    ├── modes.py             Screen mode tests
    ├── queries.py           Query/response tests
    └── stress.py            Performance/stress tests
```

## Adding New Tests

```python
from ..reporter import TestCase, register

def _test_my_feature(term) -> TestResult:
    """Test something."""
    # ... use term.write(), term.query_dsr(), etc.
    return TestResult(status=TestStatus.PASS, ...)

register(TestCase(
    test_id='my-feature',
    category='my-category',
    name='My Feature',
    description='What this test verifies',
    run=_test_my_feature,
    required_caps=['truecolor'],   # skip if missing
    interactive=False,
    auto_verify=True,
))
```

## Related Tools

- **[terminfo.dev](https://terminfo.dev)** — Terminal feature comparison matrix
- **[vttest](https://invisible-island.net/vttest/)** — VT100/VT220 compatibility test
- **[Terminal::Tests](https://github.com/japhb/Terminal-Tests)** — Raku terminal test suite
- **[scoutty](https://github.com/a-kenji/scoutty)** — Terminal capability probe
- **[lsix](https://github.com/hackerb9/lsix)** — Sixel image viewer (also probes sixel support)
- **[ansicode test-pattern](https://ansicode.eversources.app/en/test-pattern)** — curl-pipeable shell test
