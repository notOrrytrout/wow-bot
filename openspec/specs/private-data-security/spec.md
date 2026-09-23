# Private Data Security Specification

## Purpose

Define fail-closed handling for credentials, runtime tokens, private filesystem data, and other sensitive capability material used by the bot runtime.

## Requirements

### Requirement: Owner-restricted private files
The system SHALL apply owner-restricted permissions or equivalent current-user access controls to private runtime files and directories.

#### Scenario: Private path can be secured
- **WHEN** the runtime creates a private credential, token, or capability path
- **THEN** access is restricted to the current user according to the platform's supported mechanism

#### Scenario: Private path cannot be secured
- **WHEN** required private access controls cannot be established
- **THEN** the operation fails closed

### Requirement: Unsafe path indirection rejection
The system SHALL reject unsafe filesystem indirection for security-sensitive paths when that indirection could redirect private writes or ownership checks.

#### Scenario: Secret path resolves through unsafe redirection
- **WHEN** a protected path traverses an unsafe symlink, reparse point, or equivalent redirection under the applicable platform policy
- **THEN** the protected operation is rejected

### Requirement: Secret-safe diagnostics
The system SHALL avoid writing credentials, authentication payloads, and protected capability values to ordinary logs and diagnostics.

#### Scenario: Authentication data is processed
- **WHEN** authentication material passes through the runtime
- **THEN** ordinary diagnostics omit or redact the sensitive payload

### Requirement: Secure remote memory transport
The system SHALL require identity-verifying transport security for configured remote database connections when the memory policy requires it.

#### Scenario: Remote database lacks required TLS verification
- **WHEN** a non-loopback database connection does not meet the configured identity-verification requirement
- **THEN** configuration or connection establishment fails
