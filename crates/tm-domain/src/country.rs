//! Names Google Flights uses for countries and metropolitan areas, read
//! back into codes (DECISIONS.md 0087). Explore names a destination's
//! country in words ("Portugal", "Türkiye"), and a destination it finds
//! outside the watchlist needs an ISO code to be watched.

use crate::CountryCode;

/// ISO 3166-1 alpha-2 codes and the English names Google shows, with
/// the common alternatives.
const COUNTRIES: &[(&str, &[&str])] = &[
    ("AD", &["Andorra"]),
    ("AE", &["United Arab Emirates", "UAE"]),
    ("AF", &["Afghanistan"]),
    ("AG", &["Antigua and Barbuda", "Antigua & Barbuda"]),
    ("AI", &["Anguilla"]),
    ("AL", &["Albania"]),
    ("AM", &["Armenia"]),
    ("AO", &["Angola"]),
    ("AR", &["Argentina"]),
    ("AS", &["American Samoa"]),
    ("AT", &["Austria"]),
    ("AU", &["Australia"]),
    ("AW", &["Aruba"]),
    ("AZ", &["Azerbaijan"]),
    ("BA", &["Bosnia and Herzegovina", "Bosnia & Herzegovina"]),
    ("BB", &["Barbados"]),
    ("BD", &["Bangladesh"]),
    ("BE", &["Belgium"]),
    ("BF", &["Burkina Faso"]),
    ("BG", &["Bulgaria"]),
    ("BH", &["Bahrain"]),
    ("BI", &["Burundi"]),
    ("BJ", &["Benin"]),
    ("BL", &["Saint Barthélemy", "St. Barthélemy", "St Barts"]),
    ("BM", &["Bermuda"]),
    ("BN", &["Brunei"]),
    ("BO", &["Bolivia"]),
    ("BQ", &["Caribbean Netherlands", "Bonaire"]),
    ("BR", &["Brazil", "Brasil"]),
    ("BS", &["Bahamas", "The Bahamas"]),
    ("BT", &["Bhutan"]),
    ("BW", &["Botswana"]),
    ("BY", &["Belarus"]),
    ("BZ", &["Belize"]),
    ("CA", &["Canada"]),
    (
        "CD",
        &["Democratic Republic of the Congo", "Congo - Kinshasa"],
    ),
    ("CF", &["Central African Republic"]),
    (
        "CG",
        &["Republic of the Congo", "Congo - Brazzaville", "Congo"],
    ),
    ("CH", &["Switzerland"]),
    ("CI", &["Côte d'Ivoire", "Cote d'Ivoire", "Ivory Coast"]),
    ("CK", &["Cook Islands"]),
    ("CL", &["Chile"]),
    ("CM", &["Cameroon"]),
    ("CN", &["China"]),
    ("CO", &["Colombia"]),
    ("CR", &["Costa Rica"]),
    ("CU", &["Cuba"]),
    ("CV", &["Cape Verde", "Cabo Verde"]),
    ("CW", &["Curaçao", "Curacao"]),
    ("CY", &["Cyprus"]),
    ("CZ", &["Czechia", "Czech Republic"]),
    ("DE", &["Germany"]),
    ("DJ", &["Djibouti"]),
    ("DK", &["Denmark"]),
    ("DM", &["Dominica"]),
    ("DO", &["Dominican Republic"]),
    ("DZ", &["Algeria"]),
    ("EC", &["Ecuador"]),
    ("EE", &["Estonia"]),
    ("EG", &["Egypt"]),
    ("ER", &["Eritrea"]),
    ("ES", &["Spain"]),
    ("ET", &["Ethiopia"]),
    ("FI", &["Finland"]),
    ("FJ", &["Fiji"]),
    (
        "FK",
        &["Falkland Islands", "Falkland Islands (Islas Malvinas)"],
    ),
    ("FM", &["Micronesia"]),
    ("FO", &["Faroe Islands"]),
    ("FR", &["France"]),
    ("GA", &["Gabon"]),
    ("GB", &["United Kingdom", "UK", "Great Britain"]),
    ("GD", &["Grenada"]),
    ("GE", &["Georgia"]),
    ("GF", &["French Guiana"]),
    ("GH", &["Ghana"]),
    ("GI", &["Gibraltar"]),
    ("GL", &["Greenland"]),
    ("GM", &["Gambia", "The Gambia"]),
    ("GN", &["Guinea"]),
    ("GP", &["Guadeloupe"]),
    ("GQ", &["Equatorial Guinea"]),
    ("GR", &["Greece"]),
    ("GT", &["Guatemala"]),
    ("GU", &["Guam"]),
    ("GW", &["Guinea-Bissau"]),
    ("GY", &["Guyana"]),
    ("HK", &["Hong Kong"]),
    ("HN", &["Honduras"]),
    ("HR", &["Croatia"]),
    ("HT", &["Haiti"]),
    ("HU", &["Hungary"]),
    ("ID", &["Indonesia"]),
    ("IE", &["Ireland"]),
    ("IL", &["Israel"]),
    ("IN", &["India"]),
    ("IQ", &["Iraq"]),
    ("IR", &["Iran"]),
    ("IS", &["Iceland"]),
    ("IT", &["Italy"]),
    ("JM", &["Jamaica"]),
    ("JO", &["Jordan"]),
    ("JP", &["Japan"]),
    ("KE", &["Kenya"]),
    ("KG", &["Kyrgyzstan"]),
    ("KH", &["Cambodia"]),
    ("KI", &["Kiribati"]),
    ("KM", &["Comoros"]),
    ("KN", &["Saint Kitts and Nevis", "St. Kitts & Nevis"]),
    ("KR", &["South Korea", "Korea"]),
    ("KW", &["Kuwait"]),
    ("KY", &["Cayman Islands"]),
    ("KZ", &["Kazakhstan"]),
    ("LA", &["Laos"]),
    ("LB", &["Lebanon"]),
    ("LC", &["Saint Lucia", "St. Lucia"]),
    ("LI", &["Liechtenstein"]),
    ("LK", &["Sri Lanka"]),
    ("LR", &["Liberia"]),
    ("LS", &["Lesotho"]),
    ("LT", &["Lithuania"]),
    ("LU", &["Luxembourg"]),
    ("LV", &["Latvia"]),
    ("LY", &["Libya"]),
    ("MA", &["Morocco"]),
    ("MC", &["Monaco"]),
    ("MD", &["Moldova"]),
    ("ME", &["Montenegro"]),
    ("MF", &["Saint Martin", "St. Martin"]),
    ("MG", &["Madagascar"]),
    ("MH", &["Marshall Islands"]),
    ("MK", &["North Macedonia", "Macedonia"]),
    ("ML", &["Mali"]),
    ("MM", &["Myanmar", "Myanmar (Burma)", "Burma"]),
    ("MN", &["Mongolia"]),
    ("MO", &["Macao", "Macau"]),
    ("MP", &["Northern Mariana Islands"]),
    ("MQ", &["Martinique"]),
    ("MR", &["Mauritania"]),
    ("MS", &["Montserrat"]),
    ("MT", &["Malta"]),
    ("MU", &["Mauritius"]),
    ("MV", &["Maldives"]),
    ("MW", &["Malawi"]),
    ("MX", &["Mexico"]),
    ("MY", &["Malaysia"]),
    ("MZ", &["Mozambique"]),
    ("NA", &["Namibia"]),
    ("NC", &["New Caledonia"]),
    ("NE", &["Niger"]),
    ("NG", &["Nigeria"]),
    ("NI", &["Nicaragua"]),
    ("NL", &["Netherlands", "The Netherlands"]),
    ("NO", &["Norway"]),
    ("NP", &["Nepal"]),
    ("NR", &["Nauru"]),
    ("NZ", &["New Zealand"]),
    ("OM", &["Oman"]),
    ("PA", &["Panama"]),
    ("PE", &["Peru"]),
    ("PF", &["French Polynesia"]),
    ("PG", &["Papua New Guinea"]),
    ("PH", &["Philippines"]),
    ("PK", &["Pakistan"]),
    ("PL", &["Poland"]),
    (
        "PM",
        &["Saint Pierre and Miquelon", "St. Pierre & Miquelon"],
    ),
    ("PR", &["Puerto Rico"]),
    ("PS", &["Palestine", "Palestinian Territories"]),
    ("PT", &["Portugal"]),
    ("PW", &["Palau"]),
    ("PY", &["Paraguay"]),
    ("QA", &["Qatar"]),
    ("RE", &["Réunion", "Reunion"]),
    ("RO", &["Romania"]),
    ("RS", &["Serbia"]),
    ("RU", &["Russia"]),
    ("RW", &["Rwanda"]),
    ("SA", &["Saudi Arabia"]),
    ("SB", &["Solomon Islands"]),
    ("SC", &["Seychelles"]),
    ("SD", &["Sudan"]),
    ("SE", &["Sweden"]),
    ("SG", &["Singapore"]),
    ("SI", &["Slovenia"]),
    ("SK", &["Slovakia"]),
    ("SL", &["Sierra Leone"]),
    ("SM", &["San Marino"]),
    ("SN", &["Senegal"]),
    ("SO", &["Somalia"]),
    ("SR", &["Suriname"]),
    ("SS", &["South Sudan"]),
    ("ST", &["São Tomé and Príncipe", "Sao Tome and Principe"]),
    ("SV", &["El Salvador"]),
    ("SX", &["Sint Maarten"]),
    ("SY", &["Syria"]),
    ("SZ", &["Eswatini", "Swaziland"]),
    (
        "TC",
        &["Turks and Caicos Islands", "Turks & Caicos Islands"],
    ),
    ("TD", &["Chad"]),
    ("TG", &["Togo"]),
    ("TH", &["Thailand"]),
    ("TJ", &["Tajikistan"]),
    ("TL", &["Timor-Leste", "East Timor"]),
    ("TM", &["Turkmenistan"]),
    ("TN", &["Tunisia"]),
    ("TO", &["Tonga"]),
    ("TR", &["Türkiye", "Turkiye", "Turkey"]),
    ("TT", &["Trinidad and Tobago", "Trinidad & Tobago"]),
    ("TV", &["Tuvalu"]),
    ("TW", &["Taiwan"]),
    ("TZ", &["Tanzania"]),
    ("UA", &["Ukraine"]),
    ("UG", &["Uganda"]),
    ("US", &["United States", "USA", "United States of America"]),
    ("UY", &["Uruguay"]),
    ("UZ", &["Uzbekistan"]),
    (
        "VC",
        &[
            "Saint Vincent and the Grenadines",
            "St. Vincent & Grenadines",
        ],
    ),
    ("VE", &["Venezuela"]),
    ("VG", &["British Virgin Islands"]),
    ("VI", &["U.S. Virgin Islands", "US Virgin Islands"]),
    ("VN", &["Vietnam", "Viet Nam"]),
    ("VU", &["Vanuatu"]),
    ("WS", &["Samoa"]),
    ("XK", &["Kosovo"]),
    ("YE", &["Yemen"]),
    ("YT", &["Mayotte"]),
    ("ZA", &["South Africa"]),
    ("ZM", &["Zambia"]),
    ("ZW", &["Zimbabwe"]),
];

/// The ISO code of a country as Google names it (`"Türkiye"` → `TR`),
/// ignoring case and surrounding spaces. `None` for a name not known.
pub fn country_code(name: &str) -> Option<CountryCode> {
    let name = name.trim();
    COUNTRIES
        .iter()
        .find(|(_, names)| names.iter().any(|n| n.eq_ignore_ascii_case(name)))
        .map(|(code, _)| CountryCode::new(code))
}

/// IATA metropolitan codes and the airports they group, for areas
/// Google Flights searches as one city.
const METROS: &[(&str, &[&str])] = &[
    ("BER", &["BER", "SXF", "TXL"]),
    ("BHZ", &["CNF", "PLU"]),
    ("BJS", &["PEK", "PKX"]),
    ("BKK", &["BKK", "DMK"]),
    ("BUE", &["EZE", "AEP"]),
    ("CHI", &["ORD", "MDW"]),
    ("DFW", &["DFW", "DAL"]),
    ("DXB", &["DXB", "DWC", "SHJ"]),
    ("HOU", &["IAH", "HOU"]),
    ("IST", &["IST", "SAW"]),
    ("JKT", &["CGK", "HLP"]),
    ("LON", &["LHR", "LGW", "STN", "LTN", "LCY", "SEN"]),
    ("MEX", &["MEX", "NLU", "TLC"]),
    ("MIL", &["MXP", "LIN", "BGY"]),
    ("MOW", &["SVO", "DME", "VKO"]),
    ("NYC", &["JFK", "EWR", "LGA"]),
    ("ORL", &["MCO", "SFB"]),
    ("OSA", &["KIX", "ITM", "UKB"]),
    ("OSL", &["OSL", "TRF", "RYG"]),
    ("PAR", &["CDG", "ORY", "BVA"]),
    ("REK", &["KEF", "RKV"]),
    ("RIO", &["GIG", "SDU"]),
    ("ROM", &["FCO", "CIA"]),
    ("SAO", &["GRU", "CGH", "VCP"]),
    ("SEL", &["ICN", "GMP"]),
    ("SHA", &["PVG", "SHA"]),
    ("STO", &["ARN", "BMA", "NYO"]),
    ("TPE", &["TPE", "TSA"]),
    ("TYO", &["HND", "NRT"]),
    ("WAS", &["IAD", "DCA", "BWI"]),
    ("YMQ", &["YUL", "YHU"]),
    ("YTO", &["YYZ", "YTZ", "YHM"]),
];

/// The metropolitan code of the area that groups `airport`
/// (`"SFB"` → `ORL`), if it is one of the known ones.
pub fn metro_code(airport: &str) -> Option<&'static str> {
    let airport = airport.trim();
    METROS
        .iter()
        .find(|(_, airports)| {
            airports.iter().any(|a| a.eq_ignore_ascii_case(airport))
        })
        .map(|(code, _)| *code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_country_names_have_codes() {
        for (name, code) in [
            ("Portugal", "PT"),
            ("United States", "US"),
            ("Türkiye", "TR"),
            ("turkey", "TR"),
            ("United Kingdom", "GB"),
            ("South Korea", "KR"),
            ("French Polynesia", "PF"),
            (" Japan ", "JP"),
            ("Hong Kong", "HK"),
        ] {
            assert_eq!(
                country_code(name).as_ref().map(CountryCode::as_str),
                Some(code),
                "{name}"
            );
        }
        assert_eq!(country_code("Atlantis"), None);
        assert_eq!(country_code(""), None);
    }

    #[test]
    fn codes_and_names_are_unique() {
        let mut codes: Vec<&str> = COUNTRIES.iter().map(|(c, _)| *c).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), COUNTRIES.len());
        let mut names: Vec<String> = COUNTRIES
            .iter()
            .flat_map(|(_, n)| n.iter().map(|n| n.to_ascii_lowercase()))
            .collect();
        let all = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), all);
    }

    #[test]
    fn airports_belong_to_one_metro_area() {
        assert_eq!(metro_code("SFB"), Some("ORL"));
        assert_eq!(metro_code("kix"), Some("OSA"));
        assert_eq!(metro_code("HOU"), Some("HOU"));
        assert_eq!(metro_code("LIS"), None);
        let mut airports: Vec<&str> =
            METROS.iter().flat_map(|(_, a)| a.iter().copied()).collect();
        let all = airports.len();
        airports.sort_unstable();
        airports.dedup();
        assert_eq!(airports.len(), all);
    }
}
