# Design: `.bot clear`

The parser returns a typed clear command with a fresh mission ID. The proxy routes an idle mission to the command's configured lane and then uses the existing BotOff session transition. Installing idle first cancels mission-scoped work; stopping bot control then hands control back under the current ownership rules.

The command uses the same configured-account and supported-chat-family gates as other local `.bot` commands. It does not change other lanes. SQL-backed memory clearing is outside this change because the current runtime has no SQL memory store or database configuration.
