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

Literal NUL bytes inside regex **patterns** are not portable across native
engine versions. Antalya 26.6.8.20004 qualification treated a literal-NUL pattern
as empty, while chDB/ClickHouse 26.11.1 preserved it. Use the regex escape `\x00`
to match a NUL byte across these versions. NUL bytes in input values remain
preserved and covered by the differential tests; this limitation concerns the
pattern, and the SQL helpers do not normalize it.

The accepted regex syntax is a checked portable subset, not every construct an
RE2 implementation might accept. Backreferences, look-around, class set
operations, `(?x)` and `\Q...\E` are rejected. Unicode properties are limited
to RE2's [documented general categories and script names](https://github.com/google/re2/wiki/Syntax)
(plus `Any`); Rust-only
aliases/binary properties and `\u`/`\U` escapes are rejected. Counted repeats,
including the product of nested counts, cannot exceed 1000. `wc_re2_check`
returns portability issues. Matching uses a linear-time engine; admission and
memory settings are described below.

## Pattern admission and matcher retention

Source patterns are limited to **16 KiB of bytes**, checked before parsing or
translation by `Matcher::new`, RE2 validation, the public regex planner and
native predicate generation. Oversized patterns return an error. PostgreSQL's
planner declines rewrites that would bypass this check. RE2 translation can
expand an admitted source pattern beyond 16 KiB; the matcher plans that bounded
translation without treating it as a new source pattern.

Rust callers parsing untrusted patterns directly should use
`pattern::parse_like_checked`, `pattern::parse_wildcard_checked` or
`verify::pieces_for_checked`. The original infallible helpers remain available
as low-level APIs and do not enforce the source-size limit themselves.

The regex engine is configured with an **8 MiB NFA compilation limit** and an
**8 MiB lazy-DFA cache capacity**. These settings do not bound all parser,
compiler or search allocations. Each matcher owns one synchronized search
cache. Its retained-scratch allowance is 16 times the engine's reported compiled
size, clamped to **64 KiB–1 MiB**. A search may temporarily exceed that allowance;
scratch exceeding it is dropped before the search returns. The allowance is a
retention policy, not a prediction of search memory. Library callers can use
`Matcher::create_cache()` and `is_match_with(&mut cache, raw)` to keep scratch
per worker and avoid the shared mutex. Reusing a caller cache with another
matcher resets it safely. Each caller cache has the same retention policy but
is outside `Matcher::cache_charge()`; bound the number of worker caches too.

The PostgreSQL backend cache defaults to **256 matchers** and **32 MiB of
accounted state**. Random replacement evicts individual entries instead of
clearing the cache, avoiding the clear-all cliff on cyclic watchlists. Tune
`pg_wildcard.matcher_cache_entries` (0–65536) and
`pg_wildcard.matcher_cache_kb` (0–1048576 KiB) per backend; zero in either disables
retention. Limits apply on the next match or `wc_matcher_cache_stats()` call,
which reports entries, charged bytes, hits, misses and evictions. These counters
are backend-local and survive limit changes. Its charge includes the engine's approximate compiled-memory
report, gram-plan vector capacities, matcher storage, the scratch allowance,
pattern-key bytes and an entry metadata allowance. Entries that exceed the
budget still execute but are not cached. Cache eviction releases its own
references; caller-held references can outlive eviction. A new compilation can
overlap retained entries before its charge is known. This accounting excludes
allocator overhead, caller-owned values and temporary work, and is not a
process RSS limit.

Direct `Matcher::is_match` calls do not impose a raw-value size limit. Callers
must bound those values separately. The raw view borrows its input; transformed
views allocate output proportional to the input. The decoded view lowercases
its decoded buffer in place.

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
