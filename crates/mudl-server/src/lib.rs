pub mod assets;
pub mod document;
pub mod fs;
pub mod http;
pub mod mime;
pub mod routes;
pub mod server;
pub mod version;

#[cfg(test)]
mod tests {
    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn crate_compiles() {
        assert!(true);
    }
}
