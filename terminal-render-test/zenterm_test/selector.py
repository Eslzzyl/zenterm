"""Interactive test selector — lets user pick which tests to run at startup.

Provides a terminal UI that shows all registered tests grouped by category,
letting the user toggle individual tests, whole categories, or select-all/none
before confirming and starting the run.
"""

import sys
from typing import Dict, List, Optional, Set

from .terminal import Terminal
from .reporter import TestCase, list_tests


# ── helpers ────────────────────────────────────────────────────────────────


def _is_number_or_range(text: str) -> bool:
    """Check if text looks like a number / range expression (e.g. '1', '1-5', '1,3,5-7')."""
    text = text.replace(" ", "")
    if not text:
        return False
    for part in text.split(","):
        part = part.strip()
        if not part:
            return False
        if "-" in part:
            bits = part.split("-", 1)
            if not bits[0].isdigit() or not bits[1].isdigit():
                return False
        elif not part.isdigit():
            return False
    return True


def _parse_range(text: str) -> List[int]:
    """Parse a range expression like '1,3,5-7' into a flat list of ints."""
    text = text.replace(" ", "")
    result: List[int] = []
    for part in text.split(","):
        part = part.strip()
        if not part:
            continue
        if "-" in part:
            a_str, b_str = part.split("-", 1)
            start, end = int(a_str), int(b_str)
            if start <= end:
                result.extend(range(start, end + 1))
            else:
                result.extend(range(end, start + 1))
        else:
            result.append(int(part))
    return result


def _checkbox(mark: bool) -> str:
    """Return a coloured checkbox string: green ✓ or dim space."""
    return "\033[32m\u2713\033[0m" if mark else " "


# ── drawing ────────────────────────────────────────────────────────────────


def _draw_selector(
    term: Terminal,
    all_tests: List[TestCase],
    selected: Set[str],
) -> None:
    """Redraw the full selector UI on screen."""
    total = len(all_tests)
    sel_count = len(selected)

    # ── clear screen ───────────────────────────────────��──────────────
    term.write(b"\033[H\033[J")

    # ── banner ────────────────────────────────────────────────────────
    term.write(b"\033[36;1m")
    term.write(
        b"  \xe2\x95\x94\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90"
        b"\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90"
        b"\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90"
        b"\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90"
        b"\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90"
        b"\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90"
        b"\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90"
        b"\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90"
        b"\xe2\x95\x90\xe2\x95\x97\n"
    )
    term.write(
        b"  \xe2\x95\x91         Terminal Render Test Suite          \xe2\x95\x91\n"
    )
    term.write(
        b"  \xe2\x95\x91            Interactive Test Selection           \xe2\x95\x91\n"
    )
    term.write(
        b"  \xe2\x95\x9a\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90"
        b"\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90"
        b"\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90"
        b"\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90"
        b"\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90"
        b"\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90"
        b"\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90"
        b"\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90"
        b"\xe2\x95\x90\xe2\x95\x9d\n"
    )
    term.write(b"\033[0m\n")

    # ── selection summary ──────────���──────────────────────────────────
    pct = int(sel_count / total * 100) if total else 0
    if pct == 100:
        colour = "32"
    elif pct >= 50:
        colour = "33"
    else:
        colour = "31"
    term.write(
        f"  \033[1mSelected: \033[{colour};1m{sel_count}/{total}\033[0m"
        f"  \033[90m({pct}%)\033[0m\n\n".encode()
    )

    # ── tests grouped by category ──────────────────────────────────────
    categories: Dict[str, List[TestCase]] = {}
    for tc in all_tests:
        categories.setdefault(tc.category, []).append(tc)

    for cat_name in sorted(categories.keys()):
        cat_tests = categories[cat_name]
        cat_sel = sum(1 for t in cat_tests if t.test_id in selected)
        total_cat = len(cat_tests)

        # category header line
        if cat_sel == 0:
            head_icon = "\033[90m\u25cb\033[0m"     # empty circle
        elif cat_sel == total_cat:
            head_icon = "\033[32m\u25c9\033[0m"     # full circle
        else:
            head_icon = "\033[33m\u25ce\033[0m"     # half circle

        term.write(
            f"  {head_icon} \033[1m{cat_name}\033[0m"
            f"  \033[90m({cat_sel}/{total_cat})\033[0m\n".encode()
        )

        # individual test lines
        for tc in cat_tests:
            idx = all_tests.index(tc) + 1
            cb = _checkbox(tc.test_id in selected)
            term.write(
                f"    [{cb}] \033[90m{idx:3d}\033[0m. "
                f"\033[1m{tc.test_id:<28s}\033[0m "
                f"{tc.name}\n".encode()
            )

        term.write(b"\n")

    # ── footer help ──────────────────────────────────────────────────
    term.write(b"  \033[90m")
    term.write(
        b"\xe2\x94\x8c\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80"
        b" Commands "
        b"\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80"
        b"\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80"
        b"\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80"
        b"\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80"
        b"\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80"
        b"\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80"
        b"\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80"
        b"\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80"
        b"\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80"
        b"\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80"
        b"\xe2\x94\x80\xe2\x94\x80\xe2\x94\x90\n"
    )
    term.write(b"  \xe2\x94\x82  <n>         Toggle test by number             \xe2\x94\x82\n")
    term.write(b"  \xe2\x94\x82  <n>-<m>     Toggle range (e.g. 1-5)          \xe2\x94\x82\n")
    term.write(b"  \xe2\x94\x82  <cat>       Toggle category (e.g. sgr)       \xe2\x94\x82\n")
    term.write(b"  \xe2\x94\x82  a           Select all tests                 \xe2\x94\x82\n")
    term.write(b"  \xe2\x94\x82  n           Deselect all tests               \xe2\x94\x82\n")
    term.write(b"  \xe2\x94\x82  <Enter>     Run selected tests               \xe2\x94\x82\n")
    term.write(b"  \xe2\x94\x82  q           Quit                             \xe2\x94\x82\n")
    term.write(b"  \xe2\x94\x94")
    term.write(b"\xe2\x94\x80" * 48)
    term.write(b"\xe2\x94\x98\n")
    term.write(b"\033[0m\n")
    term.flush()


# ── public API ─────────────────────────────────────────────────────────────


def interactive_select(term: Terminal) -> Optional[Set[str]]:
    """Show interactive test selection UI and return chosen test IDs.

    Returns
        Set[str]  — the set of test IDs the user selected to run.
        None      — user chose to quit.
    """
    all_tests = list_tests()
    if not all_tests:
        term.write(b"  \033[31;1mNo tests registered!\033[0m\n")
        term.flush()
        return set()

    # sort deterministically
    all_tests.sort(key=lambda t: (t.category, t.test_id))

    # default: all selected
    selected: Set[str] = {tc.test_id for tc in all_tests}

    # pre-build category -> test_ids mapping
    cat_to_ids: Dict[str, List[str]] = {}
    for tc in all_tests:
        cat_to_ids.setdefault(tc.category, []).append(tc.test_id)

    while True:
        _draw_selector(term, all_tests, selected)

        # read a command line
        term.write(b"  \033[36m> \033[0m")
        term.flush()

        try:
            raw = sys.stdin.readline()
        except (EOFError, KeyboardInterrupt):
            return None

        cmd = raw.strip()
        if not cmd:
            # Enter — confirm selection
            if not selected:
                term.write(
                    b"  \033[33mNo tests selected."
                    b" Press Enter again to run empty set.\033[0m\n"
                )
                term.flush()
                try:
                    sys.stdin.readline()
                except (EOFError, KeyboardInterrupt):
                    return None
                return selected
            return selected

        if cmd == "q":
            return None

        if cmd == "a":
            selected = {tc.test_id for tc in all_tests}
            continue

        if cmd == "n":
            selected = set()
            continue

        # number / range expression
        if _is_number_or_range(cmd):
            indices = _parse_range(cmd)
            for idx in indices:
                if 1 <= idx <= len(all_tests):
                    tc = all_tests[idx - 1]
                    if tc.test_id in selected:
                        selected.remove(tc.test_id)
                    else:
                        selected.add(tc.test_id)
            continue

        # category name
        if cmd in cat_to_ids:
            cat_ids = set(cat_to_ids[cmd])
            # if any deselected → select all; otherwise deselect all
            if cat_ids.issubset(selected):
                selected -= cat_ids
            else:
                selected |= cat_ids
            continue

        # unknown command — redraw (error message visible briefly)
        term.write(f"  \033[31mUnknown: {cmd!r}\033[0m\n".encode())
        term.write(b"  \033[90mPress Enter to continue...\033[0m")
        term.flush()
        try:
            sys.stdin.readline()
        except (EOFError, KeyboardInterrupt):
            return None
