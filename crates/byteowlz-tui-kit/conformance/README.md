# OSC 7501 emitter conformance, v1

Shared, language-neutral checker and JSON fixtures for byteowlz tools. Python 3.8+
standard library only; no dependency on xlatch, Rust, Pi or a running terminal.
Protocol source: [Program Status Protocol revision 0.3](https://www.superlogical.com/rex/docs/build/program-status)
(2026-10-07). Fixture schema version: 1. `schema_version` in CLI JSON is independently 1.

```sh
python3 -m unittest discover -s crates/byteowlz-tui-kit/conformance -v
python3 crates/byteowlz-tui-kit/conformance/osc7501.py capture.bin --json \
  --require-state working --require-state blocked --require-state done
python3 crates/byteowlz-tui-kit/conformance/osc7501.py redirected.bin --expect-none
# Or pipe raw captured bytes to stdin; ordinary terminal output is ignored.
```

Exit 0: valid; 1: invalid/missing/unexpected emissions; 2: input/CLI error. Captures
are bounded to 4 MiB by default (`--max-bytes`, maximum 64 MiB); frames to 4096
bytes; at most 65536 frames are examined. Both BEL and ST framing work. Diagnostics
contain fixed reason codes, frame indices and state names—not captured messages,
titles, keys or IDs. Do not publish raw captures containing sensitive data.

## Scope—not a certification of the whole protocol

This validates **emitted wire records**, including pair grammar, base64/UTF-8,
control-character exclusion, text/ID/app/key limits, allowed state/kind/progress,
feature queries, repeated-field validation and last-value-wins parsing. Fixtures
are original, specification-derived data. Existing Pi emitter tests and Ghostty
PR #14560 parser tests were inspected as independent coverage references; no code
or fixtures were imported from them.

An emitter should not deliberately emit invalid optional fields. Accordingly this
checker is **stricter than a terminal consumer**: malformed pairs, unknown states,
invalid app/kind/progress/IDs fail emitter validation where a conforming consumer
may skip the pair, ignore a field, or ignore the report. Unknown well-formed keys
are accepted. Reports remain untrusted; no status grants authorization. Literal
ID segments such as `..` are protocol labels, not filesystem navigation.

These checks DO NOT prove full-record replacement/subtree-clear behavior, record
lifetimes, observation/notification semantics, feature-reply parsing, application
meaning, deduplication, process ownership or terminal/mux/SSH propagation. Consumer
model fixtures and real interoperability adapters remain future work. Never label
this result "full OSC 7501 compliance".

## Per-app integration

Each application owns its PTY scenarios. Capture actual executable output, pass it
to this checker and also assert semantics: preview/cancel cannot mutate, a confirmed
operation runs once, settled states reflect only the current targets, cancellation
returns idle, messages are static/redacted, terminal flags restore, and redirected/
JSON/headless outputs contain no OSC frames. OS-specific PTY tests are not needed
to run the portable shared fixture suite.

Eavs integration example (from the eavs checkout):

```sh
BYTEOWLZ_OSC7501_CHECKER=/absolute/path/to/this/osc7501.py just test-fleet-tui
```

Publishing/pinning this uncommitted suite is a separate authorized step. Until then,
passing an explicit local checker path proves local cross-repo reuse, not that an
old published toolkit revision contains the suite or that CI runs the shared gate.
