//! Woher eine Liste kommt: aus einer Datei oder über HTTP.
//!
//! Heruntergeladene Listen landen zusätzlich auf Platte. Das ist kein
//! Geschwindigkeitstrick, sondern die Voraussetzung dafür, dass ein Neustart
//! ohne Internet **gefiltert** startet statt ungefiltert (ARCHITECTURE.md §9,
//! B.1 Regel 6).

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::parser::Format;

/// Obergrenze für eine einzelne Liste.
///
/// Die größten gebräuchlichen Listen liegen bei wenigen Megabyte. 64 MB lassen
/// reichlich Luft und verhindern zugleich, dass eine falsch konfigurierte URL
/// den Speicher füllt.
const MAX_LIST_BYTES: u64 = 64 * 1024 * 1024;

/// Zeitbudget für einen Listen-Download.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);

/// Woher eine Liste stammt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    File(PathBuf),
    Url(String),
}

/// Eine konfigurierte Liste.
#[derive(Debug, Clone)]
pub struct ListSpec {
    pub name: String,
    pub source: Source,
    pub format: Format,
}

/// Wie eine Liste diesmal zustande kam. Für Logs und Metriken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Aus einer lokalen Datei.
    File,
    /// Frisch heruntergeladen.
    Network,
    /// Der Server meldete 304, es gilt die Fassung von Platte.
    NotModified,
    /// Der Server war nicht erreichbar, es gilt die Fassung von Platte.
    StaleCache,
}

#[derive(Debug)]
pub struct Loaded {
    pub text: String,
    pub origin: Origin,
}

/// Wie alt eine Liste ist.
///
/// Zwei Zeitpunkte, weil sie zwei verschiedene Fragen beantworten: eine Liste,
/// die seit acht Monaten unverändert ist, ist ein anderes Problem als eine, die
/// seit acht Monaten nicht geholt wurde. Der erste sagt, ob das Nachladen noch
/// läuft; der zweite, ob die Quelle überhaupt noch etwas hergibt.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Age {
    /// Wann diese Fassung geholt wurde — die Änderungszeit der Datei, aus der
    /// sie kam.
    ///
    /// Bei einer heruntergeladenen Liste ist das die Cache-Datei, und sie wird
    /// auch dann angefasst, wenn der Server mit 304 antwortet. Die Frage lautet
    /// nicht "wann wurde zuletzt geschrieben", sondern "wann wurde diese Fassung
    /// zuletzt bestätigt".
    pub fetched_at: Option<std::time::SystemTime>,
    /// Was der Herausgeber als letzten Änderungszeitpunkt nennt, aus dem
    /// `last-modified`-Kopf. RFC 3339, weil ihn nur die Oberfläche liest.
    pub published_at: Option<String>,
}

impl Age {
    /// Der Holzeitpunkt als RFC 3339 — das Format, das die API ausliefert.
    pub fn fetched_rfc3339(&self) -> Option<String> {
        rfc3339(self.fetched_at?)
    }

    /// Wie viele Sekunden diese Fassung alt ist.
    ///
    /// `None`, wenn der Zeitpunkt unbekannt ist *oder* in der Zukunft liegt —
    /// letzteres heißt, dass die Uhr gestellt wurde, und dann ist "unbekannt"
    /// die ehrlichere Antwort als eine 0, die wie "gerade geholt" aussieht.
    pub fn seconds_since_fetch(&self, now: std::time::SystemTime) -> Option<u64> {
        Some(now.duration_since(self.fetched_at?).ok()?.as_secs())
    }
}

/// Übersetzt die Listen aus der Konfiguration in das, was der Loader braucht.
///
/// Abgeschaltete Listen fallen hier heraus; die Validierung hat schon
/// sichergestellt, dass genau eine Quelle angegeben ist. Der Reload nutzt
/// dieselbe Übersetzung wie der Erststart, damit beide denselben Listenbestand
/// sehen.
pub fn specs_from_config(
    blocklists: &[crate::config::ListConfig],
    allowlists: &[crate::config::ListConfig],
) -> Vec<ListSpec> {
    blocklists
        .iter()
        .chain(allowlists.iter())
        .filter(|list| list.enabled)
        .filter_map(|list| {
            let source = match (&list.url, &list.path) {
                (Some(url), _) => Source::Url(url.clone()),
                (None, Some(path)) => Source::File(path.clone()),
                (None, None) => return None,
            };
            Some(ListSpec {
                name: list.name.clone(),
                source,
                format: list.format,
            })
        })
        .collect()
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("Liste '{name}' konnte nicht gelesen werden: {source}")]
    Io {
        name: String,
        #[source]
        source: std::io::Error,
    },
    #[error("Liste '{name}' konnte nicht geladen werden und liegt auch nicht im Cache: {reason}")]
    Unavailable { name: String, reason: String },
    #[error("Liste '{name}' ist größer als {MAX_LIST_BYTES} Byte")]
    TooLarge { name: String },
    #[error("HTTP-Client konnte nicht gebaut werden: {0}")]
    Client(String),
}

/// Installiert `ring` als Krypto-Backend für rustls.
///
/// `reqwest` ist mit `rustls-no-provider` gebaut und nimmt den Prozess-Default.
/// Ohne diesen Aufruf schlägt der erste HTTPS-Download fehl. Mehrfaches
/// Aufrufen ist unschädlich.
pub fn install_crypto_provider() {
    // Ein Fehler heißt nur: es war schon eines installiert.
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Lädt Listen aus Dateien oder über HTTP und pflegt den Platten-Cache.
///
/// `Clone` ist billig: `reqwest::Client` klont intern nur einen `Arc`. Der
/// Reload baut daraus ein zweites [`Lists`](super::Lists) mit demselben
/// Cache-Verzeichnis.
#[derive(Debug, Clone)]
pub struct Loader {
    client: reqwest::Client,
    cache_dir: PathBuf,
}

impl Loader {
    pub fn new(cache_dir: PathBuf) -> Result<Self, LoadError> {
        install_crypto_provider();
        let client = reqwest::Client::builder()
            .timeout(DOWNLOAD_TIMEOUT)
            .user_agent(concat!("alpendns/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| LoadError::Client(e.to_string()))?;
        Ok(Self { client, cache_dir })
    }

    /// Dateiname im Cache. Der Listenname kommt aus der Konfiguration und darf
    /// keinen Pfad ergeben, der woanders hinzeigt.
    fn cache_path(&self, name: &str, extension: &str) -> PathBuf {
        let safe: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.cache_dir.join(format!("{safe}.{extension}"))
    }

    pub async fn load(&self, spec: &ListSpec) -> Result<Loaded, LoadError> {
        match &spec.source {
            Source::File(path) => {
                let text = read_limited(path).await.map_err(|source| LoadError::Io {
                    name: spec.name.clone(),
                    source,
                })?;
                Ok(Loaded {
                    text,
                    origin: Origin::File,
                })
            }
            Source::Url(url) => self.download(spec, url).await,
        }
    }

    /// Wie alt die Fassung ist, die gerade gilt.
    ///
    /// Bewusst **synchron** und bei jedem Aufruf neu von Platte gelesen: der
    /// Listenbestand, den die API ausliefert, wird beim Start einmal gebaut und
    /// vom Updater nicht angefasst. Ein dort abgelegtes Datum würde deshalb nur
    /// messen, wie lange der Prozess läuft — genau der Fehler, den diese Anzeige
    /// beheben soll. Ein `stat` je Anfrage ist dafür billig genug, und die
    /// Antwort ist die, die jetzt gilt.
    ///
    /// Beide Zeitpunkte bleiben `None`, wenn es sie nicht gibt: eine Liste aus
    /// der Konfiguration hat kein `last-modified`, eine noch nie geholte keine
    /// Cache-Datei. Was daraus folgt, entscheidet der Aufrufer — hier wird
    /// nichts erfunden.
    pub fn age(&self, spec: &ListSpec) -> Age {
        match &spec.source {
            // Eine lokale Datei altert mit ihrer Änderungszeit. Wer sie
            // bearbeitet, macht sie damit jünger, und das stimmt.
            Source::File(path) => Age {
                fetched_at: modified(path),
                published_at: None,
            },
            Source::Url(_) => Age {
                fetched_at: modified(&self.cache_path(&spec.name, "list")),
                published_at: std::fs::read_to_string(self.cache_path(&spec.name, "meta"))
                    .ok()
                    .map(|text| parse_meta(&text))
                    .and_then(|meta| meta.last_modified)
                    .and_then(|http_date| http_date_as_rfc3339(&http_date)),
            },
        }
    }

    async fn download(&self, spec: &ListSpec, url: &str) -> Result<Loaded, LoadError> {
        let body_path = self.cache_path(&spec.name, "list");
        let meta_path = self.cache_path(&spec.name, "meta");
        let meta = read_meta(&meta_path).await;

        let mut request = self.client.get(url);
        if let Some(etag) = &meta.etag {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }
        if let Some(modified) = &meta.last_modified {
            request = request.header(reqwest::header::IF_MODIFIED_SINCE, modified);
        }

        let mut response = match request.send().await {
            Ok(response) => response,
            Err(error) => return self.fall_back(spec, &body_path, &error.to_string()).await,
        };

        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            let text = read_limited(&body_path)
                .await
                .map_err(|source| LoadError::Io {
                    name: spec.name.clone(),
                    source,
                })?;
            // Erst nach dem Lesen: was nicht gelesen werden konnte, ist auch
            // nicht bestätigt.
            touch(&body_path);
            return Ok(Loaded {
                text,
                origin: Origin::NotModified,
            });
        }
        if !response.status().is_success() {
            let status = response.status();
            return self
                .fall_back(spec, &body_path, &format!("HTTP {status}"))
                .await;
        }

        // Vor dem Lesen prüfen, soweit der Server die Größe nennt.
        if response
            .content_length()
            .is_some_and(|len| len > MAX_LIST_BYTES)
        {
            return self.too_large(spec, &body_path).await;
        }
        let new_meta = Meta {
            etag: header(&response, reqwest::header::ETAG),
            last_modified: header(&response, reqwest::header::LAST_MODIFIED),
        };
        // Stückweise lesen und mitzählen: `content-length` fehlt bei
        // `Transfer-Encoding: chunked`, die Prüfung oben greift dort also
        // nicht, und `text()` würde erst den ganzen Body puffern und die
        // Grenze danach anwenden (CWE-770).
        let mut body = Vec::new();
        loop {
            let chunk = match response.chunk().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(error) => {
                    return self.fall_back(spec, &body_path, &error.to_string()).await;
                }
            };
            if body.len() as u64 + chunk.len() as u64 > MAX_LIST_BYTES {
                return self.too_large(spec, &body_path).await;
            }
            body.extend_from_slice(&chunk);
        }

        // Die Grenze muss für das gelten, was geparst und zwischengespeichert
        // wird, nicht für die Rohbytes: `from_utf8_lossy` ersetzt jedes
        // ungültige Byte durch U+FFFD und bläht die Länge damit auf bis zu das
        // Dreifache auf. Eine Rohgröße unter der Grenze kann so eine
        // Cache-Datei über der Grenze erzeugen, die `read_limited` nie wieder
        // liest — der Rückfall auf die letzte Fassung (B.1 Regel 6) wäre
        // dauerhaft zerstört.
        let text = String::from_utf8_lossy(&body).into_owned();
        if text.len() as u64 > MAX_LIST_BYTES {
            return self.too_large(spec, &body_path).await;
        }

        // Schreibfehler sind kein Grund, die frisch geladene Liste zu verwerfen —
        // sie kosten nur den Vorteil beim nächsten Start.
        if let Err(error) =
            write_cache(&self.cache_dir, &body_path, &meta_path, &text, &new_meta).await
        {
            tracing::warn!(list = %spec.name, %error, "Liste konnte nicht zwischengespeichert werden");
        }

        Ok(Loaded {
            text,
            origin: Origin::Network,
        })
    }

    /// Der Server war nicht erreichbar: die letzte gecachte Fassung gilt weiter.
    ///
    /// Gibt es keine, ist das ein Fehler — der Aufrufer entscheidet, ob das den
    /// Start verhindert (Erststart) oder nur eine Warnung wert ist (Update).
    async fn fall_back(
        &self,
        spec: &ListSpec,
        body_path: &Path,
        reason: &str,
    ) -> Result<Loaded, LoadError> {
        match read_limited(body_path).await {
            Ok(text) => {
                tracing::warn!(
                    list = %spec.name,
                    reason,
                    "Liste nicht erreichbar, es gilt die zwischengespeicherte Fassung"
                );
                Ok(Loaded {
                    text,
                    origin: Origin::StaleCache,
                })
            }
            Err(_) => Err(LoadError::Unavailable {
                name: spec.name.clone(),
                reason: reason.to_owned(),
            }),
        }
    }

    /// Die Liste ist über der Größengrenze: die letzte gecachte Fassung gilt weiter.
    ///
    /// Dieselbe Regel wie bei `fall_back` (B.1 Regel 6) — eine zu große Liste ist
    /// kein Grund, ungefiltert zu starten. Der Fehler bleibt trotzdem `TooLarge`
    /// und wird nicht zu `Unavailable`: die Liste *war* erreichbar, sie ist nur
    /// unbrauchbar, und beim Erststart soll genau das im Log stehen, statt einer
    /// Meldung über einen Server, der nie geantwortet hat.
    async fn too_large(&self, spec: &ListSpec, body_path: &Path) -> Result<Loaded, LoadError> {
        match read_limited(body_path).await {
            Ok(text) => {
                tracing::warn!(
                    list = %spec.name,
                    limit = MAX_LIST_BYTES,
                    "Liste über der Größengrenze, es gilt die zwischengespeicherte Fassung"
                );
                Ok(Loaded {
                    text,
                    origin: Origin::StaleCache,
                })
            }
            Err(_) => Err(LoadError::TooLarge {
                name: spec.name.clone(),
            }),
        }
    }
}

#[derive(Debug, Default)]
struct Meta {
    etag: Option<String>,
    last_modified: Option<String>,
}

fn header(response: &reqwest::Response, name: reqwest::header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)?
        .to_str()
        .ok()
        .map(str::to_owned)
}

/// Zwei Zeilen, `etag:` und `last-modified:`. Ein eigenes Format statt JSON,
/// weil dafür kein Dependency nötig ist und der Inhalt zwei Zeichenketten sind.
async fn read_meta(path: &Path) -> Meta {
    let Ok(text) = tokio::fs::read_to_string(path).await else {
        return Meta::default();
    };
    parse_meta(&text)
}

/// Das Format selbst — getrennt vom Lesen, weil [`Loader::age`] dieselbe Datei
/// synchron liest und die Zeilen nicht zweimal auslegen soll.
fn parse_meta(text: &str) -> Meta {
    let mut meta = Meta::default();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("etag:") {
            meta.etag = Some(value.trim().to_owned());
        } else if let Some(value) = line.strip_prefix("last-modified:") {
            meta.last_modified = Some(value.trim().to_owned());
        }
    }
    meta
}

/// Setzt die Änderungszeit einer Datei auf jetzt.
///
/// „Der Server sagt, die Liste ist unverändert" heißt „diese Fassung ist
/// bestätigt" — und das ist die Frage, die das Alter beantwortet. Ohne diesen
/// Handgriff zeigte eine Liste, die seit einem Jahr täglich mit 304 bestätigt
/// wird, das Datum ihres einzigen Downloads.
///
/// Fehler bleiben still: wer die Zeit nicht setzen darf, soll deswegen keine
/// Liste verlieren. Dann steht das Alter eben auf dem letzten Schreibvorgang.
/// `write(true)` ohne `truncate`, weil `futimens` einen Schreibzugriff auf die
/// Datei verlangt — abgeschnitten wird nichts.
fn touch(path: &Path) {
    let Ok(file) = std::fs::File::options().write(true).open(path) else {
        return;
    };
    let _ = file.set_modified(std::time::SystemTime::now());
}

/// Die Änderungszeit einer Datei, wenn es sie gibt.
fn modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// Ein Zeitpunkt als RFC 3339 — das eine Format, in dem die API Zeiten
/// ausliefert. Die Oberfläche muss dadurch nicht drei Formate kennen.
fn rfc3339(at: std::time::SystemTime) -> Option<String> {
    Some(jiff::Timestamp::try_from(at).ok()?.to_string())
}

/// Übersetzt einen HTTP-Datumsstempel (`Mon, 02 Jan 2006 15:04:05 GMT`) in
/// RFC 3339.
///
/// Ein unlesbarer Stempel wird zu `None` und nicht zu einem Fehler: er steht in
/// der `.meta`, die von einem fremden Server stammt, und ein fehlendes
/// Herausgeberdatum ist kein Grund, gar kein Alter anzuzeigen.
fn http_date_as_rfc3339(http_date: &str) -> Option<String> {
    if let Ok(zoned) = jiff::fmt::rfc2822::parse(http_date) {
        return Some(zoned.timestamp().to_string());
    }
    // Ein zweiter Versuch ohne den Wochentag. RFC 2822 nennt ihn redundant, und
    // der Parser prüft ihn gegen das Datum: "Wed, 02 Jan 2006" ist für ihn ein
    // Fehler, weil der 2. Januar ein Montag war. Ein Server, der sich im
    // Wochentag vertut, hat trotzdem ein richtiges Datum geschrieben. Alles
    // andere bleibt ein Fehler — ein kaputtes Datum wird nicht geraten.
    let (_, rest) = http_date.split_once(',')?;
    Some(
        jiff::fmt::rfc2822::parse(rest.trim_start())
            .ok()?
            .timestamp()
            .to_string(),
    )
}

async fn write_cache(
    dir: &Path,
    body_path: &Path,
    meta_path: &Path,
    text: &str,
    meta: &Meta,
) -> std::io::Result<()> {
    tokio::fs::create_dir_all(dir).await?;
    // Erst daneben schreiben, dann umbenennen: ein Absturz mittendrin darf
    // keine halbe Liste hinterlassen, die beim nächsten Start geladen wird.
    let temporary = body_path.with_extension("list.new");
    tokio::fs::write(&temporary, text).await?;
    tokio::fs::rename(&temporary, body_path).await?;

    let mut rendered = String::new();
    if let Some(etag) = &meta.etag {
        rendered.push_str(&format!("etag: {etag}\n"));
    }
    if let Some(modified) = &meta.last_modified {
        rendered.push_str(&format!("last-modified: {modified}\n"));
    }
    tokio::fs::write(meta_path, rendered).await
}

/// Liest eine Datei und bricht ab, wenn sie zu groß ist.
async fn read_limited(path: &Path) -> std::io::Result<String> {
    let metadata = tokio::fs::metadata(path).await?;
    if metadata.len() > MAX_LIST_BYTES {
        return Err(std::io::Error::other(format!(
            "{} ist größer als {MAX_LIST_BYTES} Byte",
            path.display()
        )));
    }
    tokio::fs::read_to_string(path).await
}
