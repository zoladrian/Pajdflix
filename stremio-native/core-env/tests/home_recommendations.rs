//! Exercise the actual home recommendation policy without starting the GUI,
//! opening user databases or linking the player.
mod config {
    pub struct Config {
        pub children_recommendations: bool,
    }
    pub fn with_config<T>(read: impl FnOnce(&Config) -> T) -> T {
        read(&Config {
            children_recommendations: false,
        })
    }
}

#[path = "../../app/src/home_catalogs.rs"]
mod home_catalogs;
