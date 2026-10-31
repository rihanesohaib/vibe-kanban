#!/usr/bin/env bash
# Guards the SQLx migrations in crates/db/migrations:
#   1. No two migrations may share a version number (the numeric filename prefix).
#   2. Released migrations are immutable: every file listed in
#      crates/db/migrations.sha256 must exist with an unchanged SHA-256.
#   3. Every migration on disk must be listed in the manifest.
#
# Usage:
#   ./scripts/check-migrations.sh            # verify (used by CI)
#   ./scripts/check-migrations.sh --update   # append new migrations to the manifest
#
# --update only appends entries for files not yet in the manifest; it never
# rewrites or removes existing entries. See crates/db/README.md.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MIGRATIONS_DIR="$REPO_ROOT/crates/db/migrations"
MANIFEST="$REPO_ROOT/crates/db/migrations.sha256"
MANIFEST_REL="crates/db/migrations.sha256"

mode="check"
case "${1:-}" in
  "") ;;
  --update) mode="update" ;;
  -h|--help) sed -n '2,13p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
  *) echo "Unknown argument: $1 (expected --update)" >&2; exit 2 ;;
esac

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

echo "▶️  Checking database migrations..."

errors=0
fail() {
  echo "❌ $*" >&2
  errors=$((errors + 1))
}

if [ ! -d "$MIGRATIONS_DIR" ]; then
  echo "❌ Missing migrations directory: $MIGRATIONS_DIR" >&2
  exit 1
fi

migrations=()
while IFS= read -r name; do
  migrations+=("$name")
done < <(find "$MIGRATIONS_DIR" -mindepth 1 -maxdepth 1 -name '*.sql' -exec basename {} \; | LC_ALL=C sort)

# --- 1. Filename format and duplicate versions -------------------------------
versions=()
for name in "${migrations[@]}"; do
  if [[ ! "$name" =~ ^([0-9]+)_[A-Za-z0-9_-]+\.sql$ ]]; then
    fail "Invalid migration filename '$name' (expected <version>_<description>.sql, e.g. 20260101120000_add_foo.sql)"
    continue
  fi
  versions+=("${BASH_REMATCH[1]}")
done

duplicate_versions="$(printf '%s\n' "${versions[@]:-}" | sed '/^$/d' | LC_ALL=C sort | uniq -d)"
if [ -n "$duplicate_versions" ]; then
  while IFS= read -r version; do
    fail "Duplicate migration version $version used by:"
    for name in "${migrations[@]}"; do
      [[ "$name" == "${version}_"* ]] && echo "     crates/db/migrations/$name" >&2
    done
  done <<< "$duplicate_versions"
  echo "   Rename the newer migration to a fresh, unique timestamp version." >&2
fi

# --- 2. Released migrations are unchanged ------------------------------------
if [ ! -f "$MANIFEST" ]; then
  echo "❌ Missing manifest: $MANIFEST_REL" >&2
  exit 1
fi

# Newline-delimited list of manifest entries (bash 3 has no associative arrays).
listed=""
is_listed() { printf '%s\n' "$listed" | grep -Fxq -- "$1"; }

line_no=0
while IFS= read -r line || [ -n "$line" ]; do
  line_no=$((line_no + 1))
  [[ -z "${line//[[:space:]]/}" || "$line" == \#* ]] && continue
  if [[ ! "$line" =~ ^([0-9a-f]{64})\ \ ([^/[:space:]]+\.sql)$ ]]; then
    fail "$MANIFEST_REL:$line_no: malformed line (expected '<sha256>  <file>.sql')"
    continue
  fi
  expected="${BASH_REMATCH[1]}"
  name="${BASH_REMATCH[2]}"
  if is_listed "$name"; then
    fail "$MANIFEST_REL:$line_no: '$name' is listed more than once"
    continue
  fi
  listed="$listed$name"$'\n'

  path="$MIGRATIONS_DIR/$name"
  if [ ! -f "$path" ]; then
    fail "Released migration crates/db/migrations/$name was deleted or renamed."
    echo "   Released migrations must never be removed; restore it from git." >&2
    continue
  fi
  actual="$(sha256_of "$path")"
  if [ "$actual" != "$expected" ]; then
    fail "Released migration crates/db/migrations/$name has been modified."
    echo "   expected sha256: $expected" >&2
    echo "   actual sha256:   $actual" >&2
    echo "   Revert the edit and add a new migration with the change instead." >&2
  fi
done < "$MANIFEST"

# --- 3. Every migration is listed --------------------------------------------
unlisted=()
for name in "${migrations[@]}"; do
  is_listed "$name" || unlisted+=("$name")
done

if [ ${#unlisted[@]} -gt 0 ]; then
  if [ "$mode" = "update" ]; then
    if [ "$errors" -gt 0 ]; then
      echo "❌ Not updating $MANIFEST_REL until the errors above are fixed." >&2
      exit 1
    fi
    # Ensure the manifest ends with a newline before appending.
    if [ -s "$MANIFEST" ] && [ -n "$(tail -c1 "$MANIFEST")" ]; then
      echo >> "$MANIFEST"
    fi
    for name in "${unlisted[@]}"; do
      printf '%s  %s\n' "$(sha256_of "$MIGRATIONS_DIR/$name")" "$name" >> "$MANIFEST"
      echo "➕ Added $name to $MANIFEST_REL"
    done
  else
    fail "Migrations missing from $MANIFEST_REL:"
    for name in "${unlisted[@]}"; do
      echo "     crates/db/migrations/$name" >&2
    done
    echo "   Run ./scripts/check-migrations.sh --update and commit the manifest." >&2
  fi
fi

if [ "$errors" -gt 0 ]; then
  echo "❌ Migration check failed with $errors error(s)." >&2
  exit 1
fi

echo "✅ ${#migrations[@]} migrations OK (unique versions, released migrations unchanged)."
