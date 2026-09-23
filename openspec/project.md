# wow-bot OpenSpec Project Context

## System

`wow-bot` is a multi-lane WotLK automation runtime with an in-process control proxy in the supervisory process and separately supervised worker lanes for normal operation. Baseline behavior for the bot, supervisor, configuration, and the workspace `wow-proxy` crate is defined in `openspec/specs/`. Proxy specifications cover authentication, world relay, per-account ownership, live player/bot handoff, local commands, observation, supervisor coordination, and diagnostics. The target Rust architecture uses one mutable gameplay-state owner per lane, typed worker/proxy IPC, reducer-owned authoritative state, and exclusive configured-session upstream writes by the proxy.

## Specification rules

- Treat packet-derived dynamic state as authoritative.
- Treat model output as untrusted proposed intent.
- Preserve mission and permission generation invalidation.
- Treat player takeover and automatic hand-back as per-account ownership transitions; never as a fleet-wide mode switch.
- When bot assistance is enabled, player movement yields locomotion immediately and bot locomotion may resume only after the idle/handoff barriers are satisfied.
- Keep private coordinates, credentials, and privileged identifiers out of unrestricted model control.
- Prefer fail-closed behavior when required authority or live evidence is missing.
- For pure refactors, change implementation without changing baseline specs unless observable behavior intentionally changes.
- Keep Tentacli behind a narrow protocol/object adapter; Tentacli decoded state is input evidence, not wow-bot semantic state authority.
- Keep state, mission, permission, worker, ownership, movement, and activity invalidation domains independently representable.
- Permit configured-account upstream gameplay writes only through that account's proxy session owner after worker semantic validation and proxy transport-authority fencing.
- Use typed, versioned worker/proxy control IPC rather than a synthetic WoW WorldSession between wow-bot processes.
- Treat upstream AzerothCore addresses, proxy listener bind addresses, and client-advertised proxy addresses as separate configuration concerns; first-run setup must derive them from whether the WoW server is local and whether remote clients will connect.
- Perform a bounded startup reachability preflight for both AzerothCore auth and world services before workers are activated, with topology-aware errors when either service is unavailable.
- When remote stock clients are enabled, automatically discover the bot host's reachable non-loopback address before asking the user to enter one manually.
- Treat the AzerothCore DBC/maps/VMaps/mmaps tree as read-only external input; all wow-bot-generated files belong under the bot-owned `<repo>/wow-bot-data` root by default, with `WOW_BOT_HOME` as an explicit override.
- The normal desktop first-run experience must be guided and must not require manual JSON editing for network topology or initial account setup; internal lane/account identifiers are application-managed.
- Every recognized configured-account `.bot` or `.log` command must be operator-visible when consumed; control commands must also report whether the requested ownership/control transition was committed.
- Normal runtime diagnostics must be emitted to the interactive console and persisted under the bot-owned `logs/` directory; runtime or Action Log files must never default into the read-only AzerothCore data tree.
- `.log start`, `.log mark`, `.log status`, and `.log stop` are proxy-local operations; active/completed capture paths must be reported to the operator and captures must redact protected authentication, chat, and Warden payload content according to policy.

- A recognized non-idle mission is not operationally successful merely because it was parsed or stored; the runtime must close the observation → policy → validation → transport loop or emit a bounded, operator-visible waiting reason.

### Evidence release gate
Baseline OpenSpec requirements are the acceptance contract. A subsystem SHALL NOT be described as complete merely because it compiles or because one command path is wired. Review each baseline spec against implementation and regression evidence before reporting it complete. A baseline feature may be called complete only when its required runtime path and critical scenarios have implementation and regression evidence.

### Assisted-control idle resume must be live, not declarative
When player physical movement temporarily transfers locomotion away from the bot while automation remains enabled, the configured-session runtime MUST own a periodic timer that evaluates the idle-resume deadline. A stored deadline without a live timer is non-compliant. After the idle window elapses, bot ownership MUST be republished and the worker's PLAYER_CONTROL pause MUST be cleared. The transition and the resume MUST be observable in persistent runtime logs.

### Worker diagnostics must reach the supervisor log
Worker process stdout/stderr or equivalent structured worker diagnostics MUST be collected into the supervisor's persistent runtime log. A worker-side waiting reason that only appears in an unattached child terminal is not sufficient observability.
- Every configured lane owns an execution clock; runnable mission progress must not depend on incidental packet arrival.
- Active mission work has semantic work identity distinct from mission identity and transient actions.
- `NeedsMovement` is an execution handoff into owned movement work with a typed resume intent, never a log-only terminal state.
- Movement decisions start from canonical server-aligned movement state that reconciles player, bot, correction, teleport, and handoff events.
- Bot ownership activation uses generation-stamped prepare/commit with worker readiness before ownership is committed.
- Configured world packets are classified by gameplay relevance before reducer/planner projection.
- Expensive planning uses semantic planning keys; final send validation still checks current authoritative revisions and generations.
- Active movement emits bounded progress diagnostics and exactly one terminal result.
- Prefer one canonical deterministic implementation for each semantic operation. When questing, gathering, combat, movement, inventory, targeting, validation, encoding, retry, or state-projection behavior is needed from multiple paths, callers SHALL reuse the shared function/component rather than reimplement equivalent logic locally.
- Mission-specific code SHALL orchestrate shared deterministic subsystems rather than duplicate their mechanics. For example, quest-scoped gathering reuses the normal gather/loot runtime, quest-scoped movement reuses the movement runtime, and controlled spell actions reuse the shared cast validation/encoding path.
- Duplicated deterministic behavior is permitted only when the semantics intentionally differ; the distinction SHALL be represented by typed inputs/policy and documented by tests rather than by copy-pasted branches.
- Buff maintenance is a cross-mission execution-clock service: authoritative aura/spellbook/class state selects declarative persistent buff families; missing buffs use shared validation/cast encoding with bounded per-target retry; controlled movers, death/recovery, movement, and higher-priority mission work preempt maintenance.
- All targeted gameplay actions SHALL reuse one canonical spatial-precondition pipeline: authoritative range, server-backed LOS recovery, facing, then action dispatch. Mission/combat/gather/PvP code may supply typed action requirements but SHALL NOT duplicate range, facing, LOS, or reposition mechanics.
- Facing is a shared movement primitive and SHALL use the canonical active mover plus the normal movement-generation/transport fencing path. Bot-authored facing mirrored to a stock client must be recognized as bot feedback before attended-control takeover classification.
