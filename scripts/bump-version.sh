#!/usr/bin/env bash
# Zählt die Version in Cargo.toml und Cargo.lock eine Stufe hoch — vor dem Push,
# nicht von der CI. Warum nicht nachträglich per Bot? Ein Bot-Commit nach jedem
# echten Commit würde main vor jedem Hand-Push "überholen" und jeden nächsten
# Push als non-fast-forward ablehnen lassen.
#
# Aufruf: scripts/bump-version.sh major|minor|patch
#   major = etwas ist neu dazugekommen, oder etwas Bestehendes bricht
#   minor = etwas Bestehendes wird repariert oder angepasst
#   patch = Tippfehler, Rechtschreibung, einzelne Sätze in der Doku
#
# Erhöht wird einmal je Release, nicht je Commit: die Stufe richtet sich nach
# dem, was seit dem letzten Tag insgesamt zusammengekommen ist, und die
# schwerste Art gewinnt. Die veröffentlichte Nummer beschreibt damit den
# Release — was ein Leser seit dem vorigen zu erwarten hat — und nicht den
# letzten Commit. Die Stufen und ihre Beispiele: docs/OPERATIONS.md.
set -euo pipefail

bump="${1:-}"
case "$bump" in
  major|minor|patch) ;;
  *) echo "usage: $0 major|minor|patch" >&2; exit 1 ;;
esac

current="$(sed -nE 's/^version = "([0-9]+\.[0-9]+\.[0-9]+)"$/\1/p' Cargo.toml | head -n1)"
if [ -z "$current" ]; then
  echo "no version found in Cargo.toml" >&2
  exit 1
fi

IFS=. read -r major minor patch <<< "$current"
if [ "$bump" = major ]; then
  major=$((major + 1)); minor=0; patch=0
elif [ "$bump" = minor ]; then
  minor=$((minor + 1)); patch=0
else
  patch=$((patch + 1))
fi
new="$major.$minor.$patch"

# Cargo.toml: die einzige Zeile der Form `version = "…"` steht unter
# [workspace.package]; Abhängigkeiten führen ihre Version inline (`foo = { version = … }`).
sed -i -E 's/^(version = ")[0-9]+\.[0-9]+\.[0-9]+(")$/\1'"$new"'\2/' Cargo.toml

# Cargo.lock: nur der Block des Pakets `alpendns`, nicht das Lockfile-Format
# (`version = 4` ganz oben, ohne Anführungszeichen).
sed -i -E '/^name = "alpendns"$/,/^$/ s/^(version = ")[0-9]+\.[0-9]+\.[0-9]+(")$/\1'"$new"'\2/' Cargo.lock

echo "$new"
