// Rebuild when a migration is added or changed: `sqlx::migrate!` embeds `migrations/` at compile
// time and cannot track the directory itself on stable.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
