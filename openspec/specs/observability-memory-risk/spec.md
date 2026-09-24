# Observability, Memory, and Risk Specification

## Purpose

Define safe logging, structured action history, optional persistent memory, bounded risk evidence, and non-blocking observability for multi-lane operation.

## Requirements

### Requirement: Per-run managed logging
The system SHALL create or adopt managed logging paths only when ownership safety rules permit it and SHALL separate fleet-level and per-bot diagnostics where configured.

#### Scenario: Managed log directory is safe
- **WHEN** the log directory is empty or carries valid ownership evidence
- **THEN** the runtime may adopt or rotate it according to log policy

#### Scenario: Non-empty directory lacks ownership proof
- **WHEN** a non-empty candidate log directory lacks valid ownership evidence
- **THEN** the runtime does not destructively replace it as managed logs

### Requirement: Structured action history
The system SHALL record enough structured lifecycle information to correlate logical actions with their bot, task, mission, permission, and terminal outcome where those fields apply.

#### Scenario: Action reaches terminal failure
- **WHEN** a logical action fails permanently
- **THEN** its structured history records one terminal failure with the relevant typed context

### Requirement: Observability does not serialize the fleet
The system SHALL avoid holding broad fleet synchronization while performing slow per-bot log I/O.

#### Scenario: One bot's log sink stalls
- **WHEN** a per-bot logging operation is slow
- **THEN** unrelated bot lanes are not intentionally blocked on that file I/O through a fleet-wide lock

### Requirement: High-frequency diagnostics are bounded
The system SHALL throttle, aggregate, or otherwise bound high-frequency diagnostics. Structured runtime diagnostics SHALL use bounded nonblocking queues so slow log storage does not block bot work.

#### Scenario: Movement heartbeat is stable
- **WHEN** high-frequency movement state remains materially unchanged
- **THEN** logging does not grow without bound from redundant per-tick messages

#### Scenario: Structured diagnostic storage falls behind
- **WHEN** a diagnostic queue is full
- **THEN** the runtime drops diagnostic records and reports the drop total
- **AND** the bot continues its work without waiting for the log writer

### Requirement: Configurable persistent memory
The system SHALL honor whether persistent memory is required or optional and SHALL not fabricate persistence when storage is unavailable.

#### Scenario: Required memory backend is unavailable
- **WHEN** memory is configured as required and the backend cannot be established securely
- **THEN** startup or the required memory feature fails explicitly

#### Scenario: Optional memory backend is unavailable
- **WHEN** memory is optional and the backend cannot be established
- **THEN** memory-dependent features may degrade
- **AND** the system does not pretend the failed data was persisted

### Requirement: Bounded risk evidence
The system SHALL keep risk evidence finite and bounded in lifetime or size and SHALL preserve uncertainty when evidence is incomplete.

#### Scenario: Historical risk evidence expires
- **WHEN** risk evidence exceeds its configured lifetime or retention rule
- **THEN** it no longer contributes as current evidence

### Requirement: Risk-aware route choice
The system SHALL permit a materially safer viable route to replace a riskier route while avoiding oscillation on immaterial score differences.

#### Scenario: Safer detour is materially better
- **WHEN** a viable alternative provides a material safety improvement within route policy
- **THEN** the runtime may replace the current route
