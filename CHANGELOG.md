# Changelog

## 1.1.0 — 2026-11-02

- Match Atuin 18.16.1 default GLOBAL fuzzy search in the relevance view,
  including query syntax, regex, smart case, author filtering, deduplication,
  the 200-command window, and minspan ordering.
- Add compact `sort by: time | relevance` controls below the search box.
  Relevance is the default; time preserves all matching records for management.
- Rank before web pagination and ignore stale search responses.
- Add search regression tests, a direct CLI parity check, and Atuin attribution.
