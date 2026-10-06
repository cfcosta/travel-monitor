//! Rebuild when a migration changes: `sqlx::migrate!` embeds them.

fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
