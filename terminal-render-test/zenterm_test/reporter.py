"""Test result types and reporting utilities."""

import json
import time
from dataclasses import dataclass, field
from enum import Enum, auto
from typing import Callable, List, Optional, Tuple

from .terminal import Terminal


# ── Test case registry ────────────────────────────────────────────────────

_registry: List['TestCase'] = []


def register(test: 'TestCase') -> None:
    """Register a test case in the global registry."""
    _registry.append(test)


def get_test(test_id: str) -> Optional['TestCase']:
    for t in _registry:
        if t.test_id == test_id:
            return t
    return None


def list_tests() -> List['TestCase']:
    return list(_registry)


def tests_by_category() -> dict:
    cats = {}
    for t in _registry:
        cats.setdefault(t.category, []).append(t)
    return cats


# ── Test case ------------------------------------------------------------------

@dataclass
class TestCase:
    """A callable test case with metadata."""
    test_id: str
    category: str
    name: str
    description: str
    run: Callable
    required_caps: List[str] = field(default_factory=list)
    interactive: bool = False
    auto_verify: bool = False  # Can be verified via DSR without human input


class TestQuit(Exception):
    """Raised when user chooses to quit the test run early."""
    pass


class TestStatus(Enum):
    PASS = auto()
    FAIL = auto()
    SKIP = auto()
    ERROR = auto()
    VISUAL = auto()  # Requires human visual verification
    QUIT = auto()    # User requested early exit


@dataclass
class TestResult:
    """Outcome of a single test case."""
    test_id: str = ''
    category: str = ''
    name: str = ''
    status: TestStatus = TestStatus.SKIP
    message: str = ''
    duration_ms: float = 0.0
    details: str = ''


# Type alias for functions that return either TestResult or TestStatus
TestResultOrStatus = TestResult | TestStatus


# ── Report ────────────────────────────────────────────────────────────────

@dataclass
class Report:
    """Full test run report."""
    timestamp: float = field(default_factory=time.time)
    terminal_name: str = ''
    capabilities: dict = field(default_factory=dict)
    results: List[TestResult] = field(default_factory=list)
    duration_seconds: float = 0.0

    def summary(self) -> Tuple[int, int, int, int, int]:
        """Return (total, pass, fail, skip, error) counts."""
        total = len(self.results)
        passed = sum(1 for r in self.results if r.status == TestStatus.PASS)
        failed = sum(1 for r in self.results if r.status == TestStatus.FAIL)
        skipped = sum(1 for r in self.results if r.status == TestStatus.SKIP)
        errors = sum(1 for r in self.results if r.status == TestStatus.ERROR)
        return total, passed, failed, skipped, errors

    def to_json(self) -> str:
        """Serialize to JSON."""
        d = {
            'timestamp': self.timestamp,
            'terminal_name': self.terminal_name,
            'capabilities': self.capabilities,
            'duration_seconds': round(self.duration_seconds, 2),
            'results': [],
        }
        for r in self.results:
            d['results'].append({
                'test_id': r.test_id,
                'category': r.category,
                'name': r.name,
                'status': r.status.name.lower(),
                'message': r.message,
                'duration_ms': round(r.duration_ms, 1),
            })
        return json.dumps(d, indent=2)

    @staticmethod
    def from_json(data: str) -> 'Report':
        d = json.loads(data)
        report = Report(
            timestamp=d.get('timestamp', 0),
            terminal_name=d.get('terminal_name', ''),
            capabilities=d.get('capabilities', {}),
            duration_seconds=d.get('duration_seconds', 0),
        )
        for rd in d.get('results', []):
            report.results.append(TestResult(
                test_id=rd['test_id'],
                category=rd['category'],
                name=rd['name'],
                status=TestStatus[rd['status'].upper()],
                message=rd.get('message', ''),
                duration_ms=rd.get('duration_ms', 0),
            ))
        return report


# ── Terminal output helpers ───────────────────────────────────────────────

def heading(term: Terminal, title: str, char: str = '━') -> None:
    """Print a section heading."""
    term.reset()
    term.write(f'\033[36;1m  {title}\033[0m\n'.encode())
    term.write(f'\033[36m  {char * min(len(title) + 2, 60)}\033[0m\n\n'.encode())
    time.sleep(0.3)


def subheading(term: Terminal, title: str) -> None:
    term.write(f'\033[33;1m◆ {title}\033[0m\n'.encode())
    term.flush()


def info(term: Terminal, msg: str) -> None:
    term.write(f'\033[90m  {msg}\033[0m\n'.encode())
    term.flush()


def pass_msg(term: Terminal, msg: str) -> None:
    term.write(f'\033[32m  ✓ PASS\033[0m  {msg}\n'.encode())
    term.flush()


def fail_msg(term: Terminal, msg: str) -> None:
    term.write(f'\033[31m  ✗ FAIL\033[0m  {msg}\n'.encode())
    term.flush()


def skip_msg(term: Terminal, msg: str) -> None:
    term.write(f'\033[33m  ⊘ SKIP\033[0m  {msg}\n'.encode())
    term.flush()


def prompt_visual(term: Terminal, prompt_text: str = '') -> TestStatus:
    """Prompt user for visual verification result.

    Returns PASS if user presses Enter/p, FAIL if f, SKIP if s,
    QUIT if q. In non-TTY / auto mode, returns SKIP without prompting.
    """
    # If terminal is not interactive, skip automatically
    if not hasattr(term, '_is_tty') or not term._is_tty:
        return TestStatus.SKIP

    term.write(f'\033[36m  ── {prompt_text} ──\033[0m\n'.encode())
    term.write(b'  \033[90m[p]ass [f]ail [s]kip [q]uit ? \033[0m')
    term.flush()

    try:
        term.enter_raw_mode()
        while True:
            ch = term.read(timeout=10.0, max_bytes=1)
            if not ch:
                continue
            c = ch.lower()
            if c == b'p' or c == b'\r' or c == b'\n':
                term.write(b'\n')
                return TestStatus.PASS
            elif c == b'f':
                term.write(b'\n')
                return TestStatus.FAIL
            elif c == b's':
                term.write(b'\n')
                return TestStatus.SKIP
            elif c == b'q' or c == b'\x1b':  # q or Escape
                term.write(b'\n')
                return TestStatus.QUIT
    finally:
        term.exit_raw_mode()


def prompt_continue(term: Terminal) -> None:
    """Wait for user to press Enter to continue.

    Pressing 'q' or Escape raises TestQuit to abort the test run.
    In non-TTY / auto mode, returns immediately.
    """
    if not hasattr(term, '_is_tty') or not term._is_tty:
        return
    term.write(b'  \033[90m[Enter=continue q=quit]\033[0m')
    term.flush()
    term.enter_raw_mode()
    try:
        while True:
            ch = term.read(timeout=30.0, max_bytes=1)
            if ch in (b'\r', b'\n'):
                break
            if ch.lower() == b'q' or ch == b'\x1b':
                raise TestQuit()
    finally:
        term.exit_raw_mode()
    term.write(b'\n')
