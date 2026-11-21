<!-- SPDX-License-Identifier: Apache-2.0 -->
# Matching semantics

ClickHouse `like` and `match` define the engine semantics implemented by the
Rust matcher and PostgreSQL `wc_*` functions. These differ from PostgreSQL's
collation-dependent native operators. The [differential tests](testing.md) check
parity across patterns, views and byte values.

## Pattern languages

| Kind | Meaning |
| --- | --- |
| `wildcard` | Whole-value pattern: `*` matches a sequence, `?` one UTF-8 character; backslash escapes literals |
| `like` | Whole-value SQL LIKE: `%` matches a sequence, `_` one UTF-8 character; backslash escapes literals |
| `regex` | Unanchored search using the supported RE2-compatible syntax |
| `iregex` | The same regex search with case-insensitive matching |

LIKE wildcards match newlines. `_` matches one character, not one byte:
`wc_like('aéb', 'a_b')` is true and `wc_like('aéb', 'a__b')` is false.
Leading and trailing `%` remove the corresponding value anchors. This matters
for binary data: `%b` can find a suffix after an invalid UTF-8 byte, while an
internal `%` cannot cross that byte. A `%`-only pattern matches such values.

Regex `.` matches newline by default; `$` is end of text by default.
`\d`, `\w`, `\s` and word boundaries use ASCII classes. Case-insensitive regex
matching uses Unicode case folding, unlike the ASCII-only `lower` view.
Character matching does not consume invalid UTF-8 bytes, but literal matches
can occur on either side of them.

The accepted regex syntax is a checked portable subset, not every construct an
RE2 implementation might accept. Backreferences, look-around, class set
operations, `(?x)` and `\Q...\E` are rejected. `wc_re2_check` returns portability
issues. Compilation and DFA caches have separate 8 MiB limits; matching uses a
linear-time engine.

## Views

| Name | Transformation |
| --- | --- |
| `raw` | Original bytes |
| `lower` | ASCII lowercase; non-ASCII bytes unchanged |
| `decoded` | Percent-decode, then ASCII lowercase |
| `revdomain` | Extract host, lowercase, remove one trailing dot, reverse labels; bracketed IPv6 stays intact |
| `winpath` | ASCII lowercase and replace each backslash with `/` |

Percent decoding follows `decodeURLComponent`: `%XY` decodes only when both
characters are hexadecimal. An invalid three-byte escape is copied as a unit:
`%4%41` stays `%4%41`, while `%4G%41` becomes `%4GA`. A short trailing escape
is copied. `+` stays `+`.

Host extraction removes the scheme, user information, numeric port and
path/query/fragment when present. For example,
`https://User@WWW.Example.com.:8080/x` becomes `com.example.www` in `revdomain`.
This is a search representation, not URL validation or a public-suffix parser.

`winpath` preserves separator counts, UNC structure, dot segments, percent
escapes, NUL and non-ASCII bytes. It does not resolve filesystem paths.
Write patterns using forward slashes, such as
`*/windows/system32/*.exe`.

Wildcard and LIKE literal pieces are ASCII-lowercased for non-raw views after
escape parsing; `winpath` also maps their separators. Patterns for `decoded`
and `revdomain` are written in the derived representation: pattern literals
are not percent-decoded or label-reversed. Regex syntax is unchanged and is
applied to the transformed value.

The byte-oriented Rust API preserves bytes. PostgreSQL `wc_view` is a display
helper and replaces invalid UTF-8; `wc_url_decode` returns `bytea` to preserve
decoded bytes. `wc_regex_bytes` accepts binary input. The text matching functions
are strict: a NULL argument yields NULL.

## Candidate plans

Grams are three-symbol windows over bytes with separate start/end symbols and
a namespace for each view. Literal runs produce conjunctions; alternatives
produce disjunctions. Unsupported or over-budget analysis widens the plan.
`wc_explain` displays it; `ALL` means that the pattern cannot prune candidates.
View IDs are `raw=0`, `lower=1`, `decoded=2`, `revdomain=3`, `winpath=4`.

Every exact match must satisfy its candidate plan. A candidate is not a match:
always retain the original exact predicate, including when using native
candidate staging, Puffin file pruning or experimental group/identity guards.
The [FDW](fdw.md) also retains PostgreSQL rechecks, but these cannot recover rows
that an incorrect remote predicate or incomplete file allowlist has excluded.
