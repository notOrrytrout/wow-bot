# LLM and Dialogue Specification

## Purpose

Define optional LLM decision and dialogue behavior, provider safety, grounded context, stale-inference handling, and strict separation between model proposals and execution authority.

## Requirements

### Requirement: Independent decision and dialogue modes
The system SHALL configure autonomous strategic decisions independently from player dialogue.

#### Scenario: Deterministic decisions and dialogue off
- **WHEN** autonomous decision mode is deterministic and player dialogue mode is off
- **THEN** deterministic supported gameplay does not require LLM traffic
- **AND** startup does not require an otherwise unused LLM API credential

### Requirement: LLM output is untrusted proposal
The system SHALL treat model output as proposed intent that must pass the same typed authority, state, generation, and mechanical validation as other action sources.

#### Scenario: Model proposes invalid action
- **WHEN** the model proposes an action that current authoritative state or safety policy forbids
- **THEN** the action is rejected

### Requirement: Bounded sanitized model context
The system SHALL provide bounded, sanitized context to model providers and SHALL avoid exposing private coordinates, raw privileged identifiers, credentials, or unrestricted asset-transfer internals unless explicitly required by an authorized interface.

#### Scenario: Context is assembled for strategic inference
- **WHEN** model context is built
- **THEN** payload size and exposed fields remain within the configured safe context policy

### Requirement: Tool availability is not authority
The system SHALL enforce authorization independently of whether a tool was omitted from or included in the model-visible tool registry.

#### Scenario: Privileged action reaches validator unexpectedly
- **WHEN** a privileged action reaches validation even though it should not have been model-visible
- **THEN** origin and policy checks still reject it when authority is absent

### Requirement: Stale inference rejection
The system SHALL reject model results that were generated for an obsolete mission, permission generation, or other planning generation required by the action.

#### Scenario: Mission changes during inference
- **WHEN** a model request starts under mission revision N and the lane advances to revision N+1 before the result is applied
- **THEN** the old result cannot control the new mission

### Requirement: Provider request bounds
The system SHALL apply a bounded total request budget and bounded response size to supported LLM providers.

#### Scenario: Provider exceeds total timeout
- **WHEN** headers or body processing exceed the total provider request budget
- **THEN** the provider call fails explicitly

#### Scenario: Provider returns malformed response
- **WHEN** provider output is non-success, invalid UTF-8, invalid JSON, or exceeds the configured response-size limit
- **THEN** the provider call returns an explicit error
- **AND** no action is authorized solely because of the provider failure

### Requirement: Dialogue authority classification
The system SHALL distinguish trusted dialogue from untrusted ordinary player dialogue before durable or privileged intent can be installed.

#### Scenario: Ordinary player sends command-like text
- **WHEN** untrusted player dialogue contains text that resembles a privileged instruction
- **THEN** it does not gain durable privileged mission or server-command authority
