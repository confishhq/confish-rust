# Changelog

## 0.2.0 - 2026-07-09

Coordinated minor across all five confish SDKs: the new feeds resource,
plus a one-time pre-adoption reshuffle of the existing surface. The
surface is treated as informally frozen after this release.

### Added

- **Feeds.** `client.feed(slug)` returns a bound handle (no HTTP on
  construction) with:
  - `set(external_id, data, ttl)` — declarative upsert (PUT). `ttl` is an
    `Option<Duration>` sent as whole seconds; passing `None` makes the
    item permanent and clears any existing TTL.
  - `list::<T>()` — live items, newest first, as `Vec<FeedItem<T>>`.
  - `delete(external_id)` — idempotent.
  - `replace(items)` — declarative whole-partition replace (collection
    PUT), built for sync-style cron jobs pushing their full dataset in
    one request. The feed becomes exactly the given `&[FeedItemInput<V>]`
    (per-item optional TTL): existing IDs update in place, new IDs are
    created, absent items are deleted, and an empty slice clears the
    feed. All-or-nothing on validation failure. Returns
    `FeedReplaceResult { created, updated, deleted }`.
- `FeedItem<T>` type with `id`, `external_id`, `data`, `expires_at`,
  `created_at`, `updated_at`; `FeedItemInput<V>` and `FeedReplaceResult`
  for `replace`.
- `Error::NotFound` for 404 responses (e.g. an unknown feed slug), which
  previously fell through to `Error::Api`.
- `LogLevel::Emergency` and `logs().emergency(...)` — the level set now
  covers all of RFC 5424.
- `logs().write(level, message, context)` for logging with an explicit
  level.

### Changed (breaking)

- Config methods moved off the client root onto a namespace handle:
  `client.fetch()` / `client.update()` / `client.replace()` are now
  `client.config().fetch()` / `.update()` / `.replace()`. Signatures are
  unchanged.
- `client.logger()` renamed to `client.logs()`; the flat `client.log(...)`
  was removed — use `client.logs().write(...)`. The `Logger<'_>` type is
  now the owned, cloneable `Logs` handle.
- `webhook::verify` now parses and verifies in one operation: it returns
  `Result<Payload, WebhookError>` instead of `bool`. `WebhookError`
  distinguishes `InvalidSignature` from `TimestampOutsideTolerance` (plus
  `MissingSignature`, `MalformedSignature`, and `InvalidPayload`).
- `actions().update(...)` renamed to `actions().progress(...)`, and
  `ActionContext::update` to `ActionContext::progress` (same wire call —
  it appends a progress note to the action's timeline).

## 0.1.0 - 2026-05-01

- Initial release: typed configuration (`fetch`/`update`/`replace`),
  logging, actions with a long-running consumer, and webhook signature
  verification.
