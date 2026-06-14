"""Test runner: discovers, filters, and executes tests."""

import os
import time
from typing import List, Optional, Set

from .terminal import Terminal
from .capability import Capabilities
from .reporter import (
    Report,
    TestResult,
    TestStatus,
    TestCase,
    TestQuit,
    list_tests,
    info,
    pass_msg,
    fail_msg,
    skip_msg,
)


# ── The runner ────────────────────────────────────────────────────────────


class TestRunner:
    """Orchestrates test execution and reporting."""

    def __init__(
        self,
        term: Terminal,
        caps: Capabilities,
        include: Optional[Set[str]] = None,
        exclude: Optional[Set[str]] = None,
        auto_mode: bool = False,
        quick_mode: bool = False,
        quiet_mode: bool = False,
    ):
        self.term = term
        self.caps = caps
        self.include = include  # if set, only these categories
        self.exclude = exclude or set()
        self.auto_mode = auto_mode  # skip interactive / visual tests
        self.quick_mode = quick_mode  # only core tests
        self.quiet_mode = quiet_mode  # suppress per-test output

    def run(self) -> Report:
        """Execute all matching tests and return a Report."""
        report = Report(
            terminal_name=os.environ.get("TERM_PROGRAM", self.caps.term_env),
            capabilities={
                "termenv": self.caps.term_env,
                "colorterm": self.caps.colorterm_env,
                "truecolor": self.caps.has_truecolor,
                "sixel": self.caps.has_sixel,
                "kitty_graphics": self.caps.has_kitty_graphics,
                "kitty_keyboard": self.caps.has_kitty_keyboard,
                "sync_output": self.caps.has_sync_output,
                "bracketed_paste": self.caps.has_bracketed_paste,
                "focus_events": self.caps.has_focus_events,
                "sgr_mouse": self.caps.has_sgr_mouse,
                "osc8_hyperlinks": self.caps.has_osc8_hyperlinks,
            },
        )

        tests = self._select_tests()
        start = time.monotonic()

        try:
            try:
                for tc in tests:
                    result = self._run_single(tc)
                    report.results.append(result)
            except TestQuit:
                info(self.term, "Test run aborted by user.")
        finally:
            self.term.reset_modes()

        report.duration_seconds = time.monotonic() - start
        return report

    def _select_tests(self) -> List[TestCase]:
        """Return the list of tests to run based on filters."""
        all_tests = list_tests()

        # If include is set, only those categories
        if self.include:
            all_tests = [t for t in all_tests if t.category in self.include]

        # Remove excluded categories
        if self.exclude:
            all_tests = [t for t in all_tests if t.category not in self.exclude]

        # Quick mode: only non-interactive, auto-verify tests
        if self.quick_mode:
            all_tests = [t for t in all_tests if t.auto_verify]

        # Sort by (category, test_id)
        all_tests.sort(key=lambda t: (t.category, t.test_id))
        return all_tests

    def _run_single(self, tc: TestCase) -> TestResult:
        """Execute a single test case."""
        # Check required capabilities
        skip_reason = self._check_required(tc)
        if skip_reason:
            return TestResult(
                test_id=tc.test_id,
                category=tc.category,
                name=tc.name,
                status=TestStatus.SKIP,
                message=skip_reason,
            )

        # Skip interactive tests in auto mode
        if self.auto_mode and tc.interactive:
            return TestResult(
                test_id=tc.test_id,
                category=tc.category,
                name=tc.name,
                status=TestStatus.SKIP,
                message="Skipped in auto mode",
            )

        # Run
        t0 = time.perf_counter()
        try:
            raw_result = tc.run(self.term)
            # Test functions can return either TestResult or TestStatus
            if isinstance(raw_result, TestStatus):
                result = TestResult(
                    test_id=tc.test_id,
                    category=tc.category,
                    name=tc.name,
                    status=raw_result,
                    duration_ms=(time.perf_counter() - t0) * 1000,
                )
            elif isinstance(raw_result, TestResult):
                result = raw_result
                result.test_id = tc.test_id
                result.category = tc.category
                result.name = tc.name
                result.duration_ms = (time.perf_counter() - t0) * 1000
            else:
                result = TestResult(
                    test_id=tc.test_id,
                    category=tc.category,
                    name=tc.name,
                    status=TestStatus.ERROR,
                    message=f"Unexpected return type: {type(raw_result).__name__}",
                    duration_ms=(time.perf_counter() - t0) * 1000,
                )
        except TestQuit:
            raise  # Propagate quit immediately without wrapping
        except Exception as e:
            result = TestResult(
                test_id=tc.test_id,
                category=tc.category,
                name=tc.name,
                status=TestStatus.ERROR,
                message=f"Exception: {e}",
                duration_ms=(time.perf_counter() - t0) * 1000,
            )

        # Map VISUAL status in auto mode
        if self.auto_mode and result.status == TestStatus.VISUAL:
            result.status = TestStatus.SKIP
            result.message = "Skipped visual check in auto mode"

        # QUIT → propagate up to stop the entire test run
        if result.status == TestStatus.QUIT:
            result.status = TestStatus.SKIP
            result.message = "User quit during this test"
            self._print_result(result)
            raise TestQuit()

        # Print result
        self._print_result(result)
        return result

    def _check_required(self, tc: TestCase) -> Optional[str]:
        """Return a skip message if a required capability is missing."""
        cap_map = {
            "truecolor": self.caps.has_truecolor,
            "256color": self.caps.has_256color,
            "sixel": self.caps.has_sixel,
            "kitty_graphics": self.caps.has_kitty_graphics,
            "kitty_keyboard": self.caps.has_kitty_keyboard,
            "sync_output": self.caps.has_sync_output,
            "bracketed_paste": self.caps.has_bracketed_paste,
            "focus_events": self.caps.has_focus_events,
            "sgr_mouse": self.caps.has_sgr_mouse,
        }
        for cap in tc.required_caps:
            if not cap_map.get(cap, False):
                return f"Missing capability: {cap}"
        return None

    def _print_result(self, result: TestResult) -> None:
        """Print a single result line to the terminal."""
        if self.quiet_mode:
            return
        status_map = {
            TestStatus.PASS: ("  \033[32m✓\033[0m", pass_msg),
            TestStatus.FAIL: ("  \033[31m✗\033[0m", fail_msg),
            TestStatus.SKIP: ("  \033[33m⊘\033[0m", skip_msg),
            TestStatus.ERROR: ("  \033[31;1m!\033[0m", fail_msg),
            TestStatus.VISUAL: ("  \033[36m◉\033[0m", info),
            TestStatus.QUIT: ("  \033[33;1m■\033[0m", info),
        }
        icon, _ = status_map.get(result.status, ("  ?", info))
        self.term.write(
            f"{icon} \033[1m{result.test_id}\033[0m  {result.name}".encode()
        )
        if result.message:
            self.term.write(f"  \033[90m({result.message})\033[0m".encode())
        self.term.write(b"\n")
        self.term.flush()
