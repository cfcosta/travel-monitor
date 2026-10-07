//! The scripts the LATAM reader runs in the results page (DECISIONS.md
//! 0084, 0085, 0090). Each only reads the page or finds where a person
//! would click; the clicks themselves are mouse events at the point a
//! script returns (`tm_browser::Page::click_at`), the input a person
//! uses. Nothing is typed and no API is called (0052).
//!
//! Two things click: the date strip, only when switched on
//! (`Config::latam_strip`), and a round trip's outbound fare, whose
//! families are not scheduled (`sites::latam::unsupported`). In the
//! live run a search a strip click made got Akamai's 403 (0090).

use chrono::NaiveDate;

/// The document's title and visible text, as `[title, text]`.
pub const TITLE_AND_TEXT_JS: &str = "[document.title || '', \
     (document.body && document.body.innerText || '').slice(0, 20000)]";

/// Finds the date strip's control for `date` and, when `scroll`, brings
/// it into view. Returns `{x, y}`, its centre in the viewport, `{found:
/// true}` when it is there but covered or disabled, or `null`.
///
/// The strip is not described beyond "seven days around the searched
/// date" (latam-pass.md, "Measuring cash against miles" §4), so it is
/// found by its shape: a control whose own short text names the day
/// (`16/06`, `16 jun`, `qua., 16 de jun.`) or whose attributes hold the
/// ISO date, inside a container with at least five such dated controls.
/// Controls in forms, inputs and dialogs (the search box, its date
/// picker) are left out.
pub fn strip_day_js(date: NaiveDate, scroll: bool) -> String {
    format!(
        r#"((iso, scroll) => {{
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
  if (scroll) el.scrollIntoView({{block: 'center', inline: 'center'}});
  const r = el.getBoundingClientRect();
  const x = r.left + r.width / 2, y2 = r.top + r.height / 2;
  const top = document.elementFromPoint(x, y2);
  if (!top || !(top === el || el.contains(top) || top.contains(el))) return {{found: true}};
  return {{x, y: y2}};
}})({iso:?}, {scroll})"#,
        iso = date.format("%Y-%m-%d").to_string(),
    )
}

/// The centre of the first visible element matching `selector`, after
/// scrolling it into view, or `null`.
pub fn locate_js(selector: &str) -> String {
    format!(
        r#"((sel) => {{
  const el = [...document.querySelectorAll(sel)].find(e => {{ const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; }});
  if (!el || el.disabled) return null;
  el.scrollIntoView({{block: 'center'}});
  const r = el.getBoundingClientRect();
  return {{x: r.left + r.width / 2, y: r.top + r.height / 2}};
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
        let js = strip_day_js(day, true);
        assert!(js.ends_with(r#"})("2027-06-16", true)"#), "{js}");
        assert!(!strip_day_js(day, false).contains("\"2027-06-16\", true"));
        assert_eq!(card_header(3), "[data-testid=\"wrapper-card-header-3\"]");
        assert_eq!(brand_select(0), "#bundle-detail-0-flight-select");
        assert!(locate_js("#a\"b").contains(r##"("#a\"b")"##));
    }
}
