//! Page-level knowledge as data: JavaScript the daemon evaluates
//! through `tm-browser`, parsers for what it returns, and block
//! detection. Everything assumes `hl=en`.
//!
//! Each snippet is a single expression. Snippets that act return
//! `true` when they found their target; when one returns `false`, the
//! daemon ends the check as `Unexpected` (DECISIONS.md 0059).

use chrono::{Datelike, NaiveDate};
use thiserror::Error;
use tm_domain::QueryOutcome;

/// Opens the "Price graph" view of a results page (it sits next to
/// "Date grid" under the price insights). The first `GetCalendarGraph`
/// follows, covering about 60 days.
pub const OPEN_PRICE_GRAPH_JS: &str = r#"(() => {
  const el = [...document.querySelectorAll('button, [role="button"], [role="tab"]')]
    .find(e => e.offsetParent !== null && (e.innerText || e.getAttribute('aria-label') || '').trim() === 'Price graph');
  if (!el) return false;
  el.click();
  return true;
})()"#;

/// Opens the "Date grid" view (`GetCalendarGrid`).
pub const OPEN_DATE_GRID_JS: &str = r#"(() => {
  const el = [...document.querySelectorAll('button, [role="button"], [role="tab"]')]
    .find(e => e.offsetParent !== null && (e.innerText || e.getAttribute('aria-label') || '').trim() === 'Date grid');
  if (!el) return false;
  el.click();
  return true;
})()"#;

/// Presses the visible, enabled "Scroll forward" control of the open
/// graph or grid. Each press on the price graph fetches 21 more days.
pub const SCROLL_FORWARD_JS: &str = r#"(() => {
  const els = [...document.querySelectorAll('[aria-label="Scroll forward"]')]
    .filter(e => e.offsetParent !== null && !e.disabled && e.getAttribute('aria-disabled') !== 'true');
  if (!els.length) return false;
  els[els.length - 1].click();
  return true;
})()"#;

/// The aria-labels of the date grid's cells, as a JSON array string,
/// for [`parse_grid_label`]: the fallback when the RPC format changes
/// (DECISIONS.md 0032).
pub const READ_DATE_GRID_LABELS_JS: &str = r#"JSON.stringify(
  [...document.querySelectorAll('[aria-label]')]
    .map(e => e.getAttribute('aria-label'))
    .filter(l => /\b(Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]* \d{1,2}\b/.test(l)
              && /\d[\d,.]*/.test(l.split(/\b(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)/)[0]))
)"#;

/// The page's `AF_dataServiceRequests` script, or `null`: what a `q=`
/// page resolved its text to, for
/// [`crate::rpc::parse_data_service_requests`].
pub const DATA_SERVICE_REQUESTS_JS: &str = r#"(() => {
  const s = [...document.scripts].find(s => s.text.includes('AF_dataServiceRequests'));
  return s ? s.text : null;
})()"#;

/// The `AF_initDataCallback` script for `key` (`"ds:1"` holds a results
/// page's first itineraries), or `null`. A page loaded from a search
/// URL renders its first results from this data, without an RPC.
pub fn init_data_js(key: &str) -> String {
    let key = key.replace(['\\', '\'', '"'], "");
    format!(
        "(() => {{ const s = [...document.scripts].find(s => s.text.includes(\"AF_initDataCallback({{key: '{key}'\")); return s ? s.text : null; }})()"
    )
}

/// The page's visible text, for [`detect_block`] and
/// [`detect_no_results`].
pub const PAGE_TEXT_JS: &str =
    "document.body ? document.body.innerText.slice(0, 20000) : ''";

/// True once a results page shows itineraries (or says it has none).
pub const RESULTS_READY_JS: &str = r#"(() => {
  const t = document.body ? document.body.innerText : '';
  return /Select flight|results returned|No results|No options matching/.test(t)
      || !![...document.querySelectorAll('[aria-label$="Select flight"]')].length;
})()"#;

/// An XHR hook recording every flights RPC into `window.__cap` as
/// `{url, body, status, response}`; install it before navigation (CDP
/// `Page.addScriptToEvaluateOnNewDocument`). The research captured
/// every fixture this way. `tm-browser`'s Network capture does the
/// same without touching the page.
pub const XHR_CAPTURE_HOOK_JS: &str = r#"(function () {
  if (window.__capInstalled) return;
  window.__capInstalled = true;
  window.__cap = [];
  const open = XMLHttpRequest.prototype.open, send = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.open = function (m, u) { this.__u = String(u); return open.apply(this, arguments); };
  XMLHttpRequest.prototype.send = function (body) {
    const u = this.__u || '';
    if (u.indexOf('/data/travel.frontend.flights.') >= 0) {
      const rec = { url: u, body: typeof body === 'string' ? body : (body == null ? null : String(body)) };
      this.addEventListener('loadend', () => {
        rec.status = this.status;
        try { rec.response = this.responseText; } catch (e) { rec.response = null; }
        window.__cap.push(rec);
      });
    }
    return send.apply(this, arguments);
  };
})();"#;

/// Reads and clears `window.__cap` as a JSON string.
pub const DRAIN_CAPTURE_JS: &str = "(() => { const c = window.__cap || []; window.__cap = []; return JSON.stringify(c); })()";

/// Why a page is not the page asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Block {
    /// Redirected to `google.com/sorry/`.
    Sorry,
    /// "Our systems have detected unusual traffic".
    UnusualTraffic,
    /// A reCAPTCHA challenge.
    Captcha,
    /// The cookie consent interstitial (`consent.google.com`). Not a
    /// rate limit: the profile has to answer it once.
    Consent,
}

impl Block {
    pub fn detail(self) -> &'static str {
        match self {
            Block::Sorry => "redirected to /sorry/",
            Block::UnusualTraffic => "unusual traffic page",
            Block::Captcha => "reCAPTCHA challenge",
            Block::Consent => "cookie consent page",
        }
    }

    /// Counts against the page-load budget (DECISIONS.md 0039).
    pub fn is_rate_limit(self) -> bool {
        !matches!(self, Block::Consent)
    }

    pub fn outcome(self) -> QueryOutcome {
        QueryOutcome::Blocked {
            detail: self.detail().to_owned(),
        }
    }
}

/// Detects Google's block pages from the page URL and visible text.
pub fn detect_block(url: &str, text: &str) -> Option<Block> {
    let url_l = url.to_ascii_lowercase();
    let path_start = url_l.find("://").map_or(0, |i| i + 3);
    let host = url_l[path_start..]
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("");
    let path = &url_l[path_start + host.len()..];
    if path.starts_with("/sorry/")
        || path.starts_with("/sorry?")
        || path == "/sorry"
    {
        return Some(Block::Sorry);
    }
    let text_l = text.to_lowercase();
    if text_l.contains("unusual traffic") {
        return Some(Block::UnusualTraffic);
    }
    if text_l.contains("recaptcha")
        || text_l.contains("i'm not a robot")
        || text_l.contains("i’m not a robot")
    {
        return Some(Block::Captcha);
    }
    if host == "consent.google.com" || host.starts_with("consent.") {
        return Some(Block::Consent);
    }
    None
}

/// Whether a results or Explore page says it has nothing to sell
/// (DECISIONS.md 0067), from its visible text ([`PAGE_TEXT_JS`]):
///
/// - "No options matching your search" (economy);
/// - "No Business Class flights found", "No Premium Economy flights
///   found" (a cabin no one flies on the route), or any "No … flights
///   found" of a few words.
///
/// "No results returned." alone does not count: the page shows it
/// above "Oops, something went wrong. Reload" too, a failed load that
/// must stay `Unexpected`, and that error wins over any other text.
pub fn detect_no_results(text: &str) -> bool {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = text.to_lowercase();
    if text.contains("something went wrong") {
        return false;
    }
    if text.contains("no options matching your search") {
        return true;
    }
    text.match_indices("no ").any(|(i, _)| {
        let starts_word = text[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let words: Vec<&str> = text[i + 3..].split(' ').take(6).collect();
        starts_word
            && words
                .iter()
                .position(|w| *w == "flights")
                .is_some_and(|at| {
                    words.get(at + 1) == Some(&"found")
                        && words[..at].iter().all(|w| {
                            !w.is_empty() && w.chars().all(char::is_alphabetic)
                        })
                })
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LabelError {
    #[error("no date in {0:?}")]
    NoDate(String),
    #[error("unreadable price in {0:?}")]
    Price(String),
}

/// One date-grid cell, read from its aria-label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridLabel {
    /// Whole units, `None` when the cell shows no price.
    pub price: Option<i64>,
    /// The symbol as shown (`"R$"`, `"$"`, `"€"`).
    pub currency_symbol: Option<String>,
    /// "cheapest price" in the label.
    pub cheapest: bool,
    pub departure: NaiveDate,
    pub return_date: Option<NaiveDate>,
}

fn month(word: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct",
        "nov", "dec",
    ];
    let w = word.to_ascii_lowercase();
    let w = w.trim_end_matches('.');
    if w.len() < 3 {
        return None;
    }
    let full = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    MONTHS
        .iter()
        .zip(full)
        .position(|(short, full)| {
            w == *short || w == full || (w == "sept" && *short == "sep")
        })
        .map(|i| i as u32 + 1)
}

/// The first `(month, day) ≥ after` (within a year).
fn next_date(m: u32, d: u32, after: NaiveDate) -> Option<NaiveDate> {
    let this = NaiveDate::from_ymd_opt(after.year(), m, d);
    match this {
        Some(date) if date >= after => Some(date),
        _ => NaiveDate::from_ymd_opt(after.year() + 1, m, d),
    }
}

/// Parses a date-grid aria-label such as
/// `"R$3,972, cheapest price, Nov 7 to Nov 18"` (round trip) or
/// `"R$1,827, Nov 8"` (one way). Labels carry no year: a departure is
/// placed on or after `reference − 30 days` (pass today), a return on
/// or after its departure.
pub fn parse_grid_label(
    label: &str,
    reference: NaiveDate,
) -> Result<GridLabel, LabelError> {
    let words: Vec<&str> = label
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| c == ',' || c == ';'))
        .collect();
    let mut dates = Vec::new();
    let mut first_date_word = None;
    let mut i = 0;
    while i + 1 < words.len() {
        if let (Some(m), Ok(d)) = (month(words[i]), words[i + 1].parse::<u32>())
        {
            first_date_word.get_or_insert(i);
            dates.push((m, d));
            i += 2;
        } else {
            i += 1;
        }
    }
    let no_date = || LabelError::NoDate(label.to_owned());
    let (&(m, d), rest) = dates.split_first().ok_or_else(no_date)?;
    let earliest = reference - chrono::Days::new(30);
    let departure = next_date(m, d, earliest).ok_or_else(no_date)?;
    let return_date = match rest.first() {
        Some(&(m, d)) => Some(next_date(m, d, departure).ok_or_else(no_date)?),
        None => None,
    };
    let before = &words[..first_date_word.unwrap_or(0)];
    let price_word = before
        .iter()
        .find(|w| w.chars().any(|c| c.is_ascii_digit()));
    let (price, currency_symbol) = match price_word {
        None => (None, None),
        Some(w) => {
            let symbol: String =
                w.chars().take_while(|c| !c.is_ascii_digit()).collect();
            let digits: String = w
                .chars()
                .skip_while(|c| !c.is_ascii_digit())
                .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
                .filter(|c| *c != ',')
                .collect();
            let whole = digits.split('.').next().unwrap_or("");
            let price = whole
                .parse::<i64>()
                .map_err(|_| LabelError::Price(label.to_owned()))?;
            (Some(price), (!symbol.is_empty()).then_some(symbol))
        }
    };
    Ok(GridLabel {
        price,
        currency_symbol,
        cheapest: label.to_ascii_lowercase().contains("cheapest price"),
        departure,
        return_date,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap_or_default()
    }

    #[test]
    fn research_label() {
        let l = parse_grid_label(
            "R$3,972, cheapest price, Nov 7 to Nov 18",
            d(2026, 10, 6),
        );
        assert_eq!(
            l,
            Ok(GridLabel {
                price: Some(3972),
                currency_symbol: Some("R$".into()),
                cheapest: true,
                departure: d(2026, 11, 7),
                return_date: Some(d(2026, 11, 18)),
            })
        );
    }

    #[test]
    fn labels_across_new_year() {
        let l =
            parse_grid_label("R$5,212, Dec 28 to Jan 4", d(2026, 10, 6)).ok();
        assert_eq!(l.as_ref().map(|l| l.departure), Some(d(2026, 12, 28)));
        assert_eq!(l.as_ref().and_then(|l| l.return_date), Some(d(2027, 1, 4)));
        assert_eq!(l.map(|l| l.cheapest), Some(false));
        // In October, a February departure is next year's.
        let l = parse_grid_label("$746, February 3", d(2026, 10, 6)).ok();
        assert_eq!(l.as_ref().map(|l| l.departure), Some(d(2027, 2, 3)));
        assert_eq!(l.as_ref().and_then(|l| l.price), Some(746));
        assert_eq!(l.and_then(|l| l.currency_symbol).as_deref(), Some("$"));
    }

    #[test]
    fn labels_without_price_or_date() {
        let l =
            parse_grid_label("No price, Nov 7 to Nov 14", d(2026, 10, 6)).ok();
        assert_eq!(l.map(|l| l.price), Some(None));
        assert!(
            parse_grid_label("R$3,972, cheapest price", d(2026, 10, 6))
                .is_err()
        );
    }

    #[test]
    fn blocks() {
        assert_eq!(
            detect_block("https://www.google.com/sorry/index?continue=x", ""),
            Some(Block::Sorry)
        );
        assert_eq!(
            detect_block(
                "https://www.google.com/travel/flights",
                "Our systems have detected unusual traffic from your computer network."
            ),
            Some(Block::UnusualTraffic)
        );
        assert_eq!(
            detect_block(
                "https://www.google.com/x",
                "please solve this reCAPTCHA"
            ),
            Some(Block::Captcha)
        );
        assert_eq!(
            detect_block(
                "https://consent.google.com/ml?continue=x",
                "Before you continue"
            ),
            Some(Block::Consent)
        );
        assert_eq!(
            detect_block(
                "https://www.google.com/travel/flights/search?tfs=x&q=sorry",
                "São Paulo to Lisbon"
            ),
            None
        );
        assert!(Block::Sorry.is_rate_limit());
        assert!(Block::Sorry.outcome().is_block());
        assert!(!Block::Consent.is_rate_limit());
    }

    /// Page text the daemon logged for São Paulo ↔ Nuuk (GOH), where
    /// Google Flights sells nothing (0067, 0068), and for a page that
    /// failed to load.
    const GOH_ECONOMY: &str = "Skip to main content Accessibility feedback \
        Explore Flights Hotels Vacation rentals Change appearance Sign in \
        Loading results Flight search One way 1 Economy São Paulo Nuuk GOH \
        Filters All filters Stops Airlines Bags Price Times Emissions \
        Connecting airports Duration Search results No results returned. \
        No options matching your search Try changing your dates or \
        destination to see results Language\u{200b}English (United States)";
    const GOH_BUSINESS: &str = "Skip to main content Accessibility feedback \
        Explore Flights Hotels Vacation rentals Change appearance Sign in \
        Loading results Flight search Round trip 1 Business São Paulo Nuuk \
        GOH Filters All filters Stops Airlines Bags Price Times Emissions \
        Connecting airports Duration Search results No results returned. \
        No Business Class flights found There might be no Business Class \
        flights to Nuuk (GOH). Try";
    const GOH_PREMIUM: &str = "Flight search One way 1 Premium economy \
        Nuuk GOH São Paulo Filters All filters Stops Airlines Bags Price \
        Times Emissions Connecting airports Duration Search results\n\
        No results returned.\nNo Premium Economy flights found\nThere \
        might be no Premium Economy flights to São Paulo.";
    const CGB_OOPS: &str = "Skip to main content Accessibility feedback \
        Explore Flights Hotels Vacation rentals Change appearance Sign in \
        Loading results Flight search One way 1 Economy São Paulo Cuiabá CGB \
        Filters All filters Stops Airlines Bags Price Times Emissions \
        Connecting airports Duration Search results No results returned. \
        Oops, something went wrong. Reload Language\u{200b}English (United \
        States) Location\u{200b}Brazil CurrencyBRL";

    #[test]
    fn no_results_pages() {
        assert!(detect_no_results(GOH_ECONOMY));
        assert!(detect_no_results(GOH_BUSINESS));
        assert!(detect_no_results(GOH_PREMIUM));
        assert!(detect_no_results("No First Class flights found"));
        assert!(detect_no_results("Search results\nNo flights found"));
        // A failed load says "No results returned." too: not an answer.
        assert!(!detect_no_results(CGB_OOPS));
        assert!(!detect_no_results("Search results No results returned."));
        assert!(!detect_no_results(&format!(
            "{GOH_ECONOMY} Oops, something went wrong."
        )));
        // Ordinary results, and look-alikes.
        assert!(!detect_no_results(
            "Search results Top departing flights R$3,972 round trip \
             Nonstop 11 hr 5 min Select flight"
        ));
        assert!(!detect_no_results("Casino flights found here"));
        assert!(!detect_no_results("No flights"));
        assert!(!detect_no_results("no 2 flights found"));
        assert!(!detect_no_results(""));
    }

    #[test]
    fn init_data_snippet_names_its_key() {
        assert!(
            init_data_js("ds:1").contains("AF_initDataCallback({key: 'ds:1'")
        );
        assert!(!init_data_js("x'); alert(1); ('").contains("x')"));
    }
}
