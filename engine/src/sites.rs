//! Offline site presets: common New Zealand and Australian locations with
//! their coordinates and standard UTC offsets, so a user can start from a
//! realistic site without knowing the latitude and longitude.
//!
//! Offsets are standard time. The simulation has no daylight saving, so a
//! site in summer time should keep the standard offset, which is what the
//! preset gives.

use crate::Site;

/// A named location with the values the site inputs take.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SitePreset {
    /// Display name, city and country.
    pub name: &'static str,
    /// Degrees north (negative = southern hemisphere).
    pub latitude_deg: f64,
    /// Degrees east.
    pub longitude_deg: f64,
    /// Standard-time UTC offset, hours.
    pub timezone_offset_hours: f64,
}

impl SitePreset {
    /// The preset as a [`Site`], at the given altitude.
    pub fn site(&self, altitude_m: f64) -> Site {
        Site {
            latitude_deg: self.latitude_deg,
            longitude_deg: self.longitude_deg,
            timezone_offset_hours: self.timezone_offset_hours,
            altitude_m,
        }
    }
}

/// Built-in presets, in UI order. The first is the app's default site.
const PRESETS: [SitePreset; 10] = [
    SitePreset {
        name: "Wellington, NZ",
        latitude_deg: -41.2866,
        longitude_deg: 174.7756,
        timezone_offset_hours: 12.0,
    },
    SitePreset {
        name: "Auckland, NZ",
        latitude_deg: -36.8485,
        longitude_deg: 174.7633,
        timezone_offset_hours: 12.0,
    },
    SitePreset {
        name: "Christchurch, NZ",
        latitude_deg: -43.5321,
        longitude_deg: 172.6362,
        timezone_offset_hours: 12.0,
    },
    SitePreset {
        name: "Queenstown, NZ",
        latitude_deg: -45.0312,
        longitude_deg: 168.6626,
        timezone_offset_hours: 12.0,
    },
    SitePreset {
        name: "Sydney, AU",
        latitude_deg: -33.8688,
        longitude_deg: 151.2093,
        timezone_offset_hours: 10.0,
    },
    SitePreset {
        name: "Melbourne, AU",
        latitude_deg: -37.8136,
        longitude_deg: 144.9631,
        timezone_offset_hours: 10.0,
    },
    SitePreset {
        name: "Brisbane, AU",
        latitude_deg: -27.4698,
        longitude_deg: 153.0251,
        timezone_offset_hours: 10.0,
    },
    SitePreset {
        name: "Adelaide, AU",
        latitude_deg: -34.9285,
        longitude_deg: 138.6007,
        timezone_offset_hours: 9.5,
    },
    SitePreset {
        name: "Perth, AU",
        latitude_deg: -31.9505,
        longitude_deg: 115.8605,
        timezone_offset_hours: 8.0,
    },
    SitePreset {
        name: "Hobart, AU",
        latitude_deg: -42.8821,
        longitude_deg: 147.3272,
        timezone_offset_hours: 10.0,
    },
];

/// The built-in site presets, in UI order.
pub fn site_presets() -> &'static [SitePreset] {
    &PRESETS
}

/// The UTC offset a site at `longitude_deg` would nominally use: one hour per
/// 15 degrees, to the nearest whole hour. A suggestion only: real offsets
/// follow time-zone borders, and half-hour zones, so the user can override it.
pub fn suggested_utc_offset(longitude_deg: f64) -> f64 {
    (longitude_deg / 15.0).round()
}

/// The suggested UTC offset for a position. New Zealand (the mainland and its
/// offshore islands) is all on +12 standard time, which longitude alone gets
/// wrong at the western end of the South Island, so it is special-cased;
/// everywhere else falls back to [`suggested_utc_offset`].
pub fn suggested_utc_offset_at(latitude_deg: f64, longitude_deg: f64) -> f64 {
    let in_new_zealand = (-48.0..=-34.0).contains(&latitude_deg)
        && (166.0..=179.0).contains(&longitude_deg);
    if in_new_zealand {
        12.0
    } else {
        suggested_utc_offset(longitude_deg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_zealand_is_plus_twelve_everywhere() {
        // Queenstown is west enough that longitude alone would say +11.
        assert_eq!(suggested_utc_offset_at(-45.0312, 168.6626), 12.0);
        assert_eq!(suggested_utc_offset_at(-41.2866, 174.7756), 12.0);
        // Elsewhere the longitude rule applies.
        assert_eq!(suggested_utc_offset_at(-33.8688, 151.2093), 10.0);
    }

    #[test]
    fn presets_are_valid_sites() {
        for preset in site_presets() {
            let issues = preset.site(30.0).validate();
            assert!(issues.is_empty(), "{}: {issues:?}", preset.name);
        }
        // The first preset is the app's default site.
        assert_eq!(site_presets()[0].name, "Wellington, NZ");
    }

    #[test]
    fn preset_offsets_agree_with_longitude() {
        // Time zones follow borders, so allow a generous margin; a typo of a
        // whole zone (or a sign error) still fails.
        for preset in site_presets() {
            let suggested = suggested_utc_offset(preset.longitude_deg);
            assert!(
                (suggested - preset.timezone_offset_hours).abs() <= 1.5,
                "{}: offset {} vs longitude suggests {}",
                preset.name,
                preset.timezone_offset_hours,
                suggested
            );
        }
    }

    #[test]
    fn suggested_offset_is_the_nearest_whole_hour() {
        assert_eq!(suggested_utc_offset(174.7756), 12.0);
        assert_eq!(suggested_utc_offset(138.6), 9.0);
        assert_eq!(suggested_utc_offset(-75.0), -5.0);
        assert_eq!(suggested_utc_offset(0.0), 0.0);
    }
}
