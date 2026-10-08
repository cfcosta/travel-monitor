//! The scripts the LATAM reader runs in the results page (DECISIONS.md
//! 0084, 0085, 0090, 0091). Each only reads the page: it finds what a
//! person would click and names it with a CSS selector of its own
//! ([`PATH_JS`]), which `tm_browser::Mouse::click` then scrolls into view
//! with the wheel and clicks as a hand does. No script scrolls or
//! clicks, nothing is typed and no API is called (0052).
//!
//! Two things click: the date strip, only when switched on
//! (`Config::latam_strip`), and a round trip's outbound fare, whose
//! families are not scheduled (`sites::latam::unsupported`). In the
//! live run a search a strip click made, with no movement before it,
//! got Akamai's 403 (0090, 0091).

use chrono::NaiveDate;

/// The document's title and visible text, as `[title, text]`.
pub const TITLE_AND_TEXT_JS: &str = "[document.title || '', \
     (document.body && document.body.innerText || '').slice(0, 20000)]";

/// A function of an element that returns a selector matching it and
/// nothing before it: the nearest unique id, then `:nth-child` steps.
/// The mouse finds the element by it; the page is not changed.
const PATH_JS: &str = r#"(el) => {
  const parts = [];
  for (let e = el; e && e.nodeType === 1; e = e.parentElement) {
    if (e.id && document.querySelectorAll('#' + CSS.escape(e.id)).length === 1) {
      parts.unshift('#' + CSS.escape(e.id));
      break;
    }
    if (!e.parentElement) { parts.unshift(e.tagName.toLowerCase()); break; }
    parts.unshift(e.tagName.toLowerCase() + ':nth-child(' + ([...e.parentElement.children].indexOf(e) + 1) + ')');
  }
  const path = parts.join(' > ');
  return document.querySelector(path) === el ? path : null;
}"#;

/// Finds the date strip's control for `date`. Returns `{path}`, a
/// selector for it, `{found: true}` when it is there but disabled, or
/// `null`.
///
/// The strip is not described beyond "seven days around the searched
/// date" (latam-pass.md, "Measuring cash against miles" §4), so it is
/// found by its shape: a control whose own short text names the day
/// (`16/06`, `16 jun`, `qua., 16 de jun.`) or whose attributes hold the
/// ISO date, inside a container with at least five such dated controls.
/// Controls in forms, inputs and dialogs (the search box, its date
/// picker) are left out.
pub fn strip_day_js(date: NaiveDate) -> String {
    format!(
        r#"((iso) => {{
  const path = {PATH_JS};
  const [y, m, d] = iso.split('-').map(Number);
  const pt = ['jan','fev','mar','abr','mai','jun','jul','ago','set','out','nov','dez'][m - 1];
  const en = ['jan','feb','mar','apr','may','jun','jul','aug','sep','oct','nov','dec'][m - 1];
  const dd = String(d).padStart(2, '0'), mm = String(m).padStart(2, '0');
  const text = e => (e.innerText || e.textContent || '').replace(/\s+/g, ' ').trim().toLowerCase();
  const slash = new RegExp('(^|[^0-9])0?' + d + '\\s*/\\s*0?' + m + '([^0-9]|$)');
  const named = new RegExp('(^|[^0-9])0?' + d + '\\s*(de\\s+)?(' + pt + '|' + en + ')');
  const anyDay = /(^|[^0-9])[0-3]?[0-9]\s*\/\s*[01]?[0-9]([^0-9]|$)|(^|[^0-9])[0-3]?[0-9]\s*(de\s+)?(jan|fev|feb|mar|abr|apr|mai|may|jun|jul|ago|aug|set|sep|out|oct|nov|dez|dec)/;
  const ownDay = e => {{
    for (const a of e.attributes || []) {{
      if (a.value && (a.value.includes(iso) || a.value.includes(dd + '/' + mm + '/' + y))) return true;
    }}
    const t = text(e);
    return t.length < 60 && (slash.test(t) || named.test(t));
  }};
  const shown = e => {{ const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; }};
  const controls = [...document.querySelectorAll('button, [role="button"], [role="tab"], [role="option"], [role="radio"], a, li')]
    .filter(shown)
    .filter(e => !e.closest('form, [role="dialog"], input, select, textarea'));
  const dated = controls.filter(e => {{ const t = text(e); return t.length < 60 && anyDay.test(t); }});
  const inStrip = e => {{
    let p = e.parentElement;
    for (let i = 0; p && i < 4; i++, p = p.parentElement) {{
      if (dated.filter(o => p.contains(o)).length >= 5) return true;
    }}
    return false;
  }};
  const hits = controls.filter(ownDay).filter(inStrip);
  const el = hits.find(e => !hits.some(o => o !== e && e.contains(o))) || hits[0];
  if (!el) return null;
  if (el.disabled || el.getAttribute('aria-disabled') === 'true') return {{found: true}};
  const p = path(el);
  return p ? {{path: p}} : {{found: true}};
}})({iso:?})"#,
        iso = date.format("%Y-%m-%d").to_string(),
    )
}

/// A selector for the first visible, enabled element matching
/// `selector` (a page may keep hidden copies), or `null`.
pub fn shown_js(selector: &str) -> String {
    format!(
        r#"((sel) => {{
  const path = {PATH_JS};
  const el = [...document.querySelectorAll(sel)].find(e => {{ const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; }});
  if (!el || el.disabled) return null;
  return path(el);
}})({})"#,
        serde_json::to_string(selector).unwrap_or_default()
    )
}

/// The `index`-th result card's header: a click opens its brands
/// (latam.md §2).
pub fn card_header(index: usize) -> String {
    format!("[data-testid=\"wrapper-card-header-{index}\"]")
}

/// The `brand`-th brand's "Escolher" button of the open card: a click
/// chooses that outbound fare.
pub fn brand_select(brand: usize) -> String {
    format!("#bundle-detail-{brand}-flight-select")
}

/// The "Continuar" of the LATAM+ upsell that may follow the choice.
pub const UPSELL_CONTINUE: &str = "#latam-plus-continue";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_name_their_targets() {
        let day = NaiveDate::from_ymd_opt(2027, 6, 16).expect("a date");
        let js = strip_day_js(day);
        assert!(js.ends_with(r#"})("2027-06-16")"#), "{js}");
        assert!(!js.contains("scrollIntoView") && !js.contains(".click("));
        assert_eq!(card_header(3), "[data-testid=\"wrapper-card-header-3\"]");
        assert_eq!(brand_select(0), "#bundle-detail-0-flight-select");
        let js = shown_js("#a\"b");
        assert!(js.contains(r##"("#a\"b")"##), "{js}");
        assert!(!js.contains("scrollIntoView") && !js.contains(".click("));
    }
}
