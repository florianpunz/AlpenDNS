# ADR-0024: Config compatibility on renames

**Status:** accepted · **Date:** 2026-10-16 · **Affects:** config parsing, [TODOS.md](../TODOS.md) item 7

## Context

AlpenDNS now builds public releases (phase 9). When a configuration key is
renamed between versions, an existing installation with the old key name
fails to start with what serde says: `unknown field 'x'` — and thanks to
`deny_unknown_fields` (B.1 rule 5) the start fails, which is correct but does
not reveal what to do.

The first renamed option breaks every foreign installation. That is the
problem this ADR addresses.

## Decision

**No automatic migration, no version field in the configuration.**

Instead, two measures that apply on the first rename and after:

1. **`serde(alias)` with a deprecation warning.** The old name stays as an alias
   for at least one major version. When loaded, the alias triggers a
   `tracing::warn!` with the new name. `alias` and `deny_unknown_fields` do
   **not** conflict: an alias is a known field, not an unknown one.

   ```rust
   #[serde(alias = "old_name")]
   pub new_name: String,
   ```

   This ADR records the **pattern**. Nothing is built in advance: how the
   loader notices that the alias was used, and with it the warning, is
   implemented together with the first rename, with a test that loads the old
   name and checks result and warning (TODOS.md item 7, step 2).

2. **Better error messages for typos.** On an unknown key, suggest the most
   similar known one (Levenshtein). serde's error for `deny_unknown_fields`
   already lists the expected fields, so the comparison works on that list;
   `deny_unknown_fields` itself stays (B.1 rule 5). Not built by this ADR
   either — it is TODOS.md item 7, step 1.

## What this means for operators

* Once a key is renamed in a release, the existing configuration keeps working,
  and the server logs a warning on every start naming the new key.
* The old name stays as a deprecated alias for at least one major version.
* A typo in a key name results in a startup error, as before.

## Reversal condition

This ADR disappears when AlpenDNS switches to a different configuration
format or when serde gains native support for deprecation warnings on
aliases — whichever comes first. Neither is on the horizon.

## Alternatives considered

* **A `version` field.** Would require migration logic, and automatic migration
  breaks the Debian conffile contract (TODOS.md item 7c): a program that
  rewrites `/etc/alpendns/alpendns.toml` by itself breaks the conffile
  contract; on the next upgrade `dpkg` asks about local changes that the
  operator never made.
* **Automatic migration on startup.** Same problem: would modify the
  conffile, breaking the Debian contract.
* **Migration via a separate command.** Acceptable (`alpendns migrate`), but
  requires the operator to run it manually. The alias approach is
  transparent and requires no action.
