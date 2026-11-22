# Final performance-stack review resolution

The independent whole-stack review of `b5334661..94194e15` found one critical
correctness issue and four important benchmark-tooling issues. All five were
accepted and fixed; no minor findings were deferred. Review completion is not
an assertion that subsequent GitHub CI or integration has completed.

| Finding | Fix and regression evidence |
|---|---|
| Hidden source aliases can lose watchlist matches | Isolate the projected raw value before key extraction. The previous renderer failed nine checks; the fixed renderer passes three seeds on both chDB and native Antalya, with both alias settings and complete MV lifecycle multisets. [Logs and performance transfer](watchlist-alias-fix/README.md). |
| Late PostgreSQL responses could count as successful 60-second requests | Reject completed backend/COPY responses exceeding 60 seconds, even within the longer cancellation/protocol window. Both simulated 61-second responses failed before the fix and pass as deadline errors afterward. All 171 archived successful PG requests were already below 60 seconds (maximum 51.909 seconds); no timing was recensored. |
| Fixture lifecycle could affect unowned files or discard state on inspection errors | Create an exclusive state file and marked private artifact directory; preserve parent modes and siblings; validate Docker ownership; keep state on ambiguous inspection failures. Five ownership tests cover cleanup and failure cases. Legacy states do not authorize sibling artifact deletion. |
| Missing FDW counters appeared as zero | Preserve missing values as JSON null / blank CSV, with observed sample counts; retain measured zero. Two tests cover fully missing and partially observed counters. Regenerated 56 grouped rows from 384 samples; all 36 raw files remain byte-identical. |
| Older FDW profiler could wait indefinitely | Bound host/container/SQL waits, reap child processes and observers, and clean only tagged PG sessions. Save failed cohorts and stop instead of measuring a following cohort with unconfirmed remote completion. Four tests cover hung children, cancellation and failures. A live 50 ms lock timeout cleaned its blocked session in 0.871 seconds while preserving the independent lock holder. |

Post-fix local qualification passed 204 all-feature library tests, strict Clippy,
formatting, standalone package verification, six native/chDB seeded runs and
51 benchmark-helper tests. The watchlist transfer uses five interleaved pairs
for each of two 1M-row × 1,000-rule fixtures. All exact 10,000-pair results are
preserved; median changes of −3.11% and +0.22% stay below the regression gate.
Historical measurements, unfavorable cases and refusals remain unchanged.

The retained decision is the explicit raw-view rule-anchor join with staged
needles as default. No regex engine, syntax, flags, Windows-path translation,
cache or dependency change was selected by this performance experiment.
Elastic percolation, concurrent rule-update isolation, a global RSS bound and
universal engine rankings remain outside the measured claims. Final CI and
merge status belong to the linked PRs, not a permanent `merge_ready` boolean.
