//! Geography for display and travel: WGS84 longitude/latitude to and from
//! the region frame, and the sun's position.
//!
//! The region frame (RFC 0001) is the topocentric East-North-Up frame at
//! the region's anchor on the WGS84 ellipsoid, with glTF axes: X east,
//! Y up, Z south, in metres. The ingest builds it with PROJ; this is the
//! same transformation written out.
//!
//! Client only: the simulation never uses these functions (they use
//! platform floating-point functions, which ADR-0003 keeps out of the
//! simulation).

use op_world::world::AnchorE9;

const A: f64 = 6_378_137.0;
const F: f64 = 1.0 / 298.257_223_563;
const E2: f64 = F * (2.0 - F);

/// WGS84 geodetic (radians, metres) to Earth-centred coordinates.
fn ecef(lat: f64, lon: f64, h: f64) -> [f64; 3] {
    let (sl, cl) = lat.sin_cos();
    let n = A / (1.0 - E2 * sl * sl).sqrt();
    let (so, co) = lon.sin_cos();
    [
        (n + h) * cl * co,
        (n + h) * cl * so,
        (n * (1.0 - E2) + h) * sl,
    ]
}

/// The region frame of a world.
#[derive(Clone, Copy, Debug)]
pub struct RegionFrame {
    lat0: f64,
    lon0: f64,
    origin: [f64; 3],
}

impl RegionFrame {
    pub fn new(anchor: AnchorE9) -> Self {
        let lat0 = (anchor.lat as f64 / 1e9).to_radians();
        let lon0 = (anchor.lon as f64 / 1e9).to_radians();
        RegionFrame {
            lat0,
            lon0,
            origin: ecef(lat0, lon0, 0.0),
        }
    }

    /// Longitude and latitude (degrees) and ellipsoidal height (m) to the
    /// region frame (m).
    pub fn project(&self, lon: f64, lat: f64, h: f64) -> [f64; 3] {
        let p = ecef(lat.to_radians(), lon.to_radians(), h);
        let d = [
            p[0] - self.origin[0],
            p[1] - self.origin[1],
            p[2] - self.origin[2],
        ];
        let (sp, cp) = self.lat0.sin_cos();
        let (sl, cl) = self.lon0.sin_cos();
        let e = -sl * d[0] + cl * d[1];
        let n = -sp * cl * d[0] - sp * sl * d[1] + cp * d[2];
        let u = cp * cl * d[0] + cp * sl * d[1] + sp * d[2];
        [e, u, -n]
    }

    /// The region frame (m) to longitude, latitude (degrees) and
    /// ellipsoidal height (m).
    pub fn unproject(&self, p: [f64; 3]) -> (f64, f64, f64) {
        let (e, u, n) = (p[0], p[1], -p[2]);
        let (sp, cp) = self.lat0.sin_cos();
        let (sl, cl) = self.lon0.sin_cos();
        let x = self.origin[0] - sl * e - sp * cl * n + cp * cl * u;
        let y = self.origin[1] + cl * e - sp * sl * n + cp * sl * u;
        let z = self.origin[2] + cp * n + sp * u;
        let lon = y.atan2(x);
        let r = x.hypot(y);
        let mut lat = z.atan2(r * (1.0 - E2));
        let mut h = 0.0;
        for _ in 0..6 {
            let s = lat.sin();
            let nn = A / (1.0 - E2 * s * s).sqrt();
            h = r / lat.cos() - nn;
            lat = z.atan2(r * (1.0 - E2 * nn / (nn + h)));
        }
        (lon.to_degrees(), lat.to_degrees(), h)
    }
}

/// A civil date and time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    /// Minutes since midnight.
    pub minute: u32,
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = i64::from(if m <= 2 { y - 1 } else { y });
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a day number from [`days_from_civil`].
pub fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = (yoe + era * 400 + i64::from(m <= 2)) as i32;
    (y, m, d)
}

/// The last Sunday of a month, as a day number.
fn last_sunday(year: i32, month: u32) -> i64 {
    let next = if month == 12 {
        days_from_civil(year + 1, 1, 1)
    } else {
        days_from_civil(year, month + 1, 1)
    };
    let last = next - 1;
    // 1970-01-01 was a Thursday (weekday 4, Sunday = 0).
    last - (last + 4).rem_euclid(7)
}

/// Minutes to add to UTC for Swiss civil time (CET, or CEST from the last
/// Sunday in March to the last Sunday in October, at 01:00 UTC).
pub fn swiss_offset_minutes(utc_days: i64, utc_minute: u32) -> i64 {
    let (year, _, _) = civil_from_days(utc_days);
    let t = utc_days * 1440 + i64::from(utc_minute);
    let start = last_sunday(year, 3) * 1440 + 60;
    let end = last_sunday(year, 10) * 1440 + 60;
    if (start..end).contains(&t) { 120 } else { 60 }
}

/// UTC (days since 1970, minute of day) of a Swiss civil date and time.
pub fn swiss_to_utc(local: DateTime) -> (i64, u32) {
    let local_t =
        days_from_civil(local.year, local.month, local.day) * 1440 + i64::from(local.minute);
    // The offset at the UTC instant one hour earlier decides; around the
    // changeover this picks the earlier reading, which is good enough for
    // lighting.
    let guess = local_t - 60;
    let offset = swiss_offset_minutes(guess.div_euclid(1440), guess.rem_euclid(1440) as u32);
    let t = local_t - offset;
    (t.div_euclid(1440), t.rem_euclid(1440) as u32)
}

/// The sun's azimuth (degrees clockwise from north) and elevation above the
/// horizon (degrees, without refraction) at a UTC instant, seen from a
/// longitude and latitude. NOAA's simplified algorithm, about 0.1 degree.
pub fn sun_position(utc_days: i64, utc_minute: u32, lon: f64, lat: f64) -> (f64, f64) {
    let jd = utc_days as f64 + 2_440_587.5 + f64::from(utc_minute) / 1440.0;
    let t = (jd - 2_451_545.0) / 36_525.0;
    let l0 = (280.466_46 + t * (36_000.769_83 + t * 0.000_303_2)).rem_euclid(360.0);
    let m = 357.529_11 + t * (35_999.050_29 - 0.000_153_7 * t);
    let e = 0.016_708_634 - t * (0.000_042_037 + 0.000_000_126_7 * t);
    let mr = m.to_radians();
    let c = mr.sin() * (1.914_602 - t * (0.004_817 + 0.000_014 * t))
        + (2.0 * mr).sin() * (0.019_993 - 0.000_101 * t)
        + (3.0 * mr).sin() * 0.000_289;
    let true_long = l0 + c;
    let omega = 125.04 - 1_934.136 * t;
    let lambda = true_long - 0.005_69 - 0.004_78 * omega.to_radians().sin();
    let eps0 =
        23.0 + (26.0 + (21.448 - t * (46.815 + t * (0.000_59 - t * 0.001_813))) / 60.0) / 60.0;
    let eps = eps0 + 0.002_56 * omega.to_radians().cos();
    let decl = (eps.to_radians().sin() * lambda.to_radians().sin()).asin();
    let y = (eps.to_radians() / 2.0).tan().powi(2);
    let l0r = l0.to_radians();
    let eq_time = 4.0
        * (y * (2.0 * l0r).sin() - 2.0 * e * mr.sin() + 4.0 * e * y * mr.sin() * (2.0 * l0r).cos()
            - 0.5 * y * y * (4.0 * l0r).sin()
            - 1.25 * e * e * (2.0 * mr).sin())
        .to_degrees();
    let true_solar_minutes = (f64::from(utc_minute) + eq_time + 4.0 * lon).rem_euclid(1440.0);
    let hour_angle = (true_solar_minutes / 4.0 - 180.0).to_radians();
    let latr = lat.to_radians();
    let cos_zenith =
        (latr.sin() * decl.sin() + latr.cos() * decl.cos() * hour_angle.cos()).clamp(-1.0, 1.0);
    let zenith = cos_zenith.acos();
    let elevation = 90.0 - zenith.to_degrees();
    let az_denominator = latr.cos() * zenith.sin();
    let azimuth = if az_denominator.abs() < 1e-9 {
        180.0
    } else {
        let a = ((latr.sin() * cos_zenith - decl.sin()) / az_denominator).clamp(-1.0, 1.0);
        let a = a.acos().to_degrees();
        if hour_angle > 0.0 {
            (a + 180.0).rem_euclid(360.0)
        } else {
            (540.0 - a).rem_euclid(360.0)
        }
    };
    (azimuth, elevation)
}

/// The Moon seen from a place.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MoonPosition {
    /// Degrees clockwise from north.
    pub azimuth: f64,
    /// Degrees above the horizon, seen from the ground (topocentric),
    /// without refraction.
    pub elevation: f64,
    /// The share of its disc that is lit, 0 (new) to 1 (full).
    pub illuminated: f64,
    /// The angle Sun-Moon-Earth (degrees): 0 at full moon, 180 at new moon.
    pub phase_angle: f64,
}

/// The Moon's position and phase at a UTC instant, seen from a longitude
/// and latitude (degrees).
///
/// The Astronomical Almanac's low-precision formulae for the Moon's
/// ecliptic longitude, latitude and horizontal parallax (section D, "Low
/// precision formulas for the Moon's coordinates"; about 0.3° in longitude
/// and 0.2° in latitude), turned to the horizon with the mean obliquity and
/// mean sidereal time, then corrected for parallax, which lowers the Moon
/// by up to a degree near the horizon. The Sun's longitude for the phase
/// comes from the Almanac's low-precision formula for the Sun (section C).
/// Within about a quarter of a degree of JPL Horizons in the tests below;
/// rendering only.
pub fn moon_position(utc_days: i64, utc_minute: u32, lon: f64, lat: f64) -> MoonPosition {
    let sin = |deg: f64| deg.to_radians().sin();
    let cos = |deg: f64| deg.to_radians().cos();
    // Days and Julian centuries from J2000.0 (2000-01-01 12:00 UT).
    let d = (utc_days - 10_957) as f64 + (f64::from(utc_minute) - 720.0) / 1440.0;
    let t = d / 36_525.0;
    let lambda = 218.32 + 481_267.881 * t + 6.29 * sin(135.0 + 477_198.87 * t)
        - 1.27 * sin(259.3 - 413_335.36 * t)
        + 0.66 * sin(235.7 + 890_534.22 * t)
        + 0.21 * sin(269.9 + 954_397.74 * t)
        - 0.19 * sin(357.5 + 35_999.05 * t)
        - 0.11 * sin(186.5 + 966_404.03 * t);
    let beta = 5.13 * sin(93.3 + 483_202.02 * t) + 0.28 * sin(228.2 + 960_400.89 * t)
        - 0.28 * sin(318.3 + 6_003.15 * t)
        - 0.17 * sin(217.6 - 407_332.21 * t);
    let parallax = 0.9508
        + 0.0518 * cos(135.0 + 477_198.87 * t)
        + 0.0095 * cos(259.3 - 413_335.36 * t)
        + 0.0078 * cos(235.7 + 890_534.22 * t)
        + 0.0028 * cos(269.9 + 954_397.74 * t);
    // Ecliptic to equatorial, with the mean obliquity.
    let eps = 23.439 - 0.013 * t;
    let (l, m, n) = (
        cos(beta) * cos(lambda),
        cos(eps) * cos(beta) * sin(lambda) - sin(eps) * sin(beta),
        sin(eps) * cos(beta) * sin(lambda) + cos(eps) * sin(beta),
    );
    let ra = m.atan2(l).to_degrees();
    let dec = n.clamp(-1.0, 1.0).asin().to_degrees();
    // Local hour angle from the mean sidereal time at Greenwich.
    let gmst = 280.460_618_37 + 360.985_647_366_29 * d;
    let h = gmst + lon - ra;
    let geocentric = (sin(lat) * sin(dec) + cos(lat) * cos(dec) * cos(h))
        .clamp(-1.0, 1.0)
        .asin()
        .to_degrees();
    let azimuth = (-cos(dec) * sin(h))
        .atan2(sin(dec) * cos(lat) - cos(dec) * sin(lat) * cos(h))
        .to_degrees()
        .rem_euclid(360.0);
    // The Sun's ecliptic longitude, for the elongation and the phase.
    let (mean_long, anomaly) = (280.460 + 0.985_647_4 * d, 357.528 + 0.985_600_3 * d);
    let sun = mean_long + 1.915 * sin(anomaly) + 0.020 * sin(2.0 * anomaly);
    let elongation = (cos(beta) * cos(lambda - sun))
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees();
    let phase_angle = 180.0 - elongation;
    MoonPosition {
        azimuth,
        elevation: geocentric - parallax * cos(geocentric),
        illuminated: (1.0 + cos(phase_angle)) / 2.0,
        phase_angle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture_world() -> op_world::world::WorldManifest {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/world/world.json");
        op_world::world::WorldManifest::from_bytes(&std::fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn matches_the_ingest_frame_at_the_fixture_spawn() {
        // worlds/fixture-grossmuenster.json puts the spawn at 8.544082,
        // 47.369816; the ingest (PROJ) wrote its frame position.
        let w = fixture_world();
        let frame = RegionFrame::new(w.region.anchor_e9);
        let p = w.spawn.position_mm.map(|v| v as f64 / 1000.0);
        let (lon, lat, h) = frame.unproject(p);
        assert!(
            (lon - 8.544_082).abs() < 1e-8 && (lat - 47.369_816).abs() < 1e-8,
            "{lon} {lat}"
        );
        let back = frame.project(lon, lat, h);
        for k in 0..3 {
            assert!((back[k] - p[k]).abs() < 1e-6, "{back:?} {p:?}");
        }
    }

    #[test]
    fn calendar_round_trips() {
        for (y, m, d) in [(1970, 1, 1), (2000, 2, 29), (2026, 11, 24), (2100, 12, 31)] {
            assert_eq!(civil_from_days(days_from_civil(y, m, d)), (y, m, d));
        }
        assert_eq!(days_from_civil(1970, 1, 2), 1);
        // Swiss summer time 2026: 29 March to 25 October.
        assert_eq!(last_sunday(2026, 3), days_from_civil(2026, 3, 29));
        assert_eq!(last_sunday(2026, 10), days_from_civil(2026, 10, 25));
        let noon = DateTime {
            year: 2026,
            month: 7,
            day: 1,
            minute: 12 * 60,
        };
        assert_eq!(swiss_to_utc(noon), (days_from_civil(2026, 7, 1), 10 * 60));
        let winter = DateTime {
            year: 2026,
            month: 11,
            day: 24,
            minute: 14 * 60,
        };
        assert_eq!(
            swiss_to_utc(winter),
            (days_from_civil(2026, 11, 24), 13 * 60)
        );
    }

    #[test]
    fn sun_over_zurich() {
        let (lon, lat) = (8.54, 47.37);
        // Highest at the June solstice: 90 - 47.37 + 23.44 = 66.07 degrees,
        // due south, at about 11:26 UTC.
        let june = days_from_civil(2026, 6, 21);
        let best = (600..780)
            .map(|m| (m, sun_position(june, m, lon, lat)))
            .max_by(|a, b| a.1.1.total_cmp(&b.1.1))
            .unwrap();
        assert!((best.1.1 - 66.07).abs() < 0.2, "{best:?}");
        assert!((best.1.0 - 180.0).abs() < 2.0, "{best:?}");
        assert!((680..=692).contains(&best.0), "{best:?}");
        // At the December solstice noon it stays below 20 degrees.
        let dec = days_from_civil(2026, 12, 21);
        let (_, el) = sun_position(dec, 11 * 60 + 30, lon, lat);
        assert!((el - 19.2).abs() < 0.3, "{el}");
        // It rises in the east and is below the horizon at midnight.
        let (az, el) = sun_position(june, 4 * 60, lon, lat);
        assert!(el > 0.0 && (40.0..80.0).contains(&az), "{az} {el}");
        assert!(sun_position(june, 23 * 60, lon, lat).1 < -10.0);
    }

    /// Against JPL Horizons (DE441 ephemeris; observer at 8.541° E,
    /// 47.372° N, 430 m; airless apparent azimuth and elevation, and the
    /// illuminated share; queried 2026-11-25 at
    /// <https://ssd.jpl.nasa.gov/horizons/>): within a degree, as a
    /// low-precision formula allows, and the phase within a few percent.
    #[test]
    fn moon_over_zurich_matches_horizons() {
        // (UTC date, minute, Horizons azimuth, elevation, illuminated %).
        let cases = [
            // 18:00 Swiss time on 21 December 2026: waxing gibbous, east.
            ((2026, 12, 21), 17 * 60, 94.662, 37.459, 91.958),
            // 22:30 Swiss time on 21 June 2026: first quarter, south-west.
            ((2026, 6, 21), 20 * 60 + 30, 234.473, 24.775, 49.384),
            // The 2025 Lindenhof photos: a new moon below the horizon.
            ((2025, 3, 28), 18 * 60 + 30, 287.637, -17.090, 0.841),
            ((2026, 11, 25), 20 * 60, 86.467, 34.854, 97.538),
        ];
        for ((y, m, d), minute, az, el, lit) in cases {
            let p = moon_position(days_from_civil(y, m, d), minute, 8.541, 47.372);
            let daz = (p.azimuth - az + 540.0).rem_euclid(360.0) - 180.0;
            assert!(
                daz.abs() < 1.0,
                "{y}-{m}-{d}: azimuth {} against {az}",
                p.azimuth
            );
            assert!(
                (p.elevation - el).abs() < 1.0,
                "{y}-{m}-{d}: elevation {} against {el}",
                p.elevation
            );
            assert!(
                (p.illuminated * 100.0 - lit).abs() < 3.0,
                "{y}-{m}-{d}: {}% against {lit}%",
                p.illuminated * 100.0
            );
        }
        // Full moon is near 0° of phase angle, new moon near 180°.
        let full = moon_position(days_from_civil(2026, 11, 24), 15 * 60, 8.541, 47.372);
        assert!(full.phase_angle < 10.0, "{full:?}");
    }
}
