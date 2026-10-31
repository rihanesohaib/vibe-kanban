# db

SQLx models and SQLite migrations for the local server.

## Migrations

Migrations live in `migrations/` and are embedded with `sqlx::migrate!`. SQLx
records each applied migration's version and checksum, so two rules apply:

- **Every version is unique.** The version is the numeric filename prefix
  (`20260101120000_add_foo.sql` has version `20260101120000`). Two files with
  the same version collide: SQLx tracks migrations by version, so one gets
  skipped on some databases or startup fails with a checksum mismatch.
- **Released migrations are immutable.** Editing a migration that has already
  run on a user's database makes SQLx refuse to start with a checksum mismatch.
  Fix mistakes with a new migration instead.

`scripts/check-migrations.sh` enforces both rules in CI (`backend-schema`
job, also run by `./scripts/ci-local.sh backend`). It checks every file in
`migrations/` against the append-only manifest `migrations.sha256`, which
lists the SHA-256 checksum of each released migration. The check fails if:

- two migrations share a version, or a filename is not `<version>_<description>.sql`;
- a migration listed in the manifest was edited, renamed or deleted;
- a migration exists on disk but is not listed in the manifest.

### Adding a new migration

1. Create the file with a fresh UTC timestamp version, e.g.
   `sqlx migrate add <description>` from `crates/db`, or by hand:
   `crates/db/migrations/$(date -u +%Y%m%d%H%M%S)_<description>.sql`.
2. Write the SQL, then run `pnpm run prepare-db` to refresh the SQLx offline
   query data.
3. Append its checksum to the manifest:

   ```bash
   ./scripts/check-migrations.sh --update
   ```

   This only appends lines for new files; it never rewrites existing entries.
4. Commit the migration and the updated `crates/db/migrations.sha256` together.

While a migration is still unreleased on your branch you may keep editing it;
just remove its line from `migrations.sha256` and re-run `--update`. Once it is
merged, treat it as frozen. If another branch lands a migration with the same
version first, rename yours to a new timestamp before merging.
