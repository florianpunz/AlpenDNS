//! Lokale Zonen: Namen, die AlpenDNS selbst beantwortet.
//!
//! Eine lokale Zone ist eine **Tabelle**, kein Zonenfile. Kein SOA, kein NS,
//! keine AXFR/IXFR, kein NOTIFY, kein Signieren, und `AA` bleibt in der Antwort
//! aus — AlpenDNS ist nicht autoritativ (README: "to host a zone, use Knot or
//! NSD"). Was hier steht, ist der Bedarf davor: ein paar Namen im eigenen Netz
//! sollen auf feste Adressen zeigen, und ein paar Namen sollen den Upstream
//! **nie** sehen.
//!
//! Zwei Dinge folgen daraus, die man an der Konfiguration sieht:
//!
//! * Namen, die in der Tabelle stehen, sind ganz lokal — auch für Typen, die
//!   dort nicht stehen (dann NOERROR ohne Antwortsatz). Ein Name mit zwei
//!   Horizonten wäre nicht mehr vorhersagbar.
//! * `fallback` entscheidet je Zone, was mit Namen geschieht, die *nicht* in
//!   der Tabelle stehen: `upstream` (Default, Split-Horizon) oder `nxdomain`
//!   (die Zone ist zu).

use std::collections::HashMap;
use std::sync::Arc;

use hickory_proto::op::{Message, ResponseCode};
use hickory_proto::rr::rdata::{CNAME, PTR, SRV};
use hickory_proto::rr::{DNSClass, Name, RData, Record, RecordType};
use serde::Deserialize;

use crate::config::{ConfigError, ZoneName};
use crate::resolve::{ResolveBackend, ResolveError};
use crate::trace::{Ctx, Step};

/// Wie lange ein Client eine Antwort aus einer lokalen Zone behalten soll.
///
/// Länger als `BLOCK_TTL`, weil sich Tabellendaten nur mit einem Neustart
/// ändern — der Cache im Prozess ist dann ohnehin leer. Kurz genug, dass eine
/// Korrektur nach dem Neustart schnell greift.
const DEFAULT_TTL: u32 = 300;

/// Die Record-Typen, die eine lokale Zone kennt.
///
/// Bewusst eine Liste und nicht „alles, was hickory parst": MX, NS, DS oder
/// DNSKEY in einer Tabelle wären der erste Schritt zu einem autoritativen
/// Server, der keiner werden soll.
const ALLOWED_TYPES: &str = "A, AAAA, CNAME, TXT, PTR und SRV";

const fn default_ttl() -> u32 {
    DEFAULT_TTL
}

/// Was für Namen in dieser Zone gilt, die keinen Eintrag haben.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fallback {
    /// Sie werden wie gewohnt aufgelöst — Split-Horizon. Der Default.
    #[default]
    Upstream,
    /// Es gibt sie nicht. Die Zone ist zu, und nichts daraus geht nach draußen.
    Nxdomain,
}

impl Fallback {
    /// Wie der Wert in der Konfiguration heißt.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Upstream => "upstream",
            Self::Nxdomain => "nxdomain",
        }
    }
}

/// Eine Zone, die AlpenDNS selbst beantwortet.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalZoneConfig {
    /// Die Zone, auf die sich die Einträge beziehen.
    pub zone: ZoneName,
    /// Die Gültigkeitsdauer aller Antworten aus dieser Zone.
    #[serde(default = "default_ttl")]
    pub ttl: u32,
    /// Was mit Namen geschieht, die nicht in der Tabelle stehen.
    #[serde(default)]
    pub fallback: Fallback,
    #[serde(default)]
    pub records: Vec<LocalRecord>,
}

/// Ein Eintrag der Tabelle.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalRecord {
    /// `@` für die Zone selbst, sonst relativ zu ihr (`nas`) oder absolut
    /// (`nas.miloo.at.`).
    pub name: String,
    #[serde(rename = "type")]
    pub rtype: String,
    pub value: String,
}

/// Eine gebaute Tabelle.
#[derive(Debug)]
pub struct LocalZone {
    zone: Name,
    /// Für Trace und Ausgabe vorberechnet — sonst allokierte jede einzelne
    /// lokale Antwort auf dem heißen Pfad.
    label: Arc<str>,
    ttl: u32,
    fallback: Fallback,
    /// Die Schlüssel sind absolut und kleingeschrieben — siehe [`qualify`].
    entries: HashMap<Name, Vec<RData>>,
}

impl LocalZone {
    /// Der Zonenname.
    #[must_use]
    pub const fn zone(&self) -> &Name {
        &self.zone
    }

    /// Der Zonenname, wie er im Trace und in `alpendns check` erscheint —
    /// ohne den Punkt am Ende, den nur der Draht braucht.
    #[must_use]
    pub fn label(&self) -> Arc<str> {
        Arc::clone(&self.label)
    }

    /// Was mit Namen geschieht, die nicht in der Tabelle stehen.
    #[must_use]
    pub const fn fallback(&self) -> Fallback {
        self.fallback
    }

    /// Ob Namen ohne Eintrag in dieser Zone verschwinden.
    #[must_use]
    pub const fn is_closed(&self) -> bool {
        matches!(self.fallback, Fallback::Nxdomain)
    }

    /// Die Namen, für die es hier Einträge gibt. Absolut und kleingeschrieben.
    pub fn names(&self) -> impl Iterator<Item = &Name> {
        self.entries.keys()
    }

    /// Die Gültigkeitsdauer der Antworten aus dieser Zone.
    #[must_use]
    pub const fn ttl(&self) -> u32 {
        self.ttl
    }

    /// Die Records zu einem Namen, oder `None`, wenn er nicht in der Tabelle
    /// steht.
    ///
    /// Die Suche braucht kein `to_lowercase`: `Label`s `PartialEq` ist
    /// case-insensitiv und sein `Hash` schreibt klein (hickory-proto,
    /// `label.rs`). Die Schreibweise des Clients — 0x20-Kodierung — findet den
    /// kleingeschriebenen Schlüssel also ohne eine Allokation im heißen Pfad.
    fn entry(&self, name: &Name) -> Option<&[RData]> {
        self.entries.get(name).map(Vec::as_slice)
    }
}

/// Beantwortet Namen aus den lokalen Zonen und reicht alles andere weiter.
///
/// Steht **unter** dem Cache (lokale Antworten sollen wie jede andere ihre TTL
/// bekommen) und **über** dem [`ZoneRouter`](crate::router::ZoneRouter): ein
/// Eintrag ist spezifischer als eine Zone, dieselbe Regel wie dort eine Ebene
/// feiner.
#[derive(Debug)]
pub struct LocalBackend<B> {
    /// Absteigend nach Tiefe sortiert, damit die spezifischste Zone gewinnt.
    zones: Vec<LocalZone>,
    inner: B,
}

impl<B: ResolveBackend> LocalBackend<B> {
    pub fn new(mut zones: Vec<LocalZone>, inner: B) -> Self {
        zones.sort_by_key(|zone| std::cmp::Reverse(zone.zone.num_labels()));
        Self { zones, inner }
    }

    /// Der Eintrag zu einem Namen, mit der Zone, in der er steht.
    fn entry(&self, name: &Name) -> Option<(&LocalZone, &[RData])> {
        self.zones
            .iter()
            .find_map(|zone| zone.entry(name).map(|data| (zone, data)))
    }
}

impl<B: ResolveBackend> ResolveBackend for LocalBackend<B> {
    fn resolve(
        &self,
        request: &Message,
        ctx: &mut Ctx,
    ) -> impl std::future::Future<Output = Result<Message, ResolveError>> + Send {
        let query = request.queries.first().cloned();
        async move {
            // Ohne Frage gibt es nichts zu vergleichen, und alles außer IN
            // gehört nicht in diese Tabelle.
            let Some(query) = query.filter(|q| q.query_class() == DNSClass::IN) else {
                return self.inner.resolve(request, ctx).await;
            };

            // 1. Ein Eintrag gewinnt immer — auch gegen eine geschlossene
            //    Zone, die den Namen ebenfalls enthält. Wer ihn hinschreibt,
            //    meint ihn.
            if let Some((zone, data)) = self.entry(query.name()) {
                let mut response = response_to(request);
                let answers: Vec<&RData> = match query.query_type() {
                    // RFC 8482: die Tabelle ist klein und statisch, alles
                    // aufzulisten hätte keinen Nutzen.
                    RecordType::ANY => Vec::new(),
                    // Ein CNAME antwortet auf *jeden* Typ an diesem Namen
                    // (RFC 1034 §3.6.2) — ohne ihn gäbe es auf "A drucker"
                    // ein NODATA, und der Client käme nie zum Ziel. Dass hier
                    // kein zweiter Typ daneben stehen kann, hat `build_zone`
                    // schon geprüft.
                    qtype => data
                        .iter()
                        .filter(|d| d.record_type() == qtype || matches!(d, RData::CNAME(_)))
                        .collect(),
                };
                // Ein Name darf nicht zwei Horizonte haben: fehlt der Typ,
                // ist die Antwort NODATA statt "frag den Upstream". Sonst
                // lieferte A lokal und AAAA von draußen, und der fremde Typ
                // wäre der Ausgang aus der Zone.
                for data in &answers {
                    response.add_answer(Record::from_rdata(
                        query.name().clone(),
                        zone.ttl,
                        (*data).clone(),
                    ));
                }
                ctx.record(Step::LocalAnswer {
                    zone: zone.label(),
                    records: answers.len(),
                });
                return Ok(response);
            }

            // 2. Kein Eintrag: die spezifischste Zone, die den Namen enthält,
            //    entscheidet, ob er überhaupt hinausgeht.
            if let Some(zone) = self
                .zones
                .iter()
                .find(|zone| zone.zone.zone_of(query.name()))
                && zone.is_closed()
            {
                let mut response = response_to(request);
                response.metadata.response_code = ResponseCode::NXDomain;
                ctx.record(Step::LocalAnswer {
                    zone: zone.label(),
                    records: 0,
                });
                return Ok(response);
            }

            self.inner.resolve(request, ctx).await
        }
    }
}

/// Baut das Antwortgerüst zu einer Anfrage.
///
/// `AA` bleibt aus: AlpenDNS ist nicht autoritativ, auch nicht für eine lokale
/// Zone (README: "to host a zone, use Knot or NSD"). Ein gesetztes `AA` wäre
/// eine Behauptung, die wir nicht einlösen.
fn response_to(request: &Message) -> Message {
    let mut response = Message::response(request.metadata.id, request.metadata.op_code);
    response.metadata.recursion_desired = request.metadata.recursion_desired;
    response.metadata.recursion_available = true;
    response.add_queries(request.queries.iter().cloned());
    response
}

/// Baut eine Tabelle aus ihrer Konfiguration.
///
/// Der eine Ort, an dem die Regeln stehen — `Config::validate` und `main` rufen
/// beide hier an, damit `alpendns check` nicht auseinanderläuft mit dem, was
/// der Start tut.
pub fn build_zone(config: &LocalZoneConfig) -> Result<LocalZone, ConfigError> {
    let zone = config.zone.0.clone();
    if zone.is_root() {
        return Err(ConfigError::Invalid(
            "local_zone '.': die Wurzelzone fängt jede Anfrage ab. Gemeint ist vermutlich \
             eine einzelne Zone, etwa \"miloo.at\"."
                .to_owned(),
        ));
    }
    let mut entries: HashMap<Name, Vec<RData>> = HashMap::new();

    for record in &config.records {
        let name = qualify(&record.name, &zone).map_err(|detail| {
            ConfigError::Invalid(format!(
                "local_zone '{zone}', Eintrag '{}': {detail}",
                record.name
            ))
        })?;
        if !zone.zone_of(&name) {
            return Err(ConfigError::Invalid(format!(
                "local_zone '{zone}', Eintrag '{}': der Name liegt außerhalb der Zone. \
                 Erwartet wird '@' für die Zone selbst, ein Name relativ zu ihr ('nas', \
                 '_https._tcp') oder ein absoluter, der auf '{zone}' endet. Ein Name ohne \
                 Punkt am Ende wird relativ zur Zone gelesen — 'nas.example.com' wäre also \
                 'nas.example.com.{zone}'.",
                record.name
            )));
        }

        let rtype = record_type_of(&record.rtype).ok_or_else(|| {
            ConfigError::Invalid(format!(
                "local_zone '{zone}', Eintrag '{}': Typ '{}' wird nicht unterstützt. \
                 Möglich sind {ALLOWED_TYPES}. Eine lokale Zone ist eine Tabelle, kein \
                 Zonenfile.",
                record.name, record.rtype
            ))
        })?;
        let value = record.value.trim();
        let data = RData::try_from_str(rtype, value)
            .map_err(|error| {
                ConfigError::Invalid(format!(
                    "local_zone '{zone}', Eintrag '{}' ({rtype}): '{value}' ist kein gültiger \
                     Wert: {error}",
                    record.name
                ))
            })
            .and_then(|data| {
                qualify_target(data, &zone).map_err(|detail| {
                    ConfigError::Invalid(format!(
                        "local_zone '{zone}', Eintrag '{}' ({rtype}): {detail}",
                        record.name
                    ))
                })
            })?;

        entries.entry(name).or_default().push(data);
    }

    for (name, data) in &entries {
        // Ein CNAME schließt alles andere an demselben Namen aus (RFC 1034
        // §3.6.2). Zwei CNAMEs sind derselbe Fall: welcher gilt, wäre nicht
        // vorhersagbar.
        if data.len() > 1 && data.iter().any(|d| matches!(d, RData::CNAME(_))) {
            return Err(ConfigError::Invalid(format!(
                "local_zone '{zone}', Eintrag '{name}': zu einem CNAME gehört kein weiterer \
                 Record. Entweder der CNAME oder die anderen Einträge."
            )));
        }
        if let Some(duplicate) = first_duplicate(data) {
            return Err(ConfigError::Invalid(format!(
                "local_zone '{zone}', Eintrag '{name}': '{duplicate}' steht doppelt in der \
                 Tabelle."
            )));
        }
    }

    Ok(LocalZone {
        label: Arc::from(zone.to_ascii().trim_end_matches('.').to_owned()),
        zone,
        ttl: config.ttl,
        fallback: config.fallback,
        entries,
    })
}

/// Baut alle Zonen und prüft, was nur über Zonen hinweg sichtbar ist.
pub fn build_zones(configs: &[LocalZoneConfig]) -> Result<Vec<LocalZone>, ConfigError> {
    let zones: Vec<LocalZone> = configs.iter().map(build_zone).collect::<Result<_, _>>()?;

    // Zweimal dieselbe Zone: die Tabellen stünden beide da, und welche einen
    // Namen beantwortet, hinge an der Reihenfolge in der Datei.
    let mut declared: HashMap<&Name, ()> = HashMap::new();
    for zone in &zones {
        if declared.insert(zone.zone(), ()).is_some() {
            return Err(ConfigError::Invalid(format!(
                "local_zone '{}' steht zweimal in der Konfiguration. Fasse die Einträge in \
                 einer Zone zusammen.",
                zone.zone()
            )));
        }
    }

    // Ein Name in zwei Tabellen: derselbe Grund.
    let mut seen: HashMap<&Name, &Name> = HashMap::new();
    for zone in &zones {
        for name in zone.names() {
            if let Some(first) = seen.insert(name, zone.zone()) {
                return Err(ConfigError::Invalid(format!(
                    "local_zone '{first}' und '{}': der Name '{name}' steht in beiden Zonen. \
                     Ein Name gehört in eine Tabelle.",
                    zone.zone()
                )));
            }
        }
    }
    Ok(zones)
}

/// Der erste Wert, der in dieser Liste schon einmal vorkommt.
fn first_duplicate<T: PartialEq>(items: &[T]) -> Option<&T> {
    items
        .iter()
        .enumerate()
        .find(|(index, item)| items.iter().take(*index).any(|seen| seen == *item))
        .map(|(_, item)| item)
}

/// Der Record-Typ zu einem Namen aus der Konfiguration.
///
/// Von Hand statt über `RecordType::from_str`: das hat ein
/// `debug_assert!(!is_ascii_lowercase)` und würde in einem Debug-Build bei
/// `type = "a"` panicken — auf einem Weg, der von der Konfiguration kommt.
fn record_type_of(text: &str) -> Option<RecordType> {
    match text.trim().to_ascii_uppercase().as_str() {
        "A" => Some(RecordType::A),
        "AAAA" => Some(RecordType::AAAA),
        "CNAME" => Some(RecordType::CNAME),
        "TXT" => Some(RecordType::TXT),
        "PTR" => Some(RecordType::PTR),
        "SRV" => Some(RecordType::SRV),
        _ => None,
    }
}

/// Macht aus einem Namen aus der Konfiguration einen absoluten,
/// kleingeschriebenen.
///
/// **Beides ist Pflicht, nicht Kosmetik.** `Name`s `PartialEq` liefert `false`,
/// sobald sich `is_fqdn` unterscheidet, und `Hash` mischt das Flag ein. Ein
/// relativer Schlüssel `nas` fände den absoluten Abfragenamen `nas.miloo.at.`
/// also nie — ohne Fehlermeldung, die Suche liefe einfach ins Leere.
fn qualify(raw: &str, zone: &Name) -> Result<Name, String> {
    let trimmed = raw.trim();
    // `@` ist die Zone selbst — die Schreibweise aus dem Zonenfile, und die
    // einzige, die ein Mensch für "die Zone" erwartet.
    if trimmed.is_empty() || trimmed == "@" {
        return Ok(zone.clone());
    }
    let name = Name::from_str_relaxed(trimmed)
        .map_err(|error| format!("'{trimmed}' ist kein Name: {error}"))?;
    let name = if name.is_fqdn() {
        name
    } else {
        // Setzt auch `is_fqdn` (hickory-proto, `name.rs`).
        name.append_domain(zone)
            .map_err(|error| format!("'{trimmed}' ließ sich nicht ergänzen: {error}"))?
    };
    Ok(name.to_lowercase())
}

/// Qualifiziert die Ziele, die in einem Record stecken.
///
/// `RData::try_from_str` parst ohne Origin (`origin = None`), ein CNAME-Ziel
/// `nas2` käme also **relativ** zurück und wäre auf dem Draht Müll. A, AAAA und
/// TXT haben kein Ziel und gehen unverändert durch.
fn qualify_target(data: RData, zone: &Name) -> Result<RData, String> {
    let target = |name: &Name| qualify(&name.to_string(), zone);
    Ok(match data {
        RData::CNAME(CNAME(name)) => RData::CNAME(CNAME(target(&name)?)),
        RData::PTR(PTR(name)) => RData::PTR(PTR(target(&name)?)),
        RData::SRV(srv) => {
            let name = target(&srv.target)?;
            RData::SRV(SRV::new(srv.priority, srv.weight, srv.port, name))
        }
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use hickory_proto::op::{MessageType, OpCode, Query};
    use hickory_proto::rr::rdata::A;

    use super::*;

    fn zone(name: &str) -> Name {
        ZoneName::try_from(name.to_owned()).expect("gültige Zone").0
    }

    fn config(text: &str) -> LocalZoneConfig {
        toml::from_str(text).expect("muss parsen")
    }

    fn built(text: &str) -> LocalZone {
        build_zone(&config(text)).expect("muss bauen")
    }

    fn error(text: &str) -> String {
        build_zone(&config(text))
            .expect_err("muss scheitern")
            .to_string()
    }

    #[test]
    fn every_spelling_of_a_name_lands_on_the_same_key() {
        // Der wichtigste Test der Datei: `Name` vergleicht `is_fqdn` mit, ein
        // relativer Schlüssel fände den absoluten Abfragenamen also nie.
        for name in ["nas", "nas.miloo.at.", "NAS", "Nas.Miloo.At."] {
            let table = built(&format!(
                r#"
zone = "miloo.at"
records = [{{ name = "{name}", type = "A", value = "192.168.1.5" }}]
"#
            ));
            assert!(
                table.entry(&zone("nas.miloo.at")).is_some(),
                "'{name}' ergab keinen absoluten Schlüssel"
            );
            assert!(
                table.entry(&zone("NAS.MILOO.AT")).is_some(),
                "'{name}' ist nicht kleingeschrieben"
            );
        }
    }

    #[test]
    fn the_at_sign_is_the_zone_itself() {
        let table = built(
            r#"
zone = "miloo.at"
records = [{ name = "@", type = "A", value = "192.168.1.5" }]
"#,
        );
        assert!(table.entry(&zone("miloo.at")).is_some());
    }

    #[test]
    fn cname_targets_are_qualified_and_lowercased() {
        // Ohne Qualifizierung käme `nas2` relativ zurück — auf dem Draht wäre
        // das ein anderer Name als gemeint.
        let table = built(
            r#"
zone = "miloo.at"
records = [{ name = "drucker", type = "CNAME", value = "Nas2" }]
"#,
        );
        let data = &table.entries[&zone("drucker.miloo.at")][0];
        match data {
            RData::CNAME(CNAME(name)) => {
                assert!(name.is_fqdn(), "CNAME-Ziel ist nicht absolut: {name}");
                assert_eq!(name, &zone("nas2.miloo.at"));
            }
            other => panic!("kein CNAME: {other:?}"),
        }
    }

    #[test]
    fn srv_and_ptr_targets_are_qualified() {
        let table = built(
            r#"
zone = "1.168.192.in-addr.arpa"
records = [
  { name = "5", type = "PTR", value = "nas.miloo.at." },
  { name = "_https._tcp", type = "SRV", value = "0 0 443 nas.miloo.at." },
]
"#,
        );
        let ptr = &table.entries[&zone("5.1.168.192.in-addr.arpa")][0];
        assert!(matches!(ptr, RData::PTR(PTR(name)) if name.is_fqdn()));
        let srv = &table.entries[&zone("_https._tcp.1.168.192.in-addr.arpa")][0];
        assert!(matches!(srv, RData::SRV(srv) if srv.target.is_fqdn()));
    }

    #[test]
    fn a_relative_srv_target_is_taken_relative_to_the_zone() {
        let table = built(
            r#"
zone = "miloo.at"
records = [{ name = "_https._tcp", type = "SRV", value = "0 0 443 nas" }]
"#,
        );
        let srv = &table.entries[&zone("_https._tcp.miloo.at")][0];
        assert!(matches!(srv, RData::SRV(srv) if srv.target == zone("nas.miloo.at")));
    }

    #[test]
    fn a_name_outside_the_zone_is_an_error() {
        let message = error(
            r#"
zone = "miloo.at"
records = [{ name = "nas.example.com.", type = "A", value = "192.168.1.5" }]
"#,
        );
        assert!(message.contains("außerhalb der Zone"), "{message}");
    }

    #[test]
    fn a_dotted_name_without_a_trailing_dot_is_read_relative() {
        // Zonenfile-Semantik, und die klassische Falle: `nas.example.com` ist
        // *relativ*, nicht absolut. `_https._tcp` sieht genauso aus und ist
        // wirklich relativ — an der Form ist beides nicht zu unterscheiden,
        // deshalb gilt eine Regel für beide.
        let table = built(
            r#"
zone = "miloo.at"
records = [
  { name = "_https._tcp", type = "SRV", value = "0 0 443 nas" },
  { name = "nas.example.com", type = "A", value = "192.168.1.5" },
]
"#,
        );
        assert!(table.entry(&zone("_https._tcp.miloo.at")).is_some());
        assert!(table.entry(&zone("nas.example.com.miloo.at")).is_some());
    }

    #[test]
    fn an_unknown_type_is_an_error_that_names_the_allowed_ones() {
        let message = error(
            r#"
zone = "miloo.at"
records = [{ name = "mail", type = "MX", value = "10 mail.miloo.at." }]
"#,
        );
        assert!(message.contains("MX"), "{message}");
        assert!(message.contains("AAAA"), "{message}");
    }

    #[test]
    fn a_lowercase_type_is_accepted() {
        // `RecordType::from_str` würde hier panicken (debug_assert); deshalb
        // geht der Typ durch unsere eigene Liste.
        let table = built(
            r#"
zone = "miloo.at"
records = [{ name = "nas", type = "a", value = "192.168.1.5" }]
"#,
        );
        assert!(table.entry(&zone("nas.miloo.at")).is_some());
    }

    #[test]
    fn a_broken_value_is_an_error() {
        let message = error(
            r#"
zone = "miloo.at"
records = [{ name = "nas", type = "A", value = "192.168.1.999" }]
"#,
        );
        assert!(message.contains("kein gültiger Wert"), "{message}");
    }

    #[test]
    fn the_same_entry_twice_is_an_error() {
        let message = error(
            r#"
zone = "miloo.at"
records = [
  { name = "nas", type = "A", value = "192.168.1.5" },
  { name = "nas", type = "A", value = "192.168.1.5" },
]
"#,
        );
        assert!(message.contains("doppelt"), "{message}");
    }

    #[test]
    fn two_addresses_for_one_name_are_allowed() {
        // Der Normalfall: dual-stack und Round-Robin.
        let table = built(
            r#"
zone = "miloo.at"
records = [
  { name = "nas", type = "A", value = "192.168.1.5" },
  { name = "nas", type = "A", value = "192.168.1.6" },
  { name = "nas", type = "AAAA", value = "fd00::5" },
]
"#,
        );
        assert_eq!(table.entries[&zone("nas.miloo.at")].len(), 3);
    }

    #[test]
    fn a_cname_beside_another_record_is_an_error() {
        let message = error(
            r#"
zone = "miloo.at"
records = [
  { name = "drucker", type = "CNAME", value = "nas" },
  { name = "drucker", type = "A", value = "192.168.1.7" },
]
"#,
        );
        assert!(message.contains("CNAME"), "{message}");
    }

    #[test]
    fn two_cnames_for_one_name_are_an_error() {
        let message = error(
            r#"
zone = "miloo.at"
records = [
  { name = "drucker", type = "CNAME", value = "nas" },
  { name = "drucker", type = "CNAME", value = "nas2" },
]
"#,
        );
        assert!(message.contains("CNAME"), "{message}");
    }

    #[test]
    fn a_name_in_two_zones_is_an_error() {
        let zones = vec![
            config(
                r#"
zone = "miloo.at"
records = [{ name = "nas.sub.miloo.at.", type = "A", value = "192.168.1.5" }]
"#,
            ),
            config(
                r#"
zone = "sub.miloo.at"
records = [{ name = "nas", type = "A", value = "192.168.1.6" }]
"#,
            ),
        ];
        let message = build_zones(&zones).expect_err("muss scheitern").to_string();
        assert!(message.contains("in beiden Zonen"), "{message}");
    }

    #[test]
    fn an_empty_table_is_not_an_error() {
        // Die ausgelieferte Form von "diese Zone geht niemanden etwas an".
        let table = built(
            r#"
zone = "miloo.at"
fallback = "nxdomain"
"#,
        );
        assert!(table.is_closed());
        assert_eq!(table.names().count(), 0);
    }

    #[test]
    fn the_defaults_are_upstream_and_three_hundred_seconds() {
        let table = built(r#"zone = "miloo.at""#);
        assert_eq!(table.ttl, DEFAULT_TTL);
        assert_eq!(table.fallback, Fallback::Upstream);
        assert!(!table.is_closed());
    }

    #[test]
    fn an_unknown_fallback_is_a_parse_error() {
        let error = toml::from_str::<LocalZoneConfig>(
            r#"
zone = "miloo.at"
fallback = "closed"
"#,
        )
        .expect_err("muss scheitern");
        assert!(error.to_string().contains("closed"), "{error}");
    }

    #[test]
    fn an_unknown_key_in_a_record_is_a_parse_error() {
        // B.1 Regel 5: ein Tippfehler ist ein Startfehler, kein stilles
        // Ignorieren.
        let error = toml::from_str::<LocalZoneConfig>(
            r#"
zone = "miloo.at"
records = [{ name = "nas", typ = "A", value = "192.168.1.5" }]
"#,
        )
        .expect_err("muss scheitern");
        assert!(error.to_string().contains("typ"), "{error}");
    }

    #[test]
    fn a_txt_value_keeps_its_spaces() {
        // Die Zone-File-Schreibweise: der Wert wird in Anführungszeichen
        // gesetzt, damit der Lexer ihn als *einen* String liest.
        let table = built(
            r#"
zone = "miloo.at"
records = [{ name = "_dmarc", type = "TXT", value = '"v=DMARC1; p=none"' }]
"#,
        );
        let data = &table.entries[&zone("_dmarc.miloo.at")][0];
        match data {
            RData::TXT(txt) => assert_eq!(txt.to_string(), "v=DMARC1; p=none"),
            other => panic!("kein TXT: {other:?}"),
        }
    }

    // --- Der Layer selbst -------------------------------------------------

    /// Backend, das nur mitzählt — und damit beweist, dass eine Anfrage den
    /// Upstream **nicht** erreicht hat.
    #[derive(Debug, Default)]
    struct Counter {
        hits: AtomicUsize,
    }

    impl ResolveBackend for Arc<Counter> {
        fn resolve(
            &self,
            request: &Message,
            _ctx: &mut Ctx,
        ) -> impl std::future::Future<Output = Result<Message, ResolveError>> + Send {
            let id = request.metadata.id;
            async move {
                self.hits.fetch_add(1, Ordering::SeqCst);
                let mut response = Message::response(id, request.metadata.op_code);
                response.add_queries(request.queries.iter().cloned());
                response.metadata.response_code = ResponseCode::NoError;
                Ok(response)
            }
        }
    }

    fn ask(name: &str, qtype: RecordType, class: DNSClass) -> Message {
        // Bewusst nicht `zone()`: das schreibt klein und würde die
        // 0x20-Kodierung des Clients schon im Test wegnehmen.
        let mut query = Query::query(Name::from_ascii(name).expect("gültiger Name"), qtype);
        query.set_query_class(class);
        let mut message = Message::new(0x1234, MessageType::Query, OpCode::Query);
        message.metadata.recursion_desired = true;
        message.add_query(query);
        message
    }

    fn ctx() -> Ctx {
        Ctx::new(std::net::SocketAddr::from(([127, 0, 0, 1], 5555)))
    }

    /// Fragt und gibt Antwort wie Zählerstand zurück.
    async fn query_zones(text: &str, name: &str, qtype: RecordType) -> (Message, usize, Ctx) {
        let inner = Arc::new(Counter::default());
        let backend = LocalBackend::new(
            build_zones(&[config(text)]).expect("gültige Zonen"),
            Arc::clone(&inner),
        );
        let mut ctx = ctx();
        let response = backend
            .resolve(&ask(name, qtype, DNSClass::IN), &mut ctx)
            .await
            .expect("Antwort");
        (response, inner.hits.load(Ordering::SeqCst), ctx)
    }

    const SPLIT_HORIZON: &str = r#"
zone = "miloo.at"
records = [
  { name = "nas", type = "A", value = "192.168.1.5" },
  { name = "nas", type = "AAAA", value = "fd00::5" },
  { name = "drucker", type = "CNAME", value = "nas" },
]
"#;

    #[tokio::test]
    async fn an_entry_is_answered_locally_and_never_reaches_the_upstream() {
        let (response, hits, ctx) =
            query_zones(SPLIT_HORIZON, "nas.miloo.at.", RecordType::A).await;
        assert_eq!(hits, 0, "der Upstream wurde gefragt");
        assert_eq!(response.metadata.response_code, ResponseCode::NoError);
        let record = response.answers.first().expect("ein Record");
        assert_eq!(record.data, RData::A(A(Ipv4Addr::new(192, 168, 1, 5))));
        assert_eq!(record.ttl, DEFAULT_TTL);
        assert_eq!(record.name, zone("nas.miloo.at"));
        assert!(
            ctx.steps()
                .iter()
                .any(|step| matches!(step, Step::LocalAnswer { records: 1, .. })),
            "der Trace nennt die Zone nicht"
        );
    }

    #[tokio::test]
    async fn the_clients_spelling_of_the_name_is_kept() {
        // 0x20-Kodierung: der Owner-Name kommt so zurück, wie gefragt wurde.
        let (response, _, _) = query_zones(SPLIT_HORIZON, "NaS.MiLoO.aT.", RecordType::A).await;
        assert_eq!(
            response.answers.first().map(|r| r.name.to_ascii()),
            Some("NaS.MiLoO.aT.".to_owned())
        );
    }

    #[tokio::test]
    async fn a_name_without_the_type_is_nodata_not_an_upstream_question() {
        // Der Kern der Zusage: ein Name in der Tabelle hat *einen* Horizont.
        // Sonst lieferte A lokal und MX von draußen.
        let (response, hits, _) = query_zones(SPLIT_HORIZON, "nas.miloo.at.", RecordType::MX).await;
        assert_eq!(hits, 0, "der Upstream wurde gefragt");
        assert_eq!(response.metadata.response_code, ResponseCode::NoError);
        assert!(response.answers.is_empty(), "NODATA heißt ohne Antwortsatz");
    }

    #[tokio::test]
    async fn any_is_answered_with_nodata() {
        let (response, hits, _) =
            query_zones(SPLIT_HORIZON, "nas.miloo.at.", RecordType::ANY).await;
        assert_eq!(hits, 0);
        assert!(response.answers.is_empty());
    }

    #[tokio::test]
    async fn a_cname_is_given_back_as_it_stands() {
        // Kein Chasing: den Zielnamen fragt der Client selbst nach und trifft
        // dieselbe Tabelle.
        let (response, hits, _) =
            query_zones(SPLIT_HORIZON, "drucker.miloo.at.", RecordType::A).await;
        assert_eq!(hits, 0);
        assert!(matches!(
            response.answers.first().map(|r| &r.data),
            Some(RData::CNAME(_))
        ));
    }

    #[tokio::test]
    async fn a_closed_zone_answers_nxdomain_without_asking_anyone() {
        let (response, hits, _) = query_zones(
            r#"
zone = "miloo.at"
fallback = "nxdomain"
"#,
            "www.miloo.at.",
            RecordType::A,
        )
        .await;
        assert_eq!(hits, 0, "aus einer geschlossenen Zone ging etwas hinaus");
        assert_eq!(response.metadata.response_code, ResponseCode::NXDomain);
        assert!(response.answers.is_empty());
    }

    #[tokio::test]
    async fn an_entry_also_answers_inside_a_closed_zone() {
        let (response, hits, _) = query_zones(
            r#"
zone = "miloo.at"
fallback = "nxdomain"
records = [{ name = "nas", type = "A", value = "192.168.1.5" }]
"#,
            "nas.miloo.at.",
            RecordType::A,
        )
        .await;
        assert_eq!(hits, 0);
        assert_eq!(response.answers.len(), 1);
    }

    #[tokio::test]
    async fn split_horizon_lets_everything_else_through() {
        let (_, hits, _) = query_zones(SPLIT_HORIZON, "www.miloo.at.", RecordType::A).await;
        assert_eq!(hits, 1, "der Upstream wurde nicht gefragt");
    }

    #[tokio::test]
    async fn the_most_specific_zone_decides_for_the_rest() {
        // `sub.miloo.at` ist zu, `miloo.at` nicht: der Name geht nicht hinaus.
        let zones = [
            config(
                r#"
zone = "miloo.at"
"#,
            ),
            config(
                r#"
zone = "sub.miloo.at"
fallback = "nxdomain"
"#,
            ),
        ];
        let inner = Arc::new(Counter::default());
        let backend = LocalBackend::new(
            build_zones(&zones).expect("gültige Zonen"),
            Arc::clone(&inner),
        );
        let response = backend
            .resolve(
                &ask("x.sub.miloo.at.", RecordType::A, DNSClass::IN),
                &mut ctx(),
            )
            .await
            .expect("Antwort");
        assert_eq!(inner.hits.load(Ordering::SeqCst), 0);
        assert_eq!(response.metadata.response_code, ResponseCode::NXDomain);
    }

    #[tokio::test]
    async fn a_class_other_than_in_is_passed_through() {
        // Die Tabelle sind IN-Zonen; für CH gibt es hier nichts zu sagen.
        let inner = Arc::new(Counter::default());
        let backend = LocalBackend::new(
            build_zones(&[config(SPLIT_HORIZON)]).expect("gültige Zonen"),
            Arc::clone(&inner),
        );
        let response = backend
            .resolve(
                &ask("nas.miloo.at.", RecordType::A, DNSClass::CH),
                &mut ctx(),
            )
            .await
            .expect("Antwort");
        assert_eq!(inner.hits.load(Ordering::SeqCst), 1);
        assert!(response.answers.is_empty());
    }

    #[test]
    fn a_request_without_a_question_is_passed_through() {
        // Ein Paket ohne Frage darf hier nicht panicken.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("Runtime");
        runtime.block_on(async {
            let inner = Arc::new(Counter::default());
            let backend = LocalBackend::new(
                build_zones(&[config(SPLIT_HORIZON)]).expect("gültige Zonen"),
                Arc::clone(&inner),
            );
            let message = Message::new(1, MessageType::Query, OpCode::Query);
            backend
                .resolve(&message, &mut ctx())
                .await
                .expect("Antwort");
            assert_eq!(inner.hits.load(Ordering::SeqCst), 1);
        });
    }

    #[tokio::test]
    async fn the_answer_mirrors_the_request_and_claims_no_authority() {
        let (response, _, _) = query_zones(SPLIT_HORIZON, "nas.miloo.at.", RecordType::A).await;
        assert_eq!(response.metadata.id, 0x1234);
        assert_eq!(response.metadata.message_type, MessageType::Response);
        assert!(response.metadata.recursion_available);
        assert!(response.metadata.recursion_desired, "RD wird gespiegelt");
        assert_eq!(
            response.queries.first().map(|q| q.name().to_ascii()),
            Some("nas.miloo.at.".to_owned()),
            "die Frage gehört zurückgespiegelt"
        );
        assert!(
            !response.metadata.authoritative,
            "AA wäre eine Autorität, die wir nicht haben"
        );
    }
}
