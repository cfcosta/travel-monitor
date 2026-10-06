# American Airlines fixtures

No price response was captured: the user's Chrome got Akamai's
"Challenge Validation" CAPTCHA on the first results page, and the
headless Chromium got "Access Denied" on the home page (see
`../../american-airlines.md`).

- `home-search-form.json`: the fields of the home page search form
  (`#reservationFlightSearchForm`), as read from the DOM of
  `https://www.aa.com/homePage.do?locale=pt_BR` on 2026-10-06. No
  values that identify the session or the user.
