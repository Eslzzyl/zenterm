"""Cross-platform terminal I/O abstraction layer.

Handles raw-mode TTY interaction on both Unix (termios) and Windows (msvcrt).
Provides high-level helpers for sending escape sequences and reading responses.
"""

import os
import sys
import time
import struct
import platform
from abc import ABC, abstractmethod
from typing import Optional, Tuple


SYSTEM = platform.system()


# ---------------------------------------------------------------------------
# Platform-detection helpers
# ---------------------------------------------------------------------------


def _is_fd_tty(fd: int) -> bool:
    try:
        return os.isatty(fd)
    except OSError:
        return False


# ---------------------------------------------------------------------------
# Abstract base
# ---------------------------------------------------------------------------


class Terminal(ABC):
    """Abstract interface to the terminal this process is running inside."""

    def __init__(self) -> None:
        self._raw = False

    # ── Lifecycle ──────────────────────────────────────────────────────────

    @abstractmethod
    def enter_raw_mode(self) -> None:
        """Put terminal into raw / no-echo mode so we can read responses."""
        ...

    @abstractmethod
    def exit_raw_mode(self) -> None:
        """Restore terminal to original settings."""
        ...

    def __enter__(self):
        self.enter_raw_mode()
        return self

    def __exit__(self, *exc) -> None:
        self.exit_raw_mode()

    # ── I/O ────────────────────────────────────────────────────────────────

    @abstractmethod
    def write(self, data: bytes) -> None:
        """Write raw bytes to the terminal (stdout)."""
        ...

    def writeln(self, data: bytes) -> None:
        """Write bytes followed by newline."""
        self.write(data + b"\n")

    def write_sgr(self, params: str, text: str = "") -> None:
        """Write an SGR sequence: ESC[<params>m + optional text + reset."""
        self.write(f"\033[{params}m{text}\033[0m".encode())

    @abstractmethod
    def read(self, timeout: float = 1.0, max_bytes: int = 4096) -> bytes:
        """Read up to *max_bytes* from stdin with *timeout* seconds."""
        ...

    def read_until(self, terminator: bytes, timeout: float = 2.0) -> Optional[bytes]:
        """Read until *terminator* byte is seen, or timeout."""
        deadline = time.monotonic() + timeout
        buf = bytearray()
        while time.monotonic() < deadline:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            chunk = self.read(min(remaining, 0.05), 1)
            if not chunk:
                continue
            buf.extend(chunk)
            if terminator in buf:
                return bytes(buf)
        return None if not buf else bytes(buf)

    # ── Queries ────────────────────────────────────────────────────────────

    def query_dsr(self, timeout: float = 2.0) -> Optional[Tuple[int, int]]:
        """Query cursor position via DSR ``ESC[6n``.

        Returns ``(row, col)`` 1-based, or *None* on timeout.
        """
        raw_before = self._raw
        if not raw_before:
            self.enter_raw_mode()
        try:
            self.write(b"\033[6n")
            resp = self.read_until(b"R", timeout=timeout)
            if not resp:
                return None
            # Expected: ESC[row;colR
            try:
                s = resp.decode("ascii", errors="replace")
                # Strip everything before '['
                idx = s.rfind("[")
                if idx < 0:
                    return None
                s = s[idx + 1 :]
                s = s.rstrip("R").rstrip("\r").rstrip("\n")
                parts = s.split(";")
                if len(parts) >= 2:
                    return int(parts[0]), int(parts[1])
            except (ValueError, IndexError):
                pass
            return None
        finally:
            if not raw_before:
                self.exit_raw_mode()

    def query_da1(self, timeout: float = 2.0) -> Optional[bytes]:
        """Query primary Device Attributes via ``ESC[c``.

        Returns the raw response bytes (e.g. ``ESC[?1;2c``) or *None*.
        """
        raw_before = self._raw
        if not raw_before:
            self.enter_raw_mode()
        try:
            self.write(b"\033[c")
            return self.read_until(b"c", timeout=timeout)
        finally:
            if not raw_before:
                self.exit_raw_mode()

    def query_da2(self, timeout: float = 2.0) -> Optional[bytes]:
        """Query secondary Device Attributes via ``ESC[>c``."""
        raw_before = self._raw
        if not raw_before:
            self.enter_raw_mode()
        try:
            self.write(b"\033[>c")
            return self.read_until(b"c", timeout=timeout)
        finally:
            if not raw_before:
                self.exit_raw_mode()

    def query_decrm(self, mode: int, timeout: float = 2.0) -> Optional[bytes]:
        """Query DEC private mode status via ``ESC[?mode$p``."""
        raw_before = self._raw
        if not raw_before:
            self.enter_raw_mode()
        try:
            self.write(f"\033[?{mode}$p".encode())
            return self.read_until(b"$y", timeout=timeout)
        finally:
            if not raw_before:
                self.exit_raw_mode()

    def query_osc_color(self, n: int, timeout: float = 2.0) -> Optional[str]:
        """Query OSC color value: ``ESC]4;n?ST``."""
        raw_before = self._raw
        if not raw_before:
            self.enter_raw_mode()
        try:
            self.write(f"\033]4;{n}?\033\\".encode())
            resp = self.read_until(b"\033\\", timeout=timeout)
            if resp:
                return resp.decode("ascii", errors="replace").strip()
            return None
        finally:
            if not raw_before:
                self.exit_raw_mode()

    # ── Terminal info ──────────────────────────────────────────────────────

    @abstractmethod
    def get_size(self) -> Tuple[int, int]:
        """Return ``(rows, cols)`` of the terminal window."""
        ...

    def reset(self) -> None:
        """Soft reset: SGR 0 + clear screen + cursor home."""
        self.write(b"\033[0m\033[2J\033[H")

    def reset_modes(self) -> None:
        """Restore terminal modes that tests may have changed.

        Call this before exit / on interrupt to avoid leaving the terminal
        in a broken state (e.g. Kitty keyboard protocol, mouse tracking).
        """
        self.write(
            b"\033[<u"           # Kitty keyboard: restore default
            b"\033[?1000l"       # Mouse: normal tracking off
            b"\033[?1002l"       # Mouse: button-event motion off
            b"\033[?1003l"       # Mouse: any-event tracking off
            b"\033[?1006l"       # Mouse: SGR encoding off
            b"\033[?1016l"       # Mouse: SGR pixels off
            b"\033[?25h"         # Cursor: show
            b"\033[?1049l"       # Alternate screen: restore main
            b"\033[?2004l"       # Bracketed paste: off
            b"\033[?2026l"       # Sync output: off
            b"\033[?1004l"       # Focus events: off
            b"\033[0m"           # SGR: reset
        )
        self.flush()
        self._input_drain()

    def _input_drain(self) -> None:
        """Flush and discard any pending input bytes.

        This prevents stale escape sequences (e.g. Kitty keyboard protocol
        responses generated before the mode reset) from leaking to the shell.
        """
        try:
            import termios
            termios.tcflush(sys.stdin.fileno(), termios.TCIFLUSH)
        except Exception:
            # Fallback: read & discard for a short time
            try:
                import select
                deadline = time.monotonic() + 0.3
                while time.monotonic() < deadline:
                    if select.select([sys.stdin], [], [], 0.01)[0]:
                        os.read(sys.stdin.fileno(), 4096)
                    else:
                        break
            except Exception:
                pass

    def clear(self) -> None:
        self.write(b"\033[2J\033[H")

    def flush(self) -> None:
        if hasattr(sys.stdout, "flush"):
            sys.stdout.flush()


# ---------------------------------------------------------------------------
# Unix implementation (termios + select)
# ---------------------------------------------------------------------------


class _UnixTerminal(Terminal):
    """Terminal handling for Linux / macOS via termios."""

    def __init__(self) -> None:
        super().__init__()
        self._fd = sys.stdin.fileno()
        self._orig_termios: Optional[list] = None
        self._orig_fl: Optional[int] = None
        self._is_tty = _is_fd_tty(self._fd) and _is_fd_tty(sys.stdout.fileno())

    def enter_raw_mode(self) -> None:
        if self._raw:
            return
        if not self._is_tty:
            self._raw = True  # pretend
            return
        import termios
        import tty

        self._orig_termios = termios.tcgetattr(self._fd)
        tty.setraw(self._fd)
        self._raw = True

    def exit_raw_mode(self) -> None:
        if not self._raw:
            return
        if not self._is_tty or self._orig_termios is None:
            self._raw = False
            return
        import termios

        termios.tcsetattr(self._fd, termios.TCSADRAIN, self._orig_termios)
        self._orig_termios = None
        self._raw = False

    def write(self, data: bytes) -> None:
        sys.stdout.buffer.write(data)
        sys.stdout.buffer.flush()

    def read(self, timeout: float = 1.0, max_bytes: int = 4096) -> bytes:
        if not self._is_tty:
            return b""
        import select

        if select.select([self._fd], [], [], timeout)[0]:
            return os.read(self._fd, max_bytes)
        return b""

    def get_size(self) -> Tuple[int, int]:
        if not self._is_tty:
            return 24, 80
        try:
            import fcntl
            import termios as t

            buf = fcntl.ioctl(self._fd, t.TIOCGWINSZ, b"\x00" * 8)
            rows, cols = struct.unpack("hh", buf)
            return rows, cols
        except Exception:
            return 24, 80


# ---------------------------------------------------------------------------
# Windows implementation (msvcrt + ctypes)
# ---------------------------------------------------------------------------


class _WindowsTerminal(Terminal):
    """Terminal handling for Windows via msvcrt and console API."""

    def __init__(self) -> None:
        super().__init__()
        self._stdin_handle = None
        self._orig_console_mode = None
        self._is_tty = _is_fd_tty(sys.stdin.fileno())
        if self._is_tty:
            self._enable_vtp()

    @staticmethod
    def _enable_vtp() -> None:
        """Enable Virtual Terminal Processing on Windows 10+.

        This lets us use ANSI escape sequences natively.
        """
        try:
            import ctypes

            kernel32 = ctypes.windll.kernel32  # type: ignore
            STD_OUTPUT_HANDLE = -11
            h = kernel32.GetStdHandle(STD_OUTPUT_HANDLE)
            # ENABLE_VIRTUAL_TERMINAL_PROCESSING = 0x0004
            ENABLE_VTP = 0x0004
            mode = ctypes.c_uint32(0)
            kernel32.GetConsoleMode(h, ctypes.byref(mode))
            kernel32.SetConsoleMode(h, mode.value | ENABLE_VTP)
        except Exception:
            pass  # VTP might already be on, or not an interactive console

    def enter_raw_mode(self) -> None:
        if self._raw:
            return
        try:
            import ctypes

            kernel32 = ctypes.windll.kernel32  # type: ignore
            self._stdin_handle = kernel32.GetStdHandle(-10)  # STD_INPUT_HANDLE
            mode = ctypes.c_uint32(0)
            if kernel32.GetConsoleMode(self._stdin_handle, ctypes.byref(mode)):
                self._orig_console_mode = mode.value
                # Disable ENABLE_ECHO_INPUT (0x0004) and ENABLE_LINE_INPUT (0x0002)
                new_mode = mode.value & ~(0x0004 | 0x0002)
                kernel32.SetConsoleMode(self._stdin_handle, new_mode)
        except Exception:
            pass
        self._raw = True

    def exit_raw_mode(self) -> None:
        if not self._raw:
            return
        try:
            import ctypes

            kernel32 = ctypes.windll.kernel32  # type: ignore
            if self._stdin_handle and self._orig_console_mode is not None:
                kernel32.SetConsoleMode(self._stdin_handle, self._orig_console_mode)
                self._orig_console_mode = None
        except Exception:
            pass
        self._raw = False

    def write(self, data: bytes) -> None:
        sys.stdout.buffer.write(data)
        sys.stdout.buffer.flush()

    def read(self, timeout: float = 1.0, max_bytes: int = 4096) -> bytes:
        if not self._is_tty:
            return b""
        import msvcrt

        deadline = time.monotonic() + timeout
        buf = bytearray()
        while time.monotonic() < deadline:
            if msvcrt.kbhit():  # type: ignore
                c = msvcrt.getch()  # type: ignore
                buf.append(c[0] if isinstance(c, bytes) else c)
                if len(buf) >= max_bytes:
                    break
            else:
                time.sleep(0.01)
        return bytes(buf)

    def get_size(self) -> Tuple[int, int]:
        if not self._is_tty:
            return 24, 80
        try:
            import ctypes

            kernel32 = ctypes.windll.kernel32  # type: ignore
            h = kernel32.GetStdHandle(-11)  # STD_OUTPUT_HANDLE
            buf = ctypes.create_string_buffer(22)
            if kernel32.GetConsoleScreenBufferInfo(h, buf):
                # CONSOLE_SCREEN_BUFFER_INFO layout:
                #   srWindow (RECT) at offset 8: Top, Left, Bottom, Right
                #   dwSize (COORD) at offset 0: X, Y
                import struct

                x, y, *rest = struct.unpack("HHHHHHHHH", buf.raw[:18])
                cols = x
                rows = y
                return rows, cols
        except Exception:
            pass
        return 24, 80


# ---------------------------------------------------------------------------
# Factory
# ---------------------------------------------------------------------------


def get_terminal() -> Terminal:
    """Detect platform and return the appropriate Terminal instance."""
    if SYSTEM == "Windows":
        return _WindowsTerminal()
    return _UnixTerminal()
