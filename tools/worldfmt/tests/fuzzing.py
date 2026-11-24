"""Seeded, deterministic fuzzing for the readers of untrusted files (roadmap
M1, "fuzzing of the strict file readers").

A fuzz test mutates a corpus (the committed fixtures plus small hand-made
edge cases) and checks that a reader either accepts the input or raises its
documented error: never another exception, never a warning, never slowly
and, for the readers whose output should stay proportional to their input,
never with memory out of proportion (tracemalloc). Every case is derived
from the seed, the test's name and the case number alone, so a failure
names the case that reproduces it.

    STREETWIDE_FUZZ_ITERS   scales every test, as for the Rust fuzz targets:
                            a test whose default is d runs ITERS x d / 2000
                            cases (2000 is the default of the cheapest
                            tests), so 40000 runs every test 20 times longer
    STREETWIDE_FUZZ_SEED    the base seed (default 0x5EED), e.g. 12 or 0xC0FFEE

Standard library only: the tests add no dependency.
"""

from __future__ import annotations

import contextlib
import copy
import faulthandler
import json
import math
import os
import re
import sys
import time
import tracemalloc
import warnings
from collections.abc import Callable, Iterator
from typing import Any

MASK64 = (1 << 64) - 1
REFERENCE_CASES = 2000
"""The default case count of the cheapest tests (canonical JSON)."""
SLOW_CASE_S = 10.0
"""A case slower than this fails: typical cases take milliseconds."""
GROWTH_BYTES = 1 << 20
"""About the most one mutation adds to a JSON document."""
HANG_S = 300.0
MEMORY = (128, 16 << 20)
"""The memory allowance of the readers whose work is proportional to their
input (see run_cases): 128 bytes per input byte (parsed Python objects take
tens of times the text they come from) plus 16 MiB."""

_readings: list[tuple[int, int]] = []


@contextlib.contextmanager
def reading(size: int) -> Iterator[None]:
    """Measures, with tracemalloc (which numpy reports to), the most memory
    the code inside takes: a reader working on an input of `size` bytes. For
    run_cases(memory=...), which checks every reading of a case."""
    outer = tracemalloc.is_tracing()
    if outer:
        tracemalloc.reset_peak()
        before = tracemalloc.get_traced_memory()[0]
    else:
        tracemalloc.start()
        before = 0
    try:
        yield
    finally:
        _readings.append((tracemalloc.get_traced_memory()[1] - before, size))
        if not outer:
            tracemalloc.stop()
"""A test that makes no progress for this long dumps every thread's stack
and ends the process (faulthandler), so a hang fails CI instead of stalling
it. Run pytest with -s to see the stacks."""


class Rng:
    """splitmix64: tiny, fast and identical on every platform."""

    def __init__(self, seed: int):
        self.state = seed & MASK64

    def next(self) -> int:
        self.state = (self.state + 0x9E3779B97F4A7C15) & MASK64
        z = self.state
        z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & MASK64
        z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & MASK64
        return z ^ (z >> 31)

    def below(self, n: int) -> int:
        """A number in 0..n-1 (n > 0)."""
        return self.next() % n

    def chance(self, percent: int) -> bool:
        return self.below(100) < percent

    def choice(self, seq):
        return seq[self.below(len(seq))]

    def bytes(self, n: int) -> bytes:
        return bytes(self.next() & 0xFF for _ in range(n))


def iterations(default: int) -> int:
    """`default`, or STREETWIDE_FUZZ_ITERS scaled to the test's share."""
    value = os.environ.get("STREETWIDE_FUZZ_ITERS")
    return max(1, int(value) * default // REFERENCE_CASES) if value else default


def base_seed() -> int:
    return int(os.environ.get("STREETWIDE_FUZZ_SEED", "0x5EED"), 0)


def case_rng(name: str, case: int) -> Rng:
    """The generator of one case: independent of the other cases."""
    h = 0xCBF29CE484222325  # FNV-1a of the test name, stable across runs
    for b in name.encode():
        h = ((h ^ b) * 0x100000001B3) & MASK64
    rng = Rng(base_seed() ^ h ^ (case * 0xD1B54A32D192ED03))
    rng.next()
    return rng


def run_cases(name: str, default: int, case: Callable[[Rng], None], *,
              memory: tuple[int, int] | None = None) -> int:
    """Runs `case` once per case number. A case that raises (the case itself
    lets only the reader's documented errors through), warns or runs slowly
    fails the test, naming the case. A warning is a silent coercion (a
    complex height made real, an invalid value cast), which a reader should
    refuse instead.

    With `memory` = (per_byte, fixed), each case runs its reader inside
    `reading(len(input))`, and the reader may take at most `per_byte` per
    input byte plus `fixed` bytes: it must not allocate what a length field
    or a count claims. Returns the number of cases run."""
    count = iterations(default)
    armed = -math.inf
    watchdog = sys.__stderr__  # a real file descriptor, whatever pytest captures
    worst = (0, 0)  # the most memory a reading took, and its input's length
    try:
        for i in range(count):
            start = time.perf_counter()
            if watchdog is not None and start - armed > 60:  # re-arming costs milliseconds
                faulthandler.dump_traceback_later(HANG_S, exit=True, file=watchdog)
                armed = start
            _readings.clear()
            try:
                with warnings.catch_warnings():
                    warnings.simplefilter("error")
                    case(case_rng(name, i))
            except Exception as e:
                raise AssertionError(
                    f"{name}: case {i} (STREETWIDE_FUZZ_SEED={base_seed():#x}) raised {type(e).__name__}: {e}"
                ) from e
            elapsed = time.perf_counter() - start
            if elapsed > SLOW_CASE_S:
                raise AssertionError(f"{name}: case {i} (STREETWIDE_FUZZ_SEED={base_seed():#x}) took {elapsed:.1f} s")
            if memory is not None:
                assert _readings, f"{name}: case {i} did not measure its reader with reading()"
                for used, size in _readings:
                    allowed = memory[0] * size + memory[1]
                    if used > allowed:
                        raise AssertionError(
                            f"{name}: case {i} (STREETWIDE_FUZZ_SEED={base_seed():#x}) took {used} bytes for an "
                            f"input of {size} bytes, more than {allowed} ({memory[0]} per byte plus {memory[1]})"
                        )
                    worst = max(worst, (used, size))
    finally:
        faulthandler.cancel_dump_traceback_later()
    if memory is not None:
        print(f"{name}: {count} cases, most memory {worst[0]} bytes (input of {worst[1]} bytes)")
    return count


# Byte-level mutations ------------------------------------------------------

INTERESTING_BYTES = (0x00, 0x01, 0x20, 0x22, 0x2F, 0x5C, 0x7F, 0x80, 0xC0, 0xFE, 0xFF)
INTERESTING_U32 = (
    0, 1, 2, 3, 4, 8, 12, 16, 0x7F, 0x80, 0xFF, 0x100, 0x7FFF, 0x8000, 0xFFFF, 0x10000,
    0x7FFFFFFF, 0x80000000, 0xFFFFFFF0, 0xFFFFFFFC, 0xFFFFFFFE, 0xFFFFFFFF,
)


def mutate_bytes(rng: Rng, data: bytes, corpus: list[bytes], max_len: int = 1 << 22) -> bytes:
    """One to four byte-level edits: bit flips, interesting bytes, 32-bit
    fields set to interesting values (lengths near u32::MAX), insertions,
    deletions, truncation, duplicated chunks and splices with another file."""
    out = bytearray(data)
    for _ in range(1 + rng.below(4)):
        op = rng.below(10)
        at = rng.below(len(out) + 1)
        if op == 0 and out:
            out[min(at, len(out) - 1)] ^= 1 << rng.below(8)
        elif op == 1 and out:
            out[min(at, len(out) - 1)] = rng.choice(INTERESTING_BYTES)
        elif op == 2 and len(out) >= 4:
            at = min(at, len(out) - 4) & ~(rng.below(2) * 3)  # often 4-aligned
            value = rng.choice(INTERESTING_U32)
            if rng.chance(30):
                value = (len(data) + rng.below(64) - 32) & 0xFFFFFFFF  # a near-miss length
            out[at : at + 4] = value.to_bytes(4, "little")
        elif op == 3:
            fill = rng.bytes(1 + rng.below(16)) if rng.chance(50) else bytes([rng.choice(INTERESTING_BYTES)]) * (1 + rng.below(256))
            out[at:at] = fill
        elif op == 4 and out:
            del out[at : at + 1 + rng.below(64)]
        elif op == 5 and out:
            del out[at:]
        elif op == 6 and out:
            a = rng.below(len(out))
            chunk = out[a : a + 1 + rng.below(256)]
            out[at:at] = chunk * (1 + rng.below(4))
        elif op == 7 and corpus:
            other = rng.choice(corpus)
            out[at:] = other[rng.below(len(other) + 1) :]
        elif op == 8 and out:
            out[at : at + 1 + rng.below(16)] = rng.bytes(1 + rng.below(16))
        elif op == 9 and len(out) >= 8:
            # Swap two chunks: moves headers and offsets around.
            a, b = sorted((rng.below(len(out)), rng.below(len(out))))
            n = min(1 + rng.below(32), b - a)
            out[a : a + n], out[b : b + n] = out[b : b + n], out[a : a + n]
    return bytes(out[:max_len])


# JSON-level mutations ------------------------------------------------------

_BIG_NUMBERS = (
    2**31 - 1, 2**31, 2**32 - 1, 2**32, 2**53 - 1, 2**53, -(2**53) + 1, -(2**53), 2**63 - 1, 2**63, 2**64,
    -1, 0, 1, 10**30, -(10**30), 10**400, -(10**400),  # the last two overflow a float
)
_STRINGS = (
    "", "a", "\u00e9\u00e8", "\u20ac", "\U0001F600", "\ud800", "\udc00", "\x00", "\x1f", "\x7f", '"', "\\", "/", "\n",
    "\ufeff", "\ufffe", "../../../../etc/passwd", "..\\..\\x", "C:\\x", "/abs/path", "%00", "NaN", "blake3:" + "0" * 64,
    "blake3:" + "f" * 63 + "g", "17/68646/45903", "12/2145/1434", "streetwide.asset", "streetwide.tile",
    "streetwide.world", "streetwide.change", "streetwide.check", "streetwide.merge", "streetwide.replay",
)
_KEYS = ("a", "format", "", "A", "a-b", "z9_", "\u00e9", "nodes", "children", "count", "id", "type", "tags", "__proto__")


def _interesting_value(rng: Rng) -> Any:
    k = rng.below(10)
    if k == 0:
        return None
    if k == 1:
        return rng.chance(50)
    if k == 2:
        return rng.choice(_BIG_NUMBERS)
    if k == 3:
        return rng.choice((0.5, -0.0, 1e308, -1e308, 5e-324, 1.0, float("nan"), float("inf")))
    if k == 4:
        return rng.choice(_STRINGS)
    if k == 5:
        return rng.choice(_STRINGS) * (1 + rng.below(4000))  # long strings
    if k == 6:
        return []
    if k == 7:
        return {}
    if k == 8:
        return rng.below(100) - 10
    return rng.choice(("x", 3, None)) if rng.chance(50) else [rng.below(3)]


def _json_size(value: Any) -> int:
    try:
        return len(json.dumps(value))
    except (ValueError, RecursionError):
        return GROWTH_BYTES


def _nodes(value: Any) -> list[tuple[Any, Any]]:
    """(container, key) of every value inside `value`, depth-first."""
    out = []
    stack = [value]
    while stack:
        v = stack.pop()
        items = v.items() if isinstance(v, dict) else enumerate(v) if isinstance(v, list) else ()
        for k, child in items:
            out.append((v, k))
            stack.append(child)
    return out


def mutate_json(rng: Rng, value: Any, donors: list[Any], edits: int | None = None) -> Any:
    """A copy of `value` with one to three structural edits: interesting
    scalars (huge numbers, long or odd strings), swapped types, deep nesting,
    deleted or added keys, long arrays and subtrees spliced from donors."""
    v = copy.deepcopy(value)
    for _ in range(edits or 1 + rng.below(3)):
        nodes = _nodes(v)
        if not nodes:
            v = _interesting_value(rng) if rng.chance(50) else {rng.choice(_KEYS): v}
            continue
        parent, key = rng.choice(nodes)
        child = parent[key]
        op = rng.below(12)
        if op <= 2:
            parent[key] = _interesting_value(rng)
        elif op == 3:
            depth = rng.choice((2, 10, 100, 126, 127, 128, 300))
            for _ in range(depth):
                child = [child] if rng.chance(50) else {"a": child}
            parent[key] = child
        elif op == 4:
            parent[key] = {"a": child} if isinstance(child, list) else [child] if isinstance(child, dict) else str(child)
        elif op == 5 and isinstance(parent, dict):
            del parent[key]
        elif op == 5:
            parent.pop(key)
        elif op == 6 and isinstance(child, dict):
            child[rng.choice(_KEYS + tuple(child))] = _interesting_value(rng)
        elif op == 7 and isinstance(child, list):
            # A long array of about GROWTH_BYTES at most: repeating arrays of
            # arrays would multiply into gigabytes, which the readers handle
            # in linear time but which only slows the run.
            items = child or [_interesting_value(rng)]
            per_item = max(1, _json_size(items) // len(items))
            n = max(2, min(rng.choice((2, 10, 200, 3000)), GROWTH_BYTES // per_item))
            repeated = (items * n)[: max(n, len(items))]
            try:  # independent copies: a later edit must not change them all at once
                repeated = json.loads(json.dumps(repeated))
            except (ValueError, RecursionError):
                repeated = repeated[:2]
            parent[key] = repeated
        elif op == 8 and donors:
            donor_nodes = _nodes(rng.choice(donors))
            if donor_nodes:
                p, k = rng.choice(donor_nodes)
                parent[key] = copy.deepcopy(p[k])
        elif op == 9 and isinstance(child, int) and not isinstance(child, bool):
            parent[key] = rng.choice((child + 1, child - 1, -child, child * 1000, child + 2**32, child - 2**32))
        elif op == 10 and isinstance(child, str):
            at = rng.below(len(child) + 1)
            parent[key] = child[:at] + rng.choice(_STRINGS) + child[at:]
        elif op == 11 and isinstance(parent, dict):
            # Swap two values of one object.
            other = rng.choice(list(parent))
            parent[key], parent[other] = parent[other], parent[key]
    return v


_NUMBER = re.compile(r"-?\d+")
_KEY_TOKEN = re.compile(r'"([^"\\]{1,40})":')
_ODD_NUMBERS = ("1" * 5000, "-" + "9" * 400, "1e400", "-0", "0.0", "1E2", "01", "9007199254740992", "1.5", "NaN", "-Infinity")
_ODD_ESCAPES = ("\\u0000", "\\ud800", "\\udc00\\ud800", "\\/", "\\u00e9", "\\uFFFF", "\\b", "\\u0041", "\\\\", '\\"')


def mutate_json_text(rng: Rng, text: str) -> str:
    """Textual edits a parsed value cannot express: duplicate keys, numbers
    with thousands of digits or exponents, unusual escapes, whitespace, a
    byte-order mark, unbalanced deep nesting and NaN."""
    op = rng.below(8)
    if op == 0:
        keys = _KEY_TOKEN.findall(text)
        opens = [m.start() for m in re.finditer(r"\{", text)]
        if keys and opens:
            at = rng.choice(opens) + 1
            return text[:at] + f'"{rng.choice(keys)}":{rng.choice(("0", "null", "{}", "[]"))},' + text[at:]
    if op == 1:
        numbers = list(_NUMBER.finditer(text))
        if numbers:
            m = rng.choice(numbers)
            return text[: m.start()] + rng.choice(_ODD_NUMBERS) + text[m.end() :]
    if op == 2:
        quotes = [m.start() for m in re.finditer('"', text)]
        if quotes:
            at = rng.choice(quotes) + 1
            return text[:at] + rng.choice(_ODD_ESCAPES) + text[at:]
    if op == 3:
        at = rng.below(len(text) + 1)
        return text[:at] + rng.choice((" ", "\n", "\t", "\r\n", "\u00a0")) + text[at:]
    if op == 4:
        return "\ufeff" + text if rng.chance(50) else text + rng.choice(("\n", " ", "x", "}", "]", "{}"))
    if op == 5:
        n = rng.choice((126, 127, 128, 129, 1000, 100_000))
        return "[" * n + (text + "]" * n if rng.chance(50) else "")
    if op == 6:
        n = rng.choice((127, 128, 2000))
        return '{"a":' * n + text + "}" * n
    return text.replace("0", rng.choice(("NaN", "Infinity", "-0", "1e9")), 1)


def dumps_like(rng: Rng, value: Any) -> bytes:
    """`value` as JSON bytes, usually in the canonical layout (sorted keys,
    no spaces, raw UTF-8) so mutated documents reach the canonical checks."""
    try:
        if rng.chance(80):
            return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8", "surrogatepass")
        return json.dumps(value, indent=rng.choice((None, 1)), ensure_ascii=rng.chance(50)).encode("utf-8", "surrogatepass")
    except (TypeError, ValueError, RecursionError):  # keys json cannot sort, or too deep
        return json.dumps(value, ensure_ascii=True, check_circular=False).encode() if _shallow(value) else b"[]"


def _shallow(value: Any) -> bool:
    try:
        json.dumps(value)
        return True
    except (TypeError, ValueError, RecursionError):
        return False


def mutate_json_bytes(rng: Rng, value: Any, donors: list[Any], corpus: list[bytes]) -> bytes:
    """A mutated JSON document: structural edits, then sometimes textual
    edits and byte edits on top."""
    v = mutate_json(rng, value, donors) if rng.chance(85) else value
    data = dumps_like(rng, v)
    if rng.chance(25):
        data = mutate_json_text(rng, data.decode("utf-8", "surrogatepass")).encode("utf-8", "surrogatepass")
    if rng.chance(20):
        data = mutate_bytes(rng, data, corpus)
    return data
