pub mod assets;
pub mod http;
pub mod mime;
pub mod routes;
pub mod server;

#[cfg(test)]
mod tests {
    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn crate_compiles() {
        assert!(true);
    }
}
