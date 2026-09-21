#!/usr/bin/env bash
# Baut docs/images/ui-card.png aus docs/images/ui.png: abgerundete Ecken und
# ein weicher Schatten.
#
# Warum eingebrannt und nicht per CSS? GitHub schickt jedes README durch einen
# Sanitizer, der `style` (und `class`/`id`) entfernt — in einem README gibt es
# also kein border-radius und keine box-shadow. Was nicht im Pixel steht, steht
# nirgends. Nebeneffekt: die Karte sieht in jedem Renderer gleich aus, nicht nur
# auf GitHub.
#
# Aufruf: scripts/make-readme-images.sh   (braucht ImageMagick 6)
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
src="$root/docs/images/ui.png"
out="$root/docs/images/ui-card.png"

# Der Radius ist auf 1600 px Aufnahmebreite bemessen — auf GitHub rund 14 px
# sichtbar. Er wächst mit der Breite mit: sonst sähe dieselbe Karte bei einer
# breiteren Aufnahme eckiger aus als bei einer schmaleren.
radius_at_1600=28
pad=80              # Platz, in den der Schatten fallen kann
shadow="55x24+0+14" # Weichzeichner x Größe + Versatz nach unten

w=$(identify -format '%w' "$src")
h=$(identify -format '%h' "$src")
radius=$((w * radius_at_1600 / 1600))

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# Weißes abgerundetes Rechteck als Maske, dann als Alpha auf das Bild legen.
convert -size "${w}x${h}" xc:none -fill white \
  -draw "roundrectangle 0,0,$((w - 1)),$((h - 1)),$radius,$radius" "$tmp/mask.png"
convert "$src" "$tmp/mask.png" -alpha off -compose CopyOpacity -composite "$tmp/rounded.png"

# Schatten aus dem Alpha-Kanal, darunterlegen, transparenter Rand herum.
convert "$tmp/rounded.png" \( +clone -background black -shadow "$shadow" \) \
  +swap -background none -layers merge +repage \
  -bordercolor none -border "$pad" "$out"

echo "$out  $(identify -format '%wx%h' "$out")"
