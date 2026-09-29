# Design: scoped SQL memory clearing

The existing TOML `[memory]` section supplies `database_url_env` and `required`; the current loader ignored both. The loader now retains these fields. It also retains `[debug].enabled`, which is the existing explicit gate for destructive memory clearing. The command is intercepted only from the issuing configured account's client stream. Transparent/unconfigured sessions cannot invoke local bot commands.

The roster `bots[].id` is retained as `AccountConfig.bot_id` and passed to the proxy lane. `.bot forget` uses that stable bot ID, not an account-wide wildcard or a character name. If a roster entry is absent, the account name is the deterministic fallback identity.

The optional SQL operation uses the environment variable name from configuration and does not put the database URL in the runtime config or logs. It requires verified identity TLS for non-loopback MySQL hosts. It deletes rows from the eleven legacy memory tables inside one transaction with a bound `bot_id` parameter. A missing environment variable, unavailable database, or failed table delete returns an error and does not report success. Tests validate configuration, identity scoping, command authorization, and query construction without connecting to a database.
