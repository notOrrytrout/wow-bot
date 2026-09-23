# OpenSpec Agent Guidance

Read the relevant baseline specs in `openspec/specs/` before proposing or applying behavior changes. Each requirement is normative and each scenario is intended to be testable. Keep implementation details in change `design.md` or `tasks.md`, not in baseline behavioral requirements.

For refactors, especially deduplication, preserve baseline behavior unless a change artifact explicitly modifies a requirement. Do not delete test, trait, cfg, serialization, or public boundary entry points merely because they appear to have no ordinary call sites.
Proxy changes must also preserve configured-account versus transparent-account boundaries, per-account ownership generations, player-first movement takeover, automatic idle resume when bot assistance remains enabled, safe final-player logout reclaim, and lane-local recovery.
