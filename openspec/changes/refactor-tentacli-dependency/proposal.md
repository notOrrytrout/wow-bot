# Refactor Tentacli dependency usage

## Summary

Remove the repository-local Tentacli patch and keep wow-bot-specific lifecycle and character-creation behavior inside wow-bot.

## Motivation

The project should not carry a miniature fork of Tentacli for behavior that belongs to wow-bot control state, diagnostics, and network lifecycle handling. Intentional shutdown classification should use wow-bot cancellation state rather than a custom upstream error type.
