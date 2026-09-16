#!/bin/sh
#
# Baut die öffentliche Demo: eine einzelne HTML-Datei, die auf einen beliebigen
# Webserver gelegt werden kann — kein Rust-Prozess, kein Port, kein Token, kein
# TLS.
#
# Das Verfahren ist bewusst klein: die drei Dateien, die der Server ausliefert
# (web/index.html, web/app.css, web/app.js), werden in eine Datei gesetzt, dazu
# die Demo-Schicht aus web/demo/demo.js. Die Demo *ist* damit die echte
# Oberfläche — sie kann nicht anders aussehen als das Original, weil sie das
# Original enthält. Eigen ist ihr nur die Datenquelle.
#
# Deshalb cat und nicht sed: cat kopiert Bytes. Es gibt keine Ersetzungs- und
# keine Escaping-Frage zu beantworten. Die beiden Anker werden als gequotete
# case-Muster erkannt — ein Literal, kein regulärer Ausdruck.
#
# Aufruf:  sh web/demo/build.sh
# Ergebnis: web/dist/alpendns-demo.html  (steht in .gitignore)
#
# Exit 1 mit Klartext, sobald eine der Prüfungen unten nicht aufgeht. Ein Build,
# der still eine Seite ohne Stylesheet erzeugt, ist schlimmer als ein lauter
# Abbruch.

set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH= cd -- "$here/../.." && pwd)

src="$root/web/index.html"
css="$root/web/app.css"
js="$root/web/app.js"
demo="$here/demo.js"
out="$root/web/dist/alpendns-demo.html"

fail() {
  printf '\nbuild.sh: %s\n' "$1" >&2
  exit 1
}

for file in "$src" "$css" "$js" "$demo"; do
  [ -f "$file" ] || fail "fehlt: $file"
done

mkdir -p "$(dirname -- "$out")"
tmp=$(mktemp "$out.XXXXXX")
# Damit nie eine halbfertige Demo liegen bleibt — weder beim Abbruch noch beim
# Abbruch durch ein Signal. Nach dem mv unten ist $tmp weg und das rm folgenlos.
trap 'rm -f -- "$tmp"' EXIT INT TERM

# ── 1. Setzen ────────────────────────────────────────────────────────────────
#
# Die Schleife liest von Filedeskriptor 3 und schreibt in eine eigene Datei,
# nicht in eine Pipe: `cat … | while` liefe in einer Subshell, und die Zähler
# unten blieben stumm auf null stehen. printf statt echo, weil dash in echo ein
# \n als Steuerzeichen liest. `|| [ -n "$line" ]` fängt eine letzte Zeile ohne
# Schlussumbruch ab, die read sonst verwerfen würde.

count_css=0
count_js=0

{
  while IFS= read -r line <&3 || [ -n "$line" ]; do
    case $line in
      *'href="/app.css"'*)
        count_css=$((count_css + 1))
        printf '<style>\n'
        cat -- "$css"
        printf '</style>\n'
        ;;
      *'src="/app.js"'*)
        count_js=$((count_js + 1))
        # Erst die Demo-Schicht, dann die Oberfläche: demo.js ersetzt fetch und
        # EventSource, und app.js ruft beides beim Laden auf.
        printf '<script>\n'
        cat -- "$demo"
        printf '</script>\n'
        printf '<script>\n'
        cat -- "$js"
        printf '</script>\n'
        ;;
      *)
        printf '%s\n' "$line"
        ;;
    esac
  done 3< "$src"
} > "$tmp"

# ── 2. Anker ─────────────────────────────────────────────────────────────────

[ "$count_css" -eq 1 ] || fail "href=\"/app.css\" $count_css× gefunden, genau 1× erwartet"
[ "$count_js" -eq 1 ] || fail "src=\"/app.js\" $count_js× gefunden, genau 1× erwartet"

# Das Setzen ist ein reines Nebeneinanderstellen. Enthielte eine der Dateien die
# schließende Marke, endete der Block dort und der Rest stünde als Text auf der
# Seite — eine Aussage, die man dem Erzeugnis nicht ansieht.
check_inline() {
  if grep -q -- "$2" "$1"; then
    fail "$(basename -- "$1") enthält '$2' und lässt sich nicht einsetzen"
  fi
}

check_inline "$css" '</style'
check_inline "$js" '</script'
check_inline "$demo" '</script'

# ── 3. Kein Verweis nach außen ───────────────────────────────────────────────
#
# Dieselbe Zusicherung, die in crates/alpendns/src/api/ui.rs als Test steht:
# die Seite lädt nichts nach. Ein Verweis, der hier auftaucht, ist entweder ein
# neuer Anker — dann gehört er oben mitbehandelt — oder eine fremde Ressource,
# und die darf es nicht geben (B.6).

if external=$(grep -nE 'src="|href="|https?://' "$tmp"); then
  printf '%s\n' "$external" >&2
  fail "das Erzeugnis verweist nach außen (siehe oben)"
fi

# ── 4. Drift ─────────────────────────────────────────────────────────────────
#
# Der Ersatz für den Test, den es hier bewusst nicht gibt: wenn app.js einen
# Endpunkt dazubekommt, den die Demo nicht bedient, wird dieser Build rot statt
# die veröffentlichte Seite still unvollständig. Genau das hat preview.html
# gefehlt.
#
# Einseitig geprüft: jeder Pfad, den app.js abruft, muss bedient werden. Umgekehrt
# darf demo.js mehr kennen — GET /api/allow etwa, das nur die Knöpfe auslösen.
#
# In app.js stehen die Pfade gequotet, in seinen Kommentaren dagegen nackt. Die
# Anführungszeichen trennen deshalb Code von Prosa.

used=$(grep -oE '["`]/api/[a-z]+' "$js" | cut -c2- | sort -u)
# In demo.js sind es die Beschriftungen der Fallunterscheidung. Dazu der
# Live-Strom, der als einziger nicht über fetch läuft und deshalb kein Label hat.
# Das `|| :` an beiden Greps: findet einer nichts, gäbe er 1 zurück, und unter
# `set -eu` wäre das ein Abbruch mitten in der Zuweisung.
served=$(
  {
    grep -oE '"(GET|POST) /api/[a-z]+"' "$demo" | cut -d' ' -f2 | tr -d '"' || :
    grep -oE '"/api/[a-z]+"' "$demo" | tr -d '"' || :
  } | sort -u
)

[ -n "$used" ] || fail "keine /api/-Pfade in app.js gefunden — stimmt der Pfad?"

for path in $used; do
  # Ganze Zeile, literal verglichen: die Pfade enthalten Schrägstriche, und ein
  # Muster wäre hier nur eine Gelegenheit, sich zu vertun. Über eine zeilenweise
  # Liste ginge auch ein case-Muster — nur eben nicht, weil die Trenner Zeilen
  # und keine Leerzeichen sind.
  if ! printf '%s\n' "$served" | grep -qxF -- "$path"; then
    fail "app.js ruft $path ab, demo.js bedient ihn nicht"
  fi
done

# ── 5. Ablegen ───────────────────────────────────────────────────────────────

bytes=$(wc -c < "$tmp" | tr -d ' ')
mv -- "$tmp" "$out"

printf 'web/dist/alpendns-demo.html  %s Bytes\n\n' "$bytes"
printf 'Aus welchen Ständen sie gebaut ist:\n'
for file in "$src" "$css" "$js" "$demo"; do
  printf '  %s  %s\n' "$(sha256sum -- "$file" | cut -d' ' -f1)" "${file#"$root/"}"
done
printf '\nBediente Endpunkte: %s\n' "$(printf '%s' "$served" | tr '\n' ' ')"
