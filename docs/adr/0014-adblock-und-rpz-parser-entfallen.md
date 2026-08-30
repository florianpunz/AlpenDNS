# ADR-0014: Die Parser für Adblock-Syntax und RPZ entfallen

**Status:** angenommen · **Datum:** 2026-08-30

## Kontext

`blocklist.format` kannte fünf Werte. Zwei davon waren ausdrücklich Teilmengen
eines fremden Formats, und beide Teilmengen hatten denselben Konstruktionsfehler.

**Adblock.** Unterstützt war `||name^`. Übersprungen wurde alles andere,
darunter `@@||name^` — die *Ausnahmeregel*. Solche Zeilen stehen in
Adblock-Listen aus genau einem Grund: eine vorherige Blockregel nimmt eine
Domain mit, die nicht gemeint war, und die Ausnahme holt sie zurück. Wer eine
solche Liste in AlpenDNS lud, bekam die Blockregeln **ohne** die dazugehörigen
Korrekturen. Übersprungen wurden außerdem Regeln mit `$`-Optionen, was in die
andere Richtung wirkt — zusammen ergibt das eine Liste, deren Wirkung mit der
Absicht ihres Autors nichts mehr zu tun hat.

**RPZ.** Erkannt war `<name> [ttl] [klasse] CNAME .`, die NXDOMAIN-Regel aus
RFC 8611 §2.1. Übersprungen wurden `rpz-passthru` (wieder: die Ausnahme),
`rpz-drop`, `rpz-tcp-only` sowie sämtliche Trigger, die nicht am Namen hängen
(`rpz-client-ip`, `rpz-ip`, `rpz-nsdname`, `rpz-nsip`). Dazu kommt, dass RPZ als
Zonentransfer verteilt wird, nicht als Textdatei über HTTPS — das Format ist für
einen Weg gebaut, den AlpenDNS nicht geht.

Beides fällt unter denselben Satz: **eine Liste wurde geladen und war dabei
schärfer als gemeint.** Das ist die unangenehmste Sorte Fehler, weil sie nach
Erfolg aussieht. Der `skipped`-Zähler stand im Log, aber er ist bei diesen
Formaten *erwartbar* ungleich null — er unterscheidet nicht zwischen "Kommentare
und Element-Filter, kein Problem" und "vierzig Ausnahmen verworfen, deine
Bankseite ist jetzt geblockt".

Der Auftrag lautete: entweder die volle relevante Syntax unterstützen oder
entfernen.

## Entscheidung

Beide Parser werden **entfernt**. Es bleiben `hosts`, `domains` und `wildcard`.

**Warum nicht die volle Syntax?** Weil "voll" bei beiden Formaten bedeutet,
*Ausnahmen* zu unterstützen, und das ist kein Parser-Thema. Ein `Entry` müsste
eine Polarität tragen, der `Matcher` müsste bei einem Treffer entscheiden, ob
Block oder Ausnahme gewinnt (und dazu die spezifischere Regel finden statt der
erstbesten), und die Auswertungsreihenfolge in `policy::Engine` — heute
Allowlist vor Blocklisten — müsste eine zweite, listeninterne Ebene bekommen.
Das ist ein Eingriff in den heißen Pfad und in den Trace, in keiner Phase der
Roadmap vorgesehen, und er baut ein zweites Ausnahmesystem neben die
Allowlisten, die es schon gibt und die genau dafür da sind. Bei einer Aufgabe,
deren Ziel es ist, Fläche zu verkleinern, ist das die falsche Richtung.

Dazu kommt die Nutzenseite: `||name^` ist semantisch identisch mit einer Zeile
`name` in einer `wildcard`-Liste. Wer eine Adblock-Liste benutzen will, deren
Betreiber keine Wildcard-Fassung anbietet, konvertiert sie mit einem
`sed`-Einzeiler — und sieht dabei selbst, was er mit den `@@`-Zeilen macht. Das
ist ehrlicher als ein Parser, der diese Entscheidung stillschweigend trifft.

Beide Namen bleiben in der Konfiguration erkannt: `Format` bekommt ein
handgeschriebenes `Deserialize`, das den Grund nennt und auf `wildcard` bzw.
`hosts` verweist.

## Konsequenzen

* Der Parser schrumpft um rund 130 Zeilen und zwei Formate, für die es keinen
  Eintrag in `config/alpendns.example.toml` gab.
* `format = "adblock"` oder `"rpz"` ist ein Startfehler mit Begründung, kein
  stilles Ignorieren (B.1 Regel 5).
* **Was verloren geht:** Listen, die es nur in Adblock-Syntax gibt, sind ohne
  Vorverarbeitung nicht mehr nutzbar. Das ist der Preis, und er ist bewusst
  gezahlt: nicht lesbar ist besser als falsch gelesen.
* `!` am Zeilenanfang gilt weiter als Kommentar. Das kam aus der Adblock-Welt,
  ist aber inzwischen in heruntergeladenen Listen jeder Art üblich, und
  Verhalten zu ändern, nach dem niemand gefragt hat, gehört nicht in diesen
  Commit.

## Alternativen

* **Volle Adblock-Syntax mit Ausnahmen.** Siehe oben: Matcher mit Polarität,
  neue Auswertungsreihenfolge, zweites Ausnahmesystem. Wenn das je gebaut wird,
  dann als eigenes Feature mit eigener Phase — und dann für *ein* Format, nicht
  für zwei.
* **Nur RPZ entfernen, Adblock behalten.** Der Auftrag ließ das offen. Dagegen
  spricht, dass beide denselben Fehler haben; nur einen davon zu beheben, hieße
  den anderen bewusst stehen zu lassen.
* **Die Teilmenge behalten und beim Laden lauter warnen** (etwa: Fehler, wenn
  mehr als *x* Prozent der Zeilen übersprungen wurden). Löst das Problem nicht,
  sondern verschiebt es auf einen Schwellwert, den niemand begründen kann — und
  wäre wieder ein Konfigurationsschlüssel.
