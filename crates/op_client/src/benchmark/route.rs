//! Benchmark route files (ADR-0018): one per world, in `worlds/`.
//!
//! ```json
//! {
//!   "format": "streetwide.route",
//!   "version": 1,
//!   "world": "zurich-altstadt",
//!   "description": "optional text",
//!   "stops": [
//!     {"name": "Lindenhof, 21 June, 18:00", "place": "Lindenhof",
//!      "date": "2026-06-21", "time": "18:00"},
//!     {"name": "A corner on a Saturday", "lon": 8.5412, "lat": 47.3711,
//!      "yaw_deg": 45, "date": "06-21", "day_type": "saturday", "time": "18:00"}
//!   ]
//! }
//! ```
//!
//! A stop is a named place of the world (`place`, with the place's
//! position and heading, `yaw_deg` overriding the heading) or a position
//! (`lon`, `lat` and `yaw_deg`, degrees clockwise from north), and a date
//! and a time in Swiss civil time. The date is a full date (`YYYY-MM-DD`),
//! whose day type follows from it; or a month and day (`MM-DD`) with a day
//! type (`weekday`, `saturday` or `sunday`), which the client draws on the
//! first date with that month and day, from 2026 on, that has that day type
//! (ADR-0021: a stop shows the same people on every run only if it gives a
//! full date or a day type). Holidays are not day types yet: RFC 0005
//! carries them per tile, and the list is empty. Unknown keys are refused.

use serde_json::Value;

use crate::geo::{self, DateTime};
use crate::travel::Place;

pub const FORMAT: &str = "streetwide.route";
pub const VERSION: u64 = 1;
/// A month and day with a day type is drawn in the first year from this
/// one that has that day type on that date.
pub const DAY_TYPE_FIRST_YEAR: i32 = 2026;

/// The days a crowd's profile tells apart (ADR-0021).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DayType {
    /// Monday to Friday.
    Weekday,
    Saturday,
    Sunday,
}

impl DayType {
    pub fn name(self) -> &'static str {
        match self {
            DayType::Weekday => "weekday",
            DayType::Saturday => "saturday",
            DayType::Sunday => "sunday",
        }
    }

    fn parse(text: &str) -> Option<DayType> {
        match text {
            "weekday" => Some(DayType::Weekday),
            "saturday" => Some(DayType::Saturday),
            "sunday" => Some(DayType::Sunday),
            _ => None,
        }
    }

    /// The day type of a date.
    pub fn of(year: i32, month: u32, day: u32) -> DayType {
        // 1970-01-01, day 0, was a Thursday; 0 is Sunday here.
        match (geo::days_from_civil(year, month, day) + 4).rem_euclid(7) {
            0 => DayType::Sunday,
            6 => DayType::Saturday,
            _ => DayType::Weekday,
        }
    }
}

/// A route as written in its file.
#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    pub world: String,
    pub description: Option<String>,
    pub stops: Vec<StopSpec>,
}

/// A stop as written: a place or a position, and when.
#[derive(Clone, Debug, PartialEq)]
pub struct StopSpec {
    pub name: String,
    pub at: Where,
    /// Overrides the place's heading; required with a position (degrees).
    pub yaw_deg: Option<f64>,
    pub when: When,
    /// Minutes since midnight, Swiss time.
    pub minute: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Where {
    Place(String),
    Position { lon: f64, lat: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum When {
    Date {
        year: i32,
        month: u32,
        day: u32,
    },
    DayType {
        month: u32,
        day: u32,
        day_type: DayType,
    },
}

/// A stop ready to visit: where, facing which way, and the clock.
#[derive(Clone, Debug, PartialEq)]
pub struct Stop {
    pub name: String,
    /// The world's place it is at, if one.
    pub place: Option<String>,
    pub lon: f64,
    pub lat: f64,
    /// Degrees clockwise from north.
    pub yaw_deg: f64,
    /// The clock's date and time.
    pub clock: DateTime,
    pub day_type: DayType,
    /// The date as the route gives it: `YYYY-MM-DD`, or `MM-DD` with a day
    /// type.
    pub date_given: String,
}

impl Stop {
    pub fn date(&self) -> String {
        let d = self.clock;
        format!("{:04}-{:02}-{:02}", d.year, d.month, d.day)
    }

    pub fn time(&self) -> String {
        format!(
            "{:02}:{:02}",
            self.clock.minute / 60,
            self.clock.minute % 60
        )
    }
}

/// Whether a date exists.
fn valid_date(year: i32, month: u32, day: u32) -> bool {
    (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && geo::civil_from_days(geo::days_from_civil(year, month, day)) == (year, month, day)
}

/// `YYYY-MM-DD`, or `MM-DD` with a day type.
fn parse_date(text: &str) -> Option<(Option<i32>, u32, u32)> {
    let parts: Vec<&str> = text.split('-').collect();
    let num = |s: &str, len: usize| {
        (s.len() == len && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse::<u32>().ok())
            .flatten()
    };
    match parts[..] {
        [y, m, d] => {
            let year = i32::try_from(num(y, 4)?).ok()?;
            let (month, day) = (num(m, 2)?, num(d, 2)?);
            valid_date(year, month, day).then_some((Some(year), month, day))
        }
        // Any leap year will do to check a month and day.
        [m, d] => {
            let (month, day) = (num(m, 2)?, num(d, 2)?);
            valid_date(2028, month, day).then_some((None, month, day))
        }
        _ => None,
    }
}

/// `HH:MM`.
fn parse_time(text: &str) -> Option<u32> {
    let (h, m) = text.split_once(':')?;
    if h.len() != 2 || m.len() != 2 || !(h.bytes().chain(m.bytes()).all(|b| b.is_ascii_digit())) {
        return None;
    }
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// The first date from [`DAY_TYPE_FIRST_YEAR`] on with this month, day
/// and day type.
pub fn resolve_day_type(month: u32, day: u32, day_type: DayType) -> Option<i32> {
    (DAY_TYPE_FIRST_YEAR..DAY_TYPE_FIRST_YEAR + 400)
        .find(|&y| valid_date(y, month, day) && DayType::of(y, month, day) == day_type)
}

fn check_keys(object: &serde_json::Map<String, Value>, allowed: &[&str]) -> Result<(), String> {
    match object.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(format!("unknown key \"{k}\"")),
        None => Ok(()),
    }
}

fn degrees(v: &Value, key: &str) -> Result<Option<f64>, String> {
    match v.get(key) {
        None => Ok(None),
        Some(x) => x
            .as_f64()
            .filter(|d| d.is_finite())
            .map(Some)
            .ok_or(format!("\"{key}\" needs a number")),
    }
}

impl StopSpec {
    fn parse(v: &Value) -> Result<StopSpec, String> {
        let object = v.as_object().ok_or("a stop is an object")?;
        check_keys(
            object,
            &[
                "name", "place", "lon", "lat", "yaw_deg", "date", "day_type", "time",
            ],
        )?;
        let name = v["name"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .ok_or("a stop needs a \"name\"")?
            .to_string();
        let (lon, lat) = (degrees(v, "lon")?, degrees(v, "lat")?);
        let yaw_deg = degrees(v, "yaw_deg")?;
        let at = match (v.get("place"), lon, lat) {
            (Some(p), None, None) => Where::Place(
                p.as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or("\"place\" needs a name")?
                    .to_string(),
            ),
            (None, Some(lon), Some(lat)) => {
                if !(-180.0..=180.0).contains(&lon) || !(-90.0..=90.0).contains(&lat) {
                    return Err("\"lon\" and \"lat\" need degrees".into());
                }
                if yaw_deg.is_none() {
                    return Err("a position needs a heading, \"yaw_deg\"".into());
                }
                Where::Position { lon, lat }
            }
            _ => return Err("a stop needs a \"place\", or a \"lon\" and a \"lat\"".into()),
        };
        let date = v["date"]
            .as_str()
            .ok_or("a stop needs a \"date\", YYYY-MM-DD or MM-DD")?;
        let (year, month, day) =
            parse_date(date).ok_or(format!("\"date\" {date:?} is not YYYY-MM-DD or MM-DD"))?;
        let day_type = match v.get("day_type") {
            None => None,
            Some(d) => Some(
                d.as_str()
                    .and_then(DayType::parse)
                    .ok_or("\"day_type\" needs weekday, saturday or sunday")?,
            ),
        };
        let when = match (year, day_type) {
            (Some(year), None) => When::Date { year, month, day },
            (None, Some(day_type)) => When::DayType {
                month,
                day,
                day_type,
            },
            (Some(_), Some(_)) => {
                return Err("a full date fixes the day type: give a date or a day type".into());
            }
            (None, None) => {
                return Err("a month and day (MM-DD) needs a \"day_type\"".into());
            }
        };
        let time = v["time"].as_str().ok_or("a stop needs a \"time\", HH:MM")?;
        let minute = parse_time(time).ok_or(format!("\"time\" {time:?} is not HH:MM"))?;
        Ok(StopSpec {
            name,
            at,
            yaw_deg,
            when,
            minute,
        })
    }
}

impl Route {
    pub fn parse(text: &str) -> Result<Route, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
        let object = v.as_object().ok_or("a route is a JSON object")?;
        check_keys(
            object,
            &["format", "version", "world", "description", "stops"],
        )?;
        if v["format"] != FORMAT {
            return Err(format!("\"format\" must be \"{FORMAT}\""));
        }
        if v["version"].as_u64() != Some(VERSION) {
            return Err(format!("\"version\" must be {VERSION}"));
        }
        let world = v["world"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("a route needs its \"world\"")?
            .to_string();
        let description = match v.get("description") {
            None => None,
            Some(d) => Some(d.as_str().ok_or("\"description\" needs text")?.to_string()),
        };
        let stops = v["stops"]
            .as_array()
            .filter(|s| !s.is_empty())
            .ok_or("a route needs at least one stop in \"stops\"")?
            .iter()
            .enumerate()
            .map(|(i, s)| {
                StopSpec::parse(s).map_err(|e| match s["name"].as_str() {
                    Some(name) => format!("stop {} ({name}): {e}", i + 1),
                    None => format!("stop {}: {e}", i + 1),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Route {
            world,
            description,
            stops,
        })
    }

    /// The stops with their places' positions and headings, and their
    /// clocks. A place is named exactly as in the world definition.
    pub fn resolve(&self, places: &[Place]) -> Result<Vec<Stop>, String> {
        self.stops
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let fail = |e: String| format!("stop {} ({}): {e}", i + 1, s.name);
                let (place, lon, lat, yaw) = match &s.at {
                    Where::Place(name) => {
                        let p = places.iter().find(|p| &p.name == name).ok_or_else(|| {
                            fail(format!("no place \"{name}\" in world {}", self.world))
                        })?;
                        (Some(p.name.clone()), p.lon, p.lat, p.yaw_deg)
                    }
                    Where::Position { lon, lat } => (None, *lon, *lat, 0.0),
                };
                let (year, month, day, day_type, date_given) = match s.when {
                    When::Date { year, month, day } => (
                        year,
                        month,
                        day,
                        DayType::of(year, month, day),
                        format!("{year:04}-{month:02}-{day:02}"),
                    ),
                    When::DayType {
                        month,
                        day,
                        day_type,
                    } => {
                        let year = resolve_day_type(month, day, day_type).ok_or_else(|| {
                            fail(format!("no {} on {month:02}-{day:02}", day_type.name()))
                        })?;
                        (year, month, day, day_type, format!("{month:02}-{day:02}"))
                    }
                };
                Ok(Stop {
                    name: s.name.clone(),
                    place,
                    lon,
                    lat,
                    yaw_deg: s.yaw_deg.unwrap_or(yaw),
                    clock: DateTime {
                        year,
                        month,
                        day,
                        minute: s.minute,
                    },
                    day_type,
                    date_given,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::travel::load_places;

    fn repo() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn route(stops: &str) -> Result<Route, String> {
        Route::parse(&format!(
            r#"{{"format": "streetwide.route", "version": 1, "world": "w", "stops": [{stops}]}}"#
        ))
    }

    fn place(name: &str, lon: f64, lat: f64, yaw_deg: f64) -> Place {
        Place {
            name: name.into(),
            lon,
            lat,
            yaw_deg,
        }
    }

    #[test]
    fn day_types_follow_the_calendar() {
        // ADR-0021: 21 June is a Sunday in 2026 and a Monday in 2027.
        assert_eq!(DayType::of(2026, 6, 21), DayType::Sunday);
        assert_eq!(DayType::of(2027, 6, 21), DayType::Weekday);
        assert_eq!(DayType::of(2026, 12, 21), DayType::Weekday);
        assert_eq!(DayType::of(2026, 12, 19), DayType::Saturday);
        assert_eq!(DayType::of(1970, 1, 1), DayType::Weekday);
        assert_eq!(DayType::of(1970, 1, 4), DayType::Sunday);
        // The first year from 2026 with 21 June on a Saturday is 2031.
        assert_eq!(resolve_day_type(6, 21, DayType::Saturday), Some(2031));
        assert_eq!(resolve_day_type(6, 21, DayType::Sunday), Some(2026));
        assert_eq!(resolve_day_type(6, 21, DayType::Weekday), Some(2027));
        // 29 February needs a leap year with the day type: 2032 is a Sunday.
        assert_eq!(resolve_day_type(2, 29, DayType::Sunday), Some(2032));
        assert_eq!(DayType::of(2032, 2, 29), DayType::Sunday);
    }

    #[test]
    fn stops_at_places_and_positions() {
        let r = route(
            r#"{"name": "A", "place": "Square", "date": "2026-06-21", "time": "18:00"},
               {"name": "B", "place": "Square", "yaw_deg": 45, "date": "12-21", "day_type": "saturday", "time": "07:05"},
               {"name": "C", "lon": 8.5, "lat": 47.4, "yaw_deg": 270.5, "date": "2027-02-28", "time": "23:59"}"#,
        )
        .unwrap();
        assert_eq!(r.world, "w");
        assert_eq!(r.description, None);
        let stops = r.resolve(&[place("Square", 8.54, 47.37, 90.0)]).unwrap();
        assert_eq!(stops.len(), 3);
        let a = &stops[0];
        assert_eq!(
            (a.place.as_deref(), a.lon, a.lat, a.yaw_deg),
            (Some("Square"), 8.54, 47.37, 90.0)
        );
        assert_eq!((a.date(), a.time()), ("2026-06-21".into(), "18:00".into()));
        assert_eq!(
            (a.day_type, a.date_given.as_str()),
            (DayType::Sunday, "2026-06-21")
        );
        let b = &stops[1];
        assert_eq!(b.yaw_deg, 45.0);
        // 21 December is first a Saturday in 2030.
        assert_eq!((b.date(), b.time()), ("2030-12-21".into(), "07:05".into()));
        assert_eq!(
            (b.day_type, b.date_given.as_str()),
            (DayType::Saturday, "12-21")
        );
        assert_eq!(DayType::of(2030, 12, 21), DayType::Saturday);
        let c = &stops[2];
        assert_eq!(
            (c.place.as_deref(), c.lon, c.lat, c.yaw_deg),
            (None, 8.5, 47.4, 270.5)
        );
        assert_eq!((c.date(), c.time()), ("2027-02-28".into(), "23:59".into()));
        assert_eq!(c.day_type, DayType::Sunday);
    }

    #[test]
    fn bad_routes_are_refused_with_the_stop_named() {
        let ok = r#""name": "S", "place": "P", "time": "18:00""#;
        for (stop, why) in [
            (
                r#"{"place": "P", "date": "2026-06-21", "time": "18:00"}"#,
                "needs a \"name\"",
            ),
            (
                r#"{"name": "S", "date": "2026-06-21", "time": "18:00"}"#,
                "needs a \"place\"",
            ),
            (
                r#"{"name": "S", "place": "P", "lon": 8.5, "lat": 47.0, "date": "2026-06-21", "time": "18:00"}"#,
                "needs a \"place\", or",
            ),
            (
                r#"{"name": "S", "lon": 8.5, "lat": 47.0, "date": "2026-06-21", "time": "18:00"}"#,
                "needs a heading",
            ),
            (
                r#"{"name": "S", "lon": 8.5, "date": "2026-06-21", "time": "18:00"}"#,
                "needs a \"place\", or",
            ),
            (&format!("{{{ok}}}"), "needs a \"date\""),
            (
                &format!(r#"{{{ok}, "date": "2026-02-30"}}"#),
                "is not YYYY-MM-DD",
            ),
            (
                &format!(r#"{{{ok}, "date": "2026-6-21"}}"#),
                "is not YYYY-MM-DD",
            ),
            (
                &format!(r#"{{{ok}, "date": "06-21"}}"#),
                "needs a \"day_type\"",
            ),
            (
                &format!(r#"{{{ok}, "date": "2026-06-21", "day_type": "sunday"}}"#),
                "give a date or a day type",
            ),
            (
                &format!(r#"{{{ok}, "date": "06-21", "day_type": "holiday"}}"#),
                "weekday, saturday or sunday",
            ),
            (
                r#"{"name": "S", "place": "P", "date": "2026-06-21", "time": "24:00"}"#,
                "is not HH:MM",
            ),
            (
                r#"{"name": "S", "place": "P", "date": "2026-06-21", "time": "18:00", "heading": 3}"#,
                "unknown key \"heading\"",
            ),
        ] {
            let e = route(stop).unwrap_err();
            assert!(e.starts_with("stop 1") && e.contains(why), "{stop}: {e}");
        }
        assert!(
            route(r#"{"name": "S", "place": "P", "date": "2026-06-21", "time": "18:00"}"#).is_ok()
        );
        for (text, why) in [
            ("[]", "a JSON object"),
            (
                r#"{"format": "streetwide.world", "version": 1, "world": "w", "stops": []}"#,
                "\"format\"",
            ),
            (
                r#"{"format": "streetwide.route", "version": 2, "world": "w", "stops": []}"#,
                "\"version\"",
            ),
            (
                r#"{"format": "streetwide.route", "version": 1, "stops": []}"#,
                "\"world\"",
            ),
            (
                r#"{"format": "streetwide.route", "version": 1, "world": "w", "stops": []}"#,
                "at least one stop",
            ),
            (
                r#"{"format": "streetwide.route", "version": 1, "world": "w", "stops": [], "name": "w"}"#,
                "unknown key \"name\"",
            ),
        ] {
            let e = Route::parse(text).unwrap_err();
            assert!(e.contains(why), "{text}: {e}");
        }
        // A place the world does not have.
        let r =
            route(r#"{"name": "S", "place": "Nowhere", "date": "2026-06-21", "time": "18:00"}"#)
                .unwrap();
        let e = r
            .resolve(&[place("Somewhere", 8.5, 47.0, 0.0)])
            .unwrap_err();
        assert!(
            e.contains("stop 1 (S): no place \"Nowhere\" in world w"),
            "{e}"
        );
    }

    /// ADR-0018's route for the Zurich district: Lindenhof, Hirschenplatz
    /// and Rathausbrücke, each on 21 June at 18:00 and 22:30 and on 21
    /// December at 18:00, at the world's own places. Its file is not a
    /// world definition, so the client's places still come from the world's.
    #[test]
    fn the_zurich_route_is_adr_0018s() {
        let worlds = repo().join("worlds");
        let text = std::fs::read_to_string(worlds.join("zurich-altstadt.route.json")).unwrap();
        let r = Route::parse(&text).unwrap();
        assert_eq!(r.world, "zurich-altstadt");
        // The world definition's places, not the route's.
        let places = load_places(&worlds, "zurich-altstadt");
        assert_eq!(places.first().map(|p| p.name.as_str()), Some("Lindenhof"));
        assert!(places.len() >= 9);
        let stops = r.resolve(&places).unwrap();
        let got: Vec<(String, String, String)> = stops
            .iter()
            .map(|s| (s.place.clone().unwrap(), s.date(), s.time()))
            .collect();
        let mut want = Vec::new();
        for p in ["Lindenhof", "Hirschenplatz", "Rathausbrücke"] {
            for (d, t) in [
                ("2026-06-21", "18:00"),
                ("2026-06-21", "22:30"),
                ("2026-12-21", "18:00"),
            ] {
                want.push((p.to_string(), d.to_string(), t.to_string()));
            }
        }
        assert_eq!(got, want);
        for s in &stops {
            let p = places
                .iter()
                .find(|p| Some(&p.name) == s.place.as_ref())
                .unwrap();
            assert_eq!((s.lon, s.lat, s.yaw_deg), (p.lon, p.lat, p.yaw_deg));
        }
        // Names are unique, for screenshots and reports.
        let mut names: Vec<&str> = stops.iter().map(|s| s.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), stops.len());
    }
}
