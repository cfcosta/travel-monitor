//! Google Flights: tfs encoding, search and Explore URLs, and parsers for the page's RPC responses.
//!
//! Pure: no network, no browser (docs/architecture.md, `tm-gflights`).
//!
//! - [`tfs`]: the search protobuf in `tfs=` URLs, encoded and decoded
//!   byte-exactly.
//! - [`url`]: search, booking, Explore and resolve URLs for a
//!   [`url::Locale`] (`hl`, `gl`, `curr`).
//! - [`rpc`]: `rt=c` response frames and typed parsers for the
//!   calendar (graph and grid), Explore, results and booking RPCs.
//! - [`page`]: JavaScript snippets for the daemon, the date-grid label
//!   parser and block detection.
//! - [`plan`]: how many price-graph pages cover a horizon.
//! - [`proto`]: the minimal protobuf wire codec under all of it.

pub mod page;
pub mod plan;
pub mod proto;
pub mod rpc;
pub mod tfs;
pub mod url;

pub use rpc::{Method, RpcError, method_of};
pub use tfs::{FlightSpec, LegSpec, PlaceRef, Search, TfsError, Trip};
pub use url::{Locale, UrlError};
