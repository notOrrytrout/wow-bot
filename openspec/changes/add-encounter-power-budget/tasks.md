## 1. Encounter power budget

- [x] 1.1 Add a reusable encounter-risk calculation for bot effective power versus observed hostile pack power.
- [x] 1.2 Include bot level, hostile level range, hostile count, elite/rank contribution, equipment-condition contribution, health/resource pressure, and uncertainty.
- [x] 1.3 Treat missing gear-score evidence as neutral contribution with increased uncertainty, not as a panic or invented item score.
- [x] 1.4 Add bounded recent mob-death risk memory with a MySQL table and local fallback.

## 2. Survival integration

- [x] 2.1 Apply encounter-risk bands to solo survival escalation for both melee and ranged/caster profiles.
- [x] 2.2 Keep solo ranged/caster profiles more conservative than melee/tank profiles.
- [x] 2.3 Preserve the prior solo ranged kite-back behavior without applying it to melee profiles.

## 3. Diagnostics and tests

- [x] 3.1 Emit diagnostics with bot level, hostile level range, hostile count, elite/rank contribution, recent death-risk contribution, gear contribution, bot power, pack power, ratio, and final band.
- [x] 3.2 Add focused tests for high-level bot versus low-level pack, low-level melee versus high-level pack, ranged conservatism, elite/rank contribution, missing gear neutrality, and ranged-only kite preservation.
- [ ] 3.3 Run Cargo validation in a Rust-enabled environment.
- [ ] 3.4 Run `openspec validate --changes` in an OpenSpec-enabled environment.


## 4. Production-readiness remediations

- [x] 4.1 Persist mob-death risk counts as authoritative cumulative values rather than repeatedly adding cumulative snapshots.
- [x] 4.2 Make local mob-death risk identity preserve separate geographic hazard cells like the MySQL primary key.
- [x] 4.3 Ensure bot memory clearing removes `mob_death_risks` from both MySQL and local fallback memory.
- [x] 4.4 Handle Unix SIGTERM through the supervisor graceful-shutdown path.
- [x] 4.5 Record the current transitive `rsa` RustSec advisory as an explicit cargo-audit exception because no fixed upgrade exists and remote MySQL transport requires identity-verifying TLS.
