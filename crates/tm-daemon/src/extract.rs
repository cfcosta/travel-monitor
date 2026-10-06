//! Reading prices from the visible text of pages nobody has studied yet
//! (DECISIONS.md 0047): airline sites and miles programs. Tolerant on
//! purpose, so every result is stored as unverified.

/// The largest believable fare in BRL; anything above is a phone
/// number, a booking code or a sum of something else.
const MAX_BRL: i64 = 500_000;
/// The smallest believable amount: taxes on an award ticket can be low.
const MIN_BRL: i64 = 10;

/// BRL amounts (`R$ 1.234,56`, `R$1,234`, `BRL 980`) in `text`, in
/// minor units, in order of appearance.
pub fn brl_amounts(text: &str) -> Vec<i64> {
    let mut out = Vec::new();
    for marker in ["R$", "BRL"] {
        let mut rest = text;
        while let Some(at) = rest.find(marker) {
            let after = &rest[at + marker.len()..];
            if let Some(minor) = leading_number(after) {
                let whole = minor / 100;
                if (MIN_BRL..=MAX_BRL).contains(&whole) {
                    out.push((rest.as_ptr() as usize + at, minor));
                }
            }
            rest = after;
        }
    }
    out.sort_by_key(|(pos, _)| *pos);
    out.into_iter().map(|(_, minor)| minor).collect()
}

/// Mile or point counts (`12.500 milhas`, `12,500 miles`, `8 000
/// pontos`) in `text`.
pub fn mile_amounts(text: &str) -> Vec<i64> {
    let lower = text.to_lowercase();
    let mut out = Vec::new();
    for word in ["milhas", "miles", "pontos", "points", "pts"] {
        let mut from = 0;
        while let Some(found) = lower[from..].find(word) {
            let at = from + found;
            from = at + word.len();
            // The word must start a word.
            if lower[..at]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric)
            {
                continue;
            }
            if let Some(n) = trailing_count(&lower[..at])
                && (100..=5_000_000).contains(&n)
            {
                out.push(n);
            }
        }
    }
    out
}

/// Parses the number at the start of `text` (after spaces), as minor
/// units. Handles `1.234,56`, `1,234.56`, `1234`, `1.234` (thousands)
/// and `12,50` (decimals).
fn leading_number(text: &str) -> Option<i64> {
    let text = text.trim_start_matches([' ', '\u{a0}', '\u{202f}']);
    let raw: String = text
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
        .collect();
    let raw = raw.trim_end_matches(['.', ',']);
    if raw.is_empty() || !raw.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let last_dot = raw.rfind('.');
    let last_comma = raw.rfind(',');
    let decimal = match (last_dot, last_comma) {
        (Some(d), Some(c)) => Some(d.max(c)),
        (Some(i), None) | (None, Some(i)) => {
            let sep = raw.as_bytes()[i];
            let once = raw.bytes().filter(|b| *b == sep).count() == 1;
            let tail = raw.len() - i - 1;
            (once && tail <= 2).then_some(i)
        }
        (None, None) => None,
    };
    let (int_part, frac_part) = match decimal {
        Some(i) => (&raw[..i], &raw[i + 1..]),
        None => (raw, ""),
    };
    let int: String = int_part.chars().filter(char::is_ascii_digit).collect();
    let whole: i64 = int.parse().ok()?;
    let cents: i64 = match frac_part.len() {
        0 => 0,
        1 => frac_part.parse::<i64>().ok()? * 10,
        _ => frac_part[..2].parse().ok()?,
    };
    whole.checked_mul(100)?.checked_add(cents)
}

/// The integer that ends `text` (before trailing spaces), with `.`, `,`
/// or spaces as thousands separators.
fn trailing_count(text: &str) -> Option<i64> {
    let text = text.trim_end_matches([' ', '\u{a0}', '\u{202f}']);
    let raw: String = text
        .chars()
        .rev()
        .take_while(|c| {
            c.is_ascii_digit() || matches!(c, '.' | ',' | ' ' | '\u{a0}')
        })
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let raw = raw.trim_start_matches(['.', ',', ' ', '\u{a0}']);
    // Groups after the first must be three digits, or this was not one
    // number ("2 adults 12.500").
    let groups: Vec<&str> = raw.split(['.', ',', ' ', '\u{a0}']).collect();
    let start = groups.iter().rposition(|g| g.len() != 3).unwrap_or(0);
    let digits: String = groups[start..].concat();
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Whether a miles program's page asks the user to log in (DECISIONS.md
/// 0024): a visible password field, or the usual words.
pub fn looks_logged_out(text: &str, password_field: bool) -> bool {
    if password_field {
        return true;
    }
    let t = text.to_lowercase();
    [
        "faça login",
        "faça seu login",
        "fazer login",
        "entre na sua conta",
        "acesse sua conta",
        "log in to your account",
        "sign in to your account",
        "digite sua senha",
    ]
    .iter()
    .any(|p| t.contains(p))
}

/// JavaScript: is a password field visible?
pub const PASSWORD_FIELD_JS: &str = r#"(() => [...document.querySelectorAll('input[type="password"]')]
  .some(e => e.offsetParent !== null))()"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_brazilian_and_english_prices() {
        assert_eq!(brl_amounts("Total R$ 1.234,56 por pessoa"), vec![123_456]);
        assert_eq!(brl_amounts("from R$3,932 round trip"), vec![393_200]);
        assert_eq!(brl_amounts("R$ 980"), vec![98_000]);
        assert_eq!(brl_amounts("BRL 2.450"), vec![245_000]);
        assert_eq!(brl_amounts("taxas R$ 89,90"), vec![8_990]);
        assert_eq!(brl_amounts("R$\u{a0}1.050,00"), vec![105_000]);
        assert_eq!(
            brl_amounts("R$ 1.500 ou R$ 2.000,10 e BRL 700"),
            vec![150_000, 200_010, 70_000]
        );
    }

    #[test]
    fn ignores_what_is_not_a_fare() {
        assert!(brl_amounts("R$ 0,99 de taxa").is_empty());
        assert!(brl_amounts("R$ 12.345.678").is_empty());
        assert!(brl_amounts("R$ abc").is_empty());
        assert!(brl_amounts("no prices here 1.234").is_empty());
    }

    #[test]
    fn reads_miles() {
        assert_eq!(mile_amounts("12.500 milhas + R$ 89,90"), vec![12_500]);
        assert_eq!(mile_amounts("from 35,000 miles"), vec![35_000]);
        assert_eq!(mile_amounts("8 000 pontos"), vec![8_000]);
        assert_eq!(mile_amounts("2 adultos 12.500 milhas"), vec![12_500]);
        assert!(mile_amounts("Smiles").is_empty());
        assert!(mile_amounts("30 milhas").is_empty());
    }

    #[test]
    fn spots_login_walls() {
        assert!(looks_logged_out("Faça login para continuar", false));
        assert!(looks_logged_out("", true));
        assert!(!looks_logged_out("Olá, Maria. 120.000 milhas", false));
    }
}
