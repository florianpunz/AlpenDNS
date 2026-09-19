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
