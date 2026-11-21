use chrono::{DateTime, Datelike, Local, Timelike};
use std::f64::consts::PI;

// Default coordinates: Vietnam (Hanoi: 21.0285° N, 105.8542° E)
pub const DEFAULT_LATITUDE: f64 = 21.0285;
pub const DEFAULT_LONGITUDE: f64 = 105.8542;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    AutoSolar,
    ForceLight,
    ForceDark,
}

impl ThemeMode {
    pub fn next(&self) -> Self {
        match self {
            ThemeMode::AutoSolar => ThemeMode::ForceLight,
            ThemeMode::ForceLight => ThemeMode::ForceDark,
            ThemeMode::ForceDark => ThemeMode::AutoSolar,
        }
    }

    #[allow(dead_code)]
    pub fn to_str(&self) -> &'static str {
        match self {
            ThemeMode::AutoSolar => "auto",
            ThemeMode::ForceLight => "light",
            ThemeMode::ForceDark => "dark",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            ThemeMode::AutoSolar => "🔄",
            ThemeMode::ForceLight => "☀️",
            ThemeMode::ForceDark => "🌙",
        }
    }
}

pub struct SolarCalculator {
    latitude: f64,
    longitude: f64,
}

impl Default for SolarCalculator {
    fn default() -> Self {
        Self::new(DEFAULT_LATITUDE, DEFAULT_LONGITUDE)
    }
}

impl SolarCalculator {
    pub fn new(latitude: f64, longitude: f64) -> Self {
        Self { latitude, longitude }
    }

    /// Computes sunrise and sunset times (in minutes from midnight UTC) using NOAA solar algorithm
    pub fn calculate_solar_times(&self, now: DateTime<Local>) -> (f64, f64) {
        let day_of_year = now.ordinal() as f64;
        let hour = now.hour() as f64;

        // Fractional year in radians
        let gamma = (2.0 * PI / 365.0) * (day_of_year - 1.0 + (hour - 12.0) / 24.0);

        // Equation of time in minutes
        let eqtime = 229.18
            * (0.000075
                + 0.001868 * gamma.cos()
                - 0.032077 * gamma.sin()
                - 0.014615 * (2.0 * gamma).cos()
                - 0.040849 * (2.0 * gamma).sin());

        // Solar declination in radians
        let decl = 0.006918 - 0.399912 * gamma.cos() + 0.070257 * gamma.sin()
            - 0.006758 * (2.0 * gamma).cos()
            + 0.000907 * (2.0 * gamma).sin()
            - 0.002697 * (3.0 * gamma).cos()
            + 0.00148 * (3.0 * gamma).sin();

        let lat_rad = self.latitude * (PI / 180.0);
        let zenith_rad = 90.833 * (PI / 180.0); // 90°50' accounts for atmospheric refraction

        let cos_ha = (zenith_rad.cos() / (lat_rad.cos() * decl.cos())) - (lat_rad.tan() * decl.tan());
        let cos_ha_clamped = cos_ha.clamp(-1.0, 1.0);
        let ha_deg = cos_ha_clamped.acos() * (180.0 / PI);

        // Sunrise & Sunset in UTC minutes
        let sunrise_utc_min = 720.0 - 4.0 * (self.longitude + ha_deg) - eqtime;
        let sunset_utc_min = 720.0 - 4.0 * (self.longitude - ha_deg) - eqtime;

        (sunrise_utc_min, sunset_utc_min)
    }

    /// Determines if current local time is daylight
    pub fn is_daylight(&self, now: DateTime<Local>) -> bool {
        let (sunrise_utc, sunset_utc) = self.calculate_solar_times(now);
        let offset_minutes = (now.offset().local_minus_utc() / 60) as f64;

        // Convert UTC minutes to local minutes of the day (0..1440)
        let sunrise_local = (sunrise_utc + offset_minutes).rem_euclid(1440.0);
        let sunset_local = (sunset_utc + offset_minutes).rem_euclid(1440.0);

        let current_local_min = (now.hour() * 60 + now.minute()) as f64;

        current_local_min >= sunrise_local && current_local_min < sunset_local
    }

    /// Returns human-readable sunrise and sunset strings in local time, e.g. ("05:58", "17:34")
    #[allow(dead_code)]
    pub fn get_solar_times_display(&self, now: DateTime<Local>) -> (String, String) {
        let (sunrise_utc, sunset_utc) = self.calculate_solar_times(now);
        let offset_minutes = (now.offset().local_minus_utc() / 60) as f64;

        let to_time_str = |mins: f64| -> String {
            let local_mins = (mins + offset_minutes).rem_euclid(1440.0) as u32;
            let h = local_mins / 60;
            let m = local_mins % 60;
            format!("{:02}:{:02}", h, m)
        };

        (to_time_str(sunrise_utc), to_time_str(sunset_utc))
    }
}

/// Convenience function: returns true if dark theme should be active right now
pub fn should_be_dark_theme(mode: ThemeMode) -> bool {
    match mode {
        ThemeMode::ForceDark => true,
        ThemeMode::ForceLight => false,
        ThemeMode::AutoSolar => {
            let calc = SolarCalculator::default();
            !calc.is_daylight(Local::now())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_solar_times_reasonable() {
        let calc = SolarCalculator::default();
        let now = Local::now();
        let (sunrise_utc, sunset_utc) = calc.calculate_solar_times(now);

        // Sunset must be after sunrise
        assert!(sunset_utc > sunrise_utc);

        let (sunrise_str, sunset_str) = calc.get_solar_times_display(now);
        println!("Calculated solar times: Sunrise {} - Sunset {}", sunrise_str, sunset_str);
        
        // In Hanoi, sunrise is always between 05:00 and 07:00 local time
        assert!(sunrise_str.starts_with("05:") || sunrise_str.starts_with("06:"));
        // Sunset is always between 17:00 and 19:00 local time
        assert!(sunset_str.starts_with("17:") || sunset_str.starts_with("18:"));
    }

    #[test]
    fn test_theme_mode_switching() {
        assert_eq!(ThemeMode::AutoSolar.next(), ThemeMode::ForceLight);
        assert_eq!(ThemeMode::ForceLight.next(), ThemeMode::ForceDark);
        assert_eq!(ThemeMode::ForceDark.next(), ThemeMode::AutoSolar);
    }
}
