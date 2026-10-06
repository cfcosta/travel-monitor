//! The first-run configuration (DECISIONS.md 0006, 0012, 0023, 0033,
//! 0035): SAO, the watchlist, four points of sale, default settings,
//! airline sites and miles programs. All of it is ordinary data the app
//! can edit afterwards.

use std::collections::HashMap;

use chrono::Utc;
use tm_domain::{CountryCode, Currency, PlaceId, Settings};

use crate::{
    Store,
    config::{
        add_destination_in,
        add_origin_in,
        add_pos_in,
        set_airline_destinations_in,
        set_settings_in,
        upsert_airline_in,
        upsert_miles_in,
    },
    error::Result,
    records::NewPlace,
};

/// SAO: the whole city, so GRU, CGH and VCP all count (0005, 0041).
fn origin() -> NewPlace {
    NewPlace::city("SAO", "São Paulo", "BR", Some("/m/022pfm"), -23.55, -46.63)
}

/// The watchlist of 0006. Cities are IATA metropolitan codes; their
/// Freebase IDs are learned when first needed (0041).
fn watchlist() -> Vec<NewPlace> {
    let a = NewPlace::airport;
    let c = |code, name, country, lat, lon| {
        NewPlace::city(code, name, country, None, lat, lon)
    };
    vec![
        // Brazil
        c("RIO", "Rio de Janeiro", "BR", -22.91, -43.17),
        a("BSB", "Brasília", "BR", -15.79, -47.88),
        a("SSA", "Salvador", "BR", -12.97, -38.50),
        a("REC", "Recife", "BR", -8.05, -34.88),
        a("FOR", "Fortaleza", "BR", -3.73, -38.53),
        a("FLN", "Florianópolis", "BR", -27.60, -48.55),
        a("POA", "Porto Alegre", "BR", -30.03, -51.23),
        a("NAT", "Natal", "BR", -5.79, -35.21),
        a("MCZ", "Maceió", "BR", -9.67, -35.74),
        a("IGU", "Foz do Iguaçu", "BR", -25.55, -54.59),
        a("AJU", "Aracaju", "BR", -10.98, -37.07),
        // Brazil's other main airports (0055)
        a("CNF", "Belo Horizonte", "BR", -19.92, -43.94),
        a("CWB", "Curitiba", "BR", -25.43, -49.27),
        a("BEL", "Belém", "BR", -1.46, -48.49),
        a("MAO", "Manaus", "BR", -3.12, -60.02),
        a("GYN", "Goiânia", "BR", -16.69, -49.26),
        a("VIX", "Vitória", "BR", -20.32, -40.34),
        a("CGB", "Cuiabá", "BR", -15.60, -56.10),
        // South America
        c("BUE", "Buenos Aires", "AR", -34.60, -58.38),
        a("SCL", "Santiago", "CL", -33.45, -70.67),
        a("MVD", "Montevideo", "UY", -34.90, -56.16),
        a("LIM", "Lima", "PE", -12.05, -77.04),
        a("BOG", "Bogotá", "CO", 4.71, -74.07),
        a("BRC", "Bariloche", "AR", -41.13, -71.31),
        // Antarctic gateways (0054)
        a("USH", "Ushuaia", "AR", -54.84, -68.30),
        a("PUQ", "Punta Arenas", "CL", -53.00, -70.85),
        // South America's other main airports (0055)
        a("MDE", "Medellín", "CO", 6.24, -75.58),
        a("CTG", "Cartagena", "CO", 10.39, -75.48),
        a("CLO", "Cali", "CO", 3.45, -76.53),
        a("UIO", "Quito", "EC", -0.18, -78.47),
        a("GYE", "Guayaquil", "EC", -2.17, -79.92),
        a("CCS", "Caracas", "VE", 10.60, -66.99),
        a("ASU", "Asunción", "PY", -25.26, -57.58),
        a("VVI", "Santa Cruz de la Sierra", "BO", -17.78, -63.18),
        a("LPB", "La Paz", "BO", -16.50, -68.15),
        a("CUZ", "Cusco", "PE", -13.53, -71.97),
        a("COR", "Córdoba", "AR", -31.42, -64.18),
        a("MDZ", "Mendoza", "AR", -32.89, -68.83),
        // North America and Caribbean
        a("MIA", "Miami", "US", 25.76, -80.19),
        a("MCO", "Orlando", "US", 28.54, -81.38),
        c("NYC", "New York", "US", 40.71, -74.01),
        a("CUN", "Cancún", "MX", 21.16, -86.85),
        a("PUJ", "Punta Cana", "DO", 18.58, -68.40),
        // Europe
        a("LIS", "Lisbon", "PT", 38.72, -9.14),
        a("OPO", "Porto", "PT", 41.15, -8.61),
        a("MAD", "Madrid", "ES", 40.42, -3.70),
        a("BCN", "Barcelona", "ES", 41.39, 2.17),
        c("PAR", "Paris", "FR", 48.86, 2.35),
        c("LON", "London", "GB", 51.51, -0.13),
        c("ROM", "Rome", "IT", 41.90, 12.50),
        // Long-haul
        a("DXB", "Dubai", "AE", 25.20, 55.27),
        c("TYO", "Tokyo", "JP", 35.68, 139.69),
    ]
}

const BRAZIL: &[&str] = &[
    "RIO", "BSB", "SSA", "REC", "FOR", "FLN", "POA", "NAT", "MCZ", "IGU",
    "AJU", "CNF", "CWB", "BEL", "MAO", "GYN", "VIX", "CGB",
];
const EUROPE: &[&str] = &["LIS", "OPO", "MAD", "BCN", "PAR", "LON", "ROM"];

/// The airline sites of 0012 and the watchlist destinations each one
/// plausibly sells from São Paulo (nonstop or with its own connections).
/// A best guess, edited in the app.
fn airlines() -> Vec<(&'static str, &'static str, Vec<&'static str>)> {
    let with = |base: &[&'static str], more: &[&'static str]| {
        base.iter().chain(more).copied().collect::<Vec<_>>()
    };
    vec![
        (
            "LA",
            "LATAM",
            with(
                BRAZIL,
                &[
                    "BUE", "SCL", "MVD", "LIM", "BOG", "BRC", "USH", "PUQ",
                    "MIA", "MCO", "NYC", "CUN", "PUJ", "LIS", "MAD", "BCN",
                    "LON", "ROM",
                ],
            ),
        ),
        (
            "AD",
            "Azul",
            with(BRAZIL, &["BUE", "MVD", "MCO", "LIS", "PAR"]),
        ),
        (
            "G3",
            "GOL",
            with(
                BRAZIL,
                &[
                    "BUE", "SCL", "MVD", "LIM", "BRC", "MIA", "MCO", "CUN",
                    "PUJ",
                ],
            ),
        ),
        ("TP", "TAP Air Portugal", EUROPE.to_vec()),
        ("IB", "Iberia", EUROPE.to_vec()),
        ("AF", "Air France/KLM", EUROPE.to_vec()),
        (
            "AA",
            "American Airlines",
            vec!["MIA", "MCO", "NYC", "CUN", "PUJ"],
        ),
        ("UA", "United Airlines", vec!["NYC", "MIA", "MCO", "CUN"]),
        ("EK", "Emirates", vec!["DXB", "TYO"]),
        ("QR", "Qatar Airways", vec!["DXB", "TYO"]),
        ("TK", "Turkish Airlines", vec!["DXB", "TYO", "ROM"]),
    ]
}

const MILES: &[(&str, &str)] = &[
    ("smiles", "Smiles"),
    ("latam_pass", "LATAM Pass"),
    ("azul_fidelidade", "Azul Fidelidade"),
];

impl Store {
    /// Seeds the first-run configuration, once: on a database that was
    /// already seeded it does nothing and returns `false`. Then derives
    /// routes and families.
    pub async fn seed_defaults(&self) -> Result<bool> {
        let mut tx = self.write().await?;
        let seeded = sqlx::query_scalar!(
            r#"SELECT seeded_at IS NOT NULL AS "seeded!: bool"
               FROM meta WHERE id = 1"#
        )
        .fetch_one(&mut *tx)
        .await?;
        if seeded {
            return Ok(false);
        }

        add_origin_in(&mut tx, &origin()).await?;
        let mut ids: HashMap<String, PlaceId> = HashMap::new();
        for place in watchlist() {
            let id = add_destination_in(&mut tx, &place, 1.0).await?;
            ids.insert(place.code, id);
        }

        let brl = Currency::brl();
        for (country, home) in
            [("BR", true), ("US", false), ("PT", false), ("IN", false)]
        {
            add_pos_in(&mut tx, &CountryCode::new(country), &brl, home).await?;
        }

        let settings = Settings::default();
        set_settings_in(&mut tx, &settings).await?;

        for (code, name, served) in airlines() {
            upsert_airline_in(&mut tx, code, name, true).await?;
            let places: Vec<PlaceId> =
                served.iter().filter_map(|c| ids.get(*c).copied()).collect();
            set_airline_destinations_in(&mut tx, code, &places).await?;
        }
        for (program, name) in MILES {
            upsert_miles_in(&mut tx, program, name, true).await?;
        }

        let now = Utc::now();
        sqlx::query!("UPDATE meta SET seeded_at = ?1 WHERE id = 1", now)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        self.sync_families(&settings).await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn airline_destinations_are_on_the_watchlist() {
        let codes: Vec<String> =
            watchlist().into_iter().map(|p| p.code).collect();
        for (airline, _, served) in airlines() {
            for code in served {
                assert!(
                    codes.iter().any(|c| c == code),
                    "{airline} serves {code}, not on the watchlist"
                );
            }
        }
    }
}
