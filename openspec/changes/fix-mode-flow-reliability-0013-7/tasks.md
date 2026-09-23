## 1. Collection completion drain

- [x] 1.1 Refactor `service_execution_clock()` so collection completion enters a drain state instead of returning before timeout, combat, recovery, and loot servicing. Verify with a test where collection completes while combat is engaged and confirm no logout is sent until combat clears.
- [x] 1.2 Stop/cancel new voluntary mission work when collection first becomes complete while preserving forced `Death`/`Combat`/`Recovery` and required `Loot`. Verify that queued Quest/Gather/Fishing/Goal work cannot start after completion.
- [x] 1.3 Move safe-logout evaluation after safety-critical servicing and keep the logout latch stable during the drain. Verify an idle completed collection sends exactly one logout request and repeated ticks do not duplicate it.

## 2. Fishing lease lifetime

- [x] 2.1 Change idle voluntary lease cleanup so an active `FishingRuntime` preserves its `ActivityKind::Fishing` lease even when normal action execution is idle. Verify the lease generation survives `Cast -> WaitBobber`.
- [x] 2.2 Verify `WaitBobber -> UseBobber` continues under the same lease and that loot completion/interrupt releases the fishing lifecycle normally.
- [x] 2.3 Verify forced Death/Combat/Recovery preemption interrupts Fishing and prevents reuse of a stale lease generation.

## 3. Deterministic Fishing location navigation

- [x] 3.1 Add a travel-eligibility selector that separates Mining/Herbalism `can_harvest` checks from Fishing `can_fish` checks. Verify Fishing hints can be selected for travel without making `ProfessionSkills::can_harvest(Fishing)` true.
- [x] 3.2 Update `NavigateToGatherResource` validation/execution for `GatherKind::Fishing`: travel to the trusted hint envelope, complete on positional arrival, skip normal gather-object hydration/defer logic, and never directly use the fishing-hole object. Verify far/near state transitions with unit tests.
- [x] 3.3 Gate `maybe_run_fishing()` on destination readiness. Verify a far-away Fishing mission navigates without casting, while a player inside the trusted arrival envelope can proceed to equip/cast.
- [x] 3.4 Add supervisor handling for a Fishing resource that has no trusted target-specific location. Verify it reports `fishing_location_unavailable` and does not cast blindly.

## 4. Fishing item/resource classification

- [x] 4.1 Extend the world-knowledge generator to read authoritative Fishing loot item IDs and resolve their names through item-template data, emitting a compact deduplicated Fishing-item classification index. Verify generated fixture data includes a known Fishing item such as `Raw Brilliant Smallfish` when present in the source data.
- [x] 4.2 Load/index the generated Fishing-item metadata in `WorldKnowledge` and make `gather_kind_for_name()` fall back to this index after gather-node matching. Verify a generated Fishing item resolves to `GatherKind::Fishing` and an unrelated fishing-like name does not.
- [x] 4.3 Regenerate the bundled world-knowledge asset and validate format/version compatibility. If an additive read is not possible, bump the knowledge format explicitly and verify startup fails clearly on an incompatible pack.

## 5. Impossible gather-skill detection

- [x] 5.1 Add raw matching gather-hint access that preserves required-skill data before eligibility filtering. Verify tests can observe both eligible and ineligible candidates for one resource.
- [x] 5.2 Update supervisor feasibility logic so all-known-candidates-above-rank reports profession-rank insufficiency instead of `waiting_for_matching_node`. Verify a mixed eligible/ineligible set remains runnable.
- [x] 5.3 Apply the equivalent required-skill feasibility check to location-backed Fishing hints. Verify a low Fishing rank reports insufficiency and does not cast or roam.

## 6. Collection top-level tool gate

- [x] 6.1 Add a dedicated collection gate at the start of `apply_mission_gate()` and return it before underlying Quest/Gather/Grind/Goal gates. Verify collection behavior is unchanged by switching the stored underlying mission between those modes.
- [x] 6.2 Define allowed collection voluntary tools by collection kind while preserving self-defense, death/recovery, loot, inventory/vendor logistics, and stop/wait controls. Verify an attacking non-allowlisted creature remains defendable but cannot become a voluntary collection target.
- [x] 6.3 Verify completed collection state exposes no new voluntary collection work and relies on the execution completion-drain path for shutdown.

## 7. Private collection-quest filtering

- [x] 7.1 Add a private `target_quest_id` to sanitized `CollectionState`, excluded from serde and schema output, and populate it only for quest collection. Verify serialized model context contains neither the target quest ID nor the private creature allowlist.
- [x] 7.2 Add a collection-authorization helper for quest offers and apply it to model-facing quest choices, giver relevance, and deterministic collection quest selection without changing ordinary Quest-mode collectability semantics. Verify a giver offering target plus unrelated quests exposes only the target during collection.
- [x] 7.3 Keep `CollectionConfig::allows_quest()` in the executor as defense in depth. Verify a deliberately injected unrelated quest action is still rejected.

## 8. Regression and release gate

- [x] 8.1 Add focused regression tests for every state transition above in the nearest owning modules (`plugin`, `harness`, `fishing`, `gather`, `world_semantics`, `world_knowledge`, `supervisor`, `collection`, and `llm`). Verify each test fails against the pre-fix behavior before accepting the implementation.
- [x] 8.2 Run `cargo fmt --check`, `cargo check`, targeted new tests, and full `cargo test` in a Rust-enabled environment. Record and fix all failures before release packaging.
- [x] 8.3 Regenerate and validate the world-knowledge asset after generator changes. Verify the production loader accepts the generated artifact and classification tests use the same schema.
- [x] 8.4 Only after all checks pass, update build-stage/channel labels to `0013.7`, package the release ZIP, and record its SHA-256. Do not label or package an implementation as `0013.7` while any regression or build check is failing.
