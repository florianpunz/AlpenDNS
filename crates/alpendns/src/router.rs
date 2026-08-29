//! Weichenstellung nach Zone.
//!
//! Namen aus einer konfigurierten `forward_zone` gehen an den Nameserver im
//! eigenen Netz, alles andere an den verschlüsselten Upstream-Pool. Das ist die
//! einzige Stelle, an der eine Anfrage den Rechner im Klartext verlässt — und
//! sie verlässt dabei das eigene Netz nicht (B.1 Regel 7).

use hickory_proto::op::Message;
use hickory_proto::rr::Name;

use crate::resolve::{ResolveBackend, ResolveError};
use crate::trace::Ctx;

/// Schickt Anfragen je nach Zone an unterschiedliche Backends.
#[derive(Debug)]
pub struct ZoneRouter<Z, D> {
    /// Absteigend nach Tiefe sortiert, damit die spezifischste Zone gewinnt.
    zones: Vec<(Name, Z)>,
    default: D,
}

impl<Z: ResolveBackend, D: ResolveBackend> ZoneRouter<Z, D> {
    pub fn new(mut zones: Vec<(Name, Z)>, default: D) -> Self {
        // `home.arpa` und `dev.home.arpa` dürfen beide konfiguriert sein; dann
        // muss die längere gewinnen.
        zones.sort_by_key(|(zone, _)| std::cmp::Reverse(zone.num_labels()));
        Self { zones, default }
    }

    fn zone_for(&self, question: &Name) -> Option<&Z> {
        self.zones
            .iter()
            .find(|(zone, _)| zone.zone_of(question))
            .map(|(_, backend)| backend)
    }
}

impl<Z: ResolveBackend, D: ResolveBackend> ResolveBackend for ZoneRouter<Z, D> {
    fn resolve(
        &self,
        request: &Message,
        ctx: &Ctx,
    ) -> impl std::future::Future<Output = Result<Message, ResolveError>> + Send {
        let question = request.queries.first().map(|q| q.name().clone());
        async move {
            match question.as_ref().and_then(|name| self.zone_for(name)) {
                Some(zone) => zone.resolve(request, ctx).await,
                None => self.default.resolve(request, ctx).await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::Ctx;
    use hickory_proto::op::{MessageType, OpCode, Query};
    use hickory_proto::rr::RecordType;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Backend, das nur mitzählt und seinen Namen in die Antwort schreibt.
    #[derive(Debug)]
    struct Marker {
        label: &'static str,
        calls: AtomicUsize,
    }

    impl Marker {
        fn new(label: &'static str) -> Arc<Self> {
            Arc::new(Self {
                label,
                calls: AtomicUsize::new(0),
            })
        }
    }

    impl ResolveBackend for Marker {
        fn resolve(
            &self,
            request: &Message,
            _ctx: &Ctx,
        ) -> impl std::future::Future<Output = Result<Message, ResolveError>> + Send {
            let id = request.metadata.id;
            async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                let mut response = Message::response(id, OpCode::Query);
                response.add_query(Query::query(
                    Name::from_ascii(format!("{}.", self.label)).expect("gültig"),
                    RecordType::A,
                ));
                Ok(response)
            }
        }
    }

    /// Ein Kontext für Tests, die sich nicht für den Trace interessieren.
    fn ctx() -> Ctx {
        Ctx::new(std::net::SocketAddr::from(([127, 0, 0, 1], 5555)))
    }

    fn ask(name: &str) -> Message {
        let mut message = Message::new(1, MessageType::Query, OpCode::Query);
        message.add_query(Query::query(
            Name::from_ascii(name).expect("gültiger Name"),
            RecordType::A,
        ));
        message
    }

    fn zone(name: &str) -> Name {
        Name::from_ascii(name).expect("gültige Zone")
    }

    #[tokio::test]
    async fn a_name_inside_the_zone_goes_to_the_lan_server() {
        let lan = Marker::new("lan");
        let internet = Marker::new("internet");
        let router = ZoneRouter::new(
            vec![(zone("home.arpa."), Arc::clone(&lan))],
            Arc::clone(&internet),
        );

        router
            .resolve(&ask("nas.home.arpa."), &ctx())
            .await
            .expect("Antwort");
        assert_eq!(lan.calls.load(Ordering::SeqCst), 1);
        assert_eq!(internet.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn the_zone_itself_also_matches() {
        let lan = Marker::new("lan");
        let internet = Marker::new("internet");
        let router = ZoneRouter::new(
            vec![(zone("home.arpa."), Arc::clone(&lan))],
            Arc::clone(&internet),
        );

        router
            .resolve(&ask("home.arpa."), &ctx())
            .await
            .expect("Antwort");
        assert_eq!(lan.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn everything_else_goes_to_the_encrypted_pool() {
        let lan = Marker::new("lan");
        let internet = Marker::new("internet");
        let router = ZoneRouter::new(
            vec![(zone("home.arpa."), Arc::clone(&lan))],
            Arc::clone(&internet),
        );

        for name in [
            "example.com.",
            "arpa.",
            "nothome.arpa.",
            "home.arpa.evil.com.",
        ] {
            router.resolve(&ask(name), &ctx()).await.expect("Antwort");
        }
        assert_eq!(
            lan.calls.load(Ordering::SeqCst),
            0,
            "eine Anfrage ging ins LAN"
        );
        assert_eq!(internet.calls.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn the_most_specific_zone_wins() {
        let broad = Marker::new("broad");
        let narrow = Marker::new("narrow");
        let internet = Marker::new("internet");
        // Bewusst in der "falschen" Reihenfolge übergeben.
        let router = ZoneRouter::new(
            vec![
                (zone("home.arpa."), Arc::clone(&broad)),
                (zone("dev.home.arpa."), Arc::clone(&narrow)),
            ],
            internet,
        );

        router
            .resolve(&ask("host.dev.home.arpa."), &ctx())
            .await
            .expect("Antwort");
        assert_eq!(narrow.calls.load(Ordering::SeqCst), 1);
        assert_eq!(broad.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn zone_matching_is_case_insensitive() {
        let lan = Marker::new("lan");
        let internet = Marker::new("internet");
        let router = ZoneRouter::new(
            vec![(zone("home.arpa."), Arc::clone(&lan))],
            Arc::clone(&internet),
        );

        router
            .resolve(&ask("NAS.Home.ARPA."), &ctx())
            .await
            .expect("Antwort");
        assert_eq!(lan.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn without_zones_everything_goes_to_the_default() {
        let internet = Marker::new("internet");
        let router: ZoneRouter<Arc<Marker>, Arc<Marker>> =
            ZoneRouter::new(Vec::new(), Arc::clone(&internet));
        router
            .resolve(&ask("example.com."), &ctx())
            .await
            .expect("Antwort");
        assert_eq!(internet.calls.load(Ordering::SeqCst), 1);
    }
}
