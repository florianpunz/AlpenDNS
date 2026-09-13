# ADR-0022: Glas als Fläche

**Status:** angenommen · **Datum:** 2026-09-13 · **Betrifft:**
[ROADMAP.md](../ROADMAP.md) Phase 6, `web/` (index.html, app.css, app.js),
`crates/alpendns/src/api/ui.rs`, die Web-UI-Regeln in CLAUDE.md B.6

## Kontext

B.6 hat Glassmorphism ausdrücklich verboten — in einer Reihe mit Emoji-Icons und
animierten Zahlen: „Keine Verläufe, keine Emoji als Icons, keine animierten
Zahlen-Counter, keine Glassmorphism-Flächen, keine dekorativen Akzente." Das
Verbot stand nicht ohne Grund da. Die Regel dahinter ist, dass die Oberfläche
nichts behauptet, was sie nicht meint: Farbe trägt Bedeutung oder sie steht nicht
da, und eine Fläche ist eine Fläche.

Als Machbarkeitsstudie ist trotzdem eine Vorschau entstanden, die genau das
gegenteilige Material ausprobiert: transluzente Scheiben über einem Hintergrund
aus vier Farbfeldern. Der Blick auf die gerenderte Seite hat die Entscheidung
gedreht — das Material trägt die Oberfläche besser als die bedruckte Fläche, die
es ersetzt, und die drei Fragen aus B.6 stehen unverändert gleichzeitig da.

Damit steht die Regel gegen das Ergebnis. Dieser ADR hält fest, warum die Regel
nachgibt und an welcher Stelle sie das **nicht** tut.

## Entscheidung

**Der Hintergrund darf Farbe ohne Bedeutung tragen — und ist die einzige Stelle,
an der das erlaubt ist.**

Der Himmel besteht aus vier Farbfeldern (`--field-1` bis `--field-4`: Himmel,
Alpenglühen, Wiese, Schatten) über einer Grundfarbe `--base`, alle tief
entsättigt. Ihre Deckkraft ist der empfindlichste Wert der Datei: höher, und es
ist Dekoration; niedriger, und Glas ist ein graues Rechteck. Die Felder liegen
unter der Textschwelle, wiederholen keinen Zustand, und die Seite bleibt
vollständig lesbar, wenn man sie nicht bemerkt.

Alles andere aus B.6 bleibt unangetastet: die vier semantischen Farben im Inhalt
plus `--brand`, die Graustufenrampe für gestapelte Flächen, der zentrierte
Container, die 8er-Abstände, die eine Sorte Container. Sie ist jetzt eine
Glaskante — transluzente Fläche, `backdrop-filter`, Lichtkante an der Oberkante,
weicher Schatten — mit 18px statt 12px Radius, weil Glas weicher bricht. Sie wird
weiterhin nicht verschachtelt.

Drei Einzelheiten, die dazugehören:

1. **`--glass-strong` gibt es nur einmal.** Der Tabellenkopf ist die einzige
   Fläche, unter der sich etwas bewegt. Über laufendem Inhalt muss mehr decken,
   sonst liest man zwei Texte übereinander; über einem festen Hintergrund wäre
   dieselbe Deckkraft Verschwendung.
2. **Der Modus steht als `data-theme` am Wurzelelement, nicht in einer
   Media-Query.** Ein Umschalter in der Kopfzeile (Sonne im hellen, Mond im
   dunklen Modus) kann den Systemwunsch nur überstimmen, wenn der Wunsch und die
   Wahl zwei verschiedene Dinge sind. Ein kurzes Skript im Kopf der Seite setzt
   das Attribut vor dem ersten Zeichnen — stünde die Wahl erst am Ende des
   Dokuments, blitzte bei jedem Laden kurz das helle Thema auf.
3. **Der Kontrast ist nachgerechnet.** `the_text_stays_readable_on_every_field`
   prüft jede Textfarbe gegen jedes Feld in beiden Modi, durch das Glas, mit
   4,5:1 als Grenze. B.6 sagte bisher „Farbe ist ausschließlich semantisch" als
   Vorsatz; jetzt steht dort eine Rechnung.

Was B.6 an dieser Stelle verliert, ist das Verbot von Verläufen und
Glasflächen. Was es behält, sind Emoji-Icons, animierte Zähler und dekorative
Akzente — und die Kontrastgrenze kommt als prüfbare Regel hinzu.

## Konsequenzen

* **Der Test hat beim ersten Lauf einen echten Fehler gefunden.** `--faint` kam
  auf dem nackten Himmel auf 3,2:1; ihn so weit abzudunkeln, dass es reicht,
  hätte ihn mit `--muted` verschmelzen lassen. Die Auflösung ist keine dunklere
  Schrift, sondern ein Grund: **jede Fläche mit Text ist eine Scheibe.** Betroffen
  waren nur die beiden Stellen außerhalb des Rasters — die Anmeldeseite und der
  `noscript`-Hinweis. Der Test `no_text_sits_on_the_bare_sky` hält das fest.
* **Zwei Tests ändern sich, beide aus demselben Grund.** `nothing_decorative_crept_in`
  (verlief Verläufe und `backdrop-filter`) wird durch die Kontrastrechnung ersetzt.
  `motion_is_reduced_on_request` prüfte auf `@media (prefers-color-scheme: dark)`
  und prüft jetzt auf `:root[data-theme="dark"]`. Alles andere blieb grün,
  einschließlich der Palette (`--danger` und die drei anderen je genau zweimal
  definiert) und der Regel, dass Farbe nur in Selektoren mit Bedeutung steht.
* **Kein Korn.** Die Vorschau hatte ein feines Rauschen über dem Himmel, weil
  große Verläufe auf 8-Bit-Displays sichtbar bänderten. Nachgemessen am
  gerenderten Bild: die Farbstufen liegen bei 1/255 über 5 bis 28 px, also am
  Anschlag dessen, was 8 Bit hergeben. Korn würde daran nichts ändern, nur
  Textur hinzufügen — und ein SVG-Data-URI hätte `http://` in die CSS gebracht,
  was `the_page_references_nothing_from_outside` zu Recht verbietet. Der
  `--grain`-Token ist entfallen.
* **`prefers-reduced-transparency: reduce` ersetzt den Blur durch eine deckende
  Fläche.** Der Blur ist der teuerste Teil der Seite; lesbar bleibt es, nur ohne
  Material.
* **Die Vorschau-Seite ist gelöscht** (`web/preview.html`). Sie war eine
  Machbarkeitsstudie; ihr Inhalt steht jetzt in der echten UI, und zwei Seiten
  mit derselben Gestaltung wären zwei Seiten, die auseinanderlaufen.
* Datenquellen, Endpunkte und die Struktur der Seite blieben unverändert. Die
  Arbeit fand in `web/` und in den UI-Tests statt; kein Rust außerhalb von
  `#[cfg(test)]` wurde angefasst.

## Alternativen

* **Neumorphism statt Glassmorphism.** Geprüft und verworfen: Neumorphismus
  braucht einen gleichmäßigen Untergrund, um seine zwei Schatten zu lesen. Mit
  einem Hintergrund, der etwas zu brechen hat, fällt der Effekt entweder weg oder
  er wird zur Kante. Dazu kommt der Kontrast — weiche Schatten auf einer Fläche
  nahe der Grundfarbe sind im dunklen Modus kaum noch unterscheidbar.
* **Die flache Gestaltung behalten und nur den Himmel ergänzen.** Hätte die
  Farbregel nicht angetastet. Ohne das Glas wäre der Himmel aber Tapete: er wäre
  nur dort zu sehen, wo keine Fläche liegt, und die Panels blieben weiße Blätter
  darauf.
* **`light-dark()` statt `data-theme`.** Jede Farbe stünde genau einmal da, und
  `color-scheme` allein würde umschalten. Verworfen, weil `:root { --danger:
  light-dark(a, b) }` die Palette auf **eine** Definition bringt, während
  `the_palette_is_semantic_and_exists_in_both_schemes` auf zwei besteht — und
  weil die Regel „hell und dunkel sind gleichwertig" dann nur noch behauptet
  wäre: man sieht die beiden Werte nicht mehr nebeneinander.
* **Drei Blöcke: hell, `prefers-color-scheme` und `[data-theme]`.** Der Preis
  wäre, dass die dunklen Werte zweimal dastehen und beim nächsten Anfassen
  auseinanderlaufen. Genau die Sorte Dopplung, für die es den Test gibt.
* **Das Korn per SVG-Data-URI.** Siehe oben: `xmlns='http://www.w3.org/2000/svg'`
  in der CSS. Die Prüfung auf fremde Herkunft ist zu wertvoll, um sie für eine
  Textur aufzuweichen, die messbar nichts repariert.
* **Der Umschalter als Textknopf (hell/dunkel/automatisch)**, wie in der
  Vorschau. Verworfen zugunsten eines Zeichens: die Sonne sagt ohne Klick, in
  welchem Modus man ist, und der Mond zeigt, wohin der nächste führt. Ein dritter
  Zustand „automatisch" wäre eine Einstellung, die niemand einstellt.
