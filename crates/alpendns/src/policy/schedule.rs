//! Zeitfenster, in denen zusätzlich geblockt wird.
//!
//! Gerechnet wird in **Ortszeit**, nicht in UTC: "ab 21 Uhr" meint 21 Uhr hier,
//! auch im Sommer. Die Uhr kommt über [`crate::clock::WallClock`] herein, damit
//! ein Test zwei Uhrzeiten durchspielen kann, ohne zu warten.

use std::sync::Arc;

use jiff::Zoned;
use jiff::civil::{Time, Weekday};

use crate::trace::ScheduleEffect;

/// Ein benanntes Zeitfenster.
#[derive(Debug, Clone)]
pub struct Schedule {
    pub name: Arc<str>,
    pub days: Vec<Weekday>,
    pub from: Time,
    pub to: Time,
    pub effect: ScheduleEffect,
}

impl Schedule {
    /// Ob das Fenster gerade offen ist.
    ///
    /// **Fenster über Mitternacht** (`from > to`, etwa 21:00–07:00) gehören dem
    /// Tag, an dem sie *beginnen*. "Montag 21:00–07:00" ist also von Montag
    /// 21 Uhr bis Dienstag 7 Uhr aktiv — und Dienstagabend nicht, wenn Dienstag
    /// nicht in der Liste steht. Das ist die Lesart, die jemand meint, der
    /// "Montagabend" sagt.
    pub fn is_active(&self, now: &Zoned) -> bool {
        let today = now.weekday();
        let time = now.time();

        if self.from <= self.to {
            // Fenster innerhalb eines Tages.
            self.days.contains(&today) && time >= self.from && time < self.to
        } else {
            // Abendteil am Starttag …
            (self.days.contains(&today) && time >= self.from)
                // … und Morgenteil am Folgetag.
                || (self.days.contains(&today.previous()) && time < self.to)
        }
    }
}

/// Prüft alle Zeitpläne einer Policy und liefert den ersten aktiven.
pub fn first_active<'a>(schedules: &'a [Schedule], now: &Zoned) -> Option<&'a Schedule> {
    schedules.iter().find(|schedule| schedule.is_active(now))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Zoned {
        // Feste Zone, damit der Test unabhängig von der Maschine ist.
        format!("{text}[Europe/Vienna]")
            .parse()
            .expect("gültiger Zeitpunkt")
    }

    fn time(hour: i8, minute: i8) -> Time {
        Time::new(hour, minute, 0, 0).expect("gültige Uhrzeit")
    }

    fn bedtime(days: &[Weekday]) -> Schedule {
        Schedule {
            name: Arc::from("bedtime"),
            days: days.to_vec(),
            from: time(21, 0),
            to: time(7, 0),
            effect: ScheduleEffect::BlockAllExceptAllowlist,
        }
    }

    fn daytime(days: &[Weekday]) -> Schedule {
        Schedule {
            name: Arc::from("schule"),
            days: days.to_vec(),
            from: time(8, 0),
            to: time(16, 0),
            effect: ScheduleEffect::BlockAllExceptAllowlist,
        }
    }

    #[test]
    fn a_window_within_one_day_is_open_between_its_bounds() {
        // 2026-08-31 ist ein Montag.
        let schedule = daytime(&[Weekday::Monday]);
        assert!(!schedule.is_active(&at("2026-08-31T07:59:00")));
        assert!(schedule.is_active(&at("2026-08-31T08:00:00")));
        assert!(schedule.is_active(&at("2026-08-31T15:59:59")));
        assert!(
            !schedule.is_active(&at("2026-08-31T16:00:00")),
            "obere Grenze exklusiv"
        );
    }

    #[test]
    fn a_window_is_closed_on_days_it_does_not_cover() {
        let schedule = daytime(&[Weekday::Monday]);
        // Dienstag, gleiche Uhrzeit.
        assert!(!schedule.is_active(&at("2026-09-01T10:00:00")));
    }

    #[test]
    fn a_window_across_midnight_belongs_to_the_day_it_starts() {
        let schedule = bedtime(&[Weekday::Monday]);
        // Montagabend: offen.
        assert!(schedule.is_active(&at("2026-08-31T21:00:00")));
        assert!(schedule.is_active(&at("2026-08-31T23:59:00")));
        // Dienstag früh: immer noch dasselbe Fenster.
        assert!(schedule.is_active(&at("2026-09-01T00:30:00")));
        assert!(schedule.is_active(&at("2026-09-01T06:59:00")));
        assert!(
            !schedule.is_active(&at("2026-09-01T07:00:00")),
            "obere Grenze exklusiv"
        );
        // Dienstagabend gehört Dienstag — der steht nicht in der Liste.
        assert!(
            !schedule.is_active(&at("2026-09-01T22:00:00")),
            "das Fenster wanderte auf den falschen Tag"
        );
    }

    #[test]
    fn the_gap_between_two_nights_stays_open() {
        let schedule = bedtime(&[Weekday::Monday, Weekday::Tuesday]);
        assert!(
            !schedule.is_active(&at("2026-09-01T12:00:00")),
            "Dienstagmittag"
        );
    }

    #[test]
    fn consecutive_nights_both_work() {
        let schedule = bedtime(&[Weekday::Monday, Weekday::Tuesday]);
        assert!(
            schedule.is_active(&at("2026-08-31T22:00:00")),
            "Montagnacht"
        );
        assert!(
            schedule.is_active(&at("2026-09-01T02:00:00")),
            "Dienstag früh"
        );
        assert!(
            schedule.is_active(&at("2026-09-01T22:00:00")),
            "Dienstagnacht"
        );
        assert!(
            schedule.is_active(&at("2026-09-02T02:00:00")),
            "Mittwoch früh"
        );
        assert!(
            !schedule.is_active(&at("2026-09-02T22:00:00")),
            "Mittwochnacht"
        );
    }

    #[test]
    fn the_same_query_gets_two_answers_at_two_times() {
        // Der Nachweis aus der Roadmap, Schritt 4.
        let schedule = bedtime(&[Weekday::Monday]);
        assert!(!schedule.is_active(&at("2026-08-31T20:59:00")));
        assert!(schedule.is_active(&at("2026-08-31T21:01:00")));
    }

    #[test]
    fn the_first_active_schedule_is_returned() {
        let schedules = vec![daytime(&[Weekday::Monday]), bedtime(&[Weekday::Monday])];
        let found = first_active(&schedules, &at("2026-08-31T22:00:00"));
        assert_eq!(found.map(|s| &*s.name), Some("bedtime"));
        assert!(first_active(&schedules, &at("2026-08-31T18:00:00")).is_none());
    }

    #[test]
    fn a_window_survives_the_switch_to_winter_time() {
        // In der Nacht auf den 25.10.2026 wird die Uhr in Europa zurückgestellt;
        // 02:30 gibt es zweimal. Beide Male liegt sie im Fenster — was der
        // Nutzer erwartet, wenn er "bis 7 Uhr" sagt.
        let schedule = bedtime(&[Weekday::Saturday]);
        assert!(schedule.is_active(&at("2026-10-24T23:00:00")));
        assert!(schedule.is_active(&at("2026-10-25T05:00:00")));
    }
}
