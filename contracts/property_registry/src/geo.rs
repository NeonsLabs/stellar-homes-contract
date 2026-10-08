//! Geospatial coordinate data structures for land title verification.
use soroban_sdk::contracttype;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeoCoordinate {
    pub latitude_e6: i32,  // Latitude scaled by 1,000,000
    pub longitude_e6: i32, // Longitude scaled by 1,000,000
}

impl GeoCoordinate {
    pub fn is_valid(&self) -> bool {
        self.latitude_e6 >= -90_000_000
            && self.latitude_e6 <= 90_000_000
            && self.longitude_e6 >= -180_000_000
            && self.longitude_e6 <= 180_000_000
    }
}
