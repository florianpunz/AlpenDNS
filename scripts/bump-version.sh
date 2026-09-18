#!/usr/bin/env bash
# Zählt die Version in Cargo.toml und Cargo.lock um eine Stufe hoch — vor dem
# Commit, nicht von der CI. Warum vor dem Commit und nicht nachträglich per Bot?
# Ein Bot-Commit nach jedem echten Commit würde main vor jedem Hand-Push
# "überholen" und jeden nächsten Push als non-fast-forward ablehnen lassen.
#
# Aufruf: scripts/bump-version.sh major|minor
#   major = Breaking Change, minor = alles andere (feat, fix, docs, …).
#   Es gibt kein Patch: jeder Commit veröffentlicht eine neue Version.
set -euo pipefail

bump="${1:-}"
case "$bump" in
  major|minor) ;;
  *) echo "usage: $0 major|minor" >&2; exit 1 ;;
esac

current="$(sed -nE 's/^version = "([0-9]+\.[0-9]+\.[0-9]+)"$/\1/p' Cargo.toml | head -n1)"
if [ -z "$current" ]; then
  echo "keine version in Cargo.toml gefunden" >&2
  exit 1
fi

IFS=. read -r major minor patch <<< "$current"
if [ "$bump" = major ]; then
  major=$((major + 1)); minor=0; patch=0
else
  minor=$((minor + 1)); patch=0
fi
new="$major.$minor.$patch"

# Cargo.toml: die einzige Zeile der Form `version = "…"` steht unter
# [workspace.package]; Abhängigkeiten führen ihre Version inline (`foo = { version = … }`).
sed -i -E 's/^(version = ")[0-9]+\.[0-9]+\.[0-9]+(")$/\1'"$new"'\2/' Cargo.toml

# Cargo.lock: nur der Block des Pakets `alpendns`, nicht das Lockfile-Format
# (`version = 4` ganz oben, ohne Anführungszeichen).
sed -i -E '/^name = "alpendns"$/,/^$/ s/^(version = ")[0-9]+\.[0-9]+\.[0-9]+(")$/\1'"$new"'\2/' Cargo.lock

echo "$new"
