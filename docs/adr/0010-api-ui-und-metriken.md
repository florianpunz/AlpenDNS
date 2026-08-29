# ADR-0010: Zwei Listener, ein Token, und eine UI ohne Build-Schritt

**Status:** angenommen · **Datum:** 2026-08-29

## Kontext

Phase 6 bringt drei Dinge nach außen, die es vorher nicht gab: eine HTTP-API, die
Query-Namen und Begründungen zeigt; einen Prometheus-Endpunkt; und eine Web-UI.
Jedes davon ist eine neue Angriffsfläche an einem Dienst, dessen Zweck
Zurückhaltung ist.

## Entscheidungen

### Zwei Listener statt einem

Die API läuft auf `api.listen` mit Token, die Metriken auf `metrics.listen` ohne.
Der Grund ist nicht Bequemlichkeit, sondern dass beide verschiedene Dinge
ausliefern:

* Die **API** zeigt Namen, Clients und Begründungsketten. Ohne Token wäre sie ein
  Query-Log mit HTTP-Schnittstelle.
* Die **Metriken** enthalten per Konstruktion keine Namen — Labels gibt es nur für
  Resolver-Namen und RCODEs, beides aus der Konfiguration. Ein Prometheus-Scraper
  schickt keinen Bearer-Header; ihn dazu zu zwingen hieße, die übliche
  Einrichtung zu brechen.

Die Konfigurationsprüfung lehnt es ab, beide auf denselben Port zu legen. Sonst
wäre der tokenlose Endpunkt der Weg an der Authentifizierung vorbei.

**Ein Domainname darf nie ein Prometheus-Label werden.** Prometheus behält jede
Zeitreihe, die es einmal gesehen hat; ein Label mit einer Domain wäre ein
Query-Log mit anderem Dateinamen und ohne Ablauf. Ein Test prüft das.

### Der Token steht für Server-Sent Events in der URL

`EventSource` im Browser kann keine Header setzen. Für `/api/events` ist der Token
deshalb auch als Query-Parameter zulässig. Das ist ein Zugeständnis: er kann so in
einem Proxy-Log landen. Die Alternativen waren schlechter — den Live-Strom ohne
Authentifizierung anzubieten (er zeigt Namen), oder eine Sitzungs-Cookie-Mechanik
einzuführen, die der Server sonst nirgends braucht.

Der Vergleich läuft in konstanter Zeit. Ein Abbruch beim ersten falschen Zeichen
verrät über die Antwortzeit, wie weit ein Rateversuch gekommen ist.

### Der Token wird beim Start erzeugt, wenn er fehlt

Sonst müsste vor dem ersten Start jemand von Hand eine Datei mit Zufallszeichen
anlegen, nur um die UI zu sehen. Die Datei bekommt Rechte 0600 — sie ist das
Passwort.

### Die UI liegt im Binary, ohne Build-Schritt

Drei Dateien (`web/index.html`, `web/app.css`, `web/app.js`), über `include_str!`
einkompiliert. Kein npm, kein Bundler, kein Verzeichnis, das zur Laufzeit da sein
muss — und keine Möglichkeit, dass die UI zu einer anderen Version gehört als der
Server, der sie ausliefert.

Ein Test prüft, dass keine der drei Dateien auf eine fremde Herkunft verweist:
keine CDN-Skripte, keine externen Schriften, kein `@import`. Der Fehler wäre sonst
genau der, den niemand bemerkt, solange er selbst online ist.

Kein Framework. Die Seite zeigt vier Zahlen, zwei Tabellen und eine Liste; dafür
reicht `textContent` und ein `EventSource`. Fremde Daten werden nie als
Auszeichnung eingefügt — ein Domainname aus dem Netz ist Text, kein Markup.

### Das Prometheus-Format wird von Hand erzeugt

Eine Metrik-Bibliothek würde eine Registry einführen, in der jeder Wert ein
zweites Mal geführt wird. Die Zähler liegen schon in den Strukturen, die sie
hochzählen — Cache, Pool, Policy, Query-Log. Das Textformat ist eine Handvoll
Zeilen, und wir kontrollieren so genau, was hinausgeht.

## Konsequenzen

* Wer die API öffentlich erreichbar macht, veröffentlicht sein Query-Log, sobald
  der Token bekannt wird. Die Vorgabe bleibt: beide Listener auf localhost, Zugriff
  über SSH-Tunnel oder Reverse Proxy. Die Defaults stehen entsprechend, und beide
  Endpunkte sind per Default **aus**.
* Die UI ist an die Sprache des Servers gebunden (Deutsch) und an sein Layout.
  Eine Übersetzung wäre ein eigenes Vorhaben.
* `include_str!` heißt: eine Änderung an der UI erfordert einen Neubau. Für ein
  Projekt, das ein einzelnes Binary ausliefert, ist das der richtige Tausch.
* Der Live-Strom kostet nichts, solange niemand zuhört: das Query-Log prüft die
  Zahl der Empfänger, bevor es ein Ereignis baut.
